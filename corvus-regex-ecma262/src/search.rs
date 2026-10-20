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
}

impl Finder {
    fn new(literals: &[Vec<u8>], fingerprint: bool) -> Finder {
        if literals.len() == 1 {
            return Finder::One(memmem::Finder::new(&literals[0]).into_owned());
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

    /// The start of the leftmost occurrence at or after `from`.
    #[inline]
    fn find(&self, haystack: &[u8], from: usize) -> Option<usize> {
        if from > haystack.len() {
            return None;
        }
        match self {
            Finder::One(f) => f.find(&haystack[from..]).map(|i| from + i),
            Finder::Packed(s) => s.find_in(haystack, aho_corasick::Span::from(from..haystack.len())).map(|m| m.start()),
            Finder::Automaton(a) => {
                a.find(aho_corasick::Input::new(haystack).span(from..haystack.len())).map(|m| m.start())
            }
        }
    }
}

/// The literals of a search, over UTF-8 and over the bytes of UTF-16 code units.
#[derive(Clone, Debug)]
pub(crate) struct Literals {
    utf8: Finder,
    utf16: Finder,
}

impl Literals {
    /// The search for the literals, each a sequence of code points.
    fn new(literals: &[Vec<Rune>], fingerprint: bool) -> Literals {
        let as_string =
            |l: &Vec<Rune>| -> String { l.iter().map(|&r| char::from_u32(r as u32).expect("a literal")).collect() };
        let utf8: Vec<Vec<u8>> = literals.iter().map(|l| as_string(l).into_bytes()).collect();
        let utf16: Vec<Vec<u8>> =
            literals.iter().map(|l| as_string(l).encode_utf16().flat_map(u16::to_ne_bytes).collect()).collect();
        Literals { utf8: Finder::new(&utf8, fingerprint), utf16: Finder::new(&utf16, fingerprint) }
    }

    /// The position (in units of the text) of the leftmost occurrence at or after `from`. `bytes` is the text's
    /// bytes and `utf8` says whether a unit is one of them.
    #[inline]
    pub(crate) fn find(&self, bytes: &[u8], from: usize, utf8: bool) -> Option<usize> {
        if utf8 {
            return self.utf8.find(bytes, from);
        }
        let mut at = 2 * from;
        loop {
            let i = self.utf16.find(bytes, at)?;
            if i & 1 == 0 {
                return Some(i / 2);
            }
            at = i + 1;
        }
    }
}

/// How the search finds where a match can start, beyond the first set.
#[derive(Clone, Debug, Default)]
pub(crate) enum Plan {
    /// Every position the first set allows is tried.
    #[default]
    Scan,
    /// Every match holds one of the literals so many to so many characters (`lo` to `hi`) after its start.
    Windows { literals: Literals, lo: usize, hi: usize },
    /// Every match is runs of the loops' sets, then the literal.
    AfterLoops { literals: Literals, sets: Vec<Arc<CharSet>>, mins: Vec<usize> },
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
}

/// A search for any of a few bytes: `memchr` for up to three, and the fingerprint search of one-byte literals for
/// up to sixty-four, which is a vectorised search for a set of bytes.
#[derive(Clone, Debug)]
pub(crate) enum FirstBytes {
    One(u8),
    Two(u8, u8),
    Three(u8, u8, u8),
    Packed(packed::Searcher),
}

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
                packed::Config::new().builder().extend(list.iter().map(|&b| [b])).build().map(FirstBytes::Packed)
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
            FirstBytes::Packed(s) => {
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
fn anchors(n: &Node, mut lo: usize, mut hi: usize) -> Option<Vec<Anchor>> {
    let ts = terms(n);
    let mut k = 0;
    while k < ts.len() {
        let t = ts[k];
        if forms(t).is_some() {
            let mut chars = Vec::new();
            while k < ts.len()
                && let Some(f) = forms(ts[k])
            {
                chars.push(f);
                k += 1;
            }
            return Some(vec![Anchor { chars, lo, hi }]);
        }
        let alternation = match t.kind {
            NodeKind::Alt => Some(t),
            NodeKind::Group if t.subs[0].kind == NodeKind::Alt => Some(&t.subs[0]),
            _ => None,
        };
        if let Some(alt) = alternation {
            let mut all = Vec::new();
            for sub in &alt.subs {
                all.extend(anchors(sub, lo, hi)?);
            }
            return Some(all);
        }
        let (a, b) = width(t)?;
        lo = lo.saturating_add(a);
        hi = hi.saturating_add(b?);
        if hi > MAX_WINDOW {
            return None;
        }
        k += 1;
    }
    None
}

/// The literals of an anchor: its characters' forms written out, for as many of its first characters as keep them
/// to `budget` literals (and always the first character's).
fn expand(a: &Anchor, budget: usize) -> Vec<Vec<Rune>> {
    let mut out: Vec<Vec<Rune>> = vec![Vec::new()];
    for f in &a.chars {
        if out.len() > 1 && out.len() * f.len() > budget || out.len() == 1 && f.len() > budget.max(4) {
            break;
        }
        out = out.iter().flat_map(|prefix| f.iter().map(move |&r| [&prefix[..], &[r]].concat())).collect();
    }
    out
}

/// Whether a node can only begin with an ASCII word character, or is not known to.
fn starts_with_word(n: &Node, first: Option<&CharSet>) -> bool {
    let _ = n;
    first.is_some_and(|f| {
        f.ranges.last().is_some_and(|&last| last < 0x80) && (0..0x80u8).all(|c| !f.has_ascii(c) || is_word_byte(c))
    })
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
    let mut search = Search { first_bytes: first.and_then(|f| FirstBytes::new(f, units)), ..Search::default() };
    let ts = terms(root);
    // A leading \b before characters that are all ASCII word characters.
    let boundary = ts.first().is_some_and(|t| t.kind == NodeKind::WordBoundary && !t.fold);
    if boundary && starts_with_word(root, first) {
        search.leading_word_boundary = true;
    }
    if let Some(found) = anchors(root, 0, 0) {
        let mut literals: Vec<Vec<Rune>> = Vec::new();
        let (mut lo, mut hi) = (usize::MAX, 0);
        let mut forms = false;
        // The forms of the anchors share what the fingerprint search takes, so that each is written out for as many
        // characters as its share allows.
        let budget = (MAX_FORMS / found.len()).max(1);
        for a in &found {
            lo = lo.min(a.lo);
            hi = hi.max(a.hi);
            forms |= a.chars.iter().any(|f| f.len() > 1);
            for l in expand(a, budget) {
                if !literals.contains(&l) {
                    literals.push(l);
                }
            }
        }
        if !literals.is_empty() && literals.len() <= MAX_LITERALS && literals.iter().all(|l| !l.is_empty()) {
            let fingerprint = literals.len() >= 3 || forms;
            search.plan = Plan::Windows { literals: Literals::new(&literals, fingerprint), lo, hi };
            return search;
        }
    }
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
        while let Some(f) = ts.get(k).and_then(|t| forms(t)) {
            chars.push(f);
            k += 1;
        }
        if (!boundary || word_set) && disjoint && mins.iter().all(|&m| m <= 1) && !chars.is_empty() {
            let literals = expand(&Anchor { chars, lo: 0, hi: 0 }, MAX_FORMS);
            let fingerprint = literals.len() >= 2;
            search.plan =
                Plan::AfterLoops { literals: Literals::new(&literals, fingerprint), sets: sets.clone(), mins };
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
