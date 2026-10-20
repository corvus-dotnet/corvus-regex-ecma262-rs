//! Finding where a match can start (section 4.3 of the design of the .NET reference engine), worked out once from
//! the syntax tree when a pattern is compiled.
//!
//! A search on a long text spends most of its time looking for where to start, so before the matcher runs the
//! program applies what it learned at compile time. The reference engine has a hand-written vectorised search for
//! each kind of literal; this crate takes the searches Rust already has:
//!
//! - **One literal** (a prefix every match starts with, or a literal every match holds at a known distance) is
//!   found by `memchr::memmem`, whose prefilter compares the literal's rarest bytes over a vector of positions and
//!   the literal in full only where they agree: the rarest-position needle of the reference engine.
//! - **Two literals** are found by `aho-corasick`, whose prefilter looks for their rare bytes or their first bytes
//!   with `memchr`: the multi-literal needle.
//! - **Three or more** (an alternation, or the forms of a case-insensitive literal) are found by the packed searcher
//!   of `aho-corasick`, Teddy: the fingerprint search of the reference engine, a vector of positions looked up by
//!   the nibbles of each byte in tables of the literals' buckets.
//!
//! Over UTF-16 the same searchers look at the bytes of the code units, and an occurrence at an odd byte offset is
//! passed over.
//!
//! What is searched for (an "anchor") is a literal, each of whose characters may have several forms (a letter's
//! cases under the i flag, or the members of a small class), that every match holds at a known number of
//! characters from its start: one number, or a bounded range. A pattern that is an alternation, or that holds one
//! after a bounded prefix (`.{0,2}(Tom|Sawyer)`), has an anchor in each alternative, and all of them are searched for
//! at once. Each occurrence opens the window of starts from which some alternative could reach it; the starts are
//! tried in order and never twice (the alternation windows). Where every alternative starts with its literal, the
//! matcher runs at the occurrence alone.
//!
//! A pattern that starts with loops with no upper bound, then a literal (`[a-zA-Z]+ing`, `\w+\s+Holmes`), has the
//! literal searched for and the runs of the loops' sets before it walked back to the earliest start, which is the
//! only one tried for that occurrence: a later start in the same run can only reach a subset of the places the
//! loops can end at.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use aho_corasick::{AhoCorasick, MatchKind, packed};
use memchr::memmem;

use super::Rune;
use super::charset::{CharSet, is_word_byte};
use super::compile::literal_rune;
use super::parse::{Node, NodeKind, UNBOUNDED};

/// The most literals the forms of one anchor are expanded to: the most the fingerprint search takes.
const MAX_FORMS: usize = 64;
/// The most literals of all the anchors of a pattern.
const MAX_LITERALS: usize = 512;
/// The most characters a window of starts may span.
const MAX_WINDOW: usize = 64;

/// A search for one of a set of literals in a text's bytes.
#[derive(Clone, Debug)]
enum Finder {
    One(memmem::Finder<'static>),
    Packed(packed::Searcher),
    Automaton(AhoCorasick),
    /// A few literals that all start with one rare byte: `memchr` for the byte, then each literal compared in order.
    Shared(u8, Vec<Vec<u8>>),
}

/// Whether a byte is rare enough in text for a search for it alone to pay: not a lower case letter (but the rarest
/// five), a space or the commonest punctuation, nor a byte beyond ASCII, where the lead bytes of a script are common.
fn rare(b: u8) -> bool {
    b.is_ascii_uppercase()
        || b.is_ascii_digit()
        || b.is_ascii_punctuation() && !b",.'\"-".contains(&b)
        || b"jqxzk".contains(&b)
}

impl Finder {
    fn new(literals: &[Vec<u8>], fingerprint: bool, utf16: bool) -> Finder {
        if literals.len() == 1 {
            if utf16 {
                let finder = memmem::FinderBuilder::new().build_forward_with_ranker(Utf16Rank, &literals[0]);
                return Finder::One(finder.into_owned());
            }
            return Finder::One(memmem::Finder::new(&literals[0]).into_owned());
        }
        // Up to four literals that start with the same rare byte are found by `memchr` for that byte and a compare of
        // each (Sherlock|Street, as the regex crate finds them); any others by the fingerprint search, which looks at three bytes (Huck|Saw, whose two
        // first bytes are both capitals, measured 1.6 times regex's time with the search for two bytes).
        // Over UTF-16 every other byte of an ASCII literal is zero, which no search for rare bytes can use, so the
        // fingerprint search always takes several literals there.
        let first = literals[0].first().copied();
        let one_rare_first = first.is_some_and(rare) && literals.iter().all(|l| l.first().copied() == first);
        let fingerprint = fingerprint || utf16 || !one_rare_first;
        if !fingerprint && literals.len() <= 4 {
            return Finder::Shared(first.expect("a literal"), literals.to_vec());
        }
        if fingerprint
            && literals.len() <= MAX_FORMS
            && let Some(searcher) =
                packed::Config::new().match_kind(packed::MatchKind::LeftmostFirst).builder().extend(literals).build()
        {
            return Finder::Packed(searcher);
        }
        let automaton = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostFirst)
            .build(literals)
            .expect("an automaton of a few hundred literals builds");
        Finder::Automaton(automaton)
    }

    /// The leftmost occurrence at or after `from` (the first literal, in their order, of those that start there).
    #[inline]
    fn find(&self, haystack: &[u8], from: usize) -> Option<(usize, usize)> {
        if from > haystack.len() {
            return None;
        }
        match self {
            Finder::One(f) => f.find(&haystack[from..]).map(|i| (from + i, from + i + f.needle().len())),
            Finder::Packed(s) => {
                s.find_in(haystack, aho_corasick::Span::from(from..haystack.len())).map(|m| (m.start(), m.end()))
            }
            Finder::Automaton(a) => {
                a.find(aho_corasick::Input::new(haystack).span(from..haystack.len())).map(|m| (m.start(), m.end()))
            }
            Finder::Shared(first, literals) => {
                let mut at = from;
                loop {
                    let p = at + memchr::memchr(*first, &haystack[at..])?;
                    if let Some(l) = literals.iter().find(|l| haystack[p..].starts_with(l)) {
                        return Some((p, p + l.len()));
                    }
                    at = p + 1;
                }
            }
        }
    }
}

/// How common each byte is in the bytes of UTF-16 text, for `memmem`'s choice of the needle's rarest bytes: its own
/// table is of UTF-8 text, where a zero byte is rare, and in UTF-16 the high byte of every ASCII unit is zero. So
/// zero is the commonest, then the high byte of Cyrillic, then the low bytes roughly as English text ranks them (the
/// high bytes of the Han ideographs, 0x4E to 0x9F, are ASCII letters' bytes too, and rank as those).
struct Utf16Rank;

impl memchr::arch::all::packedpair::HeuristicFrequencyRank for Utf16Rank {
    fn rank(&self, byte: u8) -> u8 {
        match byte {
            0x00 => 255,
            0x04 => 240,
            b' ' => 230,
            b'e' | b't' | b'a' | b'o' | b'i' | b'n' | b's' | b'h' | b'r' => 220,
            b'd' | b'l' | b'u' | b'c' | b'm' | b'f' | b'w' | b'y' | b'p' | b'g' => 200,
            b'b' | b'v' | b',' | b'.' | b'\n' | b'\r' => 180,
            b'A'..=b'Z' => 120,
            b'k' | b'x' | b'j' | b'q' | b'z' => 100,
            b'0'..=b'9' => 90,
            _ => 60,
        }
    }
}

/// The literals of a search, over UTF-8 and over the bytes of UTF-16 code units.
#[derive(Clone, Debug)]
pub(crate) struct Literals {
    utf8: Finder,
    utf16: Finder,
    /// The search over UTF-16 of packed units, for several literals whose units are ASCII or among a few others: the
    /// units of a chunk of text are packed into bytes (an ASCII unit as itself, one of the few others as a byte of its
    /// own from 0x81 on, any other as 0x80, which no literal holds) and the packed literals searched for, as the .NET
    /// reference engine's packed fingerprint search does, so that the zero high bytes of ASCII units are not looked at.
    /// (One literal is found by `memmem` over the units' bytes, ranked for UTF-16.)
    packed: Option<(Finder, Vec<(u16, u8)>)>,
    /// The most code units of a literal.
    longest: usize,
    /// The bytes the UTF-8 literals start with.
    firsts: Box<[bool; 256]>,
}

/// The units packed into bytes at a time for the UTF-16 search of ASCII literals.
const PACK: usize = 4096;

impl Literals {
    /// The search for the literals, each a sequence of code points, with the most bytes of each written out over
    /// UTF-8 (see [`utf8_cuts`]).
    fn new(literals: &[Vec<Rune>], cuts: &[usize], fingerprint: bool) -> Literals {
        let as_string =
            |l: &Vec<Rune>| -> String { l.iter().map(|&r| char::from_u32(r as u32).expect("a literal")).collect() };
        // A literal cut to fewer bytes finds the same starts, and one that an earlier literal begins with finds no
        // start the earlier one does not.
        let mut utf8: Vec<Vec<u8>> = Vec::with_capacity(literals.len());
        for (l, &cut) in literals.iter().zip(cuts) {
            let string = as_string(l);
            let mut b = string.as_bytes().to_vec();
            // A cut inside a character is not made: measured, the Cyrillic letters with a three-byte form (в and
            // ᲁ) cut so made the search 1.2 times slower.
            if string.is_char_boundary(cut.min(string.len())) {
                b.truncate(cut);
            }
            if !utf8.iter().any(|k| b.starts_with(k)) {
                utf8.push(b);
            }
        }
        let utf16: Vec<Vec<u8>> =
            literals.iter().map(|l| as_string(l).encode_utf16().flat_map(u16::to_ne_bytes).collect()).collect();
        let longest = literals.iter().map(|l| as_string(l).encode_utf16().count()).max().unwrap_or(0);
        let mut map: Vec<(u16, u8)> = Vec::new();
        let mut packable = literals.len() > 1 && longest < 64;
        let packed_literals: Vec<Vec<u8>> = literals
            .iter()
            .map(|l| {
                as_string(l)
                    .encode_utf16()
                    .map(|u| {
                        if u < 0x80 {
                            return u as u8;
                        }
                        if let Some(&(_, b)) = map.iter().find(|(m, _)| *m == u) {
                            return b;
                        }
                        if map.len() >= 64 {
                            packable = false;
                            return 0x80;
                        }
                        let b = 0x81 + map.len() as u8;
                        map.push((u, b));
                        b
                    })
                    .collect()
            })
            .collect();
        // Packing pays for ASCII literals: a unit beyond them is then one compare away from its byte. Measured, a map of
        // even two units (the long s and the Kelvin sign of a case-insensitive literal) made the search slower than the
        // fingerprint search over the units' bytes, and a Cyrillic literal's map made it twenty times slower.
        let packed = (packable && map.is_empty()).then(|| (Finder::new(&packed_literals, fingerprint, false), map));
        let mut firsts = Box::new([false; 256]);
        for l in &utf8 {
            firsts[l[0] as usize] = true;
        }
        Literals {
            firsts,
            utf8: Finder::new(&utf8, fingerprint, false),
            utf16: Finder::new(&utf16, fingerprint, true),
            packed,
            longest,
        }
    }

    /// The position (in units of the text) of the leftmost occurrence at or after `from`. `bytes` is the text's
    /// bytes and `utf8` says whether a unit is one of them.
    #[inline]
    pub(crate) fn find(&self, bytes: &[u8], from: usize, utf8: bool) -> Option<usize> {
        self.find_match(bytes, from, utf8).map(|(start, _)| start)
    }

    /// The leftmost occurrence that starts at or after `from` and at or before `last`. Over UTF-8 the few bytes
    /// between are looked at one at a time for a literal's first byte before a search is called, since a call on so
    /// short a text costs more than reading it.
    #[inline]
    pub(crate) fn find_within(&self, bytes: &[u8], from: usize, last: usize, utf8: bool) -> Option<usize> {
        if !utf8 {
            let reach = (last + self.longest).min(bytes.len() / 2);
            if from >= reach {
                return None;
            }
            return self.find(&bytes[..reach * 2], from, false).filter(|&q| q <= last);
        }
        let last = last.min(bytes.len().checked_sub(1)?);
        if from > last {
            return None;
        }
        if let Finder::One(f) = &self.utf8 {
            let needle = f.needle();
            return (from..=last).find(|&p| bytes[p] == needle[0] && bytes[p..].starts_with(needle));
        }
        let p = (from..=last).find(|&p| self.firsts[bytes[p] as usize])?;
        let reach = (last + 3 * self.longest).min(bytes.len());
        self.utf8.find(&bytes[..reach], p).map(|(q, _)| q).filter(|&q| q <= last)
    }

    /// [`Literals::find`], with where the occurrence ends.
    #[inline]
    pub(crate) fn find_match(&self, bytes: &[u8], from: usize, utf8: bool) -> Option<(usize, usize)> {
        if utf8 {
            return self.utf8.find(bytes, from);
        }
        if let Some((finder, map)) = &self.packed {
            return self.find_packed(finder, map, bytes, from);
        }
        let mut at = 2 * from;
        loop {
            let (i, e) = self.utf16.find(bytes, at)?;
            if i & 1 == 0 {
                return Some((i / 2, e / 2));
            }
            at = i + 1;
        }
    }

    /// The search over UTF-16 for ASCII literals: the low bytes of a chunk of units packed into bytes, searched by the
    /// UTF-8 search. An occurrence that starts so near the end of a chunk that a longer one could start before it and
    /// end beyond the chunk is looked for again in the next chunk, which starts that far back.
    fn find_packed(&self, finder: &Finder, map: &[(u16, u8)], bytes: &[u8], from: usize) -> Option<(usize, usize)> {
        // SAFETY: the bytes are those of a slice of u16 (`Text::bytes` of UTF-16 text), so they are aligned for u16
        // and their length is even.
        let text: &[u16] = unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u16>(), bytes.len() / 2) };
        let units = text.len();
        let overlap = self.longest.saturating_sub(1);
        let mut buf = [0u8; PACK];
        let mut at = from;
        // The first chunk is small, and each is four times the one before, so a search whose occurrence is near
        // packs little more than it reads.
        let mut size = 64;
        while at <= units {
            let end = (at + size).min(units);
            size = (size * 4).min(PACK);
            // A unit beyond ASCII packs to 0x80; the loop is one the compiler makes into vector instructions.
            for (slot, &u) in buf[..end - at].iter_mut().zip(&text[at..end]) {
                *slot = u.min(0x80) as u8;
            }
            if !map.is_empty() {
                for (slot, &u) in buf[..end - at].iter_mut().zip(&text[at..end]) {
                    if u >= 0x80
                        && let Some(&(_, b)) = map.iter().find(|(m, _)| *m == u)
                    {
                        *slot = b;
                    }
                }
            }
            let last = end == units;
            if let Some((s, e)) = finder.find(&buf[..end - at], 0)
                && (last || s < end - at - overlap)
            {
                return Some((at + s, at + e));
            }
            if last {
                return None;
            }
            at = end - overlap;
        }
        None
    }
}

/// The anchored automaton of what follows an occurrence (an anchor and the terms after it), run at the occurrence
/// before any start is walked back to. A check that seldom rules an occurrence out costs more than it saves (an
/// e-mail address's `@` is nearly always followed by a domain), so the check keeps count over its first trials and is
/// dropped for good when fewer than one in eight of them ruled the occurrence out. The counts change only how fast a
/// search is, never what it finds.
#[derive(Debug)]
pub(crate) struct RestCheck {
    dfa: super::adfa::Anchored,
    checks: AtomicU32,
    rejects: AtomicU32,
}

/// The checks a [`RestCheck`] makes before it decides whether it pays.
const REST_TRIALS: u32 = 64;

impl Clone for RestCheck {
    fn clone(&self) -> RestCheck {
        RestCheck {
            dfa: self.dfa.clone(),
            checks: AtomicU32::new(self.checks.load(Ordering::Relaxed)),
            rejects: AtomicU32::new(self.rejects.load(Ordering::Relaxed)),
        }
    }
}

impl RestCheck {
    fn new(dfa: super::adfa::Anchored) -> Box<RestCheck> {
        Box::new(RestCheck { dfa, checks: AtomicU32::new(0), rejects: AtomicU32::new(0) })
    }

    /// Whether nothing from the occurrence at `q` on can match.
    #[inline]
    pub(crate) fn rules_out<U: Copy + Into<u32>>(&self, input: &[U], q: usize) -> bool {
        let checks = self.checks.load(Ordering::Relaxed);
        if checks >= REST_TRIALS && self.rejects.load(Ordering::Relaxed) * 8 < REST_TRIALS {
            return false;
        }
        let out = self.dfa.matches_at(input, q) == Some(false);
        if checks < REST_TRIALS {
            self.checks.store(checks + 1, Ordering::Relaxed);
            if out {
                self.rejects.fetch_add(1, Ordering::Relaxed);
            }
        }
        out
    }
}

/// How the search finds where a match can start, beyond the first set.
#[derive(Clone, Debug, Default)]
pub(crate) enum Plan {
    /// Every position the first set allows is tried.
    #[default]
    Scan,
    /// Every match holds one of the literals so many to so many characters (`lo` to `hi`) after its start.
    /// `rest` is the anchored automaton of the anchors and the terms after each, run at an occurrence first where a
    /// window has more than one start: where nothing from the occurrence on can match, its window is not tried.
    Windows {
        literals: Literals,
        lo: usize,
        hi: usize,
        reverse: Option<super::adfa::Reverse>,
        rest: Option<Box<RestCheck>>,
    },
    /// Every match is runs of the loops' sets, then the literal.
    /// `rest` is the anchored automaton of the literal and the terms after it, run at each occurrence first: where
    /// nothing from the occurrence on can match, no start before it is walked back to or tried.
    AfterLoops { literals: Literals, sets: Vec<Arc<CharSet>>, mins: Vec<usize>, rest: Option<Box<RestCheck>> },
}

/// The shape of a pattern whose matches the search finds without running the matcher (the .NET reference engine's
/// simple shapes). Only for a pattern with no group.
#[derive(Clone, Debug, Default)]
pub(crate) enum Shape {
    #[default]
    None,
    /// Literals and nothing else, an alternation of them or one, with every form of each written out (`the` under the
    /// i flag is eight literals): the search's own match (`Plan::Windows`), which is the first literal in the
    /// pattern's order of those that start leftmost, as ECMA-262 finds it.
    Literals,
    /// An alternation of literals each of whose characters has several forms (a letter under the i flag): at each
    /// occurrence the search finds, the alternatives are compared in order.
    Forms(Vec<Vec<Vec<Rune>>>),
    /// One class and nothing else: every member is a match.
    Set(Arc<CharSet>),
    /// One greedy loop of a class with no upper bound and a least count of zero or one: a match is the run of
    /// members from where it starts.
    Loop(Arc<CharSet>, usize, RunEnd),
}

/// How the end of a run of a loop's members is found.
#[derive(Clone, Debug)]
pub(crate) enum RunEnd {
    /// A member at a time.
    Each,
    /// The set holds every character: the run is the rest of the text.
    All,
    /// The set holds every character but up to three ASCII ones, found by `memchr`.
    Stops(Vec<u8>),
    /// The set holds every character but the four line terminators (the dot): LF and CR are found by `memchr`, with
    /// the lead byte of U+2028 and U+2029, which is looked at further where it is found.
    Line,
}

impl RunEnd {
    fn of(set: &CharSet) -> RunEnd {
        let complement = set.complement();
        if complement.ranges.is_empty() {
            return RunEnd::All;
        }
        if complement.ranges == [0x0A, 0x0A, 0x0D, 0x0D, 0x2028, 0x2029] {
            return RunEnd::Line;
        }
        let mut stops = Vec::new();
        for &[lo, hi] in complement.ranges.as_chunks::<2>().0 {
            if hi >= 0x80 || hi - lo >= 3 || stops.len() + (hi - lo + 1) as usize > 3 {
                return RunEnd::Each;
            }
            stops.extend((lo..=hi).map(|r| r as u8));
        }
        RunEnd::Stops(stops)
    }

    /// The end of the run that starts at `at` in UTF-8 text, or `None` when it is to be found a member at a time.
    #[inline]
    pub(crate) fn end8(&self, input: &[u8], at: usize) -> Option<usize> {
        let rest = &input[at..];
        Some(match self {
            RunEnd::Each => return None,
            RunEnd::All => input.len(),
            RunEnd::Stops(b) => {
                at + match b[..] {
                    [a] => memchr::memchr(a, rest),
                    [a, c] => memchr::memchr2(a, c, rest),
                    [a, c, d] => memchr::memchr3(a, c, d, rest),
                    _ => None,
                }
                .unwrap_or(rest.len())
            }
            RunEnd::Line => {
                let mut k = 0;
                loop {
                    match memchr::memchr3(b'\n', b'\r', 0xE2, &rest[k..]) {
                        None => break input.len(),
                        Some(i) => {
                            let j = k + i;
                            if rest[j] != 0xE2
                                || rest.get(j + 1) == Some(&0x80) && matches!(rest.get(j + 2), Some(0xA8 | 0xA9))
                            {
                                break at + j;
                            }
                            k = j + 1;
                        }
                    }
                }
            }
        })
    }
}

/// What a pattern's search knows of it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Search {
    pub(crate) plan: Plan,
    /// The set of a loop every match starts with, when a start that fails may move to the end of the run of its
    /// set: every start in the rest of the run can only reach a subset of the places the loop can end at.
    pub(crate) skip_run: Option<Arc<CharSet>>,
    /// The pattern starts with `\b` and every character a match can start with is an ASCII word character, so a
    /// start inside a word fails and the search passes over the rest of the word.
    pub(crate) leading_word_boundary: bool,
    /// The bytes a match can start with over UTF-8, searched for together when they are few: the first set's ASCII
    /// members and the lead bytes of its other members.
    pub(crate) first_bytes: Option<FirstBytes>,
    /// The shape of a pattern the search answers without the matcher (see [`Shape`]).
    pub(crate) shape: Shape,
    /// The code units a match can start with over UTF-16, where they can be tested for many at a time.
    pub(crate) first_units: Option<FirstUnits>,
}

/// A search for any of a few bytes: `memchr` for up to three, and the fingerprint search of one-byte literals for
/// up to sixty-four, which is a vectorised search for a set of bytes.
#[derive(Clone, Debug)]
pub(crate) enum FirstBytes {
    One(u8),
    Two(u8, u8),
    Three(u8, u8, u8),
    /// The fingerprint search, and the bytes as a table for the few bytes looked at before it is called: where the
    /// bytes are common a call costs more than it passes over.
    Packed(packed::Searcher, Box<[bool; 256]>),
}

/// A test for the code units a match can start with over UTF-16: up to four ranges of ASCII units, and with `wide` every
/// unit beyond ASCII (which the first set then has a member among). Sixteen units are tested at once, in a loop the
/// compiler makes into vector instructions, and a candidate is then tested against the first set itself.
#[derive(Clone, Debug)]
pub(crate) struct FirstUnits {
    ranges: [(u16, u16); 4],
    wide: bool,
}

impl FirstUnits {
    fn new(set: &CharSet) -> Option<FirstUnits> {
        let mut ranges = [(u16::MAX, 0u16); 4];
        let mut n = 0;
        let mut wide = false;
        for &[lo, hi] in set.ranges.as_chunks::<2>().0 {
            if hi >= 0x80 {
                wide = true;
            }
            if lo < 0x80 {
                if n == 4 {
                    return None;
                }
                ranges[n] = (lo as u16, (hi.min(0x7F) - lo) as u16);
                n += 1;
            }
        }
        Some(FirstUnits { ranges, wide })
    }

    #[inline(always)]
    fn hit(&self, u: u16) -> bool {
        let r = &self.ranges;
        (self.wide & (u >= 0x80))
            | (u.wrapping_sub(r[0].0) <= r[0].1)
            | (u.wrapping_sub(r[1].0) <= r[1].1)
            | (u.wrapping_sub(r[2].0) <= r[2].1)
            | (u.wrapping_sub(r[3].0) <= r[3].1)
    }

    /// The first unit at or after `from` that can begin a match.
    #[inline]
    pub(crate) fn find(&self, units: &[u16], from: usize) -> Option<usize> {
        // Where the units are common a few are looked at one by one first.
        let probe = units.get(from..)?.iter().take(8).position(|&u| self.hit(u));
        if let Some(i) = probe {
            return Some(from + i);
        }
        let mut at = from + 8;
        while at + 16 <= units.len() {
            let chunk: &[u16; 16] = units[at..at + 16].try_into().expect("sixteen units");
            if chunk.iter().fold(false, |any, &u| any | self.hit(u)) {
                break;
            }
            at += 16;
        }
        units.get(at..)?.iter().position(|&u| self.hit(u)).map(|i| at + i)
    }
}

/// The bytes looked at one by one before the fingerprint search is called.
const PROBE: usize = 16;

impl FirstBytes {
    fn new(set: &CharSet, units: bool) -> Option<FirstBytes> {
        let mut bytes = [false; 256];
        for &[lo, hi] in set.ranges.as_chunks::<2>().0 {
            // A surrogate without the u flag is half of a character of four bytes, and U+FFFD is also what a byte
            // that is not UTF-8 reads as: neither starts at a byte the set can name.
            if units && lo <= 0xDFFF && hi >= 0xD800 || lo <= 0xFFFD && hi >= 0xFFFD {
                return None;
            }
            for (from, to, shift, lead) in
                [(0, 0x7F, 0, 0x00), (0x80, 0x7FF, 6, 0xC0), (0x800, 0xFFFF, 12, 0xE0), (0x10000, 0x10FFFF, 18, 0xF0)]
            {
                let (a, b) = (lo.max(from), hi.min(to));
                if a <= b {
                    for byte in (lead | (a >> shift))..=(lead | (b >> shift)) {
                        bytes[byte as usize] = true;
                    }
                }
            }
        }
        let list: Vec<u8> = (0..=255u8).filter(|&b| bytes[b as usize]).collect();
        match list[..] {
            [] => None,
            [a] => Some(FirstBytes::One(a)),
            [a, b] => Some(FirstBytes::Two(a, b)),
            [a, b, c] => Some(FirstBytes::Three(a, b, c)),
            _ if list.len() <= MAX_FORMS => {
                let searcher = packed::Config::new().builder().extend(list.iter().map(|&b| [b])).build();
                searcher.map(|searcher| FirstBytes::Packed(searcher, Box::new(bytes)))
            }
            _ => None,
        }
    }

    /// The first position at or after `from` whose byte is one of the bytes.
    #[inline]
    pub(crate) fn find(&self, haystack: &[u8], from: usize) -> Option<usize> {
        let rest = haystack.get(from..)?;
        let i = match self {
            FirstBytes::One(a) => memchr::memchr(*a, rest),
            FirstBytes::Two(a, b) => memchr::memchr2(*a, *b, rest),
            FirstBytes::Three(a, b, c) => memchr::memchr3(*a, *b, *c, rest),
            FirstBytes::Packed(s, table) => {
                if let Some(i) = rest.iter().take(PROBE).position(|&b| table[b as usize]) {
                    return Some(from + i);
                }
                let from = from + PROBE.min(rest.len());
                return s.find_in(haystack, aho_corasick::Span::from(from..haystack.len())).map(|m| m.start());
            }
        };
        i.map(|i| from + i)
    }
}

/// One literal every match of an alternative holds, `lo` to `hi` characters after its start: the forms of each of
/// its characters.
struct Anchor {
    chars: Vec<Vec<Rune>>,
    lo: usize,
    hi: usize,
    /// The terms a match goes through before the anchor.
    prefix: Vec<Node>,
    /// The terms from the anchor on, to the end of the pattern.
    rest: Vec<Node>,
}

/// The forms of a character the search can look for: a literal code point, or the members of a small class.
fn forms(n: &Node) -> Option<Vec<Rune>> {
    match n.kind {
        NodeKind::Char if literal_rune(n.r) => Some(vec![n.r]),
        NodeKind::Set => {
            let set = n.class();
            let mut out = Vec::new();
            for &[lo, hi] in set.ranges.as_chunks::<2>().0 {
                if hi - lo >= 4 || out.len() + (hi - lo + 1) as usize > 4 {
                    return None;
                }
                for r in lo..=hi {
                    if !literal_rune(r) {
                        return None;
                    }
                    out.push(r);
                }
            }
            (!out.is_empty()).then_some(out)
        }
        _ => None,
    }
}

/// The terms of a node in order: a sequence and a group whose body is a sequence are spliced in.
fn terms(n: &Node) -> Vec<&Node> {
    match n.kind {
        NodeKind::Cat => n.subs.iter().flat_map(terms).collect(),
        NodeKind::Group if n.subs[0].kind != NodeKind::Alt => terms(&n.subs[0]),
        NodeKind::Empty => Vec::new(),
        _ => vec![n],
    }
}

/// The least and the greatest number of characters a node consumes (`None` for no bound), or `None` when it is not
/// known (a backreference).
fn width(n: &Node) -> Option<(usize, Option<usize>)> {
    match n.kind {
        NodeKind::Char | NodeKind::Set => Some((1, Some(1))),
        NodeKind::Cat => {
            let (mut lo, mut hi) = (0usize, Some(0usize));
            for sub in &n.subs {
                let (a, b) = width(sub)?;
                lo = lo.saturating_add(a);
                hi = hi.zip(b).map(|(x, y)| x.saturating_add(y));
            }
            Some((lo, hi))
        }
        NodeKind::Alt => {
            let mut out: Option<(usize, Option<usize>)> = None;
            for sub in &n.subs {
                let (a, b) = width(sub)?;
                out = Some(match out {
                    None => (a, b),
                    Some((x, y)) => (x.min(a), y.zip(b).map(|(y, b)| y.max(b))),
                });
            }
            Some(out.unwrap_or((0, Some(0))))
        }
        NodeKind::Group => width(&n.subs[0]),
        NodeKind::Repeat => {
            let (a, b) = width(&n.subs[0])?;
            let lo = a.saturating_mul(n.min as usize);
            let hi = if n.max == UNBOUNDED {
                if b == Some(0) { Some(0) } else { None }
            } else {
                b.map(|b| b.saturating_mul(n.max as usize))
            };
            Some((lo, hi))
        }
        NodeKind::Backref => None,
        _ => Some((0, Some(0))),
    }
}

/// The anchors of a node whose match starts `lo` to `hi` characters after the start of the whole match: the first
/// run of literal characters of each of its alternatives, or `None` when some alternative has none at a bounded
/// distance.
fn anchors(n: &Node, lo: usize, hi: usize) -> Option<Vec<Anchor>> {
    anchors_after(n, lo, hi, &[], &[])
}

/// The anchors of `n`, which the terms `before` precede and the terms `after` follow.
fn anchors_after(n: &Node, mut lo: usize, mut hi: usize, before: &[Node], after: &[Node]) -> Option<Vec<Anchor>> {
    let ts = terms(n);
    let mut prefix: Vec<Node> = before.to_vec();
    let mut k = 0;
    while k < ts.len() {
        let t = ts[k];
        if forms(t).is_some() {
            let rest: Vec<Node> = ts[k..].iter().map(|&t| t.clone()).chain(after.iter().cloned()).collect();
            let mut chars = Vec::new();
            while k < ts.len()
                && let Some(f) = forms(ts[k])
            {
                chars.push(f);
                k += 1;
            }
            return Some(vec![Anchor { chars, lo, hi, prefix, rest }]);
        }
        let alternation = match t.kind {
            NodeKind::Alt => Some(t),
            NodeKind::Group if t.subs[0].kind == NodeKind::Alt => Some(&t.subs[0]),
            _ => None,
        };
        if let Some(alt) = alternation {
            let following: Vec<Node> = ts[k + 1..].iter().map(|&t| t.clone()).chain(after.iter().cloned()).collect();
            let mut all = Vec::new();
            for sub in &alt.subs {
                all.extend(anchors_after(sub, lo, hi, &prefix, &following)?);
            }
            return Some(all);
        }
        let (a, b) = width(t)?;
        lo = lo.saturating_add(a);
        hi = hi.saturating_add(b?);
        if hi > MAX_WINDOW {
            return None;
        }
        prefix.push(t.clone());
        k += 1;
    }
    None
}

/// The literals of an anchor: the forms of its first `count` characters written out.
fn expand(a: &Anchor, count: usize) -> Vec<Vec<Rune>> {
    let mut out: Vec<Vec<Rune>> = vec![Vec::new()];
    for f in &a.chars[..count] {
        out = out.iter().flat_map(|prefix| f.iter().map(move |&r| [&prefix[..], &[r]].concat())).collect();
    }
    out
}

/// The most bytes of each of an anchor's literals written out over UTF-8: where the anchor was cut short, every
/// literal is cut to the bytes of its shortest, so that the forms of a character longer than the others (the long s,
/// two bytes beside the one of s) make no more literals than the others. This is the `regex` crate's rule, which cuts
/// literals by bytes; a script whose forms are all of one length keeps its characters, and a literal the cut would end
/// inside a character keeps its own.
fn utf8_cuts(cut_short: bool, literals: &[Vec<Rune>]) -> Vec<usize> {
    let bytes = |l: &Vec<Rune>| l.iter().map(|&r| char::from_u32(r as u32).map_or(0, char::len_utf8)).sum::<usize>();
    let cut = if cut_short { literals.iter().map(bytes).min().unwrap_or(usize::MAX) } else { usize::MAX };
    vec![cut; literals.len()]
}

/// How many of each anchor's characters to write out. Exact anchors are written out in full. Where a character has
/// several forms (a letter under the i flag), every anchor is cut to one length by the rule of the `regex` crate's
/// literal optimisation (`Seq::optimize_by_preference`), taken here because it measured best with the fingerprint
/// search the two share: the full literals if they are ten or fewer, else five characters if that makes ten or fewer,
/// else four if that makes sixty-four or fewer, else three if sixty-four or fewer, else two if ten or fewer, else one.
/// With the counts, whether the anchors were cut short (so that [`utf8_cuts`] cuts their literals' bytes too).
fn counts(anchors: &[Anchor]) -> (Vec<usize>, bool) {
    let full: Vec<usize> = anchors.iter().map(|a| a.chars.len()).collect();
    if anchors.iter().all(|a| a.chars.iter().all(|f| f.len() == 1)) {
        return (full, false);
    }
    let total = |k: usize| -> usize {
        anchors
            .iter()
            .map(|a| a.chars[..a.chars.len().min(k)].iter().map(Vec::len).fold(1usize, |p, n| p.saturating_mul(n)))
            .fold(0usize, |t, n| t.saturating_add(n))
    };
    let k = if total(usize::MAX) <= 10 {
        usize::MAX
    } else if total(5) <= 10 {
        5
    } else if total(4) <= MAX_FORMS {
        4
    } else if total(3) <= MAX_FORMS {
        3
    } else if total(2) <= 10 {
        2
    } else {
        1
    };
    (full.iter().map(|&n| n.min(k)).collect(), k != usize::MAX)
}

/// Whether a node can only begin with an ASCII word character, or is not known to.
fn starts_with_word(n: &Node, first: Option<&CharSet>) -> bool {
    let _ = n;
    first.is_some_and(|f| {
        f.ranges.last().is_some_and(|&last| last < 0x80) && (0..0x80u8).all(|c| !f.has_ascii(c) || is_word_byte(c))
    })
}

/// The top-level alternatives of a tree: the alternatives of an alternation (in a group or not), or the tree.
fn alternatives(root: &Node) -> Vec<&Node> {
    match root.kind {
        NodeKind::Alt => root.subs.iter().collect(),
        NodeKind::Group if root.group == 0 && root.subs[0].kind == NodeKind::Alt => root.subs[0].subs.iter().collect(),
        _ => vec![root],
    }
}

/// The literal shapes of a tree whose anchors are `found`: literals and nothing else, exact or with forms.
fn shape_of_literals(root: &Node, found: &[Anchor], counts: &[usize], cut_short: bool) -> Shape {
    if holds_group(root) {
        return Shape::None;
    }
    let alts = alternatives(root);
    let mut all: Vec<Vec<Vec<Rune>>> = Vec::with_capacity(alts.len());
    for alt in &alts {
        let ts = terms(alt);
        let chars: Option<Vec<Vec<Rune>>> = ts.iter().map(|t| forms(t)).collect();
        match chars {
            Some(c) if !c.is_empty() => all.push(c),
            _ => return Shape::None,
        }
    }
    // Literals written out in full, every form of each, are the search's own literals, in the pattern's order (an
    // alternative's forms together), so the search's leftmost-first match is the pattern's.
    if !cut_short
        && found.len() == all.len()
        && found.iter().zip(&all).zip(counts).all(|((a, alt), &n)| a.lo == 0 && a.hi == 0 && n == alt.len())
    {
        return Shape::Literals;
    }
    Shape::Forms(all)
}

/// The class and loop shapes of a tree that is one term.
fn shape_of_one(root: &Node) -> Shape {
    if holds_group(root) {
        return Shape::None;
    }
    let ts = terms(root);
    if ts.len() != 1 {
        return Shape::None;
    }
    let t = ts[0];
    match t.kind {
        NodeKind::Set if !t.class().is_empty() => Shape::Set(t.class().clone()),
        NodeKind::Repeat if !t.lazy && t.min <= 1 => match open_loop(t) {
            Some((set, min)) if !set.is_empty() => {
                let end = RunEnd::of(&set);
                Shape::Loop(set, min, end)
            }
            _ => Shape::None,
        },
        _ => Shape::None,
    }
}

/// Whether a node holds a capturing group.
fn holds_group(n: &Node) -> bool {
    n.kind == NodeKind::Group && n.group > 0 || n.subs.iter().any(holds_group)
}

/// A loop of one character or class with no upper bound.
fn open_loop(t: &Node) -> Option<(Arc<CharSet>, usize)> {
    if t.kind != NodeKind::Repeat || t.max != UNBOUNDED {
        return None;
    }
    let body = &t.subs[0];
    let set = match body.kind {
        NodeKind::Set => body.class().clone(),
        NodeKind::Char => CharSet::shared(&[body.r, body.r], false),
        _ => return None,
    };
    Some((set, t.min as usize))
}

/// Works out the search of a tree, after `simplify`. `backrefs` says the pattern has a backreference, which rules
/// out what depends on a start reaching a subset of another's places; `first` is the first set of the program.
pub(crate) fn plan(root: &Node, backrefs: bool, first: Option<&CharSet>, units: bool) -> Search {
    #[cfg(test)]
    if NO_SEARCH.with(std::cell::Cell::get) {
        return Search::default();
    }
    let mut search = Search {
        first_bytes: first.and_then(|f| FirstBytes::new(f, units)),
        first_units: first.and_then(FirstUnits::new),
        ..Search::default()
    };
    let ts = terms(root);
    // A leading \b before characters that are all ASCII word characters.
    let boundary = ts.first().is_some_and(|t| t.kind == NodeKind::WordBoundary && !t.fold);
    if boundary && starts_with_word(root, first) {
        search.leading_word_boundary = true;
    }
    if let Some(found) = anchors(root, 0, 0) {
        let mut literals: Vec<Vec<Rune>> = Vec::new();
        let mut cuts: Vec<usize> = Vec::new();
        let (mut lo, mut hi) = (usize::MAX, 0);
        let mut forms = false;
        let (counts, cut_short) = counts(&found);
        for (a, &count) in found.iter().zip(&counts) {
            lo = lo.min(a.lo);
            hi = hi.max(a.hi);
            forms |= a.chars.iter().any(|f| f.len() > 1);
            let expanded = expand(a, count);
            for (l, cut) in expanded.iter().zip(utf8_cuts(cut_short, &expanded)) {
                if !literals.contains(l) {
                    literals.push(l.clone());
                    cuts.push(cut);
                }
            }
        }
        if !literals.is_empty() && literals.len() <= MAX_LITERALS && literals.iter().all(|l| !l.is_empty()) {
            let fingerprint = literals.len() >= 3 || forms;
            search.shape = shape_of_literals(root, &found, &counts, cut_short);
            let finder = Literals::new(&literals, &cuts, fingerprint);
            // Where a window has more than one start, the terms before the anchors are matched backwards from each
            // occurrence, so that only the starts they allow are tried (the reverse search of the `regex` crate).
            let reverse = if hi > 0 { super::adfa::reverse(found.iter().map(|a| &a.prefix[..])) } else { None };
            // Where every anchor is the end of its alternative, the occurrence is all the rest there is to check.
            let rest = if hi > 0 && found.iter().any(|a| a.rest.len() > a.chars.len()) {
                let mut alt = Node::new(NodeKind::Alt);
                alt.subs = found
                    .iter()
                    .map(|a| {
                        let mut cat = Node::new(NodeKind::Cat);
                        cat.subs = a.rest.clone();
                        cat
                    })
                    .collect();
                super::adfa::build(&alt).map(RestCheck::new)
            } else {
                None
            };
            search.plan = Plan::Windows { literals: finder, lo, hi, reverse, rest };
            return search;
        }
    }
    search.shape = shape_of_one(root);
    if backrefs {
        return search;
    }
    // Loops with no upper bound, then a literal: leading \b allowed when the first loop's set is of word characters.
    let mut k = usize::from(boundary);
    let mut sets: Vec<Arc<CharSet>> = Vec::new();
    let mut mins = Vec::new();
    while let Some((set, min)) = ts.get(k).and_then(|t| open_loop(t)) {
        sets.push(set);
        mins.push(min);
        k += 1;
    }
    if !sets.is_empty() {
        let word_set = sets[0].ranges.last().is_some_and(|&l| l < 0x80)
            && (0..0x80u8).all(|c| !sets[0].has_ascii(c) || is_word_byte(c));
        let disjoint = sets.windows(2).all(|w| w[0].intersect(&w[1]).is_empty());
        let mut chars = Vec::new();
        let literal_at = k;
        while let Some(f) = ts.get(k).and_then(|t| forms(t)) {
            chars.push(f);
            k += 1;
        }
        if (!boundary || word_set) && disjoint && mins.iter().all(|&m| m <= 1) && !chars.is_empty() {
            let anchor = Anchor { chars, lo: 0, hi: 0, prefix: Vec::new(), rest: Vec::new() };
            let (count, cut_short) = counts(std::slice::from_ref(&anchor));
            let literals = expand(&anchor, count[0]);
            let cuts = utf8_cuts(cut_short, &literals);
            let fingerprint = literals.len() >= 2;
            let mut rest = Node::new(NodeKind::Cat);
            rest.subs = ts[literal_at..].iter().map(|&t| t.clone()).collect();
            let more = rest.subs.len() > anchor.chars.len();
            search.plan = Plan::AfterLoops {
                literals: Literals::new(&literals, &cuts, fingerprint),
                sets: sets.clone(),
                mins,
                rest: if more { super::adfa::build(&rest).map(RestCheck::new) } else { None },
            };
        }
        // A failed start may skip the rest of its leading loop's run (the loop is the first term).
        if !boundary {
            search.skip_run = Some(sets[0].clone());
        }
    }
    search
}

#[cfg(test)]
thread_local! {
    /// Makes every pattern compiled on this thread try every position its first set allows, which the tests compare
    /// the searches with.
    pub(crate) static NO_SEARCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
