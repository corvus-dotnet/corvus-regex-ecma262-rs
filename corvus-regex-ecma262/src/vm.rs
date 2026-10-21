//! The backtracking matcher (`vm.go`).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use super::Rune;
use std::sync::Arc;

use super::adfa::{Found, Scan};
use super::charset::CharSet;
use super::compile::{Inst, MAX_MEMO_LOOPS, Op, Program};
use super::fold::{canonicalize, is_fold_word_rune};
use super::search::{Literals, Plan, RunEnd, Shape};
use super::units::{Text, is_word_unit};

/// Why a match was abandoned. Neither is an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchError {
    /// The match ran for longer than its limit.
    Time,
    /// The match needed more backtracking memory than its limit: its stack reached [`MAX_STACK`] entries.
    Memory,
}

impl std::fmt::Display for MatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MatchError::Time => "ecmaregex: the match ran for longer than its limit",
            MatchError::Memory => "ecmaregex: the match needed more backtracking memory than its limit",
        })
    }
}

impl std::error::Error for MatchError {}

/// The number of steps the matcher takes between two looks at the clock and at the depth of its stack. A step is
/// one instruction, and eight characters of a single character loop or of a backreference. A step takes a few
/// nanoseconds, so the clock is read every ten to twenty microseconds, a match that ends before its first check
/// never reads it, and a match goes past its limit by that much at most.
pub(crate) const CHECK_EVERY: i64 = 1 << 12;
/// The number of entries the backtracking stack of a match with a limit may hold (24 bytes each). A match that
/// needs more is abandoned. A pattern that keeps a choice for every character of its text needs one entry or a few
/// for each, so this is room for a text of some hundred thousand characters, and a pattern that pushes without
/// consuming, such as `(?:a?){2147483647}`, reaches it in a few milliseconds. It is looked at with the clock, so
/// the stack can go past it by `CHECK_EVERY` entries.
pub const MAX_STACK: usize = 1 << 20;

/// The start `match_with` gives for no match, and for a match abandoned at its limit of time or of memory: no
/// position of a text is any of them.
const NO_MATCH: usize = usize::MAX;
const STOPPED_TIME: usize = usize::MAX - 1;
const STOPPED_MEMORY: usize = usize::MAX - 2;

// Why a match was abandoned.
const STOP_NONE: u8 = 0;
const STOP_TIME: u8 = 1;
const STOP_MEMORY: u8 = 2;

// The kinds of entry on the backtracking stack. A choice entry says where to resume. An undo entry restores a value
// that was overwritten after the entries below it were pushed.

/// Resumes at `pc` with the position `pos`.
const E_CHOICE: u8 = 0;
/// Belongs to a greedy single character loop that stopped at `pos`. Resuming gives back one character, down to the
/// position `a`.
const E_REP: u8 = 1;
/// Belongs to a lazy single character loop that has taken `a` characters and stands at `pos`. Resuming takes one
/// more.
const E_REP_LAZY: u8 = 2;
/// `E_LOOK` marks the start of a positive lookaround and `E_LOOK_NEG` of a negative one. Both hold the position to
/// restore, and `E_LOOK_NEG` the instruction to continue at when its body has failed.
const E_LOOK: u8 = 3;
const E_LOOK_NEG: u8 = 4;
/// Lies under the choices of a memo point. When it is reached, every way from the point has failed, and the bit
/// `pos` of the memory is set (see `memo` in [`State`]).
const E_MARK: u8 = 5;
/// Restores the entry position of group `pc` to `pos`.
const E_TMP: u8 = 6;
/// Restores the start and end of group `pc` to `pos` and `a`.
const E_CAP: u8 = 7;
/// Restores the count and the iteration start of loop `pc` to `pos` and `a`.
const E_COUNTER: u8 = 8;

#[derive(Clone, Copy)]
struct Entry {
    kind: u8,
    pc: i32,
    pos: usize,
    a: usize,
}

/// The value of a group that has not taken part, and of the start of an iteration that has not started.
const NONE: usize = usize::MAX;

/// The working memory of one match. Each thread keeps one and every match on the thread uses it, so a match
/// allocates only while the stack is still growing to the depth the pattern and the text need. (The Go matcher
/// pools one for each program. A thread matches one pattern at a time, so one for each thread serves, and the
/// memory a process keeps does not grow with the number of its patterns.)
#[derive(Default)]
struct State {
    stack: Vec<Entry>,
    /// `caps` holds the start and end of each tracked group, or `NONE`, and `tmp` the position at which each was
    /// entered.
    caps: Vec<usize>,
    tmp: Vec<usize>,
    /// For each loop, the number of iterations started and the position at which the last one started.
    counts: Vec<usize>,
    /// Where the match that was found ends.
    end: usize,
    /// The time the match may take, or `None` for no limit. The clock starts at the first check, so `deadline` is
    /// set when `timed` is.
    limit: Option<Duration>,
    deadline: Option<Instant>,
    /// The number of steps left before the next check, and why the match was abandoned.
    steps: i64,
    stop: u8,
    /// The memory of the places where a way has failed: one bit for each memo point of the program, each position
    /// of the text and each state of the loops around the point (see `MemoPoint`). A bit is set when every way from
    /// the place has failed, and a way that comes to a place whose bit is set fails at once. So no place is gone
    /// through twice, and a pattern of repetitions inside repetitions takes time proportional to its size times the
    /// text and not time that doubles with each character.
    ///
    /// The memory costs its size to clear, and a mark on the stack for each place gone through, so a match starts
    /// without it. It takes it at the check by which it has made as many steps as the memory has words and
    /// `MEMO_STEPS_PER_POSITION` more for each position of the text, which a match that goes through its text once
    /// or a few times never makes. A place whose way failed before then is not remembered, which loses time and
    /// nothing else. `memo_on` says the memory is in use, `memo_off` that it never will be (the program has no memo
    /// points, or the memory would be larger than `MAX_MEMO_WORDS`), and `width` is the number of positions of the
    /// text.
    memo: Vec<u64>,
    memo_on: bool,
    memo_off: bool,
    width: usize,
    checks: usize,
    points: usize,
}

/// The largest memory of failed places, in words of 64 bits: 8 MB. A match that would need more goes without it.
pub const MAX_MEMO_WORDS: usize = 1 << 20;
/// The number of steps for each position of the text that a match makes before it takes the memory into use.
const MEMO_STEPS_PER_POSITION: usize = 64;
/// The largest memory kept between matches.
const MAX_RETAINED_MEMO: usize = 1 << 16;
/// The depth of the backtracking stack beyond which a memo point is not marked. A mark is an entry of the stack,
/// and a pattern that keeps no choice for a character still marks a place for each, so on a long text the marks
/// alone would reach `MAX_STACK`. A place that is not marked is not remembered, which loses time and nothing else,
/// and the places below this depth are remembered when the stack unwinds, so a later start has the room to mark the
/// next ones.
const MAX_MARK_DEPTH: usize = MAX_STACK / 2;
/// The largest stack kept between matches.
const MAX_RETAINED_STACK: usize = 1 << 16;

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// Makes every match on this thread take its memory of failed places before its first step. The tests set it,
    /// to run every pattern they have with the memory in use.
    #[cfg(test)]
    static MEMO_FROM_THE_START: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Sets whether every match on this thread takes its memory of failed places before its first step.
#[cfg(test)]
pub(crate) fn set_memo_from_the_start(on: bool) {
    MEMO_FROM_THE_START.with(|m| m.set(on));
}

/// Whether every match takes its memory of failed places before its first step. A build with
/// `--cfg corvus_memo_from_the_start` does, which is how the test data is run with the memory in use for every
/// pattern.
#[inline(always)]
fn memo_from_the_start() -> bool {
    #[cfg(test)]
    if MEMO_FROM_THE_START.with(|m| m.get()) {
        return true;
    }
    cfg!(corvus_memo_from_the_start)
}

impl State {
    /// Takes the memory of failed places into use when the match has made enough steps to pay for it.
    fn remember(&mut self) {
        let words = (((self.points * self.width) << MAX_MEMO_LOOPS) + 63) >> 6;
        if words > MAX_MEMO_WORDS {
            self.memo_off = true;
            return;
        }
        if self.checks * (CHECK_EVERY as usize) < words + MEMO_STEPS_PER_POSITION * self.width && !memo_from_the_start()
        {
            return;
        }
        self.memo.clear();
        self.memo.resize(words, 0);
        self.memo_on = true;
    }

    /// Called every `CHECK_EVERY` steps. It reports whether the match may go on.
    #[cold]
    #[inline(never)]
    fn check(&mut self, depth: usize) -> bool {
        if !self.memo_on && !self.memo_off {
            self.checks += 1;
            self.remember();
        }
        let Some(limit) = self.limit else { return true };
        if depth > MAX_STACK {
            self.stop = STOP_MEMORY;
            return false;
        }
        let now = Instant::now();
        match self.deadline {
            None => self.deadline = Some(now + limit),
            Some(deadline) => {
                if now > deadline {
                    self.stop = STOP_TIME;
                    return false;
                }
            }
        }
        true
    }

    /// Applies one undo entry.
    #[inline(always)]
    fn undo(&mut self, e: Entry) {
        let k = e.pc as usize;
        match e.kind {
            E_TMP => self.tmp[k] = e.pos,
            E_CAP => (self.caps[2 * k], self.caps[2 * k + 1]) = (e.pos, e.a),
            E_COUNTER => (self.counts[2 * k], self.counts[2 * k + 1]) = (e.pos, e.a),
            _ => {}
        }
    }
}

/// The first place at or after `at` that the window `skip` does not rule out: one unit read rules out the starts of a
/// window none of whose positions it can be. A call of its own, so that the search it is part of keeps its size.
#[inline(never)]
fn skip_ahead<T: Text>(skip: &super::adfa::Skip, input: &[T::Unit], mut at: usize) -> usize {
    while at + skip.hi < input.len() && !skip.hit(input[at + skip.hi].into()) {
        at += skip.len;
    }
    at
}

/// Whether a position of UTF-16 text is between the halves of a pair.
#[inline]
fn mid_pair(units: &[u16], at: usize) -> bool {
    at > 0 && at < units.len() && (0xDC00..=0xDFFF).contains(&units[at]) && (0xD800..=0xDBFF).contains(&units[at - 1])
}

impl Program {
    /// The bit of the memory for the memo point `m` at a position, with the state of the loops around the point:
    /// for each, whether it would refuse an iteration that ended here as empty.
    #[inline]
    fn memo_index(&self, st: &State, m: i32, pos: usize) -> usize {
        let pt = &self.points[(m - 1) as usize];
        let mut idx = ((m - 1) as usize * st.width + pos) << MAX_MEMO_LOOPS;
        for k in 0..MAX_MEMO_LOOPS {
            let l = pt.loops[k];
            if l >= 0 && st.counts[2 * l as usize] > pt.mins[k] as usize && st.counts[2 * l as usize + 1] == pos {
                idx |= 1 << k;
            }
        }
        idx
    }

    /// Finds the first match at or after `start` (at `start` alone with `sticky`), and returns where it starts and
    /// ends. When it finds one and `caps` is given, `caps` receives the start and end of each group the program
    /// records, or `usize::MAX` for a group that did not take part. With a limit the match is abandoned when it has
    /// run for longer than that, or when its stack has reached `MAX_STACK`, and the error says which.
    pub(crate) fn exec<T: Text>(
        &self,
        input: &[T::Unit],
        start: usize,
        sticky: bool,
        limit: Option<Duration>,
        caps: Option<&mut [usize]>,
    ) -> Result<Option<(usize, usize)>, MatchError> {
        // A pattern of a simple shape with no group is answered by the search alone, with nothing of the matcher's.
        if self.groups == 0
            && !sticky
            && !self.anchored
            && let Some(found) = self.shape_search::<T>(input, start)
        {
            return Ok(found);
        }
        let (from, to) = STATE.with(|cell| match cell.try_borrow_mut() {
            Ok(mut st) => self.match_with::<T>(&mut st, input, start, sticky, limit, caps),
            // The thread is already inside a match, which nothing in the crate does. A match is then given a
            // memory of its own.
            Err(_) => self.match_with::<T>(&mut State::default(), input, start, sticky, limit, caps),
        });
        match from {
            NO_MATCH => Ok(None),
            STOPPED_TIME => Err(MatchError::Time),
            STOPPED_MEMORY => Err(MatchError::Memory),
            _ => Ok(Some((from, to))),
        }
    }

    /// The match from `start` with the memory `st`: where it starts and ends, or a start of `NO_MATCH`,
    /// `STOPPED_TIME` or `STOPPED_MEMORY`. Two words rather than a `Result`, so that they come back in registers: a
    /// `Result` came back through memory written as two words and read as one, which stalled every match.
    fn match_with<T: Text>(
        &self,
        st: &mut State,
        input: &[T::Unit],
        start: usize,
        sticky: bool,
        limit: Option<Duration>,
        caps: Option<&mut [usize]>,
    ) -> (usize, usize) {
        st.caps.clear();
        st.caps.resize(2 * self.groups, NONE);
        st.tmp.clear();
        st.tmp.resize(self.groups, 0);
        st.counts.clear();
        st.counts.resize(2 * self.loops, 0);
        (st.limit, st.deadline, st.steps, st.stop) = (limit.filter(|l| !l.is_zero()), None, CHECK_EVERY, STOP_NONE);
        (st.memo_on, st.memo_off, st.checks) = (false, self.points.is_empty(), 0);
        (st.width, st.points) = (input.len() + 1, self.points.len());
        if !st.memo_off && memo_from_the_start() {
            st.remember();
        }
        let found = self.search::<T>(st, input, start, sticky);
        let stop = st.stop;
        if let (Some(_), Some(caps)) = (found, caps) {
            caps[..st.caps.len()].copy_from_slice(&st.caps);
        }
        if st.stack.capacity() > MAX_RETAINED_STACK {
            st.stack = Vec::new();
        }
        if st.memo.capacity() > MAX_RETAINED_MEMO {
            st.memo = Vec::new();
        }
        match stop {
            STOP_TIME => (STOPPED_TIME, 0),
            STOP_MEMORY => (STOPPED_MEMORY, 0),
            _ => found.unwrap_or((NO_MATCH, 0)),
        }
    }

    fn search<T: Text>(&self, st: &mut State, input: &[T::Unit], start: usize, sticky: bool) -> Option<(usize, usize)> {
        let len = input.len();
        let need = if T::UTF8 { self.need } else { self.need16 };
        if start > len || len - start < need {
            return None;
        }
        if sticky || self.anchored {
            if self.anchored && start > 0 {
                return None;
            }
            return self.run::<T>(st, input, start).then_some((start, st.end));
        }
        match &self.search.plan {
            Plan::Windows { literals, lo, hi, reverse, rest } => self.search_windows::<T>(
                st,
                input,
                start,
                literals,
                (*lo, *hi),
                reverse.as_ref(),
                rest.as_deref(),
                need,
            ),
            Plan::AfterLoops { literals, sets, mins, rest } => {
                self.search_after_loops::<T>(st, input, start, literals, sets, mins, rest.as_deref(), need)
            }
            Plan::Scan => self.scan::<T>(st, input, start, need),
        }
    }

    /// The matches of a global search from `at` (with no limit), as many as `spans` holds, written to its start:
    /// how many, where the search goes on, and whether it has found every match. The matches `exec` finds one call
    /// at a time, in one loop over the batch: a pattern of a simple shape by the search inlined into the loop (one
    /// class and one loop of a class each by a loop of its own), any other by the matcher with the thread's memory
    /// taken once for the batch.
    pub(crate) fn exec_batch<T: Text, S: From<(usize, usize)>>(
        &self,
        input: &[T::Unit],
        mut at: usize,
        unicode: bool,
        sticky: bool,
        spans: &mut [S],
    ) -> (usize, usize, bool) {
        let mut n = 0;
        if self.groups == 0 && !sticky && !self.anchored {
            let done;
            (n, at, done) = self.shape_batch::<T, S>(input, at, unicode, spans);
            if let Some(done) = done {
                return (n, at, done);
            }
        }
        STATE.with(|cell| match cell.try_borrow_mut() {
            Ok(mut st) => self.matcher_batch::<T, S>(&mut st, input, at, unicode, sticky, spans, n),
            Err(_) => self.matcher_batch::<T, S>(&mut State::default(), input, at, unicode, sticky, spans, n),
        })
    }

    /// `exec_batch` for a pattern of a simple shape: the matches written, where the search goes on, and whether it
    /// has found every match, or `None` when the pattern is not of a simple shape (or not after all), for the matcher
    /// to go on from there.
    #[inline(always)]
    fn shape_batch<T: Text, S: From<(usize, usize)>>(
        &self,
        input: &[T::Unit],
        mut at: usize,
        unicode: bool,
        spans: &mut [S],
    ) -> (usize, usize, Option<bool>) {
        let mut n = 0;
        let (len, need) = (input.len(), if T::UTF8 { self.need } else { self.need16 });
        // One class and one loop of a class, the shapes with the most matches, each by a loop of its own.
        match &self.search.shape {
            Shape::Set(set) => {
                while n < spans.len() {
                    let start = T::normalize(input, at, unicode);
                    if start > len || len - start < need {
                        return (n, at, Some(true));
                    }
                    let Some((from, w)) = self.next_member::<T>(set, input, start) else {
                        return (n, at, Some(true));
                    };
                    // A member is never empty.
                    spans[n] = S::from((from, from + w));
                    n += 1;
                    at = from + w;
                }
                return (n, at, Some(false));
            }
            Shape::Loop(set, min, end) => {
                while n < spans.len() {
                    let start = T::normalize(input, at, unicode);
                    if start > len || len - start < need {
                        return (n, at, Some(true));
                    }
                    let Some((from, to)) = self.loop_match::<T>(set, *min, end, input, start) else {
                        return (n, at, Some(true));
                    };
                    spans[n] = S::from((from, to));
                    n += 1;
                    at = if to != from {
                        to
                    } else if to < len && input[to].into() < 0x80 {
                        // An empty match before an ASCII character (a loop of `.` before a line's end) goes on one
                        // unit later in either encoding, with or without the u flag.
                        to + 1
                    } else {
                        T::advance(input, to, unicode)
                    };
                }
                return (n, at, Some(false));
            }
            Shape::None => return (n, at, None),
            _ => {}
        }
        while n < spans.len() {
            let start = T::normalize(input, at, unicode);
            match self.shape_search::<T>(input, start) {
                Some(Some((from, to))) => {
                    spans[n] = S::from((from, to));
                    n += 1;
                    at = if to == from { T::advance(input, to, unicode) } else { to };
                }
                Some(None) => return (n, at, Some(true)),
                // Not answered by the search after all: the matcher goes on from here.
                None => return (n, at, None),
            }
        }
        (n, at, Some(false))
    }

    /// The matches of a global search from `at` (with no limit) with what each group captured, as many as `groups`
    /// holds at `width` entries a match (the match, then each group the program records, `None` for one that did
    /// not take part), written to its start: how many matches, where the search goes on, and whether it has found
    /// every match. A program that records no group is answered as `exec_batch` answers it.
    pub(crate) fn exec_batch_groups<T: Text, S: From<Option<(usize, usize)>>>(
        &self,
        input: &[T::Unit],
        mut at: usize,
        unicode: bool,
        sticky: bool,
        groups: &mut [S],
        width: usize,
    ) -> (usize, usize, bool) {
        debug_assert_eq!(width, self.groups + 1);
        if self.groups == 0 {
            let mut spans = [(0, 0); 64];
            let mut n = 0;
            while n < groups.len() {
                let want = (groups.len() - n).min(spans.len());
                let done;
                let got;
                (got, at, done) = self.exec_batch::<T, (usize, usize)>(input, at, unicode, sticky, &mut spans[..want]);
                for (slot, &span) in groups[n..].iter_mut().zip(&spans[..got]) {
                    *slot = S::from(Some(span));
                }
                n += got;
                if done {
                    return (n, at, true);
                }
            }
            return (n, at, false);
        }
        STATE.with(|cell| match cell.try_borrow_mut() {
            Ok(mut st) => self.groups_batch::<T, S>(&mut st, input, at, unicode, sticky, groups, width),
            Err(_) => self.groups_batch::<T, S>(&mut State::default(), input, at, unicode, sticky, groups, width),
        })
    }

    /// `exec_batch_groups` by the matcher, with the memory `st`.
    #[allow(clippy::too_many_arguments)]
    fn groups_batch<T: Text, S: From<Option<(usize, usize)>>>(
        &self,
        st: &mut State,
        input: &[T::Unit],
        mut at: usize,
        unicode: bool,
        sticky: bool,
        groups: &mut [S],
        width: usize,
    ) -> (usize, usize, bool) {
        let mut n = 0;
        for one in groups.chunks_exact_mut(width) {
            let start = T::normalize(input, at, unicode);
            let (from, to) = self.match_with::<T>(st, input, start, sticky, None, None);
            if from >= STOPPED_MEMORY {
                return (n, at, true);
            }
            one[0] = S::from(Some((from, to)));
            // What the match left in the memory is what each group captured.
            for (slot, &[from, to]) in one[1..].iter_mut().zip(st.caps.as_chunks::<2>().0) {
                *slot = S::from((from != NONE).then_some((from, to)));
            }
            n += 1;
            at = if to == from { T::advance(input, to, unicode) } else { to };
        }
        (n, at, false)
    }

    /// `exec_batch` by the matcher, with the memory `st`, from `n` matches written.
    #[allow(clippy::too_many_arguments)]
    fn matcher_batch<T: Text, S: From<(usize, usize)>>(
        &self,
        st: &mut State,
        input: &[T::Unit],
        mut at: usize,
        unicode: bool,
        sticky: bool,
        spans: &mut [S],
        mut n: usize,
    ) -> (usize, usize, bool) {
        while n < spans.len() {
            let start = T::normalize(input, at, unicode);
            let (from, to) = self.match_with::<T>(st, input, start, sticky, None, None);
            // No match (with no limit, a match is never abandoned).
            if from >= STOPPED_MEMORY {
                return (n, at, true);
            }
            spans[n] = S::from((from, to));
            n += 1;
            at = if to == from { T::advance(input, to, unicode) } else { to };
        }
        (n, at, false)
    }

    /// The first match at or after `start` of a pattern of a simple shape, or `None` when it has none.
    #[inline]
    fn shape_search<T: Text>(&self, input: &[T::Unit], start: usize) -> Option<Option<(usize, usize)>> {
        let len = input.len();
        let need = if T::UTF8 { self.need } else { self.need16 };
        if matches!(self.search.shape, Shape::None) {
            return None;
        }
        if start > len || len - start < need {
            return Some(None);
        }
        let units = self.units;
        let bytes = T::bytes(input);
        Some(match &self.search.shape {
            Shape::None => unreachable!(),
            Shape::Literals => {
                let Plan::Windows { literals, .. } = &self.search.plan else { return None };
                literals.find_match(bytes, start, T::UTF8)
            }
            Shape::Forms(alternatives) => {
                let Plan::Windows { literals, .. } = &self.search.plan else { return None };
                let mut next = start;
                loop {
                    let Some(q) = literals.find(bytes, next, T::UTF8) else { break None };
                    // The automaton compares several alternatives at once where the text is ASCII; one is compared
                    // faster as it is.
                    let dfa = if alternatives.len() > 1 { self.anchored_dfa.as_ref() } else { None };
                    match dfa.map(|d| d.find(input, q)) {
                        Some(Found::End(end)) => break Some((q, end)),
                        Some(Found::None) => {
                            next = q + T::read(input, q, units).1;
                            continue;
                        }
                        _ => {}
                    }
                    let mut found = None;
                    'alternatives: for alt in alternatives {
                        let mut p = q;
                        for f in alt {
                            if p >= len {
                                continue 'alternatives;
                            }
                            let c: u32 = input[p].into();
                            let (r, w) = if c < 0x80 { (c as Rune, 1) } else { T::read(input, p, units) };
                            if !f.contains(&r) {
                                continue 'alternatives;
                            }
                            p += w;
                        }
                        found = Some((q, p));
                        break;
                    }
                    if found.is_some() {
                        break found;
                    }
                    next = q + T::read(input, q, units).1;
                }
            }
            Shape::Set(set) => self.next_member::<T>(set, input, start).map(|(at, w)| (at, at + w)),
            Shape::Loop(set, min, end) => self.loop_match::<T>(set, *min, end, input, start),
        })
    }

    /// The first match at or after `start` of the shape of one greedy loop of a class (`Shape::Loop`).
    #[inline(always)]
    fn loop_match<T: Text>(
        &self,
        set: &CharSet,
        min: usize,
        end: &RunEnd,
        input: &[T::Unit],
        start: usize,
    ) -> Option<(usize, usize)> {
        let from = if min == 0 { start } else { self.next_member::<T>(set, input, start)?.0 };
        let to = if T::UTF8 { end.end8(T::bytes(input), from) } else { end.end16(T::units16(input), from) };
        Some((from, to.unwrap_or_else(|| self.end_of_run::<T>(set, input, from))))
    }

    /// The first member of the set at or after `at`, and its width.
    #[inline]
    fn next_member<T: Text>(&self, set: &CharSet, input: &[T::Unit], mut at: usize) -> Option<(usize, usize)> {
        let first_bytes = if T::UTF8 { self.search.first_bytes.as_ref() } else { None };
        let first_units = if T::UTF8 { None } else { self.search.first_units.as_ref() };
        let bytes = T::bytes(input);
        let units16 = T::units16(input);
        loop {
            if let Some(fb) = first_bytes {
                at = fb.find(bytes, at)?;
            } else if let Some(fu) = first_units {
                at = fu.find(units16, at)?;
                // With the u or v flag the trail half of a pair is not a character a match can start at.
                if !self.units && mid_pair(units16, at) {
                    at += 1;
                    continue;
                }
            }
            if at >= input.len() {
                return None;
            }
            let c: u32 = input[at].into();
            let (r, w) = if c < 0x80 { (c as Rune, 1) } else { T::read(input, at, self.units) };
            if if c < 0x80 { set.has_ascii(c as u8) } else { set.contains(r) } {
                return Some((at, w));
            }
            at += w;
        }
    }

    /// Whether a start passes the first set.
    #[inline(always)]
    fn first_allows<T: Text>(&self, input: &[T::Unit], at: usize) -> bool {
        let Some(first) = &self.first else { return true };
        if at >= input.len() {
            return false;
        }
        let c: u32 = input[at].into();
        if c < 0x80 { first.has_ascii(c as u8) } else { first.contains(T::read(input, at, self.units).0) }
    }

    /// Tries the start: whether the program matches there, the reason for stopping being in `st.stop`.
    #[inline(always)]
    fn try_at<T: Text>(&self, st: &mut State, input: &[T::Unit], at: usize) -> bool {
        self.first_allows::<T>(input, at) && self.run::<T>(st, input, at)
    }

    /// The position `n` characters before `at`, not before `floor`: the furthest it gets when there are fewer.
    #[inline]
    fn walk_back<T: Text>(&self, input: &[T::Unit], mut at: usize, n: usize, floor: usize) -> (usize, usize) {
        let mut k = 0;
        while k < n && at > floor {
            let w = T::read_last(input, at, self.units).1;
            if at - w < floor {
                break;
            }
            at -= w;
            k += 1;
        }
        (at, k)
    }

    /// The search of a pattern each of whose matches holds one of the literals `lo` to `hi` characters after its
    /// start: each occurrence opens the window of starts that could reach it, tried in order and never twice.
    #[allow(clippy::too_many_arguments)]
    fn search_windows<T: Text>(
        &self,
        st: &mut State,
        input: &[T::Unit],
        start: usize,
        literals: &Literals,
        (lo, hi): (usize, usize),
        reverse: Option<&super::adfa::Reverse>,
        rest: Option<&super::search::RestCheck>,
        need: usize,
    ) -> Option<(usize, usize)> {
        if let Some(rev) = reverse
            && hi < 127
            && let Ok(found) = self.search_reverse::<T>(st, input, start, literals, lo, hi, rev, rest, need)
        {
            return found;
        }
        let len = input.len();
        let bytes = T::bytes(input);
        let mut next = start;
        // Every start before this has been tried.
        let mut untried = start;
        loop {
            let q = literals.find(bytes, next, T::UTF8)?;
            next = q + T::read(input, q, self.units).1;
            if hi == 0 {
                // Every alternative starts with its literal: the match starts at the occurrence or not at all.
                if len - q < need {
                    return None;
                }
                if T::UTF8
                    && let (Some(after), Some(dfa)) = (&self.after_literal, &self.anchored_dfa)
                {
                    // The occurrence is the literal every match starts with: the automaton goes on after it.
                    match dfa.find_after(input, q, q + after.1, &after.0) {
                        Found::End(end) => return Some((q, end)),
                        Found::None => continue,
                        Found::Bail => {}
                    }
                }
                if self.try_at::<T>(st, input, q) {
                    return Some((q, st.end));
                }
                if st.stop != STOP_NONE {
                    return None;
                }
                continue;
            }
            let (last, back) = self.walk_back::<T>(input, q, lo, start);
            if back < lo || last < untried || rest.is_some_and(|r| r.rules_out(input, q)) {
                continue;
            }
            let (mut s, _) = self.walk_back::<T>(input, q, hi, start);
            while s <= last {
                if s >= untried {
                    if len - s < need {
                        return None;
                    }
                    if self.try_at::<T>(st, input, s) {
                        return Some((s, st.end));
                    }
                    if st.stop != STOP_NONE {
                        return None;
                    }
                }
                if s == len {
                    break;
                }
                s += T::read(input, s, self.units).1;
            }
            untried = untried.max(s);
        }
    }

    /// [`Program::search_windows`] with the terms before the anchors read backwards from each occurrence, so that only
    /// the starts they allow are tried. A start is tried only once no later occurrence could allow an earlier one (it
    /// is before the next occurrence's window), so the starts are tried in order across occurrences and the first
    /// match is the leftmost. Over ASCII a character is a unit; `Err` when a unit beyond ASCII is reached, and the
    /// search is then made again the plain way.
    #[allow(clippy::too_many_arguments)]
    fn search_reverse<T: Text>(
        &self,
        st: &mut State,
        input: &[T::Unit],
        start: usize,
        literals: &Literals,
        lo: usize,
        hi: usize,
        rev: &super::adfa::Reverse,
        rest: Option<&super::search::RestCheck>,
        need: usize,
    ) -> Result<Option<(usize, usize)>, ()> {
        let len = input.len();
        let bytes = T::bytes(input);
        // The starts waiting to be tried: bit k is the position base + k.
        let (mut pending, mut base) = (0u128, start);
        // Every start at or before this has been tried, or can no longer be allowed.
        let mut tried: Option<usize> = None;
        let Some(mut q) = literals.find(bytes, start, T::UTF8) else { return Ok(None) };
        loop {
            // The starts this occurrence's terms allow join those waiting.
            let new_base = q.saturating_sub(hi).max(start);
            if new_base > base {
                let shift = new_base - base;
                pending = if shift >= 128 { 0 } else { pending >> shift };
                base = new_base;
            }
            // An occurrence from which nothing can match allows no start.
            let mut mask = if rest.is_some_and(|r| r.rules_out(input, q)) {
                0
            } else {
                rev.starts(input, q, hi, start).ok_or(())?
            };
            mask &= !((1u128 << lo) - 1);
            while mask != 0 {
                let d = 127 - mask.leading_zeros() as usize;
                mask &= !(1u128 << d);
                let s = q - d;
                if s >= base && tried.is_none_or(|t| s > t) {
                    pending |= 1u128 << (s - base);
                }
            }
            let after = q + T::read(input, q, self.units).1;
            // The earliest waiting start is tried once no occurrence before it plus the window could allow an
            // earlier one: an occurrence within that reach is taken in first, which looks no further than the reach.
            let mut taken = false;
            while pending != 0 {
                let k = pending.trailing_zeros() as usize;
                let s = base + k;
                if after <= s + hi
                    && let Some(q2) = literals.find_within(bytes, after, s + hi, T::UTF8)
                {
                    q = q2;
                    taken = true;
                    break;
                }
                pending &= !(1u128 << k);
                tried = Some(s);
                if len - s < need {
                    return Ok(None);
                }
                if self.try_at::<T>(st, input, s) {
                    return Ok(Some((s, st.end)));
                }
                if st.stop != STOP_NONE {
                    return Ok(None);
                }
            }
            if !taken {
                let Some(q2) = literals.find(bytes, after, T::UTF8) else { return Ok(None) };
                q = q2;
            }
        }
    }

    /// The search of a pattern each of whose matches is runs of the loops' sets, then the literal: the runs before
    /// each occurrence are walked back to the earliest start, the only one tried for it.
    #[allow(clippy::too_many_arguments)]
    fn search_after_loops<T: Text>(
        &self,
        st: &mut State,
        input: &[T::Unit],
        start: usize,
        literals: &Literals,
        sets: &[Arc<CharSet>],
        mins: &[usize],
        rest: Option<&super::search::RestCheck>,
        need: usize,
    ) -> Option<(usize, usize)> {
        let len = input.len();
        let bytes = T::bytes(input);
        let units = self.units;
        let mut next = start;
        let mut tried: Option<usize> = None;
        'occurrences: loop {
            let q = literals.find(bytes, next, T::UTF8)?;
            next = q + T::read(input, q, units).1;
            // A loop that needs a member must have one just before the occurrence: a test of one character, made
            // before the rest is checked.
            let last = sets.len() - 1;
            if mins[last] == 1
                && (q <= start || {
                    let c: u32 = input[q - 1].into();
                    let r = if c < 0x80 { c as Rune } else { T::read_last(input, q, units).0 };
                    !sets[last].contains(r)
                })
            {
                continue;
            }
            if rest.is_some_and(|r| r.rules_out(input, q)) {
                continue;
            }
            let mut s = q;
            for k in (0..sets.len()).rev() {
                let run_end = s;
                while s > start {
                    let c: u32 = input[s - 1].into();
                    let (r, w) = if c < 0x80 { (c as Rune, 1) } else { T::read_last(input, s, units) };
                    if !sets[k].contains(r) || s - w < start {
                        break;
                    }
                    s -= w;
                }
                if mins[k] == 1 && s == run_end {
                    continue 'occurrences;
                }
            }
            if tried.is_some_and(|t| s <= t) {
                continue;
            }
            if len - s < need {
                return None;
            }
            if self.try_at::<T>(st, input, s) {
                return Some((s, st.end));
            }
            if st.stop != STOP_NONE {
                return None;
            }
            tried = Some(s);
            if sets.len() == 1 {
                // Every occurrence in the rest of the loop's run walks back to the start just tried.
                next = next.max(self.end_of_run::<T>(&sets[0], input, q));
            }
        }
    }

    /// The end of the run of members of a set from a position.
    #[inline]
    fn end_of_run<T: Text>(&self, set: &CharSet, input: &[T::Unit], mut at: usize) -> usize {
        while at < input.len() {
            let c: u32 = input[at].into();
            let (r, w) = if c < 0x80 { (c as Rune, 1) } else { T::read(input, at, self.units) };
            if !set.contains(r) {
                break;
            }
            at += w;
        }
        at
    }

    /// The search that tries every position the first set allows.
    /// The forward automaton, built on the first search that wants it.
    #[inline]
    fn forward(&self) -> Option<&super::adfa::Anchored> {
        let root = self.forward_root.as_ref()?;
        self.forward.get_or_init(|| super::adfa::build_forward(root).map(Box::new)).as_deref()
    }

    /// Tries the starts from `at` to `last` in order, as the plain search would: the first match, or `Err` with
    /// where the starts ran out.
    fn try_starts<T: Text>(
        &self,
        st: &mut State,
        input: &[T::Unit],
        mut at: usize,
        last: usize,
    ) -> Result<Option<(usize, usize)>, usize> {
        let units16 = T::units16(input);
        while at <= last && at <= input.len() {
            if !T::UTF8 && !self.units && mid_pair(units16, at) {
                at += 1;
                continue;
            }
            if at == input.len() {
                let found = self.first.is_none() && self.run::<T>(st, input, at);
                return Ok(found.then_some((at, st.end)));
            }
            let c: u32 = input[at].into();
            let (r, w) = if c < 0x80 { (c as Rune, 1) } else { T::read(input, at, self.units) };
            if self.first.as_ref().is_none_or(|first| first.contains(r)) && self.run::<T>(st, input, at) {
                return Ok(Some((at, st.end)));
            }
            if st.stop != STOP_NONE {
                return Ok(None);
            }
            at += w;
        }
        Err(at)
    }

    fn scan<T: Text>(&self, st: &mut State, input: &[T::Unit], start: usize, need: usize) -> Option<(usize, usize)> {
        let len = input.len();
        let forward = self.forward();
        let units = self.units;
        let search = &self.search;
        let mut at = start;
        let first_bytes = if T::UTF8 { search.first_bytes.as_ref() } else { None };
        let first_units = if T::UTF8 { None } else { search.first_units.as_ref() };
        let bytes = T::bytes(input);
        let units16 = T::units16(input);
        loop {
            if let Some(skip) = &self.skip {
                at = skip_ahead::<T>(skip, input, at);
            }
            if let Some(fb) = first_bytes {
                // Every match starts with a member of the first set, which starts with one of these bytes.
                at = fb.find(bytes, at)?;
            } else if let Some(fu) = first_units {
                at = fu.find(units16, at)?;
                // With the u or v flag the trail half of a pair is not a character a match can start at.
                if !self.units && mid_pair(units16, at) {
                    at += 1;
                    continue;
                }
            }
            if len - at < need {
                return None;
            }
            if at == len {
                return (self.first.is_none() && self.run::<T>(st, input, at)).then_some((at, st.end));
            }
            let c: u32 = input[at].into();
            let (r, w) = if c < 0x80 { (c as Rune, 1) } else { T::read(input, at, units) };
            if search.leading_word_boundary && at > 0 && is_word_unit(input[at - 1].into()) {
                // Inside a word, where \b cannot hold: the rest of the word is passed over.
                let first = self.first.as_ref().expect("a leading word boundary has a first set");
                at = self.end_of_run::<T>(first, input, at).max(at + w);
                continue;
            }
            if let Some(fw) = forward {
                // One pass of the forward automaton from here says where a match first ends, or that none starts
                // before a place where no thread is alive; only then are starts tried, from here to there.
                let last = match fw.scan(input, at) {
                    Scan::None => return None,
                    Scan::Idle(p) => {
                        at = p;
                        continue;
                    }
                    Scan::Match(p) | Scan::Bail(p) => p,
                };
                match self.try_starts::<T>(st, input, at, last) {
                    Ok(found) => return found,
                    Err(next) => at = next,
                }
                continue;
            }
            if self.first.as_ref().is_none_or(|first| first.contains(r)) && self.run::<T>(st, input, at) {
                return Some((at, st.end));
            }
            if st.stop != STOP_NONE {
                return None;
            }
            if let Some(set) = &search.skip_run
                && set.contains(r)
            {
                // The start failed after its leading loop ate the run from here: every start in the rest of the
                // run fails the same way.
                at = self.end_of_run::<T>(set, input, at + w).max(at + w);
                continue;
            }
            at += w;
        }
    }

    /// Reports whether the program matches at the position `start`. It keeps its choices on an explicit stack, so
    /// the depth of the search is bounded by memory and not by the stack of the thread. When it fails, every group
    /// and counter is back to the value it had on entry. It counts its steps, and when a check says the match has
    /// reached a limit it returns false with the reason in `st.stop`.
    fn run<T: Text>(&self, st: &mut State, input: &[T::Unit], start: usize) -> bool {
        if let Some(dfa) = &self.anchored_dfa {
            match dfa.find(input, start) {
                Found::End(end) => {
                    st.end = end;
                    return true;
                }
                Found::None => return false,
                Found::Bail => {}
            }
        }
        let mut stack = std::mem::take(&mut st.stack);
        stack.clear();
        let matched = self.run_on::<T>(st, &mut stack, input, start);
        st.stack = stack;
        matched
    }

    fn run_on<T: Text>(&self, st: &mut State, stack: &mut Vec<Entry>, input: &[T::Unit], start: usize) -> bool {
        let insts = &self.insts[..];
        let quick = &self.quick[..];
        // The text is matched by UTF-16 code unit (see `units`).
        let units = self.units;
        let len = input.len();
        let (mut pc, mut pos) = (0usize, start);
        let mut steps = st.steps;
        // Whether the match has taken the memory of failed places into use.
        let mut memo = st.memo_on;
        'run: loop {
            steps -= 1;
            if steps < 0 {
                if !st.check(stack.len()) {
                    st.steps = 0;
                    return false;
                }
                steps = CHECK_EVERY;
                memo = st.memo_on;
            }
            let i = &insts[pc];
            match i.op {
                Op::Char => {
                    if pos < len {
                        let c: u32 = input[pos].into();
                        if c < 0x80 {
                            if c as Rune == i.r {
                                pos += 1;
                                pc += 1;
                                continue 'run;
                            }
                        } else {
                            let (r, w) = T::read(input, pos, units);
                            if r == i.r {
                                pos += w;
                                pc += 1;
                                continue 'run;
                            }
                        }
                    }
                }
                Op::Set => {
                    if pos < len {
                        let set = set_of(i);
                        let c: u32 = input[pos].into();
                        if c < 0x80 {
                            if set.has_ascii(c as u8) {
                                pos += 1;
                                pc += 1;
                                continue 'run;
                            }
                        } else {
                            let (r, w) = T::read(input, pos, units);
                            if set.contains(r) {
                                pos += w;
                                pc += 1;
                                continue 'run;
                            }
                        }
                    }
                }
                Op::Lit => {
                    let lit = T::pick(&i.lit, &i.lit16);
                    let n = lit.len();
                    if len - pos >= n && input[pos..pos + n] == *lit {
                        pos += n;
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::CharBack | Op::SetBack => {
                    if pos > 0 {
                        let c: u32 = input[pos - 1].into();
                        let (r, w) = if c < 0x80 { (c as Rune, 1) } else { T::read_last(input, pos, units) };
                        if if i.op == Op::CharBack { r == i.r } else { set_of(i).contains(r) } {
                            pos -= w;
                            pc += 1;
                            continue 'run;
                        }
                    }
                }
                Op::Fail => {}
                Op::Bol => {
                    if pos == 0 {
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::Eol => {
                    if pos == len {
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::WordBoundary | Op::NotWordBoundary => {
                    let before = pos > 0 && is_word_unit(input[pos - 1].into());
                    let after = pos < len && is_word_unit(input[pos].into());
                    if (before != after) == (i.op == Op::WordBoundary) {
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::LineStart => {
                    if pos == 0 || T::line_terminator_before(input, pos) {
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::LineEnd => {
                    if pos == len || T::line_terminator_at(input, pos) {
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::WordBoundaryFold | Op::NotWordBoundaryFold => {
                    let before = pos > 0 && is_fold_word_rune(T::decode_last(input, pos).0);
                    let after = pos < len && is_fold_word_rune(T::decode(input, pos).0);
                    if (before != after) == (i.op == Op::WordBoundaryFold) {
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::Jmp => {
                    pc = i.x as usize;
                    continue 'run;
                }
                Op::Split => 'split: {
                    // A place from which every way has failed before fails at once. The mark is under whatever
                    // comes of this place, a choice or not, so that the place is remembered when it has all failed.
                    if memo && i.m != 0 {
                        let idx = self.memo_index(st, i.m, pos);
                        if (st.memo[idx >> 6] >> (idx & 63)) & 1 != 0 {
                            break 'split;
                        }
                        if stack.len() < MAX_MARK_DEPTH {
                            stack.push(Entry { kind: E_MARK, pc: 0, pos: idx, a: 0 });
                        }
                    }
                    // A way the next character rules out is not taken, and no choice is kept for it.
                    if !quick[i.y as usize].allows(input, pos) {
                        if !quick[i.x as usize].allows(input, pos) {
                            break 'split;
                        }
                        pc = i.x as usize;
                        continue 'run;
                    }
                    if !quick[i.x as usize].allows(input, pos) {
                        pc = i.y as usize;
                        continue 'run;
                    }
                    stack.push(Entry { kind: E_CHOICE, pc: i.y, pos, a: 0 });
                    pc = i.x as usize;
                    continue 'run;
                }
                Op::SaveTmp => {
                    let a = i.a as usize;
                    stack.push(Entry { kind: E_TMP, pc: i.a, pos: st.tmp[a], a: 0 });
                    st.tmp[a] = pos;
                    pc += 1;
                    continue 'run;
                }
                Op::Capture => {
                    let a = i.a as usize;
                    stack.push(Entry { kind: E_CAP, pc: i.a, pos: st.caps[2 * a], a: st.caps[2 * a + 1] });
                    if i.flag {
                        (st.caps[2 * a], st.caps[2 * a + 1]) = (pos, st.tmp[a]);
                    } else {
                        (st.caps[2 * a], st.caps[2 * a + 1]) = (st.tmp[a], pos);
                    }
                    pc += 1;
                    continue 'run;
                }
                Op::Clear => {
                    steps -= (i.b - i.a) as i64;
                    for g in i.a..=i.b {
                        let k = g as usize;
                        if st.caps[2 * k] != NONE {
                            stack.push(Entry { kind: E_CAP, pc: g, pos: st.caps[2 * k], a: st.caps[2 * k + 1] });
                            (st.caps[2 * k], st.caps[2 * k + 1]) = (NONE, NONE);
                        }
                    }
                    pc += 1;
                    continue 'run;
                }
                Op::Backref => {
                    // A group that has not taken part matches the empty string.
                    let a = i.a as usize;
                    let (from, to) = (st.caps[2 * a], st.caps[2 * a + 1]);
                    if from == NONE {
                        pc += 1;
                        continue 'run;
                    }
                    let n = to - from;
                    if len - pos >= n && input[pos..pos + n] == input[from..to] && T::whole(input, pos, pos + n) {
                        steps -= (n >> 3) as i64;
                        pos += n;
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::BackrefBack => {
                    let a = i.a as usize;
                    let (from, to) = (st.caps[2 * a], st.caps[2 * a + 1]);
                    if from == NONE {
                        pc += 1;
                        continue 'run;
                    }
                    let n = to - from;
                    if pos >= n && input[pos - n..pos] == input[from..to] && T::whole(input, pos - n, pos) {
                        steps -= (n >> 3) as i64;
                        pos -= n;
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::BackrefFold | Op::BackrefUnits => {
                    // A character at a time: to fold case, or because a group matched by code unit can begin or end
                    // between the halves of a character, where its bytes do not say what it holds.
                    let a = i.a as usize;
                    let (mut from, to) = (st.caps[2 * a], st.caps[2 * a + 1]);
                    if from == NONE {
                        pc += 1;
                        continue 'run;
                    }
                    steps -= ((to - from) >> 3) as i64;
                    let (mut at, mut ok) = (pos, true);
                    while from < to && ok {
                        let (want, w) = T::read(input, from, units);
                        from += w;
                        if at >= len {
                            ok = false;
                            break;
                        }
                        let (got, w) = T::read(input, at, units);
                        at += w;
                        ok = got == want
                            || i.op == Op::BackrefFold && canonicalize(got, i.flag) == canonicalize(want, i.flag);
                    }
                    if ok {
                        pos = at;
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::BackrefFoldBack | Op::BackrefUnitsBack => {
                    let a = i.a as usize;
                    let (from, mut to) = (st.caps[2 * a], st.caps[2 * a + 1]);
                    if from == NONE {
                        pc += 1;
                        continue 'run;
                    }
                    steps -= ((to - from) >> 3) as i64;
                    let (mut at, mut ok) = (pos, true);
                    while from < to && ok {
                        let (want, w) = T::read_last(input, to, units);
                        to -= w;
                        if at == 0 {
                            ok = false;
                            break;
                        }
                        let (got, w) = T::read_last(input, at, units);
                        at -= w;
                        ok = got == want
                            || i.op == Op::BackrefFoldBack && canonicalize(got, i.flag) == canonicalize(want, i.flag);
                    }
                    if ok {
                        pos = at;
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::Look => {
                    // What follows a lookaround starts where the lookaround does, and so does the body of a
                    // lookahead.
                    if quick[pc].allows(input, pos) {
                        stack.push(Entry { kind: if i.flag { E_LOOK_NEG } else { E_LOOK }, pc: i.x, pos, a: 0 });
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::LookOk => {
                    // The body matched. A lookaround is atomic, so the choices made inside it are dropped. The undo
                    // entries stay, because the groups it set remain set until something below this point is
                    // retried.
                    let mut f = stack.len() - 1;
                    while stack[f].kind != E_LOOK {
                        f -= 1;
                    }
                    pos = stack[f].pos;
                    let mut keep = f;
                    for k in f + 1..stack.len() {
                        if stack[k].kind >= E_TMP {
                            stack[keep] = stack[k];
                            keep += 1;
                        }
                    }
                    stack.truncate(keep);
                    pc += 1;
                    continue 'run;
                }
                Op::LookFail => {
                    // The body of a negative lookaround matched, so the lookaround fails and everything it did is
                    // undone.
                    while let Some(e) = stack.pop() {
                        if e.kind == E_LOOK_NEG {
                            break;
                        }
                        st.undo(e);
                    }
                }
                Op::Rep => 'rep: {
                    let (mut at, mut n, mut floor) = (pos, 0i32, pos);
                    if let Some(set) = &i.set {
                        while n != i.max && at < len {
                            let c: u32 = input[at].into();
                            if c < 0x80 {
                                if !set.has_ascii(c as u8) {
                                    break;
                                }
                                at += 1;
                            } else {
                                let (r, w) = T::read(input, at, units);
                                if !set.contains(r) {
                                    break;
                                }
                                at += w;
                            }
                            n += 1;
                            if n == i.min {
                                floor = at;
                            }
                        }
                    } else {
                        while n != i.max && at < len {
                            let c: u32 = input[at].into();
                            let (r, w) = if c < 0x80 { (c as Rune, 1) } else { T::read(input, at, units) };
                            if r != i.r {
                                break;
                            }
                            at += w;
                            n += 1;
                            if n == i.min {
                                floor = at;
                            }
                        }
                    }
                    steps -= ((at - pos) >> 3) as i64;
                    if n >= i.min {
                        if memo && i.m != 0 {
                            let idx = self.memo_index(st, i.m, at);
                            if (st.memo[idx >> 6] >> (idx & 63)) & 1 != 0 {
                                // What follows the loop has failed from here before. The loop gives a character
                                // back.
                                if at > floor {
                                    stack.push(Entry { kind: E_REP, pc: (pc + 1) as i32, pos: at, a: floor });
                                }
                                break 'rep;
                            }
                            if at == floor && stack.len() < MAX_MARK_DEPTH {
                                stack.push(Entry { kind: E_MARK, pc: 0, pos: idx, a: 0 });
                            }
                        }
                        if at > floor {
                            stack.push(Entry { kind: E_REP, pc: (pc + 1) as i32, pos: at, a: floor });
                        }
                        pos = at;
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::RepLazy => 'rep: {
                    let (mut at, mut n) = (pos, 0i32);
                    while n < i.min {
                        let w = one::<T>(i, input, at, units);
                        if w == 0 {
                            break;
                        }
                        at += w;
                        n += 1;
                    }
                    steps -= (n >> 3) as i64;
                    if n == i.min {
                        if memo && i.m != 0 {
                            let idx = self.memo_index(st, i.m, at);
                            if (st.memo[idx >> 6] >> (idx & 63)) & 1 != 0 {
                                // What follows the loop has failed from here before. The loop takes one more
                                // character.
                                if n != i.max {
                                    stack.push(Entry { kind: E_REP_LAZY, pc: pc as i32, pos: at, a: n as usize });
                                }
                                break 'rep;
                            }
                            if n == i.max && stack.len() < MAX_MARK_DEPTH {
                                stack.push(Entry { kind: E_MARK, pc: 0, pos: idx, a: 0 });
                            }
                        }
                        if n != i.max {
                            stack.push(Entry { kind: E_REP_LAZY, pc: pc as i32, pos: at, a: n as usize });
                        }
                        pos = at;
                        pc += 1;
                        continue 'run;
                    }
                }
                Op::LoopInit => {
                    let a = i.a as usize;
                    stack.push(Entry { kind: E_COUNTER, pc: i.a, pos: st.counts[2 * a], a: st.counts[2 * a + 1] });
                    (st.counts[2 * a], st.counts[2 * a + 1]) = (0, NONE);
                    pc += 1;
                    continue 'run;
                }
                Op::Loop => 'looping: {
                    let a = i.a as usize;
                    let n = st.counts[2 * a] as i64;
                    let (min, need, left, iter) = (
                        i.min as i64,
                        if T::UTF8 { i.need } else { i.need16 } as i64,
                        (len - pos) as i64,
                        if T::UTF8 { i.iter } else { i.iter16 } as i64,
                    );
                    // Fewer units are left than any way from here to a match consumes: what follows the loop, and
                    // the iterations it has still to make.
                    if left < need || n < min && iter != 0 && left - need < (min - n) * iter {
                        break 'looping;
                    }
                    // Once the minimum is met, ECMA-262 rejects an iteration that consumed nothing. That is what
                    // stops a loop over a body that can be empty.
                    if n > min && pos == st.counts[2 * a + 1] {
                        break 'looping;
                    }
                    if n < min {
                        pc += 1;
                        continue 'run;
                    }
                    if i.max >= 0 && n >= i.max as i64 {
                        pc = i.x as usize;
                        continue 'run;
                    }
                    // The loop may be left or entered from here. A place from which both have failed before fails
                    // at once.
                    if memo && i.m != 0 {
                        let idx = self.memo_index(st, i.m, pos);
                        if (st.memo[idx >> 6] >> (idx & 63)) & 1 != 0 {
                            break 'looping;
                        }
                        if stack.len() < MAX_MARK_DEPTH {
                            stack.push(Entry { kind: E_MARK, pc: 0, pos: idx, a: 0 });
                        }
                    }
                    if !quick[i.x as usize].allows(input, pos) {
                        // The next character rules out leaving the loop, so there is no choice.
                        pc += 1;
                    } else if !quick[pc + 1].allows(input, pos) {
                        pc = i.x as usize;
                    } else if i.flag {
                        stack.push(Entry { kind: E_CHOICE, pc: (pc + 1) as i32, pos, a: 0 });
                        pc = i.x as usize;
                    } else {
                        stack.push(Entry { kind: E_CHOICE, pc: i.x, pos, a: 0 });
                        pc += 1;
                    }
                    continue 'run;
                }
                Op::LoopIter => {
                    let a = i.a as usize;
                    stack.push(Entry { kind: E_COUNTER, pc: i.a, pos: st.counts[2 * a], a: st.counts[2 * a + 1] });
                    st.counts[2 * a] += 1;
                    st.counts[2 * a + 1] = pos;
                    pc += 1;
                    continue 'run;
                }
                Op::Match => {
                    st.steps = steps;
                    st.end = pos;
                    return true;
                }
            }
            // The instruction failed. Take up the most recent choice, undoing what was recorded since.
            loop {
                let Some(&e) = stack.last() else {
                    st.steps = steps;
                    return false;
                };
                let top = stack.len() - 1;
                match e.kind {
                    E_CHOICE => {
                        (pc, pos) = (e.pc as usize, e.pos);
                        stack.pop();
                        break;
                    }
                    E_REP => {
                        // What followed the loop from the position it stood at has failed.
                        let rep = &insts[(e.pc - 1) as usize];
                        let marked = memo && rep.m != 0;
                        if marked {
                            let idx = self.memo_index(st, rep.m, e.pos);
                            st.memo[idx >> 6] |= 1 << (idx & 63);
                        }
                        // Give back the last character taken. The characters between a and pos were decoded going
                        // forward, and decoding the last of them going backward finds the same boundary.
                        let mut at = e.pos - T::give_back(input, e.a, e.pos, units);
                        if T::UTF8 && !rep.back.is_empty() && at > e.a && !quick[e.pc as usize].allows(input, at) {
                            // What follows the loop can only start with one of a few ASCII bytes: the last of them
                            // before here is found by memrchr, and the positions after it are passed over at once.
                            let run = &T::bytes(input)[e.a..at];
                            let found = match rep.back[..] {
                                [x] => memchr::memrchr(x, run),
                                [x, y] => memchr::memrchr2(x, y, run),
                                [x, y, z] => memchr::memrchr3(x, y, z, run),
                                _ => None,
                            };
                            at = e.a + found.unwrap_or(0);
                        }
                        stack[top].pos = at;
                        (pc, pos) = (e.pc as usize, at);
                        let last = at <= e.a;
                        // A position from which what follows the loop cannot go on is passed over, and so is one
                        // from which it has failed before.
                        let mut pass = !quick[pc].allows(input, pos);
                        if marked && !pass {
                            let idx = self.memo_index(st, rep.m, pos);
                            if (st.memo[idx >> 6] >> (idx & 63)) & 1 != 0 {
                                pass = true;
                            } else if last {
                                // The loop has nothing more to give back. The entry stays to note this last way.
                                stack[top] = Entry { kind: E_MARK, pc: 0, pos: idx, a: 0 };
                                break;
                            }
                        }
                        if last {
                            stack.pop();
                        }
                        if pass {
                            steps -= 1;
                            continue;
                        }
                        break;
                    }
                    E_REP_LAZY => {
                        // What followed the loop from the position it stood at has failed.
                        let rep = &insts[e.pc as usize];
                        let marked = memo && rep.m != 0;
                        if marked {
                            let idx = self.memo_index(st, rep.m, e.pos);
                            st.memo[idx >> 6] |= 1 << (idx & 63);
                        }
                        let w = one::<T>(rep, input, e.pos, units);
                        if w == 0 {
                            stack.pop();
                            continue;
                        }
                        let (at, taken) = (e.pos + w, e.a + 1);
                        (stack[top].pos, stack[top].a) = (at, taken);
                        (pc, pos) = (e.pc as usize + 1, at);
                        let last = taken as i64 == rep.max as i64;
                        if marked {
                            let idx = self.memo_index(st, rep.m, pos);
                            if (st.memo[idx >> 6] >> (idx & 63)) & 1 != 0 {
                                if last {
                                    stack.pop();
                                }
                                steps -= 1;
                                continue;
                            }
                            if last {
                                stack[top] = Entry { kind: E_MARK, pc: 0, pos: idx, a: 0 };
                                break;
                            }
                        }
                        if last {
                            stack.pop();
                        }
                        break;
                    }
                    E_MARK => {
                        // Every way from a memo point has failed.
                        st.memo[e.pos >> 6] |= 1 << (e.pos & 63);
                        stack.pop();
                    }
                    E_LOOK => {
                        // The body of a positive lookaround has no way left to match.
                        stack.pop();
                    }
                    E_LOOK_NEG => {
                        // The body of a negative lookaround has no way to match, so the lookaround holds.
                        (pc, pos) = (e.pc as usize, e.pos);
                        stack.pop();
                        break;
                    }
                    _ => {
                        st.undo(e);
                        stack.pop();
                    }
                }
            }
        }
    }
}

#[inline(always)]
fn set_of(i: &Inst) -> &super::charset::CharSet {
    i.set.as_deref().expect("a set instruction has a class")
}

/// The width of the character at the position `at` when the instruction's character or class matches it, and 0
/// otherwise. The character is a code unit with `units` set.
#[inline]
fn one<T: Text>(i: &Inst, input: &[T::Unit], at: usize, units: bool) -> usize {
    if at >= input.len() {
        return 0;
    }
    let c: u32 = input[at].into();
    let (r, w) = if c < 0x80 { (c as Rune, 1) } else { T::read(input, at, units) };
    match &i.set {
        Some(set) => {
            if !set.contains(r) {
                return 0;
            }
        }
        None => {
            if r != i.r {
                return 0;
            }
        }
    }
    w
}
