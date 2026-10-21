//! An anchored, leftmost-first automaton over ASCII text, which finds where the match that starts at a position ends
//! for a pattern that needs no backtracking: what the `regex` crate's DFAs do for the match, taken into this engine
//! where ECMA-262's semantics allow it.
//!
//! A state is an ordered list of the pattern's states (its threads, the most preferred first, as the backtracking
//! matcher would try them), with whether the unit before it was a word character and whether it is at the start of
//! the text. When the match state is among the threads, a match ends here, and the threads after it, which the
//! backtracking matcher would only try had this way failed, are dropped; the threads before it may still find a
//! longer match, which it would prefer. So the last match found before the threads run out is the backtracking
//! matcher's match, for a pattern with no lookaround, no backreference and no repetition whose body can match the
//! empty string (where ECMA-262's rule for an empty iteration would differ from a thread's). A unit beyond ASCII hands
//! the start back to the backtracking matcher.
//!
//! Every state is worked out when the pattern is compiled, within bounds of states and steps, so that a pattern too
//! large for an automaton costs little before that is known.

use std::collections::HashMap;

use super::charset::is_word_byte;
use super::parse::{Node, NodeKind, UNBOUNDED};

const MAX_STATES: usize = 2048;
const MAX_NFA: usize = 1024;
const MAX_STEPS: i64 = 2_000_000;
/// No thread is left: no match can end further on.
const DEAD: u16 = 0;
/// The flag of a transition across which a match ends before the unit.
const MATCHED: u16 = 0x8000;
/// The same flag in the premultiplied table.
const FAST_MATCHED: u32 = 0x8000_0000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Set,
    Split,
    Bol,
    Eol,
    Word,
    NotWord,
    Match,
}

#[derive(Clone, Copy)]
struct State {
    kind: Kind,
    set: [u64; 2],
    /// Whether the set holds a character beyond ASCII, which the automaton gives up at.
    wide: bool,
    out: usize,
    alt: usize,
}

impl State {
    fn new(kind: Kind, out: usize) -> State {
        State { kind, set: [0; 2], wide: false, out, alt: 0 }
    }
}

struct Builder {
    states: Vec<State>,
    failed: bool,
}

impl Builder {
    fn add(&mut self, s: State) -> usize {
        if self.states.len() >= MAX_NFA {
            self.failed = true;
            return 0;
        }
        self.states.push(s);
        self.states.len() - 1
    }

    /// A split whose preferred way is `first`.
    fn split(&mut self, first: usize, second: usize) -> usize {
        self.add(State { alt: second, ..State::new(Kind::Split, first) })
    }

    /// The states of a node that go on to `next`; the state they start at.
    fn compile(&mut self, n: &Node, mut next: usize) -> usize {
        if self.failed {
            return 0;
        }
        match n.kind {
            NodeKind::Empty => next,
            NodeKind::Char => {
                let mut set = [0u64; 2];
                if (0..128).contains(&n.r) {
                    set[(n.r >> 6) as usize] = 1 << (n.r & 63);
                }
                self.add(State { set, wide: n.r >= 128, ..State::new(Kind::Set, next) })
            }
            NodeKind::Set => {
                let wide = n.class().ranges.last().is_some_and(|&hi| hi >= 128);
                self.add(State { set: n.class().ascii, wide, ..State::new(Kind::Set, next) })
            }
            NodeKind::Cat => {
                for sub in n.subs.iter().rev() {
                    next = self.compile(sub, next);
                }
                next
            }
            NodeKind::Alt => {
                let mut start = self.compile(&n.subs[n.subs.len() - 1], next);
                for sub in n.subs[..n.subs.len() - 1].iter().rev() {
                    let out = self.compile(sub, next);
                    start = self.split(out, start);
                }
                start
            }
            NodeKind::Group => self.compile(&n.subs[0], next),
            NodeKind::Repeat => {
                let sub = &n.subs[0];
                if n.max != 0 && n.max != 1 && super::compile::nullable_node(sub) {
                    // ECMA-262 refuses an empty iteration after the least count, which a thread does not know.
                    self.failed = true;
                    return 0;
                }
                if n.max == UNBOUNDED {
                    let loop_ = self.split(0, 0);
                    if self.failed {
                        return 0;
                    }
                    let body = self.compile(sub, loop_);
                    let s = &mut self.states[loop_];
                    (s.out, s.alt) = if n.lazy { (next, body) } else { (body, next) };
                    next = loop_;
                } else {
                    let mut i = n.min;
                    while i < n.max && !self.failed {
                        let body = self.compile(sub, next);
                        next = if n.lazy { self.split(next, body) } else { self.split(body, next) };
                        i += 1;
                    }
                }
                let mut i = 0;
                while i < n.min && !self.failed {
                    next = self.compile(sub, next);
                    i += 1;
                }
                next
            }
            NodeKind::Bol => self.add(State::new(Kind::Bol, next)),
            NodeKind::Eol => self.add(State::new(Kind::Eol, next)),
            NodeKind::WordBoundary if !n.fold => self.add(State::new(Kind::Word, next)),
            NodeKind::NotWordBoundary if !n.fold => self.add(State::new(Kind::NotWord, next)),
            _ => {
                self.failed = true;
                0
            }
        }
    }
}

/// The anchored automaton of a pattern.
#[derive(Clone, Debug)]
pub(crate) struct Anchored {
    class: [u8; 128],
    shift: u32,
    /// `fast[state * stride + class]`: the next state, premultiplied by the stride, with `FAST_MATCHED` when a match
    /// ends before the unit.
    fast: Vec<u32>,
    /// For each state: a match ends at the end of the text.
    at_end: Vec<bool>,
    /// The start state for each of: the unit before is a word character (bit 1), the start of the text (bit 0).
    starts: [u16; 4],
    /// Whether the start states differ (the pattern looks back at the unit before or at the start of the text).
    context: bool,
    /// For the forward automaton (see [`build_forward`]), whether each state is a start state: no thread of an
    /// earlier start is alive in it.
    start_like: Vec<bool>,
}

/// What the forward automaton found (see [`Anchored::scan`]).
pub(crate) enum Scan {
    /// A match ends here.
    Match(usize),
    /// A start state is reached here: no match starts before it.
    Idle(usize),
    /// A unit beyond ASCII is here.
    Bail(usize),
    /// No match ends before the end of the text.
    None,
}

/// What the automaton found from a start.
pub(crate) enum Found {
    /// The match ends here.
    End(usize),
    /// No match starts here.
    None,
    /// A unit beyond ASCII was reached: the backtracking matcher decides.
    Bail,
}

impl Anchored {
    /// For the forward automaton (see [`build_forward`]): the first place at or after `from` where a match ends, read
    /// from a start state at `from`, with the last start state's position before it (the leftmost match starts in
    /// between). `Scan::Idle(p)` when a start state is reached again at `p` first, `Scan::Bail(p)` at a unit beyond
    /// ASCII at `p`, `Scan::None` at the end of the text with no match.
    #[inline]
    pub(crate) fn scan<U: Copy + Into<u32>>(&self, input: &[U], from: usize) -> Scan {
        let mut row = (self.start(input, from) as usize) << self.shift;
        let (trans, class) = (&self.fast[..], &self.class);
        let mut p = from;
        while p < input.len() {
            let c: u32 = input[p].into();
            if c >= 0x80 {
                return Scan::Bail(p);
            }
            // SAFETY: as in `find`.
            let t = unsafe { *trans.get_unchecked(row + class[c as usize] as usize) };
            if t & FAST_MATCHED != 0 {
                return Scan::Match(p);
            }
            row = t as usize;
            p += 1;
            if self.start_like[row >> self.shift] {
                return Scan::Idle(p);
            }
        }
        if self.at_end[row >> self.shift] { Scan::Match(p) } else { Scan::None }
    }

    /// The start state at `start`: where the pattern has no boundary or anchor that looks back, the one start state.
    #[inline]
    fn start<U: Copy + Into<u32>>(&self, input: &[U], start: usize) -> u16 {
        if self.context {
            let word = start > 0 && {
                let c: u32 = input[start - 1].into();
                c < 0x80 && is_word_byte(c as u8)
            };
            self.starts[usize::from(word) << 1 | usize::from(start == 0)]
        } else {
            self.starts[0]
        }
    }

    /// Where the match that starts at `start` ends, over units that are ASCII bytes or UTF-16 code units.
    #[inline]
    pub(crate) fn find<U: Copy + Into<u32>>(&self, input: &[U], start: usize) -> Found {
        self.find_reach(input, start).0
    }

    /// [`Anchored::find`], with how far it read: the unit at which every thread had died, or the end of the text.
    #[inline]
    pub(crate) fn find_reach<U: Copy + Into<u32>>(&self, input: &[U], start: usize) -> (Found, usize) {
        let s = self.start(input, start);
        if s == DEAD {
            return (Found::None, start);
        }
        let mut last = None;
        let mut p = start;
        let (trans, class) = (&self.fast[..], &self.class);
        // The state is kept premultiplied by the stride, so a step is one load and an add.
        let mut at = (s as usize) << self.shift;
        while p < input.len() {
            let c: u32 = input[p].into();
            if c >= 0x80 {
                return (Found::Bail, p);
            }
            // SAFETY: `at` is a state's row (a multiple of the stride below the table's length, as every entry of
            // the table is) and a class is below the stride.
            let t = unsafe { *trans.get_unchecked(at + class[c as usize] as usize) };
            if t & FAST_MATCHED != 0 {
                last = Some(p);
            }
            at = (t & !FAST_MATCHED) as usize;
            if at == 0 {
                return (last.map_or(Found::None, Found::End), p);
            }
            p += 1;
        }
        if self.at_end[at >> self.shift] {
            return (Found::End(p), p);
        }
        (last.map_or(Found::None, Found::End), p)
    }
}

impl Anchored {
    /// The state after `prefix` from each start state, premultiplied by the stride (0 where no match can start with
    /// it), or `None` when a match can end inside it or it holds a byte beyond ASCII: what `find_after` goes on from
    /// where a match is known to start with `prefix`.
    pub(crate) fn after(&self, prefix: &[u8]) -> Option<[u32; 4]> {
        let mut rows = [0u32; 4];
        for (row, &s) in rows.iter_mut().zip(&self.starts) {
            let mut at = (s as usize) << self.shift;
            for &c in prefix {
                if at == 0 {
                    // No match from this start state begins with the prefix.
                    break;
                }
                if c >= 0x80 {
                    return None;
                }
                let t = self.fast[at + self.class[c as usize] as usize];
                if t & FAST_MATCHED != 0 {
                    return None;
                }
                at = t as usize;
            }
            *row = at as u32;
        }
        Some(rows)
    }

    /// [`Anchored::find`] from `start` where the units from `start` to `from` are a prefix whose states `after` gave.
    #[inline]
    pub(crate) fn find_after<U: Copy + Into<u32>>(
        &self,
        input: &[U],
        start: usize,
        from: usize,
        rows: &[u32; 4],
    ) -> Found {
        let row = if self.context {
            let word = start > 0 && {
                let c: u32 = input[start - 1].into();
                c < 0x80 && is_word_byte(c as u8)
            };
            rows[usize::from(word) << 1 | usize::from(start == 0)]
        } else {
            rows[0]
        };
        if row == 0 {
            return Found::None;
        }
        let mut last = None;
        let mut p = from;
        let (trans, class) = (&self.fast[..], &self.class);
        let mut at = row as usize;
        while p < input.len() {
            let c: u32 = input[p].into();
            if c >= 0x80 {
                return Found::Bail;
            }
            // SAFETY: as in `find_reach`.
            let t = unsafe { *trans.get_unchecked(at + class[c as usize] as usize) };
            if t & FAST_MATCHED != 0 {
                last = Some(p);
            }
            at = (t & !FAST_MATCHED) as usize;
            if at == 0 {
                return last.map_or(Found::None, Found::End);
            }
            p += 1;
        }
        if self.at_end[at >> self.shift] {
            return Found::End(p);
        }
        last.map_or(Found::None, Found::End)
    }

    /// Whether any match starts at `start`, found at the first place one ends; `None` when a unit beyond ASCII is
    /// reached first.
    #[inline]
    pub(crate) fn matches_at<U: Copy + Into<u32>>(&self, input: &[U], start: usize) -> Option<bool> {
        let s = self.start(input, start);
        if s == DEAD {
            return Some(false);
        }
        let (trans, class) = (&self.fast[..], &self.class);
        let mut at = (s as usize) << self.shift;
        for &u in &input[start..] {
            let c: u32 = u.into();
            if c >= 0x80 {
                return None;
            }
            // SAFETY: as in `find`.
            let t = unsafe { *trans.get_unchecked(at + class[c as usize] as usize) };
            if t & FAST_MATCHED != 0 {
                return Some(true);
            }
            at = t as usize;
            if at == 0 {
                return Some(false);
            }
        }
        Some(self.at_end[at >> self.shift])
    }
}

struct Det<'a> {
    nfa: &'a [State],
    mark: Vec<u32>,
    generation: u32,
    steps: i64,
}

impl Det<'_> {
    /// Appends to `out`, in order of preference, the threads that reading nothing reaches from `from`, keeping the
    /// states that read a unit, the anchors and boundaries still to pass, and the match, after which nothing more is
    /// kept. Boundaries are passed or dropped with `words`, and kept to pass later without it.
    fn closure(
        &mut self,
        out: &mut Vec<usize>,
        from: &[usize],
        at_start: bool,
        at_end: bool,
        words: Option<(bool, bool)>,
    ) {
        self.generation += 1;
        self.closure_on(out, from, at_start, at_end, words);
    }

    /// [`Det::closure`] after the closure just made, leaving out the states it reached.
    fn closure_on(
        &mut self,
        out: &mut Vec<usize>,
        from: &[usize],
        at_start: bool,
        at_end: bool,
        words: Option<(bool, bool)>,
    ) {
        let mut stack: Vec<usize> = from.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            self.steps -= 1;
            if self.mark[i] == self.generation {
                continue;
            }
            self.mark[i] = self.generation;
            let s = &self.nfa[i];
            match s.kind {
                Kind::Split => {
                    stack.push(s.alt);
                    stack.push(s.out);
                }
                Kind::Bol => {
                    if at_start {
                        stack.push(s.out);
                    }
                }
                Kind::Eol => {
                    if at_end {
                        stack.push(s.out);
                    } else {
                        out.push(i);
                    }
                }
                Kind::Word | Kind::NotWord => match words {
                    None => out.push(i),
                    Some((before, after)) => {
                        if (before != after) == (s.kind == Kind::Word) {
                            stack.push(s.out);
                        }
                    }
                },
                Kind::Match => {
                    out.push(i);
                    return;
                }
                Kind::Set => out.push(i),
            }
        }
    }
}

fn is_word(c: usize) -> bool {
    is_word_byte(c as u8)
}

/// The positions of a pattern read before a match can end: V8's Boyer-Moore lookahead (`BoyerMooreLookahead` in its
/// regexp compiler). For a window of positions `lo..=hi` of every match, the units each can be (`table` for ASCII,
/// `wide` for any other) are joined; a unit at `hi` from a place that is none of them rules out the starts from that
/// place to `hi - lo` units on, so the search reads one unit in `hi - lo + 1` where the window's units are rare.
#[derive(Clone, Debug)]
pub(crate) struct Skip {
    /// The window's last position, the one read.
    pub(crate) hi: usize,
    /// The starts one unit rules out, `hi - lo + 1`.
    pub(crate) len: usize,
    table: [bool; 128],
    wide: bool,
}

impl Skip {
    /// Whether the unit can be at some position of the window.
    #[inline(always)]
    pub(crate) fn hit(&self, c: u32) -> bool {
        if c < 128 { self.table[c as usize] } else { self.wide }
    }
}

/// The most positions looked at for a window.
const SKIP_POSITIONS: usize = 16;

/// How often a unit is found in text, roughly: what a window's worth is reckoned by.
fn frequency(c: usize) -> f64 {
    match c as u8 {
        b' ' => 0.15,
        b'a'..=b'z' => 0.06,
        b'\n' | b'\r' | b'.' | b',' => 0.015,
        b'A'..=b'Z' | b'0'..=b'9' => 0.004,
        _ => 0.003,
    }
}

/// The window of a tree (after `simplify`) worth reading one unit of instead of every unit, or `None`: the units each
/// position before a match can end can be, from the pattern's automaton without its determinisation, and the window
/// whose units are rare enough that one unit read passes over three starts or more, reckoned by `frequency`.
pub(crate) fn skip_window(root: &Node) -> Option<Skip> {
    let mut b = Builder { states: Vec::new(), failed: false };
    let match_ = b.add(State::new(Kind::Match, 0));
    let start = b.compile(root, match_);
    if b.failed {
        return None;
    }
    let nfa = &b.states[..];
    // The states each depth's threads are at, assertions taken as passed (they only rule threads out).
    let closure = |from: &[usize]| -> Option<Vec<usize>> {
        let (mut seen, mut stack, mut sets) = (vec![false; nfa.len()], from.to_vec(), Vec::new());
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            match nfa[i].kind {
                // A match can end here: no later position is known.
                Kind::Match => return None,
                Kind::Set => sets.push(i),
                Kind::Split => stack.extend([nfa[i].out, nfa[i].alt]),
                Kind::Bol | Kind::Eol | Kind::Word | Kind::NotWord => stack.push(nfa[i].out),
            }
        }
        Some(sets)
    };
    let mut positions: Vec<([bool; 128], bool)> = Vec::new();
    let mut threads = vec![start];
    while positions.len() < SKIP_POSITIONS {
        let Some(sets) = closure(&threads) else { break };
        let (mut table, mut wide) = ([false; 128], false);
        for &i in &sets {
            for (c, slot) in table.iter_mut().enumerate() {
                *slot |= (nfa[i].set[c >> 6] >> (c & 63)) & 1 == 1;
            }
            wide |= nfa[i].wide;
        }
        positions.push((table, wide));
        // A character beyond ASCII takes more than one unit of UTF-8: the positions after it are not known.
        if wide {
            break;
        }
        threads = sets.iter().map(|&i| nfa[i].out).collect();
    }
    let mut best: Option<(f64, usize, usize)> = None;
    for lo in 0..positions.len() {
        let (mut table, mut wide) = ([false; 128], false);
        for (hi, (t, w)) in positions.iter().enumerate().skip(lo) {
            for c in 0..128 {
                table[c] |= t[c];
            }
            wide |= w;
            let p: f64 = (0..128).filter(|&c| table[c]).map(frequency).sum::<f64>() + if wide { 0.1 } else { 0.0 };
            let worth = (hi - lo + 1) as f64 * (1.0 - p.min(1.0));
            if worth >= 3.0 && best.is_none_or(|(w, _, _)| worth > w) {
                best = Some((worth, lo, hi));
            }
        }
    }
    let (_, lo, hi) = best?;
    let (mut table, mut wide) = ([false; 128], false);
    for (t, w) in &positions[lo..=hi] {
        for c in 0..128 {
            table[c] |= t[c];
        }
        wide |= w;
    }
    Some(Skip { hi, len: hi - lo + 1, table, wide })
}

/// Builds the anchored automaton of a tree (after `simplify`), or `None` when the pattern has none.
pub(crate) fn build(root: &Node) -> Option<Anchored> {
    build_with(root, false)
}

/// Builds the forward automaton of a tree: the anchored automaton with a new start joined at every unit, behind every
/// thread already alive (so last in preference, as the search tries later starts after earlier ones). Read over the
/// text from a start, it says where the first match ends, and its start states say where no thread of an earlier
/// start is alive: the leftmost match starts at or after the last of them, the forward search of the `regex` crate.
pub(crate) fn build_forward(root: &Node) -> Option<Anchored> {
    build_with(root, true)
}

fn build_with(root: &Node, forward: bool) -> Option<Anchored> {
    let mut b = Builder { states: Vec::new(), failed: false };
    let match_ = b.add(State::new(Kind::Match, 0));
    let start = b.compile(root, match_);
    if b.failed {
        return None;
    }
    let nfa = &b.states[..];
    let mut d = Det { nfa, mark: vec![0; nfa.len()], generation: 0, steps: MAX_STEPS };
    // In the forward automaton a thread of the start joined at the current unit is kept apart from the threads of
    // earlier starts, as its state's number plus the number of states, so that a state whose threads are all of the
    // current start (a start state) is known: no earlier start can still match.
    let n = nfa.len();
    let decode = move |set: &[usize]| -> Vec<usize> { set.iter().map(|&i| if i >= n { i - n } else { i }).collect() };

    // The classes of units: those every set has both or neither of, and that are both word characters or neither.
    let mut class = [0u8; 128];
    let mut signatures: HashMap<Vec<u64>, u8> = HashMap::new();
    for (c, slot) in class.iter_mut().enumerate() {
        let mut sig: Vec<u64> = vec![u64::from(is_word(c))];
        for s in nfa.iter().filter(|s| s.kind == Kind::Set) {
            sig.push((s.set[c >> 6] >> (c & 63)) & 1);
        }
        let n = signatures.len();
        *slot = *signatures.entry(sig).or_insert(n as u8);
    }
    let classes = signatures.len();
    let mut shift = 0;
    while 1usize << shift < classes {
        shift += 1;
    }
    let stride = 1usize << shift;
    let mut sample = vec![0usize; classes];
    for c in (0..128usize).rev() {
        sample[class[c] as usize] = c;
    }

    let has = |set: &[usize], k: Kind| set.iter().any(|&i| nfa[i].kind == k);
    // The word bit matters only to a pattern with a boundary and the start bit only to one with a start anchor (each
    // may be reached later, behind an end anchor or a boundary still to pass, so the whole pattern is asked).
    let boundaries = nfa.iter().any(|s| matches!(s.kind, Kind::Word | Kind::NotWord));
    let anchors = nfa.iter().any(|s| s.kind == Kind::Bol);
    let mut ids: HashMap<(Vec<usize>, bool, bool), u16> = HashMap::new();
    let mut sets: Vec<(Vec<usize>, bool, bool)> = vec![(Vec::new(), false, false)];
    let mut trans: Vec<u16> = vec![DEAD; stride];
    let mut at_end: Vec<bool> = vec![false];
    let intern = |d: &mut Det<'_>,
                  ids: &mut HashMap<(Vec<usize>, bool, bool), u16>,
                  sets: &mut Vec<(Vec<usize>, bool, bool)>,
                  trans: &mut Vec<u16>,
                  at_end: &mut Vec<bool>,
                  set: Vec<usize>,
                  word: bool,
                  at_start: bool|
     -> Option<u16> {
        if set.is_empty() {
            return Some(DEAD);
        }
        let word = word && boundaries;
        let at_start = at_start && anchors;
        let key = (set, word, at_start);
        if let Some(&id) = ids.get(&key) {
            return Some(id);
        }
        if sets.len() >= MAX_STATES {
            return None;
        }
        let id = sets.len() as u16;
        let mut closed = Vec::new();
        d.closure(&mut closed, &decode(&key.0), at_start, true, Some((word, false)));
        at_end.push(has(&closed, Kind::Match));
        trans.extend(std::iter::repeat_n(DEAD, stride));
        ids.insert(key.clone(), id);
        sets.push(key);
        Some(id)
    };
    let mut starts = [DEAD; 4];
    for (k, slot) in starts.iter_mut().enumerate() {
        let (word, at_start) = (k & 2 != 0, k & 1 != 0);
        let mut first = Vec::new();
        d.closure(&mut first, &[start], at_start, false, None);
        if forward {
            first.iter_mut().for_each(|i| *i += n);
        }
        *slot = intern(&mut d, &mut ids, &mut sets, &mut trans, &mut at_end, first, word, at_start)?;
    }
    let mut id = 1;
    let (mut resolved, mut moved) = (Vec::new(), Vec::new());
    while id < sets.len() {
        let (set, word, at_start) = sets[id].clone();
        let set = decode(&set);
        for (cl, &c) in sample.iter().enumerate() {
            resolved.clear();
            d.closure(&mut resolved, &set, at_start, false, Some((word, is_word(c))));
            let mut flag = 0;
            if let Some(m) = resolved.iter().position(|&i| nfa[i].kind == Kind::Match) {
                // A match ends before this unit; only the threads it would have been tried after go on.
                flag = MATCHED;
                resolved.truncate(m);
            }
            moved.clear();
            for &i in &resolved {
                let s = &nfa[i];
                if s.kind == Kind::Set && (s.set[c >> 6] >> (c & 63)) & 1 != 0 {
                    moved.push(s.out);
                }
            }
            let mut next = Vec::new();
            d.closure(&mut next, &moved, false, false, None);
            if forward && flag == 0 {
                let mut fresh = Vec::new();
                d.closure_on(&mut fresh, &[start], false, false, None);
                next.extend(fresh.iter().map(|&i| i + n));
            }
            let to = intern(&mut d, &mut ids, &mut sets, &mut trans, &mut at_end, next, is_word(c), false)?;
            if d.steps < 0 {
                return None;
            }
            trans[id * stride + cl] = to | flag;
        }
        id += 1;
    }
    let fast = trans
        .iter()
        .map(|&t| (u32::from(t & !MATCHED) << shift) | if t & MATCHED != 0 { FAST_MATCHED } else { 0 })
        .collect();
    let context = starts.iter().any(|&s| s != starts[0]);
    let start_like = if forward {
        sets.iter().map(|(set, _, _)| !set.is_empty() && set.iter().all(|&i| i >= n)).collect()
    } else {
        Vec::new()
    };
    Some(Anchored { class, shift: shift as u32, fast, at_end, starts, context, start_like })
}

/// An automaton of the terms before the anchors of a search, read backwards (accepting where any of them can start):
/// from an occurrence of an anchor it finds, over ASCII text, every start from which some alternative's terms reach
/// it. Only terms with no assertion are taken.
#[derive(Clone, Debug)]
pub(crate) struct Reverse {
    class: [u8; 128],
    shift: u32,
    /// The transitions, each the next state's row (premultiplied by the stride, so 0 is the dead state) with
    /// [`FAST_MATCHED`] where that state accepts.
    fast: Vec<u32>,
    /// Whether the first state accepts (an empty run of terms).
    accept_first: bool,
}

impl Reverse {
    /// The starts before `q`, as a mask of their distances (bit `d` is the start `q - d`), that the terms allow,
    /// looking no further back than `most` units nor before `floor`; `None` when a unit beyond ASCII is reached.
    #[inline]
    pub(crate) fn starts<U: Copy + Into<u32>>(&self, input: &[U], q: usize, most: usize, floor: usize) -> Option<u128> {
        let mut mask = u128::from(self.accept_first);
        let mut at = 1usize << self.shift;
        let (fast, class) = (&self.fast[..], &self.class);
        for d in 1..=most.min(q.saturating_sub(floor)) {
            let c: u32 = input[q - d].into();
            if c >= 0x80 {
                return None;
            }
            // SAFETY: `at` is a state's row (a multiple of the stride below the table's length, as every entry of the
            // table is) and a class is below the stride.
            let t = unsafe { *fast.get_unchecked(at + class[c as usize] as usize) };
            if t == 0 {
                break;
            }
            if t & FAST_MATCHED != 0 {
                mask |= 1u128 << d;
            }
            at = (t & !FAST_MATCHED) as usize;
        }
        Some(mask)
    }
}

impl Builder {
    /// The states of a node read backwards that go on to `next`.
    fn compile_back(&mut self, n: &Node, mut next: usize) -> usize {
        if self.failed {
            return 0;
        }
        match n.kind {
            NodeKind::Cat => {
                for sub in &n.subs {
                    next = self.compile_back(sub, next);
                }
                next
            }
            NodeKind::Alt => {
                let mut start = self.compile_back(&n.subs[n.subs.len() - 1], next);
                for sub in n.subs[..n.subs.len() - 1].iter().rev() {
                    let out = self.compile_back(sub, next);
                    start = self.split(out, start);
                }
                start
            }
            NodeKind::Group => self.compile_back(&n.subs[0], next),
            NodeKind::Repeat => {
                let sub = &n.subs[0];
                if n.max == UNBOUNDED {
                    let loop_ = self.split(0, 0);
                    if self.failed {
                        return 0;
                    }
                    let body = self.compile_back(sub, loop_);
                    let s = &mut self.states[loop_];
                    (s.out, s.alt) = (body, next);
                    next = loop_;
                } else {
                    let mut i = n.min;
                    while i < n.max && !self.failed {
                        let body = self.compile_back(sub, next);
                        next = self.split(body, next);
                        i += 1;
                    }
                }
                let mut i = 0;
                while i < n.min && !self.failed {
                    next = self.compile_back(sub, next);
                    i += 1;
                }
                next
            }
            NodeKind::Empty | NodeKind::Char | NodeKind::Set => self.compile(n, next),
            _ => {
                self.failed = true;
                0
            }
        }
    }
}

/// The reverse automaton of the prefixes of a search's anchors, or `None` when one holds an assertion or the automaton
/// is too large.
pub(crate) fn reverse<'a>(prefixes: impl Iterator<Item = &'a [Node]>) -> Option<Reverse> {
    let mut b = Builder { states: Vec::new(), failed: false };
    let match_ = b.add(State::new(Kind::Match, 0));
    let mut starts = Vec::new();
    for prefix in prefixes {
        let mut next = match_;
        for t in prefix {
            next = b.compile_back(t, next);
        }
        starts.push(next);
    }
    if b.failed || starts.is_empty() {
        return None;
    }
    let mut start = starts[starts.len() - 1];
    for &s in starts[..starts.len() - 1].iter().rev() {
        start = b.split(s, start);
    }
    if b.failed {
        return None;
    }
    let nfa = &b.states[..];
    let mut d = Det { nfa, mark: vec![0; nfa.len()], generation: 0, steps: MAX_STEPS };
    let mut class = [0u8; 128];
    let mut signatures: HashMap<Vec<u64>, u8> = HashMap::new();
    for (c, slot) in class.iter_mut().enumerate() {
        let sig: Vec<u64> =
            nfa.iter().filter(|s| s.kind == Kind::Set).map(|s| (s.set[c >> 6] >> (c & 63)) & 1).collect();
        let n = signatures.len();
        *slot = *signatures.entry(sig).or_insert(n as u8);
    }
    let classes = signatures.len();
    let mut shift = 0;
    while 1usize << shift < classes {
        shift += 1;
    }
    let stride = 1usize << shift;
    let mut sample = vec![0usize; classes];
    for c in (0..128usize).rev() {
        sample[class[c] as usize] = c;
    }
    // A set of states, sorted, is its key; every thread is kept (any of them reaching the match is a start).
    let closure = |d: &mut Det<'_>, from: &[usize]| -> Vec<usize> {
        let mut out = Vec::new();
        d.generation += 1;
        let mut stack: Vec<usize> = from.to_vec();
        while let Some(i) = stack.pop() {
            d.steps -= 1;
            if d.mark[i] == d.generation {
                continue;
            }
            d.mark[i] = d.generation;
            let s = &d.nfa[i];
            match s.kind {
                Kind::Split => {
                    stack.push(s.alt);
                    stack.push(s.out);
                }
                _ => out.push(i),
            }
        }
        out.sort_unstable();
        out
    };
    let mut ids: HashMap<Vec<usize>, u16> = HashMap::new();
    let mut sets: Vec<Vec<usize>> = vec![Vec::new()];
    let mut trans = vec![DEAD; stride];
    let mut accept = vec![false];
    let first = closure(&mut d, &[start]);
    ids.insert(first.clone(), 1);
    accept.push(first.iter().any(|&i| nfa[i].kind == Kind::Match));
    sets.push(first);
    trans.extend(std::iter::repeat_n(DEAD, stride));
    let mut id = 1;
    while id < sets.len() {
        for (cl, &c) in sample.iter().enumerate() {
            let moved: Vec<usize> = sets[id]
                .iter()
                .filter(|&&i| nfa[i].kind == Kind::Set && (nfa[i].set[c >> 6] >> (c & 63)) & 1 != 0)
                .map(|&i| nfa[i].out)
                .collect();
            let next = closure(&mut d, &moved);
            let to = if next.is_empty() {
                DEAD
            } else if let Some(&t) = ids.get(&next) {
                t
            } else {
                if sets.len() >= MAX_STATES {
                    return None;
                }
                let t = sets.len() as u16;
                accept.push(next.iter().any(|&i| nfa[i].kind == Kind::Match));
                ids.insert(next.clone(), t);
                sets.push(next);
                trans.extend(std::iter::repeat_n(DEAD, stride));
                t
            };
            if d.steps < 0 {
                return None;
            }
            trans[id * stride + cl] = to;
        }
        id += 1;
    }
    let fast = trans
        .iter()
        .map(|&t| (u32::from(t) << shift) | if t != DEAD && accept[t as usize] { FAST_MATCHED } else { 0 })
        .collect();
    Some(Reverse { class, shift: shift as u32, fast, accept_first: accept[1] })
}
