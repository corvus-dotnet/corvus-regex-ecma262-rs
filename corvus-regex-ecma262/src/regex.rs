//! The public surface: [`Regex`], its matches and its captures, over UTF-8 (`&str`) and UTF-16 (`&[u16]`).

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::compile::{self, Program};
use super::parse::{self, Node};
use super::units::{Text, Utf8, Utf16};
use super::{Error, Facts, Flags, MAX_RE2_COST, MatchError, deep, dfa, on_a_deep_stack, read_with, shallow};

/// The value of a slot for a group that did not take part.
const UNSET: usize = usize::MAX;

/// A text the engine matches: `str` (UTF-8, the engine's own encoding) or `[u16]` (UTF-16, which may hold lone
/// surrogates). It is sealed: no other type can be one.
pub trait Haystack: sealed::Sealed + ToOwned {
    #[doc(hidden)]
    type Text: Text;
    #[doc(hidden)]
    fn units(&self) -> &[<Self::Text as Text>::Unit];
    #[doc(hidden)]
    fn piece(&self, from: usize, to: usize) -> Cow<'_, Self>;
    #[doc(hidden)]
    fn from_units(units: Vec<<Self::Text as Text>::Unit>) -> <Self as ToOwned>::Owned;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for str {}
    impl Sealed for [u16] {}
}

impl Haystack for str {
    type Text = Utf8;

    #[inline(always)]
    fn units(&self) -> &[u8] {
        self.as_bytes()
    }

    fn piece(&self, from: usize, to: usize) -> Cow<'_, str> {
        match self.get(from..to) {
            Some(s) => Cow::Borrowed(s),
            None => {
                let mut out = Vec::new();
                Utf8::push_lossy(&mut out, self.as_bytes(), from, to);
                Cow::Owned(String::from_utf8(out).expect("a lossy piece of UTF-8 is UTF-8"))
            }
        }
    }

    fn from_units(units: Vec<u8>) -> String {
        String::from_utf8(units).expect("what the engine writes from UTF-8 is UTF-8")
    }
}

impl Haystack for [u16] {
    type Text = Utf16;

    #[inline(always)]
    fn units(&self) -> &[u16] {
        self
    }

    fn piece(&self, from: usize, to: usize) -> Cow<'_, [u16]> {
        Cow::Borrowed(&self[from..to])
    }

    fn from_units(units: Vec<u16>) -> Vec<u16> {
        units
    }
}

/// A group name, and the groups that have it (more than one only in separate alternatives).
#[derive(Clone, Debug)]
pub(crate) struct GroupName {
    pub(crate) name: String,
    pub(crate) name16: Vec<u16>,
    pub(crate) groups: Vec<usize>,
}

/// What every match of a pattern shares: the number of groups and their names.
#[derive(Debug)]
pub(crate) struct GroupInfo {
    pub(crate) groups: usize,
    pub(crate) names: Vec<GroupName>,
}

impl GroupInfo {
    /// The groups that have the name.
    pub(crate) fn named<U: Copy + Into<u32>>(&self, name: &[U]) -> Option<&[usize]> {
        self.names
            .iter()
            .find(|n| {
                if size_of::<U>() == 1 {
                    n.name.len() == name.len() && n.name.bytes().zip(name).all(|(a, &b)| a as u32 == b.into())
                } else {
                    n.name16.len() == name.len() && n.name16.iter().zip(name).all(|(&a, &b)| a as u32 == b.into())
                }
            })
            .map(|n| &n.groups[..])
    }
}

/// A compiled ECMA-262 regular expression. It is immutable once compiled, and safe to share between threads; a
/// clone shares nothing that changes.
#[derive(Clone)]
pub struct Regex {
    pattern: Arc<str>,
    flags: Flags,
    /// The program for a match that needs no groups: it records only the groups some backreference names.
    prog: Program,
    /// The program that records every group, built the first time a match asks for the groups. A pattern whose
    /// every group is recorded by `prog` (one with no groups, above all) has none of its own.
    caps: OnceLock<Program>,
    info: Arc<GroupInfo>,
    /// The pattern needs no backtracking and is small enough to be tried for an automaton.
    regular: bool,
    /// The automaton of the pattern over ASCII text (see `dfa.rs`), built on the first match. `None` inside when
    /// the pattern has none.
    pub(crate) dfa: OnceLock<Option<dfa::Dfa>>,
    /// The size the parser counted for the pattern.
    #[cfg(test)]
    pub(crate) size: usize,
}

impl fmt::Debug for Regex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Regex(/{}/{})", self.pattern, self.flags)
    }
}

impl fmt::Display for Regex {
    /// The pattern as `RegExp.prototype.toString` writes it, with its flags: `/pattern/flags`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}/{}", self.pattern, self.flags)
    }
}

impl std::str::FromStr for Regex {
    type Err = Error;

    /// A pattern with no flags.
    fn from_str(pattern: &str) -> Result<Regex, Error> {
        Regex::new(pattern, "")
    }
}

impl Regex {
    /// Compiles a pattern with the flags `new RegExp(pattern, flags)` takes, such as `""`, `"u"` or `"gimsuy"`. It
    /// returns [`Error::Invalid`] for a pattern that is not valid in the grammar of its flags, [`Error::Flags`] for
    /// flags that are not valid, and [`Error::TooLarge`] for a pattern over a limit of size (see
    /// [`MAX_SIZE`](crate::MAX_SIZE)), whether or not it is valid.
    ///
    /// ```
    /// use corvus_regex_ecma262::Regex;
    ///
    /// let re = Regex::new(r"(?<year>\d{4})-(?<month>\d{2})", "u").unwrap();
    /// let caps = re.captures("on 2026-10").unwrap();
    /// assert_eq!(caps.name("year").unwrap().as_str(), "2026");
    /// ```
    pub fn new(pattern: &str, flags: &str) -> Result<Regex, Error> {
        Regex::with_flags(pattern, Flags::parse(flags)?)
    }

    /// [`Regex::new`] with the flags already read.
    pub fn with_flags(pattern: &str, flags: Flags) -> Result<Regex, Error> {
        let syntax = flags.syntax();
        on_a_deep_stack(pattern, || {
            let (root, facts, ps) = read_with(pattern, syntax)?;
            Ok(Regex::from_tree(pattern, flags, root, facts, &ps))
        })
        .unwrap_or(Err(super::NO_DEEP_STACK))
    }

    /// Whether the pattern is valid with the flags, without compiling it.
    pub fn is_valid(pattern: &str, flags: &str) -> bool {
        let Ok(flags) = Flags::parse(flags) else { return false };
        on_a_deep_stack(pattern, || parse::parse_with(pattern, flags.syntax()).is_ok()).unwrap_or(false)
    }

    pub(crate) fn from_tree(pattern: &str, flags: Flags, root: Node, facts: Facts, ps: &parse::Parser) -> Regex {
        let regular = !facts.needs_backtracking && facts.re2_cost <= MAX_RE2_COST;
        let dfa = OnceLock::new();
        if regular && !shallow(pattern) {
            // The automaton is built from the syntax tree, so that of a pattern that nests deeply is built here
            // and not on the stack of the thread that first matches.
            let _ = dfa.set(dfa::build_dfa(&root));
        }
        let names = ps
            .group_names()
            .into_iter()
            .map(|(name, groups)| {
                let name: String = name.iter().map(|&r| char::from_u32(r as u32).unwrap_or('\u{FFFD}')).collect();
                GroupName { name16: name.encode_utf16().collect(), name, groups }
            })
            .collect();
        let info = Arc::new(GroupInfo { groups: ps.group_count(), names });
        let prog = compile::compile_program(root, flags.unicode || flags.unicode_sets, None);
        let caps = OnceLock::new();
        Regex {
            pattern: pattern.into(),
            flags,
            prog,
            caps,
            info,
            regular,
            dfa,
            #[cfg(test)]
            size: facts.size,
        }
    }

    /// The source of the pattern.
    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    /// The flags of the pattern.
    pub fn flags(&self) -> Flags {
        self.flags
    }

    /// The number of groups of a match: one for the whole match, and one for each capturing group.
    pub fn captures_len(&self) -> usize {
        self.info.groups + 1
    }

    /// The names of the groups, in the order of the first group of each name.
    pub fn group_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.info.names.iter().map(|n| n.name.as_str())
    }

    pub(crate) fn unicode(&self) -> bool {
        self.flags.unicode || self.flags.unicode_sets
    }

    /// The program that records every group.
    fn caps_program(&self) -> &Program {
        if self.prog.groups == self.info.groups {
            return &self.prog;
        }
        self.caps.get_or_init(|| {
            let build = || {
                let (root, _) = parse::parse_with(&self.pattern, self.flags.syntax()).expect("the pattern was read");
                compile::compile_program(root, self.unicode(), Some(self.info.groups))
            };
            if shallow(&self.pattern) { build() } else { deep(build).expect("a thread to read a deep pattern") }
        })
    }

    /// Builds the automaton of the pattern, which opens few groups (see `from_tree`).
    #[cold]
    fn build_dfa(&self) -> Option<dfa::Dfa> {
        let (root, _) = parse::parse_with(&self.pattern, self.flags.syntax()).ok()?;
        dfa::build_dfa(&root)
    }

    /// Whether the pattern matches at or after `start`.
    fn test<T: Text>(&self, input: &[T::Unit], start: usize, limit: Option<Duration>) -> Result<bool, MatchError> {
        let sticky = self.flags.sticky;
        if start == 0
            && !sticky
            && self.regular
            && let Some(dfa) = self.dfa.get_or_init(|| self.build_dfa())
            && let Some(matched) = dfa.is_match(input)
        {
            return Ok(matched);
        }
        let start = T::normalize(input, start, self.unicode());
        Ok(self.prog.exec::<T>(input, start, sticky, limit, None)?.is_some())
    }

    /// The first match at or after `start` (at `start` alone with the y flag, or `sticky`), and when `slots` is
    /// given, what each group captured: the start and end of the match, then of each group, `usize::MAX` for a group
    /// that did not take part.
    pub(crate) fn exec<T: Text>(
        &self,
        input: &[T::Unit],
        start: usize,
        sticky: bool,
        limit: Option<Duration>,
        slots: Option<&mut [usize]>,
    ) -> Result<Option<(usize, usize)>, MatchError> {
        let start = T::normalize(input, start, self.unicode());
        match slots {
            None => self.prog.exec::<T>(input, start, sticky, limit, None),
            Some(slots) => {
                let found = self.caps_program().exec::<T>(input, start, sticky, limit, Some(&mut slots[2..]))?;
                if let Some((from, to)) = found {
                    (slots[0], slots[1]) = (from, to);
                }
                Ok(found)
            }
        }
    }

    /// The position a global search goes on from after a match `from..to`.
    pub(crate) fn next_start<T: Text>(&self, input: &[T::Unit], from: usize, to: usize) -> usize {
        if to == from { T::advance(input, to, self.unicode()) } else { to }
    }

    // -----------------------------------------------------------------------------------------------------------
    // UTF-8

    /// Whether the pattern matches anywhere in the text (at its start alone with the y flag), with no limit on the
    /// time or the memory the match takes. It allocates nothing once the thread's working memory has grown to what
    /// the pattern and the text need.
    ///
    /// ```
    /// use corvus_regex_ecma262::Regex;
    ///
    /// assert!(Regex::new(r"^\p{Lu}", "u").unwrap().is_match("Été"));
    /// ```
    pub fn is_match(&self, text: &str) -> bool {
        self.test::<Utf8>(text.as_bytes(), 0, None).unwrap_or(false)
    }

    /// [`Regex::is_match`] from the position `start` (a byte offset), as `RegExp.prototype.test` with `lastIndex`
    /// set to it.
    pub fn is_match_at(&self, text: &str, start: usize) -> bool {
        self.test::<Utf8>(text.as_bytes(), start, None).unwrap_or(false)
    }

    /// [`Regex::is_match`] with a limit on the time the match may take (none when it is zero). A match that reaches
    /// the limit is abandoned and the error is [`MatchError::Time`]. So is a match whose backtracking stack reaches
    /// [`MAX_STACK`](crate::MAX_STACK) entries, and the error is [`MatchError::Memory`]. The result is then not an
    /// answer. The clock is read every 4,096 steps of the backtracking matcher, so a match can go past the limit by
    /// the time those take, which is some tens of microseconds. A pattern without a backreference takes time in
    /// proportion to its size times the text, and the automaton is never stopped.
    pub fn try_is_match(&self, text: &str, limit: Duration) -> Result<bool, MatchError> {
        self.test::<Utf8>(text.as_bytes(), 0, Some(limit))
    }

    /// The first match in the text (at its start alone with the y flag): `exec` with `lastIndex` 0.
    ///
    /// ```
    /// use corvus_regex_ecma262::Regex;
    ///
    /// let m = Regex::new(r"\d+", "").unwrap().find("abc 123 def").unwrap();
    /// assert_eq!((m.start(), m.end(), m.as_str()), (4, 7, "123"));
    /// ```
    pub fn find<'t>(&self, text: &'t str) -> Option<Match<'t>> {
        self.find_at(text, 0)
    }

    /// The first match at or after `start`, a byte offset: `exec` with `lastIndex` set to it.
    pub fn find_at<'t>(&self, text: &'t str, start: usize) -> Option<Match<'t>> {
        self.try_find_at(text, start, Duration::ZERO).unwrap_or(None)
    }

    /// [`Regex::find_at`] with a limit on the time the match may take (see [`Regex::try_is_match`]).
    pub fn try_find_at<'t>(
        &self,
        text: &'t str,
        start: usize,
        limit: Duration,
    ) -> Result<Option<Match<'t>>, MatchError> {
        let found = self.exec::<Utf8>(text.as_bytes(), start, self.flags.sticky, Some(limit), None)?;
        Ok(found.map(|(from, to)| Match { text, start: from, end: to }))
    }

    /// Every match in the text, as a global search finds them (`matchAll`): after an empty match the search goes on
    /// one character later. With the y flag each match must start where the one before it ended. Nothing is
    /// allocated for each match.
    ///
    /// ```
    /// use corvus_regex_ecma262::Regex;
    ///
    /// let words: Vec<&str> = Regex::new(r"\w+", "").unwrap().find_iter("a bc def").map(|m| m.as_str()).collect();
    /// assert_eq!(words, ["a", "bc", "def"]);
    /// ```
    pub fn find_iter<'r, 't>(&'r self, text: &'t str) -> Matches<'r, 't> {
        Matches { re: self, text, at: 0, done: false, session: super::search::new_session() }
    }

    /// The first match and what each group captured (`exec`).
    pub fn captures<'t>(&self, text: &'t str) -> Option<Captures<'t>> {
        self.captures_at(text, 0)
    }

    /// [`Regex::captures`] from the position `start`, a byte offset.
    pub fn captures_at<'t>(&self, text: &'t str, start: usize) -> Option<Captures<'t>> {
        self.try_captures_at(text, start, Duration::ZERO).unwrap_or(None)
    }

    /// [`Regex::captures_at`] with a limit on the time the match may take (see [`Regex::try_is_match`]).
    pub fn try_captures_at<'t>(
        &self,
        text: &'t str,
        start: usize,
        limit: Duration,
    ) -> Result<Option<Captures<'t>>, MatchError> {
        self.captures_generic(text, start, self.flags.sticky, Some(limit))
    }

    /// The captures of every match, as [`Regex::find_iter`] finds them.
    pub fn captures_iter<'r, 't>(&'r self, text: &'t str) -> CaptureMatches<'r, 't> {
        CaptureMatches { re: self, text, at: 0, done: false, session: super::search::new_session() }
    }

    /// A place to read the groups of a match into, for [`Regex::captures_read_at`], so that a loop over many
    /// matches allocates nothing for each.
    pub fn capture_locations(&self) -> CaptureLocations {
        CaptureLocations { slots: vec![UNSET; 2 * self.captures_len()] }
    }

    /// The first match at or after `start` (a byte offset), with what each group captured written into `locs`.
    pub fn captures_read_at<'t>(&self, locs: &mut CaptureLocations, text: &'t str, start: usize) -> Option<Match<'t>> {
        let slots = locs.prepare(self.captures_len());
        let found = self.exec::<Utf8>(text.as_bytes(), start, self.flags.sticky, None, Some(slots)).unwrap_or(None);
        found.map(|(from, to)| Match { text, start: from, end: to })
    }

    /// The text with the first match replaced, or every match with the g flag, as `String.prototype.replace`
    /// replaces them: the replacement is a template in which `$&` is the match, `` $` `` and `$'` the text before
    /// and after it, `$1` to `$99` a group, `$<name>` a named group, and `$$` a dollar sign (`GetSubstitution`).
    ///
    /// ```
    /// use corvus_regex_ecma262::Regex;
    ///
    /// let re = Regex::new(r"(?<first>\w+) (?<last>\w+)", "").unwrap();
    /// assert_eq!(re.replace("Ada Lovelace", "$<last>, $1"), "Lovelace, Ada");
    /// ```
    pub fn replace<'t>(&self, text: &'t str, replacement: &str) -> Cow<'t, str> {
        super::replace::replace(self, text, replacement.as_bytes(), self.flags.global)
    }

    /// The text with every match replaced (`String.prototype.replaceAll`), whether or not the g flag is set.
    pub fn replace_all<'t>(&self, text: &'t str, replacement: &str) -> Cow<'t, str> {
        super::replace::replace(self, text, replacement.as_bytes(), true)
    }

    /// The text with the first match replaced, or every match with the g flag, by what the function returns for
    /// its captures.
    pub fn replace_with<'t>(&self, text: &'t str, f: impl FnMut(&Captures<'t>) -> String) -> Cow<'t, str> {
        super::replace::replace_with(self, text, self.flags.global, f)
    }

    /// The text split around the matches, as `String.prototype.split` splits it: what each group captured is
    /// spliced in after the piece before it, `None` for a group that did not take part, and an empty match at
    /// the start or the end of the text splits nothing.
    ///
    /// ```
    /// use corvus_regex_ecma262::Regex;
    ///
    /// let parts = Regex::new(r"\s*(,)\s*", "").unwrap().split("a , b,c");
    /// let parts: Vec<Option<&str>> = parts.iter().map(|p| p.as_deref()).collect();
    /// assert_eq!(parts, [Some("a"), Some(","), Some("b"), Some(","), Some("c")]);
    /// ```
    pub fn split<'t>(&self, text: &'t str) -> Vec<Option<Cow<'t, str>>> {
        super::replace::split(self, text, u32::MAX as usize)
    }

    /// [`Regex::split`] with at most `limit` items, as `split(separator, limit)`.
    pub fn splitn<'t>(&self, text: &'t str, limit: usize) -> Vec<Option<Cow<'t, str>>> {
        super::replace::split(self, text, limit)
    }

    // -----------------------------------------------------------------------------------------------------------
    // UTF-16

    /// [`Regex::is_match`] over UTF-16 text, which may hold lone surrogates.
    pub fn is_match_utf16(&self, text: &[u16]) -> bool {
        self.test::<Utf16>(text, 0, None).unwrap_or(false)
    }

    /// [`Regex::is_match_at`] over UTF-16 text; `start` is a code unit offset.
    pub fn is_match_at_utf16(&self, text: &[u16], start: usize) -> bool {
        self.test::<Utf16>(text, start, None).unwrap_or(false)
    }

    /// [`Regex::try_is_match`] over UTF-16 text.
    pub fn try_is_match_utf16(&self, text: &[u16], limit: Duration) -> Result<bool, MatchError> {
        self.test::<Utf16>(text, 0, Some(limit))
    }

    /// [`Regex::find`] over UTF-16 text.
    pub fn find_utf16<'t>(&self, text: &'t [u16]) -> Option<Match<'t, [u16]>> {
        self.find_at_utf16(text, 0)
    }

    /// [`Regex::find_at`] over UTF-16 text; `start` is a code unit offset.
    pub fn find_at_utf16<'t>(&self, text: &'t [u16], start: usize) -> Option<Match<'t, [u16]>> {
        self.try_find_at_utf16(text, start, Duration::ZERO).unwrap_or(None)
    }

    /// [`Regex::try_find_at`] over UTF-16 text.
    pub fn try_find_at_utf16<'t>(
        &self,
        text: &'t [u16],
        start: usize,
        limit: Duration,
    ) -> Result<Option<Match<'t, [u16]>>, MatchError> {
        let found = self.exec::<Utf16>(text, start, self.flags.sticky, Some(limit), None)?;
        Ok(found.map(|(from, to)| Match { text, start: from, end: to }))
    }

    /// [`Regex::find_iter`] over UTF-16 text.
    pub fn find_iter_utf16<'r, 't>(&'r self, text: &'t [u16]) -> Matches<'r, 't, [u16]> {
        Matches { re: self, text, at: 0, done: false, session: super::search::new_session() }
    }

    /// [`Regex::captures`] over UTF-16 text.
    pub fn captures_utf16<'t>(&self, text: &'t [u16]) -> Option<Captures<'t, [u16]>> {
        self.captures_at_utf16(text, 0)
    }

    /// [`Regex::captures_at`] over UTF-16 text.
    pub fn captures_at_utf16<'t>(&self, text: &'t [u16], start: usize) -> Option<Captures<'t, [u16]>> {
        self.captures_generic(text, start, self.flags.sticky, None).unwrap_or(None)
    }

    /// [`Regex::try_captures_at`] over UTF-16 text.
    pub fn try_captures_at_utf16<'t>(
        &self,
        text: &'t [u16],
        start: usize,
        limit: Duration,
    ) -> Result<Option<Captures<'t, [u16]>>, MatchError> {
        self.captures_generic(text, start, self.flags.sticky, Some(limit))
    }

    /// [`Regex::captures_iter`] over UTF-16 text.
    pub fn captures_iter_utf16<'r, 't>(&'r self, text: &'t [u16]) -> CaptureMatches<'r, 't, [u16]> {
        CaptureMatches { re: self, text, at: 0, done: false, session: super::search::new_session() }
    }

    /// [`Regex::captures_read_at`] over UTF-16 text.
    pub fn captures_read_at_utf16<'t>(
        &self,
        locs: &mut CaptureLocations,
        text: &'t [u16],
        start: usize,
    ) -> Option<Match<'t, [u16]>> {
        let slots = locs.prepare(self.captures_len());
        let found = self.exec::<Utf16>(text, start, self.flags.sticky, None, Some(slots)).unwrap_or(None);
        found.map(|(from, to)| Match { text, start: from, end: to })
    }

    /// [`Regex::replace`] over UTF-16 text, with a UTF-16 template.
    pub fn replace_utf16<'t>(&self, text: &'t [u16], replacement: &[u16]) -> Cow<'t, [u16]> {
        super::replace::replace(self, text, replacement, self.flags.global)
    }

    /// [`Regex::replace_all`] over UTF-16 text, with a UTF-16 template.
    pub fn replace_all_utf16<'t>(&self, text: &'t [u16], replacement: &[u16]) -> Cow<'t, [u16]> {
        super::replace::replace(self, text, replacement, true)
    }

    /// [`Regex::split`] over UTF-16 text.
    pub fn split_utf16<'t>(&self, text: &'t [u16]) -> Vec<Option<Cow<'t, [u16]>>> {
        super::replace::split(self, text, u32::MAX as usize)
    }

    /// [`Regex::splitn`] over UTF-16 text.
    pub fn splitn_utf16<'t>(&self, text: &'t [u16], limit: usize) -> Vec<Option<Cow<'t, [u16]>>> {
        super::replace::split(self, text, limit)
    }

    pub(crate) fn captures_generic<'t, H: Haystack + ?Sized>(
        &self,
        text: &'t H,
        start: usize,
        sticky: bool,
        limit: Option<Duration>,
    ) -> Result<Option<Captures<'t, H>>, MatchError> {
        let mut slots = vec![UNSET; 2 * self.captures_len()];
        let found = self.exec::<H::Text>(text.units(), start, sticky, limit, Some(&mut slots))?;
        Ok(found.map(|_| Captures { text, slots, info: self.info.clone() }))
    }

    pub(crate) fn info(&self) -> &GroupInfo {
        &self.info
    }

    /// The backtracking matcher alone, which the tests use to compare it with the automaton.
    #[cfg(test)]
    pub(crate) fn match_backtrack(&self, text: &str, limit: Option<Duration>) -> Result<bool, MatchError> {
        Ok(self.prog.exec::<Utf8>(text.as_bytes(), 0, false, limit, None)?.is_some())
    }

    #[cfg(test)]
    pub(crate) fn match_limit(&self, text: &str, limit: Option<Duration>) -> Result<bool, MatchError> {
        self.test::<Utf8>(text.as_bytes(), 0, limit)
    }

    #[cfg(test)]
    pub(crate) fn program(&self) -> &Program {
        &self.prog
    }

    #[cfg(test)]
    pub(crate) fn is_regular(&self) -> bool {
        self.regular
    }
}

/// One match: where it starts and ends in the text, in the text's units (bytes of UTF-8, or UTF-16 code units).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Match<'t, H: ?Sized = str> {
    text: &'t H,
    start: usize,
    end: usize,
}

impl<'t, H: ?Sized> Match<'t, H> {
    /// Where the match starts.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Where the match ends.
    pub fn end(&self) -> usize {
        self.end
    }

    /// The range of the match.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// The length of the match, in units.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the match is empty.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

impl<'t> Match<'t, str> {
    /// The text of the match. Without the u or v flag a match can start or end between the two halves of a
    /// character beyond the Basic Multilingual Plane, which has no UTF-8 form; the half character is then left
    /// out ([`Match::as_cow`] writes it as U+FFFD, and [`Match::range`] is exact).
    pub fn as_str(&self) -> &'t str {
        let mut from = self.start;
        while !self.text.is_char_boundary(from) {
            from += 1;
        }
        let mut to = self.end.max(from);
        while !self.text.is_char_boundary(to) {
            to -= 1;
        }
        &self.text[from..to.max(from)]
    }

    /// The text of the match, with a half character at either end written as U+FFFD.
    pub fn as_cow(&self) -> Cow<'t, str> {
        self.text.piece(self.start, self.end)
    }
}

impl<'t> Match<'t, [u16]> {
    /// The code units of the match.
    pub fn as_slice(&self) -> &'t [u16] {
        &self.text[self.start..self.end]
    }
}

impl<H: ?Sized> fmt::Debug for Match<'_, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Match({}..{})", self.start, self.end)
    }
}

/// What a match captured: the match as group 0, then each capturing group, which may not have taken part.
#[derive(Clone)]
pub struct Captures<'t, H: ?Sized = str> {
    text: &'t H,
    slots: Vec<usize>,
    info: Arc<GroupInfo>,
}

impl<'t, H: ?Sized> Captures<'t, H> {
    /// The group `i` (0 is the whole match), or `None` when it did not take part or there is no such group.
    pub fn get(&self, i: usize) -> Option<Match<'t, H>> {
        let (from, to) = (*self.slots.get(2 * i)?, *self.slots.get(2 * i + 1)?);
        (from != UNSET).then_some(Match { text: self.text, start: from, end: to })
    }

    /// The group of the name that took part, or `None`.
    pub fn name(&self, name: &str) -> Option<Match<'t, H>> {
        self.info.named(name.as_bytes())?.iter().find_map(|&g| self.get(g))
    }

    /// The number of groups, the whole match among them.
    pub fn len(&self) -> usize {
        self.slots.len() / 2
    }

    /// Always false: a match has group 0.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Every group in order, `None` for one that did not take part.
    pub fn iter(&self) -> impl Iterator<Item = Option<Match<'t, H>>> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }

    /// The whole match.
    pub fn get_match(&self) -> Match<'t, H> {
        self.get(0).expect("a match has group 0")
    }
}

impl<H: ?Sized> fmt::Debug for Captures<'_, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// Where each group of a match is, read by [`Regex::captures_read_at`] without allocating.
#[derive(Clone, Debug)]
pub struct CaptureLocations {
    slots: Vec<usize>,
}

impl CaptureLocations {
    fn prepare(&mut self, groups: usize) -> &mut [usize] {
        self.slots.clear();
        self.slots.resize(2 * groups, UNSET);
        &mut self.slots
    }

    /// The start and end of the group `i` (0 is the whole match), or `None`.
    pub fn get(&self, i: usize) -> Option<(usize, usize)> {
        let (from, to) = (*self.slots.get(2 * i)?, *self.slots.get(2 * i + 1)?);
        (from != UNSET).then_some((from, to))
    }

    /// The number of groups, the whole match among them.
    pub fn len(&self) -> usize {
        self.slots.len() / 2
    }

    /// Whether there are no groups (only before a first match is read).
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// The matches of a global search (see [`Regex::find_iter`]).
pub struct Matches<'r, 't, H: ?Sized = str> {
    re: &'r Regex,
    text: &'t H,
    at: usize,
    done: bool,
    /// The global search this is, for what its searches keep between them (see `search::in_session`).
    session: u64,
}

impl<'t, H: Haystack + ?Sized> Iterator for Matches<'_, 't, H> {
    type Item = Match<'t, H>;

    fn next(&mut self) -> Option<Match<'t, H>> {
        if self.done {
            return None;
        }
        let input = self.text.units();
        let exec = || self.re.exec::<H::Text>(input, self.at, self.re.flags.sticky, None, None);
        // Only the UTF-16 searches keep anything between the searches of a global search.
        let found = if <H::Text as Text>::UTF8 { exec() } else { super::search::in_session(self.session, exec) };
        match found {
            Ok(Some((from, to))) => {
                self.at = self.re.next_start::<H::Text>(input, from, to);
                Some(Match { text: self.text, start: from, end: to })
            }
            _ => {
                self.done = true;
                None
            }
        }
    }
}

impl<H: Haystack + ?Sized> Matches<'_, '_, H> {
    /// The next matches, as many as `spans` holds, written to its start as (start, end) in the units of the text, or
    /// as any type made from that pair: how many. Fewer than `spans.len()` only when the search has found every match, so 0 ends it. They are the
    /// matches `next` gives, a batch at a time, for a loop that wants very many of them (the C library's
    /// `cre_matches_next_batch`), in one loop over the batch.
    ///
    /// ```
    /// use corvus_regex_ecma262::Regex;
    ///
    /// let re = Regex::new(r"\d+", "g").unwrap();
    /// let mut it = re.find_iter("1 22 333");
    /// let mut spans = [(0, 0); 2];
    /// assert_eq!(it.next_batch(&mut spans), 2);
    /// assert_eq!(spans, [(0, 1), (2, 4)]);
    /// assert_eq!(it.next_batch(&mut spans), 1);
    /// assert_eq!(spans[0], (5, 8));
    /// assert_eq!(it.next_batch(&mut spans), 0);
    /// ```
    pub fn next_batch<S: From<(usize, usize)>>(&mut self, spans: &mut [S]) -> usize {
        if self.done || spans.is_empty() {
            return 0;
        }
        let input = self.text.units();
        let (re, at) = (self.re, self.at);
        let batch = |spans: &mut [S]| re.prog.exec_batch::<H::Text, S>(input, at, re.unicode(), re.flags.sticky, spans);
        // Only the UTF-16 searches keep anything between the searches of a global search.
        let (n, at, done) = if <H::Text as Text>::UTF8 {
            batch(spans)
        } else {
            super::search::in_session(self.session, || batch(spans))
        };
        (self.at, self.done) = (at, done);
        n
    }
}

impl<'t, H: Haystack + ?Sized> std::iter::FusedIterator for Matches<'_, 't, H> {}

/// The captures of the matches of a global search (see [`Regex::captures_iter`]).
pub struct CaptureMatches<'r, 't, H: ?Sized = str> {
    re: &'r Regex,
    text: &'t H,
    at: usize,
    done: bool,
    session: u64,
}

impl<'t, H: Haystack + ?Sized> Iterator for CaptureMatches<'_, 't, H> {
    type Item = Captures<'t, H>;

    fn next(&mut self) -> Option<Captures<'t, H>> {
        if self.done {
            return None;
        }
        let captures = || self.re.captures_generic(self.text, self.at, self.re.flags.sticky, None);
        let found =
            if <H::Text as Text>::UTF8 { captures() } else { super::search::in_session(self.session, captures) };
        match found {
            Ok(Some(caps)) => {
                let m = caps.get_match();
                self.at = self.re.next_start::<H::Text>(self.text.units(), m.start, m.end);
                Some(caps)
            }
            _ => {
                self.done = true;
                None
            }
        }
    }
}

impl<H: Haystack + ?Sized> CaptureMatches<'_, '_, H> {
    /// The next matches with what each group captured, as many as `groups` holds at `captures_len` entries a match
    /// (the match, then each group in order, `None` for a group that did not take part), written to its start as
    /// (start, end) in the units of the text, or as any type made from that: how many matches. Fewer than
    /// `groups.len() / captures_len` only when the search has found every match, so 0 ends it. They are the matches
    /// `next` gives, a batch at a time with no allocation, for a loop that wants very many of them (the C library's
    /// `cre_matches_next_captures_batch`).
    ///
    /// ```
    /// use corvus_regex_ecma262::Regex;
    ///
    /// let re = Regex::new(r"(\d)(x)?", "g").unwrap();
    /// let mut it = re.captures_iter("1x 2");
    /// let mut groups = [None; 6];
    /// assert_eq!(it.next_batch(&mut groups), 2);
    /// assert_eq!(groups, [Some((0, 2)), Some((0, 1)), Some((1, 2)), Some((3, 4)), Some((3, 4)), None]);
    /// assert_eq!(it.next_batch(&mut groups), 0);
    /// ```
    pub fn next_batch<S: From<Option<(usize, usize)>>>(&mut self, groups: &mut [S]) -> usize {
        let width = self.re.captures_len();
        if self.done || groups.len() < width {
            return 0;
        }
        let input = self.text.units();
        let (re, at) = (self.re, self.at);
        let batch = |groups: &mut [S]| {
            re.caps_program().exec_batch_groups::<H::Text, S>(input, at, re.unicode(), re.flags.sticky, groups, width)
        };
        // Only the UTF-16 searches keep anything between the searches of a global search.
        let (n, at, done) = if <H::Text as Text>::UTF8 {
            batch(groups)
        } else {
            super::search::in_session(self.session, || batch(groups))
        };
        (self.at, self.done) = (at, done);
        n
    }
}

impl<'t, H: Haystack + ?Sized> std::iter::FusedIterator for CaptureMatches<'_, 't, H> {}
