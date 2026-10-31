# corvus-regex-ecma262: design

An outline of the crate as it is: what it offers, how the engine is built and why, and how it is tested and measured.
The [getting-started page](getting-started.md) shows how to use it. The C library built from this repository has
[a design document of its own](design-c.md).

## 1. What it is

A Rust regular expression engine with ECMA-262's grammar and answers (ECMAScript 2025), and the method names of the
`regex` crate, over UTF-8 (`&str`) and UTF-16 (`&[u16]`). It is the engine of the C library and of the Python, Ruby,
PHP and R packages, which bind it, and of the C++, Swift and Fortran libraries, which bind the C library.

**Why ECMA-262.** JSON Schema, OpenAPI and many wire formats specify their patterns as ECMAScript regular
expressions. The `regex` crate has no backreferences or lookaround, and differs in its classes and its line
terminators, so a translation to it can never be exact. An engine that reads ECMA-262 directly, held to V8's answers
over a large data set (section 6), needs no translation.

## 2. Public surface

**Names.** `Regex::new(pattern, flags)`, `is_match`, `find`, `find_iter`, `captures`, `captures_iter`,
`captures_read_at` with `CaptureLocations`, `replace`, `replace_all` and `split` are the shapes a Rust programmer knows
from the `regex` crate; the `_at` forms take the position where JavaScript keeps `lastIndex`. UTF-16 has its own
methods (`find_utf16` and the rest) rather than a generic parameter, so that `&String` and `&str` keep working as the
argument of the UTF-8 ones; `Match` and `Captures` are generic over the text.

**Flags.** ECMA-262's, as a string or the `Flags` struct, and nothing else.

**Edition and Unicode version.** By default a pattern is read with today's grammar (`Edition::Current`) and V8's
version of Unicode (`UnicodeVersion::Default`, 17 today). A `RegexBuilder` overrides either, independently, beside the
flags and the match timeout, the builder being how the `regex` crate takes settings: `Edition::Es2020` is the grammar
JSON Schema 2020-12 names, and the versions are 13, 17, 18 and latest. Both are resolved when the pattern is compiled.

**Validity.** `Regex::is_valid` and `is_valid_with` read a pattern as compiling it would, so a pattern over a limit of
size is not valid there. `Regex::is_valid_pattern` and `is_valid_pattern_with` ask the grammar alone, as the .NET
engine's `IsValidPattern` does: a pattern too large to compile is valid there when its grammar is. They read the
pattern in place (`validate.rs`, a port of the .NET engine's `EcmaRegexValidator` that makes the parser's decision at
each step), with the groups and the nested classes of the v flag on stacks of their own, so no pattern can overflow
the stack, and they allocate nothing unless the groups nest more than 64 deep, there are more than 63 groups, or there are more than 12
named groups. A named group is kept as the disjunction and the alternative it lies in, so the memory is in
proportion to the pattern however deeply named groups nest.
The data-set runner asks the reader about every record in every grammar and checks its answer against V8's
(`grammar-only reader` in its report). This is what a JSON Schema `format: regex` check needs: the grammar, whatever
an engine's limits.

**Errors.** A pattern that does not compile is an `Error`: `Invalid`, `Flags` or `TooLarge` (with the limit), never a
panic. Every method that matches returns `Result<_, MatchError>`, which is `Err` only when a match reaches its limit
(`Time` or `Memory`).

**ECMA-262 algorithms.** `replace` substitutes as `GetSubstitution` does, replacing every match only with `g`;
`replace_all` always does; `replace_with` takes a function. `split` follows `RegExp.prototype[Symbol.split]`.

**Input.** A `&str` is UTF-8 by its type, matched with no copy and no check. Without u or v a match over UTF-8 can end
between the halves of a character beyond the Basic Multilingual Plane, at its third byte; a `&str` cannot be cut
there, so `Match::as_str` leaves the half out and `as_cow`, `replace` and `split` write it as U+FFFD. The UTF-16
methods are exact.

**Allocation.** A warm `is_match`, `find`, `find_iter` (and its `next_batch`, into a slice the caller owns) and
`captures_read_at` allocate nothing: the working memory is the thread's, grown to what a match needs and kept up to a
cap.

**Limits.** No pattern and no text can hang a program or overflow the stack.

- A match timeout per pattern (`Regex::with_timeout`, the builder), or a process-wide default
  (`set_default_match_timeout`), bounds every match and caps its backtracking memory; the `_within` methods give one
  match a limit of its own.
- Size limits on the pattern (`MAX_PATTERN_LENGTH`, `MAX_NESTING`, `MAX_SIZE`) are deterministic.
- A pattern that opens many groups is read, compiled and dropped on a thread of its own with a stack for the deepest
  nesting the limits allow, so a caller's thread with a few hundred kilobytes is enough; the matcher keeps its choices
  on a stack of its own.

## 3. Engine

**One matcher, two encodings.** The parser, the compiler and the matcher read UTF-8 bytes and UTF-16 code units
through one cursor trait, compiled for each. Matching by UTF-16 code unit over UTF-8 places the position between the
halves of a four-byte character at its third byte.

**The matcher.** A backtracking matcher over a flat program, with V8's quick check of the next character at each
choice. A memo of the places that failed, taken into use once a match has made sixty-four steps a position, makes a
pattern without backreferences linear in the text. The clock is read every 4,096 steps.

**Where a match can start.** What is searched for is worked out from the syntax tree once (`search.rs`): an anchor is
a literal, each character of which may have several forms, that every alternative holds at a known or bounded number
of characters from its start, and one loop tries the windows of starts each occurrence opens, in order and never
twice. The searches come from the crates every Rust program already depends on:

- one literal by `memchr::memmem`, or by `memchr` for a byte of it that is rare in text;
- a few literals two of whose byte positions hold few values, by a vector search of both positions (`pair8.rs`);
- several literals, and the forms of a case-insensitive literal, by `aho-corasick` (Teddy);
- the first set over UTF-8 by `memchr` for up to three bytes, or Teddy for up to sixty-four;
- over UTF-16 the code units themselves (`simd16.rs`, AVX2 chosen at run time, SSE2 under it, a scalar search
  elsewhere): one literal by its two rarest units, the first set by up to eight unit ranges, and several literals in
  the low bytes of the units packed into bytes.

A case-insensitive literal's forms are written out into literals and cut to one length by the `regex` crate's rule,
so the long s and the Kelvin sign are literals like the others.

**The automata.** Over ASCII, for a pattern that needs no backtracking: an anchored leftmost-first automaton for where
a match from a start ends (`adfa.rs`), a reverse automaton for the terms before a window's anchors, and a forward
automaton for a pattern with no literal, with V8's Boyer-Moore lookahead before its first set is searched for. Each
gives up to the matcher at the first unit beyond ASCII.

**Simple shapes.** A literal, an alternation of literals, one class and one unbounded loop of a class are answered by
the search alone; a run of `.` is one `memchr3` for the line terminators.

## 4. The Unicode tables

Each version of Unicode is a `Ucd` over its own generated tables (`tables13.rs`, `tables17.rs`, `tables18.rs`, with
`strings<major>.rs` for the properties of strings), each with its own cache of property sets and case-folding classes;
the 2020 grammar's property names (`es2020.rs`) come from the test data's `es2020-properties.json`. Every one of these
files is made from its sources by `tools/unicode` (each version's Unicode Character Database and regress's
`gen-unicode`, every download checked against its SHA-256), and CI makes them again and fails unless they are the
checked-in files. The Go, Java, Julia, Pascal and OCaml ports take their tables from these.

## 5. Targets and layout

Rust 1.89 and later, on any target the standard library supports; the vector code is x86-64's, with a scalar search
elsewhere. The crate has no dependencies beyond `memchr` and `aho-corasick`.

- `corvus-regex-ecma262/`: the crate.
- `corvus-regex-ecma262-capi/`: the C library ([its design](design-c.md)).
- `tools/`: the test-data runner, the stack probe and the benchmark harnesses (`tools/benchmarks` for Rust,
  `tools/c` for C); `tools/unicode`: the table generators; `tools/docs`: the C API reference's generator.

## 6. Testing

**The ECMA-262 test data.** [`ecma262-regex-test-data`](https://github.com/corvus-dotnet/ecma262-regex-test-data)
holds V8's answers: validity under each grammar, `test` answers, the positions of `exec`, the global search, `y`,
`replace` and `split`, and every `exec` call of test262. The `dataset` tool runs all of it over UTF-8 and UTF-16, with
none wrong, refused or at a limit: on Linux and Windows on every change, again with the memo from the first step, and
on Linux as the data's own scripts write it for Unicode 13 and 18 and for ECMAScript 2020.

**Other tests.** Unit and integration tests over the API, the editions and the limits; a test that a warm match
allocates nothing; the getting-started page's examples as doctests; and the stack probe, which asks 964 deep, wide and
long patterns through every method, each in a process of its own on stacks of 8 MB, 1 MB and 256 KB, none ending the
process.

## 7. Benchmarks

The harness image, `ghcr.io/corvus-dotnet/corvus-regex-ecma262-rs`, runs the Industry workloads of
`dotnet/performance` over this crate (UTF-8 and UTF-16), the `regex` crate, regress and V8. The published figures are
on [the benchmark page](https://corvus-oss.org/corvus-regex-ecma262-benchmarks/), which runs every engine's image each
night; this repository publishes none of its own.

Three rows are not like for like with an engine that is not ECMA-262's, as the price of its rules under u: `.*` (`.`
excludes CR, so the CRLF text has more matches), `Sher[a-z]+|Hol[a-z]+` under i (the search must also find the long s
and the Kelvin sign), and `[a-q][^u-z]{13}x` (a supplementary character is two units, so the distance to the `x` is
not fixed).

## 8. Differences a Rust user will meet

- The grammar is ECMA-262's: backreferences and lookaround, `(?i:...)` groups but no `(?i)` prefix, no `\A`, `\z` or
  `[[:alpha:]]`. `\d` is `[0-9]` and `\w` the 63 ASCII word characters, where `regex` makes them Unicode.
- Every method that matches returns a `Result`, since a match can reach its timeout.
- A pattern with a backreference is not linear in the text, as every pattern in `regex` is, so a pattern from outside
  is given a timeout.

The [getting-started page](getting-started.md) has the full list.
