# corvus-regex-ecma262

Regular expressions with the grammar and the answers of ECMA-262, the `RegExp` of JavaScript, for Rust and, through
its C library, for any language that calls C.

- Every flag: `d`, `g`, `i`, `m`, `s`, `u`, `v` and `y`, with the grammar of each. Without `u` or `v` a pattern is
  read by the Annex B grammar and matched by UTF-16 code unit, as ECMA-262 matches it; with them, by code point. The
  `v` flag has nested classes, the difference and the intersection of classes, strings in a class and the seven
  properties of strings.
- ECMAScript 2025: lookbehind, named groups (and a name shared between alternatives), modifier groups such as
  `(?i:...)`, and every Unicode property ECMA-262 lists, from tables of Unicode 17 the crate carries, so a pattern
  answers the same whatever toolchain built it.
- Text as `&str` (UTF-8, the engine's own encoding) or as `&[u16]` (UTF-16, lone surrogates and all). Nothing is
  transcoded, and a match allocates nothing once the thread's working memory has grown.
- `is_match`, `find`, `find_iter`, `captures`, `captures_iter`, `replace` (with every `$` form of `GetSubstitution`)
  and `split`, each as `RegExp` and `String.prototype` define it.
- A pattern without a backreference is matched in time proportional to its size times the text; a match can be
  given a limit of time, and a pattern has limits of size, so neither a pattern nor a text from outside can make a
  program hang.

**Compliance.** Over the [ECMA-262 test data](https://github.com/corvus-dotnet/ecma262-regex-test-data) (what V8
says about 117,889 patterns and 5,884,566 texts, test262's and `exec`, the global search, `replace` and `split` among
them), the crate gives 26,950,829 right answers and none wrong, refuses no valid pattern, accepts no invalid one and reaches no
limit, over UTF-8 and over UTF-16. CI runs the whole data on every change.

It is the engine of the [Corvus JSON Schema](https://github.com/corvus-dotnet/Corvus.JsonSchema) evaluators, moved
out with its history and brought to the design of the .NET engine
[Corvus.Regex.Ecma262](https://github.com/corvus-dotnet/Corvus.Regex.Ecma262).

## Install

```toml
[dependencies]
corvus-regex-ecma262 = "0.1"
```

The minimum Rust version is 1.89.

## Getting started

```rust
use corvus_regex_ecma262::Regex;

// The flags are those of new RegExp(pattern, flags).
let re = Regex::new(r"(?<year>\d{4})-(?<month>\d{2})", "gu").unwrap();

assert!(re.is_match("due 2026-10"));

let caps = re.captures("due 2026-10").unwrap();
assert_eq!(caps.name("year").unwrap().as_str(), "2026");

// Every match of a global search.
let months: Vec<&str> = re.find_iter("2026-10, 2027-01").map(|m| m.as_str()).collect();
assert_eq!(months, ["2026-10", "2027-01"]);

// replace replaces every match with the g flag, and the first without it.
assert_eq!(re.replace("2026-10, 2027-01", "$<month>/$1"), "10/2026, 01/2027");

// split splices in what the groups captured.
let parts = Regex::new(r"\s*(;)\s*", "").unwrap().split("a ; b;c");
assert_eq!(parts.len(), 5);

// UTF-16, with a lone surrogate in the text.
let text: Vec<u16> = vec![0x61, 0xD800, 0x62];
assert!(Regex::new(r"^a\ud800b$", "u").unwrap().is_match_utf16(&text));
```

A loop over many matches that needs the groups reads them into a `CaptureLocations` with `captures_read_at`, which
allocates nothing; `next_batch` on `find_iter` and `captures_iter` writes the next matches (with their groups, for
`captures_iter`) into a slice, a batch at a time, which allocates nothing either. `try_is_match`, `try_find_at` and `try_captures_at` take a limit on the time a match may take and
return `MatchError` when it is reached.

### From C

The `corvus-regex-ecma262-capi` crate builds `libcorvus_regex_ecma262` (shared and static), with the header
`corvus-regex-ecma262-capi/include/corvus_regex_ecma262.h`, which documents every function; each GitHub release has
an archive of them for each platform, with a CMake package (`find_package(corvus_regex_ecma262)`) and a pkg-config
file. Each member of `Regex`
has its C function, over UTF-8 and (with the suffix `_utf16`) over UTF-16: `is_match`, `find`, the groups by number
(into a buffer the caller owns) and by name (into a reusable `cre_captures`), the global search (`cre_matches`, a
match a call or a batch of them into an array the caller owns, with or without the groups),
`replace` and `replace_all` with every `$` form, and `split`, each matching function with a `_limit` form that takes
a limit of time. Every fallible function returns a `cre_status` with a message for the thread, owned results have a
free function each, and no panic crosses into the caller.

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
| Time | linear in the text for a pattern without a backreference (the memo of failed places); a pattern with one can take longer, and a match can be given a limit | linear for every pattern |
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

Bump `version` in `corvus-regex-ecma262/Cargo.toml` and in `corvus-regex-ecma262-capi/Cargo.toml` (the C library's
version is the engine's, with `CRE_VERSION_*` in its `src/lib.rs` and header; a test checks they agree), push it, and
publish a GitHub release tagged `v<version>` on that commit. The one release carries everything, through four
workflows: `crates-publish.yml` tests the crate, runs the ECMA-262 test data and publishes it to crates.io by trusted
publishing (no token is stored); `capi-publish.yml` builds the C library's archives for Linux (x64 and arm64, glibc and
musl), macOS (arm64 and x64) and Windows (x64 and arm64), tests each by building the smoke test against it, and
attaches them to the release; and `harness-image.yml` and `c-harness-image.yml` publish the benchmark images under the
release's tag, which the central benchmark page runs from that night. A tag that does not match the version fails the
publish. `capi-publish.yml` can also be run by hand with a tag, to attach the archives to a release made before it
existed; it refuses when the engine's sources have changed since that tag.

## Licence

Apache-2.0. See [LICENSE](LICENSE).
