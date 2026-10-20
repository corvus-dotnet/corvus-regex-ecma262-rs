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
`captures_read_at` with `CaptureLocations`, `replace`, `replace_all`, `split`. The flags are ECMA-262's, as in the
reference. `replace` follows `String.prototype.replace` (every match with the g flag, the first without), and
`replace_all` replaces every match whatever the flags, as `replaceAll` with the g flag does.

UTF-16 has its own methods (`is_match_utf16`, `find_utf16`, `captures_utf16` and the rest) rather than a generic
parameter, so that `&String` and `&str` keep working as the argument of the UTF-8 ones. `Match` and `Captures` are
generic over the text (`str` or `[u16]`).

A match over UTF-8 without the u or v flag can start or end between the halves of a character beyond the Basic
Multilingual Plane, which the reference reports as the offset of the character's third byte, as here. A `&str` cannot
be cut there, so `Match::as_str` leaves the half character out, `Match::as_cow` writes it as U+FFFD, and `replace` and
`split` write it as U+FFFD: the result is the lossy UTF-8 of what JavaScript gives. The UTF-16 methods are exact.

Limits: a match has none unless asked for one (`try_is_match` and the other `try_` methods take it per call, where the
reference keeps a timeout on the instance). A pattern without a backreference needs none: the memo keeps it linear.

## The search layer

The reference has a hand-written vectorised search for each kind of literal (`Needle`, `MultiNeedle`, `Teddy`). Rust
has these searches in crates every Rust program already depends on, so this port uses them:

- one literal: `memchr::memmem`, whose prefilter is the rarest-position needle;
- two literals with rare first bytes: `aho-corasick`'s automaton, whose prefilter looks for those bytes with `memchr`;
- three literals or more, and the forms of a case-insensitive literal: `aho-corasick`'s packed searcher, Teddy;
- the first set over UTF-8: `memchr` for up to three bytes, Teddy over one-byte literals for up to sixty-four.

Over UTF-16 the same searchers look at the bytes of the code units, and an occurrence at an odd byte offset is passed
over, where the reference has searches over `ushort` lanes.

What is searched for is worked out from the syntax tree once (`search.rs`): an anchor is a literal, each character of
which may have several forms, that every alternative holds at a known or bounded number of characters from its start.
This one notion covers what the reference keeps as separate searches (the prefix, the required literal at a fixed
offset or a bounded range, the literal after a bounded loop, the alternation of literals, the literal every
alternative holds), and one loop (`search_windows`) tries the windows of starts each occurrence opens, in order and
never twice.

Case-insensitive search writes the forms of each character out into literals (three characters of each anchor,
more where a character has one form) rather than masking the forms that differ in a bit. Teddy's nibble tables take
the forms as they come, and since its literals may differ in length, the long s and the Kelvin sign (longer in UTF-8
than s and k) are literals like the others and are not looked for apart.

## What is not here

The source generator (the reference's section 5): Rust has no build-time step here. The simple shapes other than a
lone literal, the enumeration's cursor for runs of `.`, the vectorised give-back of a loop and the vectorised first
set over UTF-16 are listed as to do in `port-checklist.md`, with the rows that would ask for them.
