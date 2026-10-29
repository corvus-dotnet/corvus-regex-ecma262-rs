# Getting started with Rust

The Rust crate is the engine the C library and every binding over it are built on. Its patterns have the grammar and
the answers of JavaScript's `RegExp`, held to V8's answers on the ECMA-262 test data, over UTF-8 (`&str`) and UTF-16
(`&[u16]`).

## Install

```toml
[dependencies]
corvus-regex-ecma262 = "0.2"
```

The minimum Rust version is 1.89. The crate has no dependencies.

## Compile a pattern

`Regex::new` takes a pattern and its flags as `new RegExp(pattern, flags)` takes them: `d`, `g`, `i`, `m`, `s`, `u`, `v`
and `y`. Without `u` or `v` a pattern is read by the Annex B grammar and matched by UTF-16 code unit, as ECMA-262
reads it; with them, by code point.

```rust
use corvus_regex_ecma262::{Error, Regex};

let re = Regex::new(r"(?<year>\d{4})-(?<month>\d{2})", "gu").unwrap();
assert_eq!(re.captures_len(), 3);

// A pattern that is not valid is the SyntaxError of new RegExp, as a value.
assert!(matches!(Regex::new("a{2,1}", "u"), Err(Error::Invalid(_))));
assert!(matches!(Regex::new("a", "uv"), Err(Error::Flags(_))));
assert!(!Regex::is_valid("(?<n>a)(?<n>b)", "u"));
```

`Regex::new` never panics on a pattern: it returns `Error::Invalid`, `Error::Flags`, or `Error::TooLarge` for a pattern
over a limit of size.

## Match

Every member that matches returns a `Result`, which is `Err` only when a match reaches the pattern's limit (below).
Positions are byte offsets into the text.

```rust
use corvus_regex_ecma262::{MatchError, Regex};
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let re = Regex::new(r"(?<year>\d{4})-(?<month>\d{2})", "gu")?;
let text = "due 2026-10, then 2027-01";

assert!(re.is_match(text)?);

let m = re.find(text)?.expect("a match");
assert_eq!((m.start(), m.end(), m.as_str()), (4, 11, "2026-10"));

let caps = re.captures(text)?.expect("a match");
assert_eq!(caps.name("year").map(|g| g.as_str()), Some("2026"));
assert_eq!(caps.get(2).map(|g| g.as_str()), Some("10"));

// Every match of a global search.
let all: Vec<&str> = re.find_iter(text).map(|m| m.map(|m| m.as_str())).collect::<Result<_, MatchError>>()?;
assert_eq!(all, ["2026-10", "2027-01"]);

// replace replaces every match with the g flag, and the first without it, with every $ form of ECMA-262.
assert_eq!(re.replace(text, "$<month>/$1")?, "due 10/2026, then 01/2027");

// split splices in what the groups captured, as RegExp.prototype[Symbol.split] does.
let parts = Regex::new(r"\s*(;)\s*", "")?.split("a ; b;c")?;
let parts: Vec<&str> = parts.iter().map(|p| p.as_deref().unwrap_or("")).collect();
assert_eq!(parts, ["a", ";", "b", ";", "c"]);
# Ok(())
# }
```

## UTF-16

Every member has a `_utf16` form over `&[u16]`, lone surrogates and all, with positions in code units.

```rust
use corvus_regex_ecma262::Regex;
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let text: Vec<u16> = "a😀b".encode_utf16().collect();
let m = Regex::new(".b", "u")?.find_utf16(&text)?.expect("a match");
assert_eq!((m.start(), m.end()), (1, 4));

// Without u, a supplementary character is two units, as in JavaScript.
let m = Regex::new(".b", "")?.find_utf16(&text)?.expect("a match");
assert_eq!((m.start(), m.end()), (2, 4));
# Ok(())
# }
```

## Patterns and texts from outside

A match has no limit by default. A program that matches a pattern or a text it did not write gives the pattern a match
timeout, as a .NET `Regex` has: a match that runs past it is abandoned with `MatchError::Time`, and one whose
backtracking memory reaches its cap with `MatchError::Memory`. No pattern can overflow the stack.

```rust
use std::time::Duration;
use corvus_regex_ecma262::{Flags, MatchError, Regex};

let re = Regex::with_timeout(r"^(?:(a+)\1*)+$", Flags::default(), Some(Duration::from_millis(100))).unwrap();
assert_eq!(re.is_match(&("a".repeat(60) + "b")), Err(MatchError::Time));
```

`set_default_match_timeout` sets the timeout of every pattern compiled after it, and `is_match_within`,
`find_at_within` and `captures_at_within` give one match a limit of its own.

## The edition and the version of Unicode

JSON Schema 2020-12 names the grammar of ECMAScript 2020. `Edition::Es2020` refuses what later editions added: the `d`
and `v` flags, modifier groups, a group name used twice, and property names outside the 2020 tables. The version of
Unicode is chosen apart from it: `UnicodeVersion::V13`, `V17`, `V18`, `Latest` (18) or `Default` (V8's, 17). Both are
read when the pattern is compiled, so a match costs the same whatever they are.

```rust
use std::time::Duration;
use corvus_regex_ecma262::{Edition, RegexBuilder, UnicodeVersion};

let re = RegexBuilder::new(r"^\p{Script=Greek}+$")
    .flags("u")
    .match_timeout(Some(Duration::from_secs(1)))
    .edition(Edition::Es2020)
    .unicode_version(UnicodeVersion::V13)
    .build()
    .unwrap();
assert!(re.is_match("αβγ").unwrap());

// Vithkuqi is in Unicode 14 and later, so not in Unicode 13.
assert!(RegexBuilder::new(r"\p{Script=Vithkuqi}").flags("u").unicode_version(UnicodeVersion::V13).build().is_err());
```

## Where next

- [The API documentation](https://docs.rs/corvus-regex-ecma262) on docs.rs.
- [The README](../README.md), with the allocation-free members (`captures_read_at`, `next_batch`) and the differences
  from the `regex` crate.
- [Getting started with C](getting-started-c.md): the C library built from this repository.
- [The benchmark page](https://corvus-oss.org/corvus-regex-ecma262-benchmarks/), with every engine measured nightly.
