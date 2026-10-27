# corvus-regex-ecma262: design

The design is that of the .NET engine
[Corvus.Regex.Ecma262](https://github.com/corvus-dotnet/Corvus.Regex.Ecma262) (`docs/design/design.md`), which is the
reference. This note says what this port does differently, and why. `port-checklist.md` lists each optimisation of
the reference's search layer and automaton and its state here.

## Where it comes from

The engine is the one the Corvus JSON Schema evaluator for Rust carried in `src-rs/corvus-json-schema/src/ecmaregex`,
a port of the Go module's `ecmaregex`, which the Object Pascal and OCaml ports carry too. It was moved out with its
history (`git filter-repo` on its path, the commits of the branch it was written on). The file names follow the Go
source (`parse.rs`, `compile.rs`, `vm.rs`, `dfa.rs`, `charset.rs`, `fold.rs`, `units.rs`, `ucd.rs`), so that a fix
made in one port can be carried to the others. The .NET reference took its compiler, matcher and automaton from the
same engine, so the two programs have the same instructions.

What the move added, from the reference's design: the flags of `new RegExp`, a UTF-16 cursor beside the UTF-8 one,
every group recorded, the surface of `RegExp` and `String.prototype` (`exec`, the global search, `replace`, `split`),
the `v` flag, the search layer and the automaton's word boundaries.

## The surface

The reference has the shape of .NET's `System.Text.RegularExpressions`; this crate has the shape a Rust user knows
from the `regex` crate: `Regex::new(pattern, flags)`, `is_match`, `find`, `find_iter`, `captures`, `captures_iter`,
`captures_read_at` with `CaptureLocations`, `replace`, `replace_all`, `split`. A global search also gives its matches
a batch at a time (`next_batch` on `find_iter` and `captures_iter`, into a slice the caller owns), which the C library
needs: a call a match across the C boundary kept the search out of the loop around it. The flags are ECMA-262's, as in the
reference. `replace` follows `String.prototype.replace` (every match with the g flag, the first without), and
`replace_all` replaces every match whatever the flags, as `replaceAll` with the g flag does.

UTF-16 has its own methods (`is_match_utf16`, `find_utf16`, `captures_utf16` and the rest) rather than a generic
parameter, so that `&String` and `&str` keep working as the argument of the UTF-8 ones. `Match` and `Captures` are
generic over the text (`str` or `[u16]`).

A match over UTF-8 without the u or v flag can start or end between the halves of a character beyond the Basic
Multilingual Plane, which the reference reports as the offset of the character's third byte, as here. A `&str` cannot
be cut there, so `Match::as_str` leaves the half character out, `Match::as_cow` writes it as U+FFFD, and `replace` and
`split` write it as U+FFFD: the result is the lossy UTF-8 of what JavaScript gives. The UTF-16 methods are exact.

Limits: a pattern keeps a match timeout, as the reference does (`Regex::with_timeout`, and `set_default_match_timeout`
for the process-wide default the reference reads from `REGEX_DEFAULT_MATCH_TIMEOUT`), none unless one is given. Where
the reference throws `RegexMatchTimeoutException`, every member that matches returns `Result<_, MatchError>`, and the
`_within` members give one match a limit of its own. A pattern without a backreference needs none: the memo keeps it
linear.

The edition and the version of Unicode (the reference's `RegexEdition` and `RegexUnicodeVersion`): `Edition` and
`UnicodeVersion`, set on a `RegexBuilder` beside the flags and the match timeout, the builder being how the `regex`
crate takes settings. Each version of Unicode is a `Ucd` (`ucd.rs`) over its own generated tables (`tables13.rs`,
`tables17.rs`, `tables18.rs`, with `strings<major>.rs` for the properties of strings), each with its own cache of
property sets and its own case-folding classes. Both settings are resolved when the pattern is compiled; the only thing
a match reads from the version is the case folding of a case-insensitive backreference, whose instruction carries the
major number in `b`. The 2020 grammar's property names (`es2020.rs`) are generated from the test data's
`es2020-properties.json`, as the reference's are. The C library takes both as `uint32_t` constants
(`cre_regex_new_with_options`).

## The search layer

The reference has a hand-written vectorised search for each kind of literal (`Needle`, `MultiNeedle`, `Teddy`). Rust
has these searches in crates every Rust program already depends on, so this port uses them:

- one literal: `memchr::memmem`, whose prefilter is the rarest-position needle, or `memchr` for a byte of the literal
  rare in text (`j`, `q`, `x`, `z` and a few capitals), as V8 searches for a literal's first character;
- a few literals two of whose byte positions hold one or two values each (`Sherlock|Holmes`, a word's cases): the
  vector search for both positions (`pair8.rs`), PCRE2's search before a match;
- two literals with rare first bytes: `aho-corasick`'s automaton, whose prefilter looks for those bytes with `memchr`;
- three literals or more, and the forms of a case-insensitive literal: `aho-corasick`'s packed searcher, Teddy;
- the first set over UTF-8: `memchr` for up to three bytes, Teddy over one-byte literals for up to sixty-four.

Over UTF-16 the searches read the code units themselves (`simd16.rs`, AVX2 chosen at run time, SSE2 under it): one
literal by the vector search for its two rarest units, the first set by up to eight unit ranges, a run of a loop to the
first unit its class leaves out, and several literals in the low bytes of the units packed into bytes, a case-insensitive
set folded to lower case so that a few literals are searched where there were many, the long s and the Kelvin sign
looked for apart. The chunk packed last is kept between the searches of one global search.

What is searched for is worked out from the syntax tree once (`search.rs`): an anchor is a literal, each character of
which may have several forms, that every alternative holds at a known or bounded number of characters from its start.
This one notion covers what the reference keeps as separate searches (the prefix, the required literal at a fixed
offset or a bounded range, the literal after a bounded loop, the alternation of literals, the literal every
alternative holds), and one loop (`search_windows`) tries the windows of starts each occurrence opens, in order and
never twice.

Case-insensitive search writes the forms of each character out into literals rather than masking the forms that
differ in a bit, and cuts them to one length by the `regex` crate's rule (`Seq::optimize_by_preference`), by bytes
over UTF-8, so the long s and the Kelvin sign (longer in UTF-8 than s and k) are literals like the others, cut to the
same bytes, and are not looked for apart.

## The automata

Two small automata over ASCII decide most starts without the backtracking matcher, each giving up to it at the first
unit beyond ASCII:

- an anchored, leftmost-first automaton (`adfa.rs`) says where the match from a start ends, for a pattern with no
  group and no backreference; its threads are kept in priority order and a match ends the lower ones, as the
  backtracking order would;
- the terms before a window's anchors are read backwards from each occurrence (the `regex` crate's reverse search),
  so only the starts they allow are tried, and the anchored automaton of the anchor and what follows it is run at the
  occurrence first, so an occurrence nothing can follow is passed over before any start is walked back to;
- for a pattern with no literal, a forward automaton (the anchored one with a start joined at every unit) reads the
  text once from each candidate its first set finds, to where a match first ends or no thread of an earlier start is
  left, so the starts are tried only where a match is known to begin; before the first set is searched for, a window
  of positions whose units are rare is read one unit at a time, V8's Boyer-Moore lookahead (`adfa::Skip`).

The simple shapes (a literal, an alternation of literals with or without forms, one class, one unbounded loop of a
class) are answered by the search alone, and a run of `.` is one `memchr3` for the line terminators.

## What is not here

The source generator (the reference's section 5): Rust has no build-time step here. An automaton over full Unicode:
the reference has none either. `port-checklist.md` marks every item of the reference's search layer, and what the
work to bring this engine level with `regex` added.
