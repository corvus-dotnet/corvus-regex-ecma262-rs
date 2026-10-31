//! Whether a pattern is valid in the grammar, decided as the parser decides it but without its limits of size, and
//! with nothing allocated for any pattern of ordinary size: no tree, no sets, no string (the design of the .NET
//! engine's `EcmaRegexValidator`).
//!
//! It reads the pattern where it lies, decoding a character only where the grammar needs it. It keeps the groups and
//! the nested classes of the v flag on stacks of its own rather than recursing, so that no pattern can overflow the
//! stack of the thread, and it keeps a group name as its place in the pattern. Each stack holds its first entries in
//! place and moves to the heap only beyond them, which takes groups nested more deeply than 32, or more than 12
//! named groups (or their paths) at once. It asks the grammar only: the parser's limits of size (the length of the
//! pattern, the nesting of groups, the size it counts) belong to compiling a pattern, and none of them applies here.
//!
//! Each method follows the method of the parser with the same name, and makes the same decision at each step. A
//! position is twice the index of a byte of the pattern, plus one for the second half of a character beyond the Basic
//! Multilingual Plane when it is read as two UTF-16 code units (without the u flag the grammar reads code units).

use super::Rune;
use super::charset::{MAX_RUNE, has_property, has_string_property};
use super::es2020;
use super::parse::Syntax;
use super::ucd::{Ucd, ucd};

/// A character escape that is not valid.
const INVALID: Rune = -2;

const KIND_GROUP: usize = 0;
const KIND_LOOKAHEAD: usize = 1;
const KIND_LOOKBEHIND: usize = 2;

// A frame of a class of the v flag: whether it is negated, its phase, its operator, whether it may contain strings.
const CLASS_FRAME: usize = 4;
const PHASE_START: usize = 0;
const PHASE_UNION: usize = 1;
const PHASE_OPERANDS: usize = 2;
const WAITING_FIRST: usize = 3;
const WAITING_UNION: usize = 4;
const WAITING_OPERAND: usize = 5;

/// Whether a pattern is valid in the grammar of the syntax (its u and v flags, its edition and its version of
/// Unicode).
pub(crate) fn is_valid(pattern: &str, syntax: Syntax) -> bool {
    Validator::new(pattern, syntax).validate()
}

/// A stack of integers whose first `N` live in place, and which moves to the heap when it outgrows them.
struct Stack<const N: usize> {
    inline: [usize; N],
    heap: Vec<usize>,
    len: usize,
}

impl<const N: usize> Stack<N> {
    fn new() -> Self {
        Stack { inline: [0; N], heap: Vec::new(), len: 0 }
    }

    fn push(&mut self, v: usize) {
        if self.heap.is_empty() {
            if self.len < N {
                self.inline[self.len] = v;
                self.len += 1;
                return;
            }
            self.heap.reserve(2 * N);
            self.heap.extend_from_slice(&self.inline);
        }
        self.heap.push(v);
        self.len += 1;
    }

    fn pop(&mut self) -> usize {
        self.len -= 1;
        if self.heap.is_empty() { self.inline[self.len] } else { self.heap.pop().expect("the stack holds something") }
    }

    fn truncate(&mut self, len: usize) {
        self.len = len;
        if !self.heap.is_empty() {
            self.heap.truncate(len);
        }
    }

    fn items(&self) -> &[usize] {
        if self.heap.is_empty() { &self.inline[..self.len] } else { &self.heap }
    }

    fn items_mut(&mut self) -> &mut [usize] {
        if self.heap.is_empty() { &mut self.inline[..self.len] } else { &mut self.heap }
    }
}

/// The places of the named groups by the hash of their names: one more than the place in `names` of the last group
/// with each name (0 for none). Open addressing, a power of two long and at most half full.
struct NameTable {
    inline: [usize; 32],
    heap: Vec<usize>,
}

impl NameTable {
    fn slots(&self) -> &[usize] {
        if self.heap.is_empty() { &self.inline } else { &self.heap }
    }

    fn slots_mut(&mut self) -> &mut [usize] {
        if self.heap.is_empty() { &mut self.inline } else { &mut self.heap }
    }
}

struct Validator<'a> {
    bytes: &'a [u8],
    end: usize,
    unicode: bool,
    sets: bool,
    es2020: bool,
    ucd: &'static Ucd,
    i: usize,
    has_names: bool,
    group_count: i64,
    disjunctions: usize,
    /// The alternatives that enclose the position, as disjunction and alternative (the parser's path).
    path: Stack<64>,
    /// The groups open at the position: the kind of each.
    frames: Stack<32>,
    /// Each named group: the start and the end of its name, the start and the length of its path in `name_paths`,
    /// and the hash of its name.
    names: Stack<64>,
    name_paths: Stack<64>,
    names_table: NameTable,
    distinct_names: usize,
    /// Each named reference: the start and the end of its name.
    references: Stack<32>,
    /// The classes of the v flag open at the position (see `CLASS_FRAME`).
    classes: Stack<64>,
}

fn is_digit(c: Rune) -> bool {
    (0x30..=0x39).contains(&c)
}

fn hex_value(c: Rune) -> Rune {
    match c {
        0x30..=0x39 => c - 0x30,
        0x61..=0x66 => c - 0x61 + 10,
        0x41..=0x46 => c - 0x41 + 10,
        _ => -1,
    }
}

fn is_one_of(c: Rune, set: &[u8]) -> bool {
    u8::try_from(c).is_ok_and(|c| set.contains(&c))
}

const fn ch(c: char) -> Rune {
    c as Rune
}

impl<'a> Validator<'a> {
    fn new(pattern: &'a str, syntax: Syntax) -> Validator<'a> {
        Validator {
            bytes: pattern.as_bytes(),
            end: pattern.len() * 2,
            unicode: syntax.unicode || syntax.sets,
            sets: syntax.sets,
            es2020: syntax.edition == super::Edition::Es2020,
            ucd: ucd(syntax.unicode_version),
            i: 0,
            has_names: false,
            group_count: 0,
            disjunctions: 0,
            path: Stack::new(),
            frames: Stack::new(),
            names: Stack::new(),
            name_paths: Stack::new(),
            names_table: NameTable { inline: [0; 32], heap: Vec::new() },
            distinct_names: 0,
            references: Stack::new(),
            classes: Stack::new(),
        }
    }

    /// The element at a position (a code point with the u flag's grammar, a UTF-16 code unit without), and the
    /// position after it.
    fn at(&self, pos: usize) -> (Rune, usize) {
        let b = pos >> 1;
        let first = self.bytes[b] as Rune;
        let (width, cp) = if first < 0x80 {
            (1, first)
        } else if first < 0xE0 {
            (2, ((first & 0x1F) << 6) | (self.bytes[b + 1] as Rune & 0x3F))
        } else if first < 0xF0 {
            (3, ((first & 0x0F) << 12) | ((self.bytes[b + 1] as Rune & 0x3F) << 6) | (self.bytes[b + 2] as Rune & 0x3F))
        } else {
            (
                4,
                ((first & 0x07) << 18)
                    | ((self.bytes[b + 1] as Rune & 0x3F) << 12)
                    | ((self.bytes[b + 2] as Rune & 0x3F) << 6)
                    | (self.bytes[b + 3] as Rune & 0x3F),
            )
        };
        if cp > 0xFFFF && !self.unicode {
            // Two code units: the high surrogate, then the low one.
            if pos & 1 == 0 {
                return (0xD800 + ((cp - 0x10000) >> 10), pos | 1);
            }
            return (0xDC00 + ((cp - 0x10000) & 0x3FF), (b + width) << 1);
        }
        (cp, (b + width) << 1)
    }

    fn more(&self) -> bool {
        self.i < self.end
    }

    fn peek(&self) -> Rune {
        if self.i < self.end { self.at(self.i).0 } else { -1 }
    }

    /// The element after the one at the position, or -1.
    fn peek_second(&self) -> Rune {
        if self.i >= self.end {
            return -1;
        }
        let (_, next) = self.at(self.i);
        if next < self.end { self.at(next).0 } else { -1 }
    }

    fn next(&mut self) -> Rune {
        let (c, next) = self.at(self.i);
        self.i = next;
        c
    }

    fn eat(&mut self, c: char) -> bool {
        if self.i < self.end {
            let (e, next) = self.at(self.i);
            if e == c as Rune {
                self.i = next;
                return true;
            }
        }
        false
    }

    fn validate(&mut self) -> bool {
        self.scan_groups();
        self.push_disjunction();
        while self.more() {
            let c = self.peek();
            if c == ch('|') {
                self.next();
                let last = self.path.len - 1;
                self.path.items_mut()[last] += 1;
                continue;
            }
            if c == ch(')') {
                // The end of a group: an unmatched ')' when none is open.
                if self.frames.len == 0 {
                    return false;
                }
                self.next();
                self.path.truncate(self.path.len - 2);
                let kind = self.frames.pop();
                // Annex B lets a quantifier follow a lookahead. The u flag does not, and neither lets one follow a
                // lookbehind.
                let quantifiable = kind == KIND_GROUP || (!self.unicode && kind == KIND_LOOKAHEAD);
                match self.quantifier() {
                    None => return false,
                    Some(true) if !quantifiable => return false,
                    _ => {}
                }
                continue;
            }
            if c == ch('(') {
                let Some(kind) = self.group_or_look() else { return false };
                self.frames.push(kind);
                self.push_disjunction();
                continue;
            }
            let Some(quantifiable) = self.term(c) else { return false };
            match self.quantifier() {
                None => return false,
                Some(true) if !quantifiable => return false,
                _ => {}
            }
        }
        // An unterminated group.
        if self.frames.len != 0 {
            return false;
        }
        // Every named reference names a group somewhere in the pattern.
        for r in (0..self.references.len).step_by(2) {
            let (start, stop) = (self.references.items()[r], self.references.items()[r + 1]);
            let hash = self.name_hash(start, stop);
            if self.find_name(start, stop, hash).is_err() {
                return false;
            }
        }
        true
    }

    fn push_disjunction(&mut self) {
        self.disjunctions += 1;
        self.path.push(self.disjunctions);
        self.path.push(0);
    }

    /// Counts the capturing groups of the pattern and notes whether any is named.
    fn scan_groups(&mut self) {
        let mut in_class = false;
        let mut pos = 0;
        while pos < self.end {
            let (c, mut next) = self.at(pos);
            if c == ch('\\') {
                if next < self.end {
                    next = self.at(next).1;
                }
            } else if c == ch('[') {
                in_class = true;
            } else if c == ch(']') {
                in_class = false;
            } else if c == ch('(') && !in_class {
                let (e1, after1) = if next < self.end { self.at(next) } else { (-1, self.end) };
                if e1 != ch('?') {
                    self.group_count += 1;
                } else {
                    let (e2, after2) = if after1 < self.end { self.at(after1) } else { (-1, self.end) };
                    let e3 = if e2 >= 0 && after2 < self.end { self.at(after2).0 } else { -1 };
                    if e2 == ch('<') && e3 >= 0 && e3 != ch('=') && e3 != ch('!') {
                        self.group_count += 1;
                        self.has_names = true;
                    }
                }
            }
            pos = next;
        }
    }

    /// Reads a term that is not a group: whether a quantifier may follow it, or `None` when it is not valid.
    fn term(&mut self, c: Rune) -> Option<bool> {
        match u8::try_from(c).unwrap_or(0) {
            b'^' | b'$' => {
                self.next();
                Some(false)
            }
            b'[' => {
                let valid = if self.sets { self.class_set_expression() } else { self.class() };
                valid.then_some(true)
            }
            b'\\' => self.atom_escape(),
            b'*' | b'+' | b'?' => None,
            b'{' => {
                if self.unicode || self.looks_like_quantifier() {
                    return None;
                }
                self.next();
                Some(true)
            }
            b']' | b'}' => {
                if self.unicode {
                    return None;
                }
                self.next();
                Some(true)
            }
            _ => {
                self.next();
                Some(true)
            }
        }
    }

    /// Whether the text at the current `{` reads as `{n}`, `{n,}` or `{n,m}`.
    fn looks_like_quantifier(&self) -> bool {
        let (_, mut k) = self.at(self.i);
        let start = k;
        while k < self.end {
            let (c, next) = self.at(k);
            if !is_digit(c) {
                break;
            }
            k = next;
        }
        if k == start || k >= self.end {
            return false;
        }
        let (c, after_digits) = self.at(k);
        if c == ch('}') {
            return true;
        }
        if c != ch(',') {
            return false;
        }
        k = after_digits;
        while k < self.end {
            let (c, next) = self.at(k);
            if !is_digit(c) {
                break;
            }
            k = next;
        }
        k < self.end && self.at(k).0 == ch('}')
    }

    /// Whether the first of two runs of decimal digits (by position) is the greater number, whatever their lengths.
    /// A digit is one byte, so two positions.
    fn greater_as_written(&self, mut a: usize, a_end: usize, mut b: usize, b_end: usize) -> bool {
        while a < a_end && self.bytes[a >> 1] == b'0' {
            a += 2;
        }
        while b < b_end && self.bytes[b >> 1] == b'0' {
            b += 2;
        }
        let (a_digits, b_digits) = (&self.bytes[a >> 1..a_end >> 1], &self.bytes[b >> 1..b_end >> 1]);
        if a_digits.len() != b_digits.len() {
            return a_digits.len() > b_digits.len();
        }
        a_digits > b_digits
    }

    /// Reads a run of decimal digits. It reports whether there was one.
    fn number(&mut self) -> bool {
        let start = self.i;
        while self.more() && is_digit(self.peek()) {
            self.next();
        }
        start != self.i
    }

    /// The decimal digits from the position, saturating as the parser's `number` does.
    fn number_value(&mut self) -> i64 {
        let mut v: i64 = 0;
        while self.more() && is_digit(self.peek()) {
            v = (v * 10 + (self.next() - ch('0')) as i64).min(i32::MAX as i64);
        }
        v
    }

    /// Reads a quantifier when one is here: `Some(false)` for none, `Some(true)` for one, `None` when it is not valid.
    fn quantifier(&mut self) -> Option<bool> {
        let c = self.peek();
        if c == ch('*') || c == ch('+') || c == ch('?') {
            self.next();
        } else if c == ch('{') && self.looks_like_quantifier() {
            self.next();
            let first = self.i;
            self.number();
            let first_end = self.i;
            if self.eat(',') && self.peek() != ch('}') {
                // The counts are compared as the numbers written, which can be past what a count is kept as.
                let second = self.i;
                self.number();
                if self.greater_as_written(first, first_end, second, self.i) {
                    return None;
                }
            }
            if !self.eat('}') {
                return None;
            }
        } else {
            return Some(false);
        }
        self.eat('?');
        Some(true)
    }

    /// Reads the start of a group or a lookaround, to its body: its kind, or `None` when it is not valid.
    fn group_or_look(&mut self) -> Option<usize> {
        self.next();
        if !self.eat('?') || self.eat(':') {
            return Some(KIND_GROUP);
        }
        let c = self.peek();
        // Modifier groups came in ECMAScript 2025, so the 2020 grammar reads `(?i` as an invalid group.
        if (c == ch('i') || c == ch('m') || c == ch('s') || c == ch('-')) && !self.es2020 {
            return self.modifier_group().then_some(KIND_GROUP);
        }
        if self.eat('=') || self.eat('!') {
            return Some(KIND_LOOKAHEAD);
        }
        if !self.eat('<') {
            return None;
        }
        if self.eat('=') || self.eat('!') {
            return Some(KIND_LOOKBEHIND);
        }
        let (start, stop) = self.group_name()?;
        // Two groups may share a name only when they are in separate alternatives of one disjunction, so that no
        // match can go through both. The 2020 grammar refuses a name used twice, wherever the groups are. The last
        // group with the name is the only one to ask: the groups before it lie in alternatives apart from it, of a
        // disjunction this group is either outside, where its alternatives are all one to this group, or inside, in
        // an alternative after theirs.
        let hash = self.name_hash(start, stop);
        let found = self.find_name(start, stop, hash);
        if let Ok(slot) = found {
            let last = self.names_table.slots()[slot] - 1;
            let (path_start, path_len) = (self.names.items()[last + 2], self.names.items()[last + 3]);
            if self.es2020 || !self.separate_alternatives(path_start, path_len) {
                return None;
            }
        }
        let entry = self.names.len;
        for v in [start, stop, self.name_paths.len, self.path.len, hash] {
            self.names.push(v);
        }
        for k in 0..self.path.len {
            let v = self.path.items()[k];
            self.name_paths.push(v);
        }
        match found {
            Ok(slot) => self.names_table.slots_mut()[slot] = entry + 1,
            Err(slot) => self.add_name(slot, entry),
        }
        Some(KIND_GROUP)
    }

    /// Whether a named group (by the start and the length of its path) and the current position lie in different
    /// alternatives of some disjunction.
    fn separate_alternatives(&self, other_start: usize, other_len: usize) -> bool {
        let (other, path) = (&self.name_paths.items()[other_start..other_start + other_len], self.path.items());
        for (x, y) in other.as_chunks::<2>().0.iter().zip(path.as_chunks::<2>().0) {
            if x[0] != y[0] {
                break;
            }
            if x[1] != y[1] {
                return true;
            }
        }
        false
    }

    /// Reads the modifiers of a group such as `(?i:...)` or `(?s-i:...)`, after the `(?`, to its `:`.
    fn modifier_group(&mut self) -> bool {
        let mut seen = 0;
        let mut removing = false;
        while !self.eat(':') {
            let c = self.peek();
            if c == ch('-') {
                if removing {
                    return false;
                }
                self.next();
                removing = true;
                continue;
            }
            let flag = match u8::try_from(c).unwrap_or(0) {
                b'i' => 1,
                b'm' => 2,
                b's' => 4,
                _ => 0,
            };
            if flag == 0 || seen & flag != 0 {
                return false;
            }
            self.next();
            seen |= flag;
        }
        seen != 0
    }

    /// Reads a group name up to and including its `>`, and gives the positions of its first character and of the
    /// `>`. A character of the name may be written as a Unicode escape.
    fn group_name(&mut self) -> Option<(usize, usize)> {
        let start = self.i;
        let mut count = 0;
        loop {
            if self.peek() == ch('>') {
                let stop = self.i;
                self.next();
                return (count != 0).then_some((start, stop));
            }
            if !self.more() {
                return None;
            }
            let c = self.name_character()?;
            let valid = if count == 0 {
                c == ch('$')
                    || c == ch('_')
                    || self
                        .ucd
                        .id_start
                        .get_or_init(|| self.ucd.binary_table("ID_Start").expect("ID_Start"))
                        .contains(c)
            } else {
                c == ch('$')
                    || c == 0x200C
                    || c == 0x200D
                    || self
                        .ucd
                        .id_continue
                        .get_or_init(|| self.ucd.binary_table("ID_Continue").expect("ID_Continue"))
                        .contains(c)
            };
            if !valid {
                return None;
            }
            count += 1;
        }
    }

    /// Reads one character of a group name: a code point, written or as a Unicode escape.
    fn name_character(&mut self) -> Option<Rune> {
        let c = self.next();
        if c == ch('\\') {
            if !self.eat('u') {
                return None;
            }
            let u = self.unicode_escape(true);
            return (u >= 0).then_some(u);
        }
        if (0xD800..=0xDBFF).contains(&c) && (0xDC00..=0xDFFF).contains(&self.peek()) {
            // Without the u flag the pattern is code units, and a name still takes a surrogate pair as one character.
            let low = self.next();
            return Some(0x10000 + ((c - 0xD800) << 10) + (low - 0xDC00));
        }
        Some(c)
    }

    /// The hash of a name, by the positions of its characters, over its code points.
    fn name_hash(&mut self, start: usize, stop: usize) -> usize {
        let saved = self.i;
        let mut hash: u32 = 2166136261;
        self.i = start;
        while self.i < stop {
            hash = (hash ^ self.name_character().unwrap_or(-1) as u32).wrapping_mul(16777619);
        }
        self.i = saved;
        hash as usize
    }

    /// Finds a name in the table: `Ok` with the slot that holds its last group, or `Err` with the empty slot it
    /// would take.
    fn find_name(&mut self, start: usize, stop: usize, hash: usize) -> Result<usize, usize> {
        let mask = self.names_table.slots().len() - 1;
        let mut slot = hash & mask;
        loop {
            let held = self.names_table.slots()[slot];
            if held == 0 {
                return Err(slot);
            }
            let n = held - 1;
            let (other_start, other_stop, other_hash) =
                (self.names.items()[n], self.names.items()[n + 1], self.names.items()[n + 4]);
            if other_hash == hash && self.same_name(start, stop, other_start, other_stop) {
                return Ok(slot);
            }
            slot = (slot + 1) & mask;
        }
    }

    /// Puts a new name's group in the empty slot found for it, and doubles the table when it is half full.
    fn add_name(&mut self, slot: usize, entry: usize) {
        self.names_table.slots_mut()[slot] = entry + 1;
        self.distinct_names += 1;
        let len = self.names_table.slots().len();
        if self.distinct_names * 2 <= len {
            return;
        }
        let mut table = vec![0; len * 2];
        let mask = table.len() - 1;
        for &held in self.names_table.slots() {
            if held != 0 {
                let mut s = self.names.items()[held - 1 + 4] & mask;
                while table[s] != 0 {
                    s = (s + 1) & mask;
                }
                table[s] = held;
            }
        }
        self.names_table.heap = table;
    }

    /// Whether two names, by the positions of their characters, are the same code points.
    fn same_name(&mut self, a_start: usize, a_stop: usize, b_start: usize, b_stop: usize) -> bool {
        let saved = self.i;
        let (mut a, mut b) = (a_start, b_start);
        let same = loop {
            let (a_done, b_done) = (a >= a_stop, b >= b_stop);
            if a_done || b_done {
                break a_done && b_done;
            }
            self.i = a;
            let ca = self.name_character();
            a = self.i;
            self.i = b;
            let cb = self.name_character();
            b = self.i;
            if ca != cb {
                break false;
            }
        };
        self.i = saved;
        same
    }

    /// Reads an escape outside a class: whether a quantifier may follow it, or `None` when it is not valid.
    fn atom_escape(&mut self) -> Option<bool> {
        self.next();
        if !self.more() {
            return None;
        }
        let c = self.peek();
        if c == ch('b') || c == ch('B') {
            self.next();
            return Some(false);
        }
        if c == ch('k') {
            self.next();
            // Annex B reads \k as the letter k in a pattern with no named group.
            if !self.unicode && !self.has_names {
                return Some(true);
            }
            if !self.eat('<') {
                return None;
            }
            let (start, stop) = self.group_name()?;
            self.references.push(start);
            self.references.push(stop);
            return Some(true);
        }
        if (ch('1')..=ch('9')).contains(&c) {
            let start = self.i;
            if self.number_value() <= self.group_count {
                return Some(true);
            }
            if self.unicode {
                return None;
            }
            // Annex B reads it as an octal escape, or as the digit itself.
            self.i = start;
            self.legacy_octal();
            return Some(true);
        }
        match self.class_escape(c) {
            Escape::Valid(_) => Some(true),
            Escape::Invalid => None,
            Escape::None => (self.character_escape(false) != INVALID).then_some(true),
        }
    }

    /// Reads an Annex B octal escape of up to three digits with a value below 256. A digit that cannot start one (8
    /// or 9) stands for itself.
    fn legacy_octal(&mut self) -> Rune {
        let mut v: Rune = 0;
        let mut k = 0;
        while k < 3
            && self.more()
            && (ch('0')..=ch('7')).contains(&self.peek())
            && v * 8 + (self.peek() - ch('0')) <= 0o377
        {
            v = v * 8 + (self.next() - ch('0'));
            k += 1;
        }
        if k == 0 { self.next() } else { v }
    }

    /// Reads a class escape such as `\d` or `\p{...}` whose letter is `c`. It reads nothing when `c` does not start
    /// one.
    fn class_escape(&mut self, c: Rune) -> Escape {
        match u8::try_from(c).unwrap_or(0) {
            b'd' | b'D' | b'w' | b'W' | b's' | b'S' => {
                self.next();
                Escape::Valid(false)
            }
            b'p' | b'P' => {
                // Without the u flag \p is the letter p.
                if !self.unicode {
                    return Escape::None;
                }
                self.next();
                if !self.eat('{') {
                    return Escape::Invalid;
                }
                let start = self.i;
                while self.more() && self.peek() != ch('}') {
                    self.next();
                }
                let stop = self.i;
                if !self.eat('}') {
                    return Escape::Invalid;
                }
                // A name ECMA-262 defines is ASCII, so an expression with any other character names nothing.
                let Ok(expr) = std::str::from_utf8(&self.bytes[start >> 1..stop >> 1]) else { return Escape::Invalid };
                if !expr.is_ascii() {
                    return Escape::Invalid;
                }
                // The 2020 grammar knows only the names and values of its own tables, whatever the version of Unicode.
                let found = (!self.es2020 || es2020::has(expr)) && has_property(self.ucd, expr);
                if !found && self.sets && c == ch('p') && has_string_property(self.ucd, expr) {
                    // A property of strings, which the v flag alone names, and which has no complement.
                    return Escape::Valid(true);
                }
                if found { Escape::Valid(false) } else { Escape::Invalid }
            }
            _ => Escape::None,
        }
    }

    /// Reads exactly `n` hexadecimal digits. It returns -1, reading nothing, when they are not there.
    fn hex(&mut self, n: usize) -> Rune {
        let mut pos = self.i;
        let mut v: Rune = 0;
        for _ in 0..n {
            if pos >= self.end {
                return -1;
            }
            let (c, next) = self.at(pos);
            let d = hex_value(c);
            if d < 0 {
                return -1;
            }
            v = v * 16 + d;
            pos = next;
        }
        self.i = pos;
        v
    }

    /// Reads the escape after a backslash, in or out of a class: the character it names, or `INVALID`.
    fn character_escape(&mut self, in_class: bool) -> Rune {
        let at = self.i;
        let c = self.next();
        match u8::try_from(c).unwrap_or(0) {
            b'f' => return 0x0C,
            b'n' => return 0x0A,
            b'r' => return 0x0D,
            b't' => return 0x09,
            b'v' => return 0x0B,
            b'c' => {
                let l = self.peek();
                if (ch('a')..=ch('z')).contains(&l) || (ch('A')..=ch('Z')).contains(&l) {
                    self.next();
                    return l % 32;
                }
                if !self.unicode && in_class && (is_digit(l) || l == ch('_')) {
                    self.next();
                    return l % 32;
                }
                if self.unicode {
                    return INVALID;
                }
                // Annex B reads a \c that is not a control escape as a backslash followed by the letter c.
                self.i = at;
                return ch('\\');
            }
            b'0' => {
                if self.more() && is_digit(self.peek()) {
                    if self.unicode {
                        return INVALID;
                    }
                    self.i = at;
                    return self.legacy_octal();
                }
                return 0;
            }
            b'x' => {
                let h = self.hex(2);
                return if h >= 0 {
                    h
                } else if self.unicode {
                    INVALID
                } else {
                    ch('x')
                };
            }
            b'u' => {
                let u = self.unicode_escape(self.unicode);
                return if u >= 0 {
                    u
                } else if self.unicode {
                    INVALID
                } else {
                    ch('u')
                };
            }
            _ => {}
        }
        if (ch('1')..=ch('9')).contains(&c) && !self.unicode && in_class {
            self.i = at;
            return self.legacy_octal();
        }
        // Identity escapes. The u flag allows only the syntax characters and '/', and '-' in a class. Annex B allows
        // any character, but k in a pattern with a named group.
        if (self.unicode && !is_one_of(c, b"^$\\.*+?()[]{}|/") && !(in_class && c == ch('-')))
            || (c == ch('k') && self.has_names)
        {
            return INVALID;
        }
        c
    }

    /// Reads what follows the u of a Unicode escape and returns the character, or -1, reading nothing, when it is not
    /// a well-formed escape. With `unicode_mode`, as with the u flag and always in a group name, it accepts the form
    /// with braces and reads a surrogate pair written as two escapes as one code point.
    fn unicode_escape(&mut self, unicode_mode: bool) -> Rune {
        let start = self.i;
        if unicode_mode && self.eat('{') {
            let digits = self.i;
            let mut v: Rune = 0;
            while self.more() && hex_value(self.peek()) >= 0 {
                if v <= MAX_RUNE {
                    v = v * 16 + hex_value(self.peek());
                }
                self.next();
            }
            if digits == self.i || !self.eat('}') || v > MAX_RUNE {
                self.i = start;
                return -1;
            }
            return v;
        }
        let u = self.hex(4);
        if u < 0 {
            return -1;
        }
        if unicode_mode && (0xD800..=0xDBFF).contains(&u) && self.peek() == ch('\\') && self.peek_second() == ch('u') {
            let save = self.i;
            self.next();
            self.next();
            let low = self.hex(4);
            if (0xDC00..=0xDFFF).contains(&low) {
                return 0x10000 + ((u - 0xD800) << 10) + (low - 0xDC00);
            }
            self.i = save;
        }
        u
    }

    /// Reads a character class from its `[` to its `]`.
    fn class(&mut self) -> bool {
        self.next();
        self.eat('^');
        loop {
            if !self.more() {
                return false;
            }
            if self.eat(']') {
                return true;
            }
            let (first, first_set) = self.class_atom();
            if first == INVALID {
                return false;
            }
            if self.peek() == ch('-') && self.peek_second() >= 0 && self.peek_second() != ch(']') {
                self.next();
                let (second, second_set) = self.class_atom();
                if second == INVALID {
                    return false;
                }
                if first_set || second_set {
                    // Annex B reads the '-' as itself when a class escape is at either end.
                    if self.unicode {
                        return false;
                    }
                    continue;
                }
                if second < first {
                    return false;
                }
            }
        }
    }

    /// Reads one member of a class: its character, or 0 and `true` for a class escape, or `INVALID`.
    fn class_atom(&mut self) -> (Rune, bool) {
        let c = self.next();
        if c != ch('\\') {
            return (c, false);
        }
        if !self.more() {
            return (INVALID, false);
        }
        let e = self.peek();
        if e == ch('b') {
            self.next();
            return (0x08, false);
        }
        match self.class_escape(e) {
            Escape::Valid(_) => (0, true),
            Escape::Invalid => (INVALID, true),
            Escape::None => (self.character_escape(true), false),
        }
    }

    // ---------------------------------------------------------------------------------------------------------------
    // The v flag.

    /// Reads a class of the v flag's grammar from its `[` to its `]`, with its nested classes on a stack of frames: a
    /// union of elements, or the intersection or the difference of operands, which may not be mixed. A negated class
    /// may not hold anything that may match a string.
    fn class_set_expression(&mut self) -> bool {
        self.open_class();
        loop {
            let top = self.classes.len - CLASS_FRAME;
            match self.classes.items()[top + 1] {
                PHASE_START => {
                    if self.peek() == ch(']') {
                        self.classes.items_mut()[top + 3] = 0;
                        match self.close_class() {
                            None => return false,
                            Some(true) => return true,
                            Some(false) => continue,
                        }
                    }
                    match self.class_set_element() {
                        Element::Invalid => return false,
                        Element::Nested => self.classes.items_mut()[top + 1] = WAITING_FIRST,
                        Element::Operand { strings, range } => {
                            if !self.first_operand(top, strings, range) {
                                return false;
                            }
                        }
                    }
                }
                PHASE_UNION => {
                    if self.more() && self.peek() != ch(']') {
                        if self.set_operator() != 0 {
                            return false;
                        }
                        match self.class_set_element() {
                            Element::Invalid => return false,
                            Element::Nested => self.classes.items_mut()[top + 1] = WAITING_UNION,
                            Element::Operand { strings, .. } => {
                                if strings {
                                    self.classes.items_mut()[top + 3] = 1;
                                }
                            }
                        }
                        continue;
                    }
                    match self.close_class() {
                        None => return false,
                        Some(true) => return true,
                        Some(false) => {}
                    }
                }
                PHASE_OPERANDS => {
                    if self.set_operator() == self.classes.items()[top + 2] as Rune {
                        self.next();
                        self.next();
                        if self.peek() == ch(']') || !self.more() {
                            return false;
                        }
                        match self.class_set_element() {
                            Element::Invalid | Element::Operand { range: true, .. } => return false,
                            Element::Nested => self.classes.items_mut()[top + 1] = WAITING_OPERAND,
                            Element::Operand { strings, .. } => self.operand(top, strings),
                        }
                        continue;
                    }
                    if self.peek() != ch(']') {
                        return false;
                    }
                    match self.close_class() {
                        None => return false,
                        Some(true) => return true,
                        Some(false) => {}
                    }
                }
                // A nested class is open above this one; it is never the top while it waits.
                _ => return false,
            }
        }
    }

    fn open_class(&mut self) {
        self.next();
        let negated = self.eat('^');
        for v in [negated as usize, PHASE_START, 0, 0] {
            self.classes.push(v);
        }
    }

    /// Closes the class at the top at its `]`, and gives what it may contain to the class it is nested in, if any:
    /// `Some(true)` when it was the outermost, `None` when it is not valid.
    fn close_class(&mut self) -> Option<bool> {
        if !self.eat(']') {
            return None;
        }
        let top = self.classes.len - CLASS_FRAME;
        let negated = self.classes.items()[top] != 0;
        let strings = self.classes.items()[top + 3] != 0;
        if negated && strings {
            return None;
        }
        self.classes.truncate(top);
        if self.classes.len == 0 {
            return Some(true);
        }
        // A nested class is an operand, never a range; a negated one may contain no strings.
        let strings = strings && !negated;
        let parent = self.classes.len - CLASS_FRAME;
        match self.classes.items()[parent + 1] {
            WAITING_FIRST => self.first_operand(parent, strings, false).then_some(false),
            WAITING_UNION => {
                self.classes.items_mut()[parent + 1] = PHASE_UNION;
                if strings {
                    self.classes.items_mut()[parent + 3] = 1;
                }
                Some(false)
            }
            _ => {
                self.classes.items_mut()[parent + 1] = PHASE_OPERANDS;
                self.operand(parent, strings);
                Some(false)
            }
        }
    }

    /// Takes the first element of a class, and decides from what follows it whether the class is a union or
    /// operations.
    fn first_operand(&mut self, top: usize, strings: bool, range: bool) -> bool {
        self.classes.items_mut()[top + 3] = strings as usize;
        let op = self.set_operator();
        if op != 0 {
            // An intersection or a difference: operands only, with the one operator between every two.
            if range {
                return false;
            }
            self.classes.items_mut()[top + 1] = PHASE_OPERANDS;
            self.classes.items_mut()[top + 2] = op as usize;
            return true;
        }
        self.classes.items_mut()[top + 1] = PHASE_UNION;
        true
    }

    /// Takes an operand of an intersection (strings only where both may hold them) or a difference (the first's).
    fn operand(&mut self, top: usize, strings: bool) {
        if self.classes.items()[top + 2] == '&' as usize && !strings {
            self.classes.items_mut()[top + 3] = 0;
        }
    }

    /// The set operator at the position (`&` for `&&`, `-` for `--`), or 0.
    fn set_operator(&self) -> Rune {
        let c = self.peek();
        if (c == ch('&') || c == ch('-')) && self.peek_second() == c { c } else { 0 }
    }

    /// Reads one element of a class of the v flag's grammar.
    fn class_set_element(&mut self) -> Element {
        let c = self.peek();
        if c == ch('[') {
            self.open_class();
            return Element::Nested;
        }
        if c == ch('\\') {
            let e = self.peek_second();
            if e < 0 {
                return Element::Invalid;
            }
            if e == ch('q') {
                self.next();
                self.next();
                return match self.class_string_disjunction() {
                    Some(strings) => Element::Operand { strings, range: false },
                    None => Element::Invalid,
                };
            }
            let save = self.i;
            self.next();
            match self.class_escape(e) {
                Escape::Invalid => return Element::Invalid,
                Escape::Valid(strings) => return Element::Operand { strings, range: false },
                Escape::None => self.i = save,
            }
        }
        let first = self.class_set_character();
        if first == INVALID {
            return Element::Invalid;
        }
        if self.peek() == ch('-') && self.set_operator() == 0 {
            self.next();
            let p = self.peek();
            if p == ch(']') || p == ch('[') || (p == ch('\\') && is_one_of(self.peek_second(), b"qdDwWsSpP")) {
                return Element::Invalid;
            }
            let second = self.class_set_character();
            if second == INVALID || second < first {
                return Element::Invalid;
            }
            return Element::Operand { strings: false, range: true };
        }
        Element::Operand { strings: false, range: false }
    }

    /// Reads one character of a class of the v flag's grammar: a character that is not a syntax character and does
    /// not begin a reserved double punctuator, or an escape, which may be of a reserved punctuator.
    fn class_set_character(&mut self) -> Rune {
        if !self.more() {
            return INVALID;
        }
        let c = self.peek();
        if c == ch('\\') {
            let e = self.peek_second();
            if e < 0 {
                return INVALID;
            }
            if e == ch('b') {
                self.next();
                self.next();
                return 0x08;
            }
            if is_one_of(e, b"&-!#%,:;<=>@`~") {
                self.next();
                self.next();
                return e;
            }
            self.next();
            return self.character_escape(true);
        }
        if is_one_of(c, b"()[]{}/-\\|") {
            return INVALID;
        }
        if self.peek_second() == c && is_one_of(c, b"&!#$%*+,.:;<=>?@^`~") {
            return INVALID;
        }
        self.next();
        c
    }

    /// Reads the strings of `\q{...}` after its `\q`: whether any is of other than one character, or `None` when it is
    /// not valid.
    fn class_string_disjunction(&mut self) -> Option<bool> {
        if !self.eat('{') {
            return None;
        }
        let mut strings = false;
        let mut count = 0;
        loop {
            if !self.more() {
                return None;
            }
            let c = self.peek();
            if c == ch('|') || c == ch('}') {
                self.next();
                if count != 1 {
                    strings = true;
                }
                count = 0;
                if c == ch('}') {
                    return Some(strings);
                }
                continue;
            }
            if self.class_set_character() == INVALID {
                return None;
            }
            count += 1;
        }
    }
}

/// What a class escape was.
enum Escape {
    /// None: the letter does not start one, and nothing was read.
    None,
    Invalid,
    /// A class escape, with whether it is a property of strings.
    Valid(bool),
}

/// One element of a class of the v flag's grammar.
enum Element {
    Invalid,
    /// A nested class, whose frame is now open.
    Nested,
    /// An operand, or a range of two characters, with whether it may hold strings.
    Operand {
        strings: bool,
        range: bool,
    },
}
