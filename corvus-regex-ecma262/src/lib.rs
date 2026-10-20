//! Regular expressions with the grammar and the answers of ECMA-262 (the `RegExp` of JavaScript), for Rust.
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
//! ECMA-262 lists, with Unicode 17 data. The properties and the case folding come from the tables of `ucd.rs` and
//! never from the standard library or from a dependency, so a pattern matches the same texts whatever built the
//! program.

mod charset;
mod compile;
mod dfa;
mod fold;
mod parse;
mod re2;
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
use std::sync::OnceLock;
use std::time::Duration;

pub use parse::{Limit, MAX_NESTING, MAX_PATTERN_LENGTH, MAX_SIZE, TooLargeError};
pub(crate) use re2::MAX_RE2_COST;
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
fn read(pattern: &str) -> Result<(parse::Node, Facts), Error> {
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
    Ok((root, facts))
}

/// The number of groups a pattern may open and still be read on the stack of the thread that asks.
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
fn on_a_deep_stack<T: Send>(pattern: &str, work: impl FnOnce() -> T + Send) -> Option<T> {
    if shallow(pattern) {
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

/// Whether a pattern opens few enough groups to be read on the stack of the thread that asks.
pub(crate) fn shallow(pattern: &str) -> bool {
    pattern.bytes().filter(|&b| b == b'(').count() <= SHALLOW
}

/// The error of a pattern that nests deeply when no thread could be started to read it: the pattern is refused as
/// one over the limit of nesting, which is the limit the thread is there to serve.
const NO_DEEP_STACK: Error = Error::TooLarge(TooLargeError { limit: Limit::Nesting, value: MAX_NESTING });

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

    fn syntax(&self) -> parse::Syntax {
        parse::Syntax {
            unicode: self.unicode || self.unicode_sets,
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

/// A compiled ECMA-262 pattern. It is immutable once compiled, and safe to share between threads.
pub struct Regex {
    pattern: String,
    flags: Flags,
    prog: compile::Program,
    /// The pattern needs no backtracking and is small enough to be tried for an automaton.
    regular: bool,
    /// The automaton of the pattern over ASCII text (see `dfa.rs`), built on the first match. `None` inside when
    /// the pattern has none. The syntax tree is let go of when the pattern is compiled, so the pattern is read
    /// again to build it, which cannot fail.
    dfa: OnceLock<Option<dfa::Dfa>>,
    /// The size the parser counted for the pattern.
    #[cfg(test)]
    size: usize,
}

impl fmt::Debug for Regex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Regex(/{}/{})", self.pattern, self.flags)
    }
}

/// Reads a pattern with one grammar.
fn read_with(pattern: &str, syntax: parse::Syntax) -> Result<(parse::Node, Facts), Error> {
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
    Ok((root, facts))
}

impl Regex {
    /// Compiles a pattern with the flags `new RegExp(pattern, flags)` takes, such as `""`, `"u"` or `"gimsuy"`. It
    /// returns [`Error::Invalid`] for a pattern that is not valid in the grammar of its flags, [`Error::Flags`] for
    /// flags that are not valid, and [`Error::TooLarge`] for a pattern over a limit of size (see [`MAX_SIZE`]),
    /// whether or not it is valid.
    pub fn new(pattern: &str, flags: &str) -> Result<Regex, Error> {
        Regex::with_flags(pattern, Flags::parse(flags)?)
    }

    /// [`Regex::new`] with the flags already read.
    pub fn with_flags(pattern: &str, flags: Flags) -> Result<Regex, Error> {
        if flags.unicode_sets {
            return Err(Error::Flags("the flag v is not supported yet".to_string()));
        }
        let syntax = flags.syntax();
        on_a_deep_stack(pattern, || {
            let (root, facts) = read_with(pattern, syntax)?;
            Ok(Regex::from_tree(pattern, flags, root, facts))
        })
        .unwrap_or(Err(NO_DEEP_STACK))
    }

    fn from_tree(pattern: &str, flags: Flags, root: parse::Node, facts: Facts) -> Regex {
        let regular = !facts.needs_backtracking && facts.re2_cost <= MAX_RE2_COST;
        let dfa = OnceLock::new();
        if regular && !shallow(pattern) {
            // The automaton is built from the syntax tree, so that of a pattern that nests deeply is built here
            // and not on the stack of the thread that first matches.
            let _ = dfa.set(dfa::build_dfa(&root));
        }
        Regex {
            pattern: pattern.to_string(),
            flags,
            prog: compile::compile_program(root, facts.unicode),
            regular,
            dfa,
            #[cfg(test)]
            size: facts.size,
        }
    }

    /// Whether the pattern matches anywhere in the text, with no limit on the time or the memory the match takes.
    /// It allocates nothing once the thread's working memory has grown to what the pattern and the text need.
    pub fn is_match(&self, text: &str) -> bool {
        self.match_limit(text, None).unwrap_or(false)
    }

    /// [`Regex::is_match`] with a limit on the time the match may take (none when it is zero). A match that reaches
    /// the limit is abandoned and the error is [`MatchError::Time`]. So is a match whose backtracking stack reaches
    /// [`MAX_STACK`] entries, and the error is [`MatchError::Memory`]. The result is then not an answer. The clock
    /// is read every 4,096 steps of the backtracking matcher, so a match can go past the limit by the time those
    /// take, which is some tens of microseconds. The automaton takes time proportional to the text and is not
    /// stopped.
    pub fn try_is_match(&self, text: &str, limit: Duration) -> Result<bool, MatchError> {
        self.match_limit(text, Some(limit))
    }

    #[inline]
    pub(crate) fn match_limit(&self, text: &str, limit: Option<Duration>) -> Result<bool, MatchError> {
        if self.regular
            && let Some(dfa) = self.dfa.get_or_init(|| self.build_dfa())
            && let Some(matched) = dfa.is_match(text.as_bytes())
        {
            return Ok(matched);
        }
        self.prog.is_match(text, limit)
    }

    /// The source of the pattern.
    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    /// The flags of the pattern.
    pub fn flags(&self) -> Flags {
        self.flags
    }

    /// The backtracking matcher alone, which the tests use to compare it with the automaton.
    #[cfg(test)]
    pub(crate) fn match_backtrack(&self, text: &str, limit: Option<Duration>) -> Result<bool, MatchError> {
        self.prog.is_match(text, limit)
    }

    /// Builds the automaton of the pattern, which opens few groups (see `with_flags`).
    #[cold]
    fn build_dfa(&self) -> Option<dfa::Dfa> {
        let (root, _) = parse::parse_with(&self.pattern, self.flags.syntax()).ok()?;
        dfa::build_dfa(&root)
    }

    /// Whether the pattern was read with the ECMA-262 u flag grammar.
    #[cfg(test)]
    pub(crate) fn unicode(&self) -> bool {
        self.flags.unicode
    }

    /// The size the parser counted for the pattern, which the tree, the program and the time to make both are
    /// proportional to (see [`MAX_SIZE`]).
    #[cfg(test)]
    pub(crate) fn size(&self) -> usize {
        self.size
    }
}

/// Compiles a pattern as JSON Schema reads one, which is what the tests of the engine's first home ask of it: with
/// the u flag grammar, and when that refuses it, with the Annex B grammar.
#[cfg(test)]
pub(crate) fn compile(pattern: &str) -> Result<Regex, Error> {
    on_a_deep_stack(pattern, || {
        let (root, facts) = read(pattern)?;
        let flags = Flags { unicode: facts.unicode, ..Flags::default() };
        Ok(Regex::from_tree(pattern, flags, root, facts))
    })
    .unwrap_or(Err(NO_DEEP_STACK))
}

/// Reports whether the pattern is a valid ECMA-262 pattern with the u flag. A pattern over a limit of size is not
/// read to its end and is reported as not valid.
#[cfg(test)]
pub(crate) fn valid(pattern: &str) -> bool {
    on_a_deep_stack(pattern, || parse::parse(pattern, true).is_ok()).unwrap_or(false)
}

/// Reads the pattern without compiling it, as [`compile`] reads it.
#[cfg(test)]
pub(crate) fn check(pattern: &str) -> Result<Facts, Error> {
    on_a_deep_stack(pattern, || read(pattern).map(|(_, facts)| facts)).unwrap_or(Err(NO_DEEP_STACK))
}
