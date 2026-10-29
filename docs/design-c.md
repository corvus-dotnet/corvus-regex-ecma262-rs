# The corvus-regex-ecma262 C library: design

An outline of the library as it is: what it offers, how it is built and why, and how it is tested and measured. The
[getting-started page](getting-started-c.md) shows how to use it.

## 1. What it is

A C library, `libcorvus_regex_ecma262` with one C99 header, `corvus_regex_ecma262.h`, of regular expressions with
ECMA-262's grammar and answers (ECMAScript 2025), over UTF-8 and UTF-16. It is the Rust crate of this repository
behind a C ABI, so any language that calls C has the engine; the C++, Swift and Fortran libraries are built on it.

**Why ECMA-262.** JSON Schema, OpenAPI and many wire formats specify their patterns as ECMAScript regular
expressions. POSIX `regex.h` and PCRE2 have their own grammars and their own meaning for `\d`, `\w`, `\s` and `.`, so a
translation to them can never be exact. An engine that reads ECMA-262 directly, held to V8's answers over a large data
set (section 6), needs no translation.

## 2. Public surface

**Names.** Every name has the prefix `cre_`. A compiled pattern is a `cre_regex`, what a match captured a
`cre_captures`, and a global search a `cre_matches`, each opaque, made by the library and freed by the caller. The
functions are those of a `RegExp`: `cre_regex_is_match`, `cre_regex_find`, `cre_regex_captures`,
`cre_regex_find_iter`, `cre_regex_replace`, `cre_regex_replace_all`, `cre_regex_split`, each taking the position where
JavaScript keeps `lastIndex`.

**Flags.** ECMA-262's, as a string with its length, and nothing else.

**Edition and Unicode version.** By default a pattern is read with today's grammar (`CRE_EDITION_CURRENT`) and V8's
version of Unicode (`CRE_UNICODE_DEFAULT`, 17 today). `cre_regex_new_with_options` overrides either, independently:
`CRE_EDITION_ES2020` is the grammar JSON Schema 2020-12 names, and the versions are `CRE_UNICODE_13`, `_17`, `_18` and
`_LATEST`. Both are resolved when the pattern is compiled.

**Errors.** Every function that can fail returns a `cre_status`: `CRE_OK`, or why it failed (`CRE_INVALID_PATTERN`,
`CRE_INVALID_FLAGS`, `CRE_PATTERN_TOO_LARGE`, `CRE_INVALID_UTF8`, `CRE_MATCH_LIMIT`, `CRE_INVALID_ARGUMENT`), with the
message and the offset in thread-local storage. On failure every out-parameter is set to its empty value. No Rust
panic crosses into the caller: a bug in the library is `CRE_PANIC`, and the library remains usable.

**ECMA-262 algorithms.** `cre_regex_replace` substitutes as `GetSubstitution` does, replacing every match only with
`g`; `cre_regex_split` follows `RegExp.prototype[Symbol.split]`.

**Input and output.** Text is a pointer and a length, never NUL-terminated on the way in: UTF-8 checked to be UTF-8
(the `_unchecked` forms skip the check for text already known to be), or UTF-16 through the `_utf16` forms, lone
surrogates and all. Positions are in the text's units. A borrowed string out is a `cre_str`, valid as long as its
function says; an owned one is a NUL-terminated `cre_string` the caller frees.

**Allocation.** `cre_regex_is_match`, `cre_regex_find`, `cre_regex_captures_read` (the groups into an array the caller
owns) and `cre_matches_next_batch` (matches into an array the caller owns) allocate nothing once the thread has read a
match with as many groups.

**Limits.** The crate's: a match timeout in milliseconds per pattern (`cre_regex_new_with_timeout`) or for every
pattern (`cre_set_default_match_timeout`), with a `_limit` form of each matching function for one match; the size
limits (`CRE_MAX_PATTERN_LENGTH`, `CRE_MAX_NESTING`, `CRE_MAX_SIZE`); and no stack overflow.

## 3. Engine

The engine is the crate's, which [its design](design.md) describes. The library's work is at the boundary:

- **Handles.** A `cre_regex` is the crate's compiled pattern, immutable, so any number of threads use one at once; a
  `cre_captures` and a `cre_matches` are used by one thread at a time.
- **Text.** UTF-8 is checked once, when a call or a global search starts, and matched where it lies; UTF-16 is read
  through a pointer aligned for `uint16_t`, with no copy.
- **Batches.** A call across the boundary for each match would keep the search out of the caller's loop, so a global
  search gives its matches (and their groups) a batch at a time into the caller's array.
- **Panics and errors.** Every entry point catches a panic and turns it into `CRE_PANIC`, and keeps the last error's
  message and offset in thread-local storage.

## 4. Packaging

Each release attaches an archive for each platform: Linux with glibc (built against glibc 2.17, so it loads on any
later one) and with musl, macOS and Windows, each on x64 and arm64. An archive holds the header, the shared and static
libraries (and on MSVC a static library for the static C runtime), a CMake package
(`corvus_regex_ecma262::corvus_regex_ecma262` and `..._static`) and a pkg-config file. An XCFramework for macOS and
iOS is attached beside them, which the Swift package takes. The API reference (`docs/api-c.md`) is generated from the
header by `tools/docs/gen-api-c.ps1`, which CI checks is current.

## 5. Targets and layout

The header is C99, and the library is built for the eight platforms above. `corvus-regex-ecma262-capi/` holds the
library: `src/` (the ABI), `include/` (the header), `cmake/` (the CMake package), `tests/native` (the C tests),
`package.ps1` (an archive) and `xcframework.ps1`. `tools/c` holds the C test-data runner and the C benchmark harness.

## 6. Testing

**The ECMA-262 test data.** A C program, `tools/c/dataset.c`, runs the whole test data through the header on every
change, judged as the Rust runner judges it, so the boundary is held to the same answers as the crate.

**Other tests.** On every platform's archive, as a consumer builds it: a smoke test and the getting-started page's
programs, through the CMake package with the shared and the static library (with AddressSanitizer and
UndefinedBehaviorSanitizer on Linux x64), and through pkg-config.

## 7. Benchmarks

The C harness image, `ghcr.io/corvus-dotnet/corvus-regex-ecma262-c`, runs the Industry workloads of
`dotnet/performance` over this library (UTF-8 and UTF-16), PCRE2, QuickJS's libregexp and V8. The published figures
are on [the benchmark page](https://corvus-oss.org/corvus-regex-ecma262-benchmarks/), which runs every engine's image
each night.

Three rows are not like for like with an engine that is not ECMA-262's, as the price of its rules under u: `.*` (`.`
excludes CR, so the CRLF text has more matches), `Sher[a-z]+|Hol[a-z]+` under i (the search must also find the long s
and the Kelvin sign), and `[a-q][^u-z]{13}x` (a supplementary character is two units, so the distance to the `x` is
not fixed).

## 8. Differences a C user will meet

- The grammar is ECMA-262's, not POSIX's or PCRE2's, and a match is the first `exec` finds, not POSIX's leftmost
  longest.
- Strings in are pointer and length, never NUL-terminated; every handle and owned string is freed by the caller.
- A pattern with a backreference is not linear in the text, so a pattern from outside is given a timeout.

The [getting-started page](getting-started-c.md) has the full list.
