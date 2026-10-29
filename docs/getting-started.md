# Getting started with Rust

## Introduction

`corvus-regex-ecma262` is a Rust crate whose patterns answer as ECMAScript does. Where the `regex` crate has its own
grammar and its own meaning for `\d`, `\w`, `\s`, `.`, case folding and Unicode properties, this crate has ECMA-262's,
so a pattern written for JavaScript, or taken from a JSON Schema, matches the same texts here as it does there. It is
the engine the C library and every binding over it are built on, held to V8's answers on the ECMA-262 test data, with
no dependencies.

This guide covers:

- **Setup**: the dependency.
- **Patterns and flags**: the flags are ECMA-262's letters, and what each one does.
- **The edition and the version of Unicode**: the grammar of ECMAScript 2020, and the Unicode tables a pattern reads.
- **Matching**: `is_match`, `find`, `captures`, the global search, `replace` and `split`, and the members that allocate
  nothing.
- **Timeouts**: the limit of time and of memory a match may take, returned as a `MatchError`.
- **UTF-8 and UTF-16**: `&str` and `&[u16]`, each matched as it is held.
- **Differences from the regex crate**: what to expect when the two meet.

## Setup

```toml
[dependencies]
corvus-regex-ecma262 = "0.2"
```

The minimum Rust version is 1.89, on any target the standard library supports, and the crate has no dependencies.

```rust
use corvus_regex_ecma262::Regex;
```

## Patterns and flags

A pattern is written as it would be in JavaScript, and the flags are the letters `RegExp` takes, as a string or as the
`Flags` struct:

| Flag | `Flags` | Meaning |
|---|---|---|
| `d` | `has_indices` | Accepted for completeness; indices are always kept. |
| `g` | `global` | `replace` replaces every match; without it, the first, as `String.prototype.replace` does. |
| `i` | `ignore_case` | Canonicalize of the mode in force: simple case folding with `u` or `v`, `toUpperCase` without. Never the locale. |
| `m` | `multiline` | `^` and `$` at line terminators. |
| `s` | `dot_all` | The dot matches line terminators. |
| `u` | `unicode` | The `u` flag grammar; matched by code point. |
| `v` | `unicode_sets` | Set notation and properties of strings; matched by code point. `u` and `v` together is an error, as in ECMA-262. |
| `y` | `sticky` | A match only at the start position. |

```rust
use corvus_regex_ecma262::Regex;

let a = Regex::new(r"\p{Script=Greek}+", "u").unwrap();
let b = Regex::new(r"[[a-z]--[aeiou]]+", "v").unwrap();
let c = Regex::new(r"^(?<year>\d{4})-(?<month>\d{2})$", "u").unwrap();

assert!(a.is_match("αβγ").unwrap());
assert!(b.is_match("rhythm").unwrap());
assert_eq!(c.group_names().collect::<Vec<_>>(), ["year", "month"]);
```

With no flag the pattern is read with the Annex B grammar and matched by UTF-16 code unit, which is what ECMA-262 does
with no flag. JSON Schema's reading, the `u` flag first and Annex B when that refuses, is two builds, each returning a
`Result` and each with the match timeout, so it costs no panic and needs no default timeout:

```rust
use std::time::Duration;
use corvus_regex_ecma262::{Error, Regex, RegexBuilder};

fn schema_pattern(pattern: &str) -> Result<Regex, Error> {
    let timeout = Some(Duration::from_secs(1));
    RegexBuilder::new(pattern).flags("u").match_timeout(timeout).build()
        .or_else(|_| RegexBuilder::new(pattern).match_timeout(timeout).build())
}

assert!(schema_pattern(r"^\p{L}+$").unwrap().flags().unicode);
// An identity escape, which only Annex B allows.
assert!(!schema_pattern(r"\-").unwrap().flags().unicode);
```

A pattern that is not valid is `Error::Invalid`, the `SyntaxError` of `new RegExp` as a value, with the reason;
invalid flags are `Error::Flags`. Every pattern has deterministic limits of size (`MAX_PATTERN_LENGTH`, `MAX_NESTING`,
`MAX_SIZE`), and `Error::TooLarge` says which one is passed. `Regex::new` never panics on a pattern.

```rust
use corvus_regex_ecma262::{Error, Regex};

assert!(matches!(Regex::new("a{2,1}", "u"), Err(Error::Invalid(_))));
assert!(matches!(Regex::new("a", "uv"), Err(Error::Flags(_))));
assert!(!Regex::is_valid("(?<n>a)(?<n>b)", "u"));
```

## The edition and the version of Unicode

JSON Schema 2020-12 names the grammar of ECMAScript 2020. `Edition::Es2020` refuses what later editions added, and
nothing else: the `d` and `v` flags, modifier groups such as `(?i:...)`, a group name used by two groups, and a
property name or value that is not in the 2020 tables (such as `\p{Script=Yezidi}`). A pattern it accepts has the same
answers as in today's grammar.

The version of Unicode is chosen apart from the edition: `UnicodeVersion::V13`, `V17`, `V18`, `Latest` (18 today) or
`Default`, the version of the V8 the answers are held to (17 today). It decides the members of each `\p{...}`, the
characters of a group name and the case folding of the `i` flag. `Regex::unicode_version` reports the version a
pattern was compiled with. Both are set on a `RegexBuilder` and read when the pattern is compiled, so a match costs the
same whichever is chosen.

```rust
use std::time::Duration;
use corvus_regex_ecma262::{Edition, RegexBuilder, UnicodeVersion};

// ECMAScript 2020 as its edition was published, with the Unicode of its year.
let greek = RegexBuilder::new(r"^\p{Script=Greek}+$")
    .flags("u")
    .match_timeout(Some(Duration::from_secs(1)))
    .edition(Edition::Es2020)
    .unicode_version(UnicodeVersion::V13)
    .build()
    .unwrap();
assert!(greek.is_match("αβγ").unwrap());
assert_eq!(greek.unicode_version(), UnicodeVersion::V13);

// Vithkuqi is in Unicode 14 and later, so not in Unicode 13.
assert!(RegexBuilder::new(r"\p{Script=Vithkuqi}").flags("u").unicode_version(UnicodeVersion::V13).build().is_err());
```

## Matching

The members are those of a `RegExp`, named as the `regex` crate names them, with the position as an argument (the
`_at` forms) where JavaScript keeps `lastIndex`. Every member that matches returns a `Result`, which is `Err` only when
a match reaches its limit (see Timeouts):

```rust
use corvus_regex_ecma262::Regex;

let date = Regex::new(r"(?<year>\d{4})-(?<month>\d{2})", "gu").unwrap();
let text = "due 2026-10, then 2027-01";

assert!(date.is_match(text).unwrap());
let m = date.find(text).unwrap().unwrap();
assert_eq!((m.start(), m.end(), m.as_str()), (4, 11, "2026-10"));
let caps = date.captures(text).unwrap().unwrap();
assert_eq!(caps.name("year").map(|g| g.as_str()), Some("2026"));
assert_eq!(caps.get(2).map(|g| g.as_str()), Some("10"));

// Every match of a global search, as matchAll makes it.
let all: Vec<&str> = date.find_iter(text).map(|m| m.unwrap().as_str()).collect();
assert_eq!(all, ["2026-10", "2027-01"]);

// replace replaces every match with the g flag, and the first without it, with every $ form of ECMA-262, or a function.
assert_eq!(date.replace(text, "$<month>/$1").unwrap(), "due 10/2026, then 01/2027");
assert_eq!(date.replace_with(text, |c| c.name("year").unwrap().as_str().to_string()).unwrap(), "due 2026, then 2027");

// split puts what the groups captured between the pieces; a group that did not take part is None.
let parts = Regex::new(r"\s*(;)\s*", "").unwrap().split("a ; b;c").unwrap();
let parts: Vec<Option<&str>> = parts.iter().map(|p| p.as_deref()).collect();
assert_eq!(parts, [Some("a"), Some(";"), Some("b"), Some(";"), Some("c")]);
```

`find_at(text, start)` is `exec` with `lastIndex` set to `start`, and `y` anchors it there. `captures`, `split` and a
`replace` that changes something allocate what they return. `is_match`, `find`, `find_iter`, its `next_batch` (spans
into a slice the caller gives) and `captures_read_at`, which writes a match's groups into a `CaptureLocations` made
once, allocate nothing once the thread's working memory has grown:

```rust
use corvus_regex_ecma262::Regex;

let date = Regex::new(r"(?<year>\d{4})-(?<month>\d{2})", "u").unwrap();
let mut locs = date.capture_locations();
assert!(date.captures_read_at(&mut locs, "due 2026-10", 0).unwrap().is_some());
assert_eq!(locs.get(1), Some((4, 8)));

// A Regex is Send and Sync, so one is shared between threads.
fn shared<T: Send + Sync>(_: &T) {}
shared(&date);
```

A `Regex` holds no mutable state, and is `Send` and `Sync`.

## Timeouts

A match has no limit by default. A pattern compiled with a match timeout abandons a match that reaches it with
`MatchError::Time`, and one that reaches the cap on backtracking memory the timeout brings with `MatchError::Memory`,
whatever the pattern:

```rust
use std::time::Duration;
use corvus_regex_ecma262::{Flags, MatchError, Regex};

let slow = Regex::with_timeout(r"^(?:(a+)\1*)+$", Flags::default(), Some(Duration::from_millis(100))).unwrap();
assert_eq!(slow.match_timeout(), Some(Duration::from_millis(100)));
assert_eq!(slow.is_match(&("a".repeat(60) + "b")), Err(MatchError::Time));
```

A pattern compiled without a timeout takes `default_match_timeout()`, which is none unless `set_default_match_timeout`
sets it. A program that matches patterns or texts from outside should set one, for example with
`set_default_match_timeout(Some(Duration::from_secs(1)))` when it starts. `is_match_within`, `find_at_within` and
`captures_at_within` give one match a limit of its own.

A pattern without a backreference is matched in time linear in the text, and no pattern can overflow the stack, so the
timeout is what bounds the others.

## UTF-8 and UTF-16

A `&str` is UTF-8 by its type, matched as its bytes with no transcoding, with positions in bytes; bytes become a `&str`
through `std::str::from_utf8`, which refuses what is not UTF-8, so nothing is ever matched or read as U+FFFD. Every
member has a `_utf16` form over `&[u16]`, lone surrogates and all, with positions in code units.

```rust
use corvus_regex_ecma262::Regex;

let dot_b = Regex::new(".b", "u").unwrap();
let m = dot_b.find("a😀b").unwrap().unwrap();
assert_eq!((m.start(), m.end()), (1, 6)); // bytes of UTF-8

let units: Vec<u16> = "a😀b".encode_utf16().collect();
let m = dot_b.find_utf16(&units).unwrap().unwrap();
assert_eq!((m.start(), m.end()), (1, 4)); // UTF-16 code units

assert!(std::str::from_utf8(b"\xff").is_err());
```

Under the `u` and `v` flags a character is a code point in both encodings, so a class, a dot or a quantifier takes a
four-byte sequence as one character. With no flag ECMA-262 matches by UTF-16 code unit; over UTF-8 the crate keeps
that meaning, so a match can start or end between the halves of a character beyond the Basic Multilingual Plane, at
the character's third byte.

## Differences from the regex crate

The member names follow the `regex` crate; the meaning follows ECMA-262. The differences a user coming from `regex`
will meet:

- **The grammar.** Backreferences, lookahead and lookbehind are there, which `regex` does not have. No `(?i)` inline
  option (ECMA-262 has `(?i:...)` groups), no `\A` or `\z`, and no `[[:alpha:]]` classes. The flags are a separate
  string.
- **The character classes.** `\d` is `[0-9]` and `\w` the 63 ASCII word characters, where `regex` makes them Unicode;
  `\s` is ECMAScript's white space and line terminators, and `.` excludes the four line terminators (LF, CR, U+2028,
  U+2029).
- **Replacement.** A template writes a group as `$1` or `$<name>`, not `${name}`, and `$&`, `` $` `` and `$'` are the
  match and the text before and after it; `replace` replaces every match only with the `g` flag, and `replace_all`
  always does.
- **Split.** `split` is JavaScript's, which puts what the groups captured between the pieces.
- **Results.** Every member that matches returns a `Result`, since a match can reach its timeout.
- **Timeouts.** `regex` is linear in the text for every pattern, having no backreferences. Here a pattern without a
  backreference is linear too, and a timeout bounds the others.
