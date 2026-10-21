//! Regular expressions with the grammar and the answers of ECMA-262 (the `RegExp` of JavaScript), for Rust.
//!
//! ```
//! use corvus_regex_ecma262::Regex;
//!
//! let re = Regex::new(r"(?<word>\p{L}+)(?=!)", "gu").unwrap();
//! let words: Vec<&str> = re.find_iter("Hej! Grüß dich!").map(|m| m.as_str()).collect();
//! assert_eq!(words, ["Hej", "dich"]);
//! assert_eq!(re.replace("Hej! Grüß dich!", "<$<word>>"), "<Hej>! Grüß <dich>!");
//! ```
//!
//! [`Regex`] has `is_match`, `find`, `find_iter`, `captures`, `captures_iter`, `replace`, `replace_all` and
//! `split` over `&str`, and the same over UTF-16 (`&[u16]`, which may hold lone surrogates) with the suffix `_utf16`.
//! Positions are in the units of the text: bytes of UTF-8, or UTF-16 code units. Nothing is transcoded, and a match
//! allocates nothing once the thread's working memory has grown to what the pattern and the text need (a
//! [`Captures`] is a vector of its groups; [`Regex::captures_read_at`] reads them into a [`CaptureLocations`]
//! without allocating).
//!
//! A pattern is read as `new RegExp(pattern, flags)` reads it: with the u flag by the grammar of the u flag and
//! matched by code point; with none by the Annex B grammar and matched by UTF-16 code unit, as ECMA-262 matches a
//! pattern with no u flag. The i, m and s flags are the modifiers in force at the start of the pattern, which a
//! modifier group of ECMAScript 2025 (`(?i:...)`, `(?-m:...)`) changes for part of it.
//!
//! The engine came out of the Corvus JSON Schema evaluator for Rust, where it was a port of the `ecmaregex`
//! package of the Go module, which the Object Pascal and OCaml ports carry too. The files and the names still
//! follow the Go source, so that a fix made in one can be carried to the others: `parse.rs` is `parse.go`,
//! `compile.rs` is `compile.go`, `vm.rs` is `vm.go`, `dfa.rs` is `dfa.go`, `charset.rs`, `fold.rs` and `units.rs`
//! are the files of the same names, and `ucd.rs` with `tables_gen.rs` is the `ucd` package.
//!
//! # Code units
//!
//! A pattern with no u flag is matched by UTF-16 code unit. A character beyond the Basic Multilingual Plane is then
//! two characters, a high surrogate and a low one, to the dot, a class, a quantifier, a group and a backreference. A
//! match can start or end between the two. Such a character in the pattern is two atoms, and a class range between
//! two of them is an error. Case is folded by the Canonicalize of a pattern with no u flag. The text stays UTF-8
//! and no UTF-16 copy of it is made (see `units.rs`): the position between the halves of a character is the offset
//! of its third byte. Only a text with a character beyond the Basic Multilingual Plane is read differently with the
//! u flag and without it.
//!
//! # The matcher
//!
//! Every pattern is compiled for a backtracking matcher. A pattern with no lookaround, no backreference, no
//! multiline anchor and no case-insensitive word boundary also gets a deterministic automaton over ASCII text when
//! it is small enough and has no word boundary (see `dfa.rs`), which decides an ASCII text with one table read per
//! byte.
//!
//! The backtracking matcher keeps its choices on an explicit stack that the thread reuses, so a long text cannot
//! overflow the stack of the thread and a match does not allocate once the stack has grown. Four things keep it
//! from the time a backtracking matcher can take. A repetition whose body cannot consume a character is run once,
//! or not at all (see `simplify`), as V8 runs it. A counted loop gives up at a position from which the rest of the
//! pattern, with the iterations the loop still owes, needs more text than is left (see `set_needs`). A way the next
//! character of the text rules out is not taken (see `set_quick`), as in V8. And where a pattern has no
//! backreference the matcher remembers the places from which every way has failed (see `memo` in `vm.rs`), so a
//! repetition inside a repetition takes time proportional to the pattern times the text. What is left, a group read
//! back inside a repetition above all, can still take time that doubles with each character, and is ended by the
//! limit of the match.
//!
//! # Limits
//!
//! [`Regex::try_is_match`] gives a match a limit of time, and with it a cap on the memory of its backtracking.
//! [`Regex::new`] refuses a pattern over a limit of size ([`MAX_PATTERN_LENGTH`], [`MAX_NESTING`], [`MAX_SIZE`])
//! with [`Error::TooLarge`], so that reading and compiling a pattern take bounded time and memory whatever the
//! pattern holds.
//!
//! # Coverage
//!
//! The grammar is that of ECMAScript 2025. It includes lookbehind, named groups, group names shared between
//! separate alternatives, modifier groups, and Unicode property escapes for every property, value and alias
//! ECMA-262 lists, with Unicode 17 data, and the v flag of ECMAScript 2024: nested classes, the difference and the
//! intersection of classes, strings in a class (`\q{...}`) and the seven properties of strings. The properties and
//! the case folding come from the tables of `ucd.rs` and `strings_gen.rs` and never from the standard library or
//! from a dependency, so a pattern matches the same texts whatever built the program.

mod adfa;
mod charset;
mod compile;
mod dfa;
mod fold;
mod pair8;
mod parse;
mod re2;
mod regex;
mod replace;
mod search;
mod simd16;
// Generated: the properties of strings of the v flag, with the version of Unicode they are from.
#[allow(dead_code)]
#[rustfmt::skip]
mod strings_gen;
// Generated: it has names of scripts and properties that only the tests read.
#[allow(dead_code)]
#[rustfmt::skip]
mod tables_gen;
#[cfg(test)]
mod tests;
mod ucd;
mod units;
mod vm;

use std::fmt::{self, Write as _};

pub use parse::{Limit, MAX_NESTING, MAX_PATTERN_LENGTH, MAX_SIZE, TooLargeError};
pub(crate) use re2::MAX_RE2_COST;
pub use regex::{CaptureLocations, CaptureMatches, Captures, Haystack, Match, Matches, Regex};
pub use vm::{MAX_STACK, MatchError};

/// A code point, or a UTF-16 code unit of a pattern read without the u flag. It is signed, as the `rune` of the Go
/// source is, and -1 stands for "none".
pub(crate) type Rune = i32;

/// Why a pattern does not compile.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The pattern is over a limit of size. It was not read to its end, so nothing is known of whether it is valid.
    TooLarge(TooLargeError),
    /// The pattern is not valid in the grammar of its flags (the `SyntaxError` of `new RegExp`).
    Invalid(String),
    /// The flags are not valid: a letter that is not a flag, a flag given twice, or `u` with `v`.
    Flags(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooLarge(e) => e.fmt(f),
            Error::Invalid(message) | Error::Flags(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// What reading a pattern tells of it, without compiling it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Facts {
    /// The pattern is valid with the u flag. When it is not, it was read with the Annex B grammar.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) unicode: bool,
    /// The pattern has a construct only the backtracking matcher can run.
    pub(crate) needs_backtracking: bool,
    /// The size the parser counted for the pattern (see [`MAX_SIZE`]).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) size: usize,
    /// The number of instructions estimated for the program a linear matcher compiles for the pattern, which stops
    /// counting above [`MAX_RE2_COST`].
    pub(crate) re2_cost: i64,
}

fn invalid(pattern: &str, msg: &str, pos: usize) -> Error {
    Error::Invalid(format!("ecmaregex: {msg} at offset {pos} in {pattern:?}"))
}

/// Reads a pattern with the u flag grammar and, when that refuses it, with the Annex B grammar.
#[cfg(test)]
fn read(pattern: &str) -> Result<(parse::Node, Facts, parse::Parser), Error> {
    let (root, ps, unicode) = match parse::parse(pattern, true) {
        Ok((root, ps)) => (root, ps, true),
        Err(parse::ParseError::TooLarge(large)) => return Err(Error::TooLarge(large)),
        Err(parse::ParseError::Syntax(msg, pos)) => match parse::parse(pattern, false) {
            Ok((root, ps)) => (root, ps, false),
            Err(parse::ParseError::TooLarge(large)) => return Err(Error::TooLarge(large)),
            Err(parse::ParseError::Syntax(..)) => return Err(invalid(pattern, msg, pos)),
        },
    };
    let facts =
        Facts { unicode, needs_backtracking: ps.needs_backtracking, size: ps.size, re2_cost: re2::re2_cost(&root) };
    Ok((root, facts, ps))
}

/// The number of groups (and with the v flag, classes) a pattern may open and still be read on the stack of the
/// thread that asks.
const SHALLOW: usize = 32;

/// The stack of the thread that reads a pattern with more groups than that. The parser, the compiler and the
/// dropping of the syntax tree go one level deeper for each level of groups, and [`MAX_NESTING`] allows a thousand
/// levels, which is more than the stack of a thread can be relied on to hold: a thread of a language runtime that
/// embeds the crate may have a few hundred kilobytes. Only the pages the work touches are used.
const DEEP_STACK: usize = 64 << 20;

/// Runs the reading of a pattern, with everything that walks its syntax tree, where the depth of the tree cannot
/// overflow the stack. A pattern that opens few groups is shallow, and is read where it is. Any other is read on a
/// thread of its own with a stack for the deepest nesting the limits allow. (Go grows the stack of a goroutine as
/// it is needed, so the Go engine has no such step.) `None` when that thread could not be started.
pub(crate) fn on_a_deep_stack<T: Send>(
    pattern: &str,
    unicode_sets: bool,
    work: impl FnOnce() -> T + Send,
) -> Option<T> {
    if shallow(pattern, unicode_sets) {
        return Some(work());
    }
    deep(work)
}

/// Runs work that goes a level deeper for each level of groups of a pattern on a thread with a stack for the
/// deepest nesting the limits allow. `None` when the thread could not be started.
pub(crate) fn deep<T: Send>(work: impl FnOnce() -> T + Send) -> Option<T> {
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new().stack_size(DEEP_STACK).spawn_scoped(scope, work).ok()?;
        match thread.join() {
            Ok(result) => Some(result),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

/// Whether a pattern opens few enough groups to be read on the stack of the thread that asks. With the v flag a class
/// may hold a class, and the parser goes a level deeper for each, as it does for a group, so there a `[` counts as a
/// `(` does. In the other grammars a class holds no class and a `[` opens no level: counting it would send a pattern
/// of many classes to a thread of its own for nothing, at some times the cost of compiling it, and refuse it where no
/// thread can be started.
pub(crate) fn shallow(pattern: &str, unicode_sets: bool) -> bool {
    pattern.bytes().filter(|&b| b == b'(' || (unicode_sets && b == b'[')).count() <= SHALLOW
}

/// The error of a pattern that nests deeply when no thread could be started to read it: the pattern is refused as
/// one over the limit of nesting, which is the limit the thread is there to serve.
pub(crate) const NO_DEEP_STACK: Error = Error::TooLarge(TooLargeError { limit: Limit::Nesting, value: MAX_NESTING });

/// The flags of a pattern, as `new RegExp(pattern, flags)` takes them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Flags {
    /// `d`: accepted for completeness. The positions of the groups are always kept, so it changes nothing.
    pub has_indices: bool,
    /// `g`: [`Regex::replace`] replaces every match rather than the first.
    pub global: bool,
    /// `i`: case is ignored, by the Canonicalize of the mode in force.
    pub ignore_case: bool,
    /// `m`: `^` and `$` hold at line terminators.
    pub multiline: bool,
    /// `s`: the dot matches line terminators.
    pub dot_all: bool,
    /// `u`: the grammar of the u flag, and matching by code point.
    pub unicode: bool,
    /// `v`: the grammar of the v flag (set notation and properties of strings), and matching by code point.
    pub unicode_sets: bool,
    /// `y`: a match only at the position the search starts from.
    pub sticky: bool,
}

impl Flags {
    /// Reads the flags as `new RegExp` does: each of the letters `dgimsuvy` at most once, and not `u` with `v`.
    pub fn parse(flags: &str) -> Result<Flags, Error> {
        let mut f = Flags::default();
        for c in flags.chars() {
            let flag = match c {
                'd' => &mut f.has_indices,
                'g' => &mut f.global,
                'i' => &mut f.ignore_case,
                'm' => &mut f.multiline,
                's' => &mut f.dot_all,
                'u' => &mut f.unicode,
                'v' => &mut f.unicode_sets,
                'y' => &mut f.sticky,
                _ => return Err(Error::Flags(format!("invalid flag {c:?} in {flags:?}"))),
            };
            if *flag {
                return Err(Error::Flags(format!("the flag {c:?} is given twice in {flags:?}")));
            }
            *flag = true;
        }
        if f.unicode && f.unicode_sets {
            return Err(Error::Flags(format!("the flags u and v are given together in {flags:?}")));
        }
        Ok(f)
    }

    pub(crate) fn syntax(&self) -> parse::Syntax {
        parse::Syntax {
            unicode: self.unicode || self.unicode_sets,
            sets: self.unicode_sets,
            icase: self.ignore_case,
            multiline: self.multiline,
            dot_all: self.dot_all,
        }
    }
}

impl fmt::Display for Flags {
    /// The flags as `RegExp.prototype.flags` writes them, in the order `dgimsuvy`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, c) in [
            (self.has_indices, 'd'),
            (self.global, 'g'),
            (self.ignore_case, 'i'),
            (self.multiline, 'm'),
            (self.dot_all, 's'),
            (self.unicode, 'u'),
            (self.unicode_sets, 'v'),
            (self.sticky, 'y'),
        ] {
            if on {
                f.write_char(c)?;
            }
        }
        Ok(())
    }
}

/// Reads a pattern with one grammar.
pub(crate) fn read_with(pattern: &str, syntax: parse::Syntax) -> Result<(parse::Node, Facts, parse::Parser), Error> {
    let (root, ps) = match parse::parse_with(pattern, syntax) {
        Ok(read) => read,
        Err(parse::ParseError::TooLarge(large)) => return Err(Error::TooLarge(large)),
        Err(parse::ParseError::Syntax(msg, pos)) => return Err(invalid(pattern, msg, pos)),
    };
    let facts = Facts {
        unicode: syntax.unicode,
        needs_backtracking: ps.needs_backtracking,
        size: ps.size,
        re2_cost: re2::re2_cost(&root),
    };
    Ok((root, facts, ps))
}

/// Compiles a pattern as JSON Schema reads one, which is what the tests of the engine's first home ask of it: with
/// the u flag grammar, and when that refuses it, with the Annex B grammar.
#[cfg(test)]
pub(crate) fn compile(pattern: &str) -> Result<Regex, Error> {
    on_a_deep_stack(pattern, false, || {
        let (root, facts, ps) = read(pattern)?;
        let flags = Flags { unicode: facts.unicode, ..Flags::default() };
        Ok(Regex::from_tree(pattern, flags, root, facts, &ps))
    })
    .unwrap_or(Err(NO_DEEP_STACK))
}

/// Reports whether the pattern is a valid ECMA-262 pattern with the u flag. A pattern over a limit of size is not
/// read to its end and is reported as not valid.
#[cfg(test)]
pub(crate) fn valid(pattern: &str) -> bool {
    on_a_deep_stack(pattern, false, || parse::parse(pattern, true).is_ok()).unwrap_or(false)
}

/// Reads the pattern without compiling it, as [`compile`] reads it.
#[cfg(test)]
pub(crate) fn check(pattern: &str) -> Result<Facts, Error> {
    on_a_deep_stack(pattern, false, || read(pattern).map(|(_, facts, _)| facts)).unwrap_or(Err(NO_DEEP_STACK))
}

/// The examples of the README, run as tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeExamples;
