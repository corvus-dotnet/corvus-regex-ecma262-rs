# corvus-regex-ecma262

Regular expressions with the grammar and the answers of ECMA-262, the `RegExp` of JavaScript, for Rust and, through
its C library, for any language that calls C.

- Every flag: `d`, `g`, `i`, `m`, `s`, `u`, `v` and `y`, with the grammar of each. Without `u` or `v` a pattern is
  read by the Annex B grammar and matched by UTF-16 code unit, as ECMA-262 matches it; with them, by code point. The
  `v` flag has nested classes, the difference and the intersection of classes, strings in a class and the seven
  properties of strings.
- ECMAScript 2025: lookbehind, named groups (and a name shared between alternatives), modifier groups such as
  `(?i:...)`, and every Unicode property ECMA-262 lists, from tables of Unicode 17 the crate carries, so a pattern
  answers the same whatever toolchain built it. The grammar of ECMAScript 2020, which JSON Schema 2020-12 names, and
  the tables of Unicode 13 or 18 are a setting away.
- Text as `&str` (UTF-8, the engine's own encoding) or as `&[u16]` (UTF-16, lone surrogates and all). Nothing is
  transcoded, and a match allocates nothing once the thread's working memory has grown.
- `is_match`, `find`, `find_iter`, `captures`, `captures_iter`, `replace` (with every `$` form of `GetSubstitution`)
  and `split`, each as `RegExp` and `String.prototype` define it.
- A pattern without a backreference is matched in time proportional to its size times the text. A pattern can be
  given a match timeout, as a `Regex` of .NET can, which bounds the time and the backtracking memory of every match,
  and a pattern has limits of size, so with a timeout neither a pattern nor a text from outside can make a program
  hang.

**Compliance.** Over the [ECMA-262 test data](https://github.com/corvus-dotnet/ecma262-regex-test-data) (what V8
says about its patterns and texts, test262's patterns and `exec` calls, the global search, `replace` and `split`
among them, and texts of bytes that are not UTF-8), the crate gives every answer right and none wrong, refuses no
valid pattern, accepts no invalid one and reaches no limit, over UTF-8 and over UTF-16. (A `str` cannot hold bytes
that are not UTF-8, so those are refused before they reach the crate, as the data asks.) CI runs the whole data on every change.

It is the engine of the [Corvus JSON Schema](https://github.com/corvus-dotnet/Corvus.JsonSchema) evaluators, moved
out with its history and brought to the design of the .NET engine
[Corvus.Regex.Ecma262](https://github.com/corvus-dotnet/Corvus.Regex.Ecma262).

## Install

```toml
[dependencies]
corvus-regex-ecma262 = "0.2"
```

The minimum Rust version is 1.89.

## Getting started

```rust
use corvus_regex_ecma262::{MatchError, Regex};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The flags are those of new RegExp(pattern, flags).
    let re = Regex::new(r"(?<year>\d{4})-(?<month>\d{2})", "gu")?;

    // Every member that matches returns a Result: Err when a match reaches the pattern's timeout.
    assert!(re.is_match("due 2026-10")?);

    let caps = re.captures("due 2026-10")?.expect("a match");
    assert_eq!(caps.name("year").unwrap().as_str(), "2026");

    // Every match of a global search.
    let months: Vec<&str> =
        re.find_iter("2026-10, 2027-01").map(|m| m.map(|m| m.as_str())).collect::<Result<_, MatchError>>()?;
    assert_eq!(months, ["2026-10", "2027-01"]);

    // replace replaces every match with the g flag, and the first without it.
    assert_eq!(re.replace("2026-10, 2027-01", "$<month>/$1")?, "10/2026, 01/2027");

    // split splices in what the groups captured.
    let parts = Regex::new(r"\s*(;)\s*", "")?.split("a ; b;c")?;
    assert_eq!(parts.len(), 5);

    // UTF-16, with a lone surrogate in the text.
    let text: Vec<u16> = vec![0x61, 0xD800, 0x62];
    assert!(Regex::new(r"^a\ud800b$", "u")?.is_match_utf16(&text)?);
    Ok(())
}
```

### Patterns and texts from outside

A pattern has a match timeout, as a `Regex` of `System.Text.RegularExpressions` has: the time each match may take
before it is abandoned with `MatchError::Time`. It brings a cap on the backtracking memory of the match with it,
which is `MatchError::Memory`. Without one, the default, a match has no limit of time or of memory, so a program that
matches a pattern or a text it did not write should give its patterns one:

```rust
use std::time::Duration;
use corvus_regex_ecma262::{Flags, MatchError, Regex, set_default_match_timeout};

// For one pattern.
let re = Regex::with_timeout(r"^(?:(a+)\1*)+$", Flags::default(), Some(Duration::from_millis(100))).unwrap();
assert_eq!(re.is_match(&("a".repeat(60) + "b")), Err(MatchError::Time));

// For every pattern compiled from here on (the process-wide default of .NET).
set_default_match_timeout(Some(Duration::from_secs(1)));
# set_default_match_timeout(None);
```

The timeout applies to every member: `is_match`, `find`, the global searches, `captures`, `replace` and `split`, for
each match they make.

A loop over many matches that needs the groups reads them into a `CaptureLocations` with `captures_read_at`, which
allocates nothing; `next_batch` on `find_iter` and `captures_iter` writes the next matches (with their groups, for
`captures_iter`) into a slice, a batch at a time, which allocates nothing either. `is_match_within`, `find_at_within`
and `captures_at_within` give one match a limit of its own in place of the pattern's timeout.

### The edition and the version of Unicode

JSON Schema 2020-12 names the grammar of ECMAScript 2020 (ECMA-262, 11th edition). `Edition::Es2020` refuses what
later editions added, and nothing else: the `d` and `v` flags, modifier groups such as `(?i:...)`, a group name used by
two groups, and a property name or value that is not in the 2020 tables (such as `\p{Script=Yezidi}`). A pattern it
accepts has the same answers as in today's grammar.

The version of Unicode is chosen apart from the edition: `UnicodeVersion::V13`, `V17` or `V18`, `Latest` (18 today)
or `Default`, the version of the V8 the answers are held to (17 today). It decides the members of each `\p{...}`, the
characters of a group name and the case folding of the `i` flag. `RegexBuilder` takes both, with the flags and the
match timeout; both are read when the pattern is compiled, so a match costs the same whichever is chosen.

```rust
use std::time::Duration;
use corvus_regex_ecma262::{Edition, RegexBuilder, UnicodeVersion};

// ECMAScript 2020 as its edition was published, with the Unicode of its year.
let re = RegexBuilder::new(r"^\p{Script=Greek}+$")
    .flags("u")
    .match_timeout(Some(Duration::from_secs(1)))
    .edition(Edition::Es2020)
    .unicode_version(UnicodeVersion::V13)
    .build()
    .unwrap();
assert!(re.is_match("αβγ").unwrap());
```

### From C

The `corvus-regex-ecma262-capi` crate builds `libcorvus_regex_ecma262` (shared and static), with the header
`corvus-regex-ecma262-capi/include/corvus_regex_ecma262.h`, which documents every function; each GitHub release has an
archive of them for each platform, with a CMake package (`find_package(corvus_regex_ecma262)`) and a pkg-config file.
Each member of `Regex` has its C function, over UTF-8 and (with the suffix `_utf16`) over UTF-16: `is_match`, `find`,
the groups by number (into a buffer the caller owns) and by name (into a reusable `cre_captures`), the global search
(`cre_matches`, a match a call or a batch of them into an array the caller owns, with or without the groups), `replace`
and `replace_all` with every `$` form, and `split`, each matching function with a `_limit` form that takes a limit of
time of its own; `cre_regex_new_with_timeout` gives a regular expression a match timeout that every function then gives
its matches, `cre_regex_new_with_options` gives it the edition and the version of Unicode too, and
`cre_set_default_match_timeout` gives a timeout to every regular expression. Every fallible function returns
a `cre_status` with a message for the thread, owned results have a free function each, and no panic crosses into the
caller. A UTF-8 text is checked to be UTF-8 on each call (once for a global search); a caller whose strings are UTF-8 by
construction can skip that read of the whole text with each UTF-8 function's `_unchecked` form, for which text that is
not UTF-8 is undefined behaviour; a caller whose strings carry no such guarantee can check a text once with
`cre_utf8_check` and then search it unchecked.

```c
cre_regex *re = NULL;
if (cre_regex_new("(?<word>\\p{Lu}\\w+)", 18, "gu", 2, &re) == CRE_OK) {
    cre_matches *it = NULL;
    cre_match m;
    bool found;
    if (cre_regex_find_iter(re, text, text_len, &it) == CRE_OK) {
        while (cre_matches_next(it, &m, &found) == CRE_OK && found) {
            printf("%.*s\n", (int)(m.end - m.start), text + m.start);
        }
        /* Or a batch at a time, which spares a call for each match: fewer than asked for ends the search. */
        cre_match batch[256];
        size_t n;
        cre_matches_reset(it);
        do {
            if (cre_matches_next_batch(it, batch, 256, &n) != CRE_OK) break;
            /* batch[0] to batch[n - 1] */
        } while (n == 256);
        cre_matches_free(it);
    }
    cre_string out;
    if (cre_regex_replace(re, text, text_len, "<$<word>>", 9, &out) == CRE_OK) {
        puts(out.ptr);
        cre_string_free(&out);
    }
    cre_regex_free(re);
}
```

## Differences from `regex`

The [`regex`](https://crates.io/crates/regex) crate is Rust's standard engine, with RE2's semantics: it guarantees
time linear in the text for every pattern, and has no backreferences and no lookaround. It is not ECMA-262, and the
same pattern answers differently in places:

| | corvus-regex-ecma262 | regex |
|---|---|---|
| Syntax | ECMA-262: lookbehind, lookahead, backreferences, named groups `(?<n>...)`, modifier groups, `\p{...}` of ECMA-262, the `v` flag's set notation | RE2: no lookaround or backreference, `(?P<n>...)`, inline flags `(?i)`, its own class syntax |
| `\w`, `\d`, `\b` | ASCII, as ECMA-262 defines them | Unicode unless turned off |
| `.` | any character but LF, CR, U+2028 and U+2029 | any character but LF |
| `$` without `m` | the end of the text only | the end of the text only |
| Case folding | ECMA-262's Canonicalize: simple case folding with `u` or `v`, `toUpperCase` without | Unicode simple case folding |
| Without `u` | the Annex B grammar, and the text matched by UTF-16 code unit | not applicable |
| Text | `&str` and `&[u16]` | `&str` and `&[u8]` |
| Time | linear in the text for a pattern without a backreference (the memo of failed places); a pattern with one can take longer, and a pattern can be given a match timeout | linear for every pattern |
| Errors | a pattern over a limit of size is `Error::TooLarge`, apart from `Error::Invalid` | a size limit on the compiled program |

Choose this crate when the pattern is a JavaScript one: from a JSON Schema, an OpenAPI or AsyncAPI document, a
browser, a configuration written for Node, or anything that must give the answer JavaScript gives.

## Benchmarks

The Industry workloads of dotnet/performance (the Sherlock Holmes, Leipzig and mariomka texts, the rebar names), this
crate against `regex`, `regress` (an ECMA-262 engine) and V8, each engine with its compliance with the test data
beside its figures: see [docs/benchmarks](docs/benchmarks/README.md), and the results on the central page,
[corvus-oss.org/corvus-regex-ecma262-benchmarks](https://corvus-oss.org/corvus-regex-ecma262-benchmarks/).

## Repository

- `corvus-regex-ecma262`: the crate.
- `corvus-regex-ecma262-capi`: the C library, its header, a smoke test in C and `package.ps1`, which lays out a release
  archive (the header, the shared and the static library, a CMake package and a pkg-config file).
- `tools/c`: the C tools over the C library, each a C program that links it through its header: the data-set runner
  (`dataset`, and `dataset-quickjs` over QuickJS's libregexp) and the benchmark harness (`tools/c/run.sh`).
- `tools`: the data-set runner (`cargo run --release --bin dataset -- <data dir> <wrong.tsv> [--engine regress]`),
  the stack probe (`cargo run --release --bin stack-probe`: deep, wide and long patterns, each in a process of its
  own on small stacks, none of which may end the process) and the benchmark harness (`tools/benchmarks/run.sh`, over
  `cargo run --release --bin industry`).
- `docs/design.md`: what differs from the .NET reference engine and why; `docs/port-checklist.md`: each optimisation
  of the reference, done, not applicable or to do.

## Releasing

Every merged pull request releases what it changed, unless it is labelled `no_release`. `auto-release.yml` runs
`tools/release/release.ps1` on the merge commit: when the engine's code changed since its last tag (the paths
`.github/release.json` lists: the crates' sources, manifests and the C library's packaging, not their tests or READMEs),
it makes a GitHub release tagged `v<version>` with the next patch version. A pull request that needs a minor or major
version sets `nextVersion` in `release.json` (a minor version is a breaking change, and the crate's README names it in
its install line). A pull request labelled `no_release` releases nothing, and its changes ship with the next release.

The tag is the version. The manifests, `CRE_VERSION_*` (in the C library's `src/lib.rs` and header) and the smoke
test's CMake request hold `0.0.0`, and each workflow that builds a release writes the tag's version into them first
(`tools/release/stamp.ps1`; a test checks they agree). The one release carries everything, through four workflows:
`crates-publish.yml` tests the crate, runs the ECMA-262 test data and publishes it to crates.io by trusted publishing (no
token is stored); `capi-publish.yml` builds the C library's archives for Linux (x64 and arm64, glibc and musl), macOS
(arm64 and x64) and Windows (x64 and arm64), tests each by building the smoke test against it, and attaches them to the
release; and `harness-image.yml` and `c-harness-image.yml` publish the benchmark images under the release's tag, which
the central benchmark page runs from that night. `capi-publish.yml` can also be run by hand with a tag, to attach the
archives to a release made before it existed; it refuses when the engine's sources have changed since that tag.

When only the benchmark harness changed (`Dockerfile`, `tools/benchmarks/`, `tools/c/`), the release is
`harness-<version>.<n>`, for the engine at `v<version>`. Only the two image workflows run for it, and the repository's
Latest stays the engine's release. The central page takes each image at the newest release of either kind.

## Licence

Apache-2.0. See [LICENSE](LICENSE).
