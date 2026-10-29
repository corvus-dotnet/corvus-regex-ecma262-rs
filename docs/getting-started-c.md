# Getting started with C

## Introduction

The C library, `libcorvus_regex_ecma262` with its one header `corvus_regex_ecma262.h`, is a regular expression engine
whose patterns answer as ECMAScript does. Where POSIX `regex.h` and PCRE2 have their own grammars and their own meaning
for `\d`, `\w`, `\s`, `.`, case folding and Unicode properties, this library has ECMA-262's, so a pattern written for
JavaScript, or taken from a JSON Schema, matches the same texts here as it does there. It is the corvus-regex-ecma262
Rust crate behind a C ABI, held to V8's answers on the ECMA-262 test data, and the library the C++, Python, Ruby, PHP,
Swift, Fortran and R bindings are built on.

This guide covers:

- **Setup**: the release archives, with CMake or pkg-config.
- **Patterns and flags**: the flags are ECMA-262's letters, and what each one does.
- **The edition and the version of Unicode**: the grammar of ECMAScript 2020, and the Unicode tables a pattern reads.
- **Matching**: is-match, find, captures, the global search, replace and split, and the forms that allocate nothing.
- **Timeouts**: the limit of time and of memory a match may take.
- **UTF-8 and UTF-16**: pointer and length in either encoding, with no transcoding.
- **Handles, strings and errors**: what the caller frees, and where a failure is reported.
- **Differences from regex.h and PCRE2**: what to expect when they meet.

## Setup

Each [release](https://github.com/corvus-dotnet/corvus-regex-ecma262-rs/releases) has an archive for each platform
(Linux with glibc or musl, macOS and Windows, each on x64 and arm64) holding the header, the shared and static
libraries, a CMake package and a pkg-config file. Unpack it and point CMake at it:

```cmake
find_package(corvus_regex_ecma262 CONFIG REQUIRED)
target_link_libraries(app PRIVATE corvus_regex_ecma262::corvus_regex_ecma262)        # or ..._static
```

or use `pkg-config --cflags --libs corvus_regex_ecma262`. The header is C99:

```c
#include "corvus_regex_ecma262.h"
```

## Patterns and flags

A pattern is written as it would be in JavaScript, and the flags are the letters `RegExp` takes, as a string:

| Flag | Meaning |
|---|---|
| `d` | Accepted for completeness; indices are always kept. |
| `g` | `cre_regex_replace` replaces every match; without it, the first, as `String.prototype.replace` does. |
| `i` | Canonicalize of the mode in force: simple case folding with `u` or `v`, `toUpperCase` without. Never the locale. |
| `m` | `^` and `$` at line terminators. |
| `s` | The dot matches line terminators. |
| `u` | The `u` flag grammar; matched by code point. |
| `v` | Set notation and properties of strings; matched by code point. `u` and `v` together is an error, as in ECMA-262. |
| `y` | A match only at the start position. |

```c
#include <assert.h>
#include <string.h>
#include "corvus_regex_ecma262.h"

static cre_regex *compile(const char *pattern, const char *flags) {
    cre_regex *re = NULL;
    assert(cre_regex_new(pattern, strlen(pattern), flags, strlen(flags), &re) == CRE_OK);
    return re;
}

int main(void) {
    cre_regex *a = compile("\\p{Script=Greek}+", "u");
    cre_regex *b = compile("[[a-z]--[aeiou]]+", "v");
    cre_regex *c = compile("^(?<year>\\d{4})-(?<month>\\d{2})$", "u");

    bool matched = false;
    assert(cre_regex_is_match(a, "\xce\xb1\xce\xb2\xce\xb3", 6, &matched) == CRE_OK && matched);  /* "αβγ" */
    assert(cre_regex_is_match(b, "rhythm", 6, &matched) == CRE_OK && matched);
    cre_str name = cre_regex_group_name(c, 0);
    assert(name.len == 4 && memcmp(name.ptr, "year", 4) == 0);

    cre_regex_free(a);
    cre_regex_free(b);
    cre_regex_free(c);
    return 0;
}
```

With no flag the pattern is read with the Annex B grammar and matched by UTF-16 code unit, which is what ECMA-262 does
with no flag. JSON Schema's reading, the `u` flag first and Annex B when that refuses, is two calls of
`cre_regex_new_with_timeout`, each giving the match timeout, so no default timeout is needed:

```c
#include <assert.h>
#include <string.h>
#include "corvus_regex_ecma262.h"

static cre_status schema_pattern(const char *pattern, cre_regex **out) {
    size_t len = strlen(pattern);
    cre_status status = cre_regex_new_with_timeout(pattern, len, "u", 1, 1000, out);
    if (status == CRE_INVALID_PATTERN) {
        status = cre_regex_new_with_timeout(pattern, len, "", 0, 1000, out);
    }
    return status;
}

int main(void) {
    cre_regex *re = NULL;
    assert(schema_pattern("^\\p{L}+$", &re) == CRE_OK && cre_regex_flags(re).len == 1);
    cre_regex_free(re);
    /* An identity escape, which only Annex B allows. */
    assert(schema_pattern("\\-", &re) == CRE_OK && cre_regex_flags(re).len == 0);
    cre_regex_free(re);
    return 0;
}
```

A pattern that is not valid is `CRE_INVALID_PATTERN`, the `SyntaxError` of `new RegExp`, with the reason in
`cre_last_error_message()`; invalid flags are `CRE_INVALID_FLAGS`. Every pattern has deterministic limits of size, and
`CRE_PATTERN_TOO_LARGE` says when one is passed.

```c
#include <assert.h>
#include "corvus_regex_ecma262.h"

int main(void) {
    cre_regex *bad = NULL;
    assert(cre_regex_new("a{2,1}", 6, "u", 1, &bad) == CRE_INVALID_PATTERN && bad == NULL);
    assert(cre_last_error_message().len > 0);
    assert(cre_regex_new("a", 1, "uv", 2, &bad) == CRE_INVALID_FLAGS);
    return 0;
}
```

## The edition and the version of Unicode

By default a pattern is read with today's grammar, ECMAScript 2025 (`CRE_EDITION_CURRENT`), and with the tables of the
version of Unicode that V8 carries, 17 today (`CRE_UNICODE_DEFAULT`). Either can be overridden when the pattern is
compiled, each independently of the other.

JSON Schema 2020-12 names the grammar of ECMAScript 2020, which `CRE_EDITION_ES2020` selects in place of the default. It
refuses what later editions added, and nothing else: the `d` and `v` flags, modifier groups such as `(?i:...)`, a group
name used by two groups, and a property name or value that is not in the 2020 tables (such as `\p{Script=Yezidi}`). A
pattern it accepts has the same answers as in today's grammar.

The version of Unicode is chosen apart from the edition: `CRE_UNICODE_13`, `CRE_UNICODE_17`, `CRE_UNICODE_18`,
`CRE_UNICODE_LATEST` (18 today) or `CRE_UNICODE_DEFAULT`, the version of the V8 the answers are held to (17 today). It
decides the members of each `\p{...}`, the characters of a group name and the case folding of the `i` flag.
`cre_regex_unicode_version` reports the version a pattern was compiled with. Both are given to
`cre_regex_new_with_options` and read when the pattern is compiled, so a match costs the same whichever is chosen.

```c
#include <assert.h>
#include <string.h>
#include "corvus_regex_ecma262.h"

int main(void) {
    /* ECMAScript 2020 as its edition was published, with the Unicode of its year. */
    const char *greek = "^\\p{Script=Greek}+$";
    cre_regex *re = NULL;
    assert(cre_regex_new_with_options(greek, strlen(greek), "u", 1, 1000, CRE_EDITION_ES2020, CRE_UNICODE_13, &re) == CRE_OK);
    bool matched = false;
    assert(cre_regex_is_match(re, "\xce\xb1\xce\xb2\xce\xb3", 6, &matched) == CRE_OK && matched);
    assert(cre_regex_unicode_version(re) == 13);
    cre_regex_free(re);

    /* Vithkuqi is in Unicode 14 and later, so not in Unicode 13. */
    const char *vith = "\\p{Script=Vithkuqi}";
    bool valid = true;
    assert(cre_regex_is_valid_with_options(vith, strlen(vith), "u", 1, CRE_EDITION_CURRENT, CRE_UNICODE_13, &valid) == CRE_OK && !valid);
    return 0;
}
```

## Matching

The functions are those of a `RegExp`, with the position as an argument where JavaScript keeps `lastIndex`. A
`cre_match` is where a match or a group is, `start` to `end`; a group that did not take part is `CRE_UNSET` to
`CRE_UNSET`.

```c
#include <assert.h>
#include <string.h>
#include "corvus_regex_ecma262.h"

int main(void) {
    const char *pattern = "(?<year>\\d{4})-(?<month>\\d{2})";
    const char *text = "due 2026-10, then 2027-01";
    size_t len = strlen(text);
    cre_regex *re = NULL;
    assert(cre_regex_new(pattern, strlen(pattern), "gu", 2, &re) == CRE_OK);

    bool matched = false;
    assert(cre_regex_is_match(re, text, len, &matched) == CRE_OK && matched);
    cre_match m;
    bool found = false;
    assert(cre_regex_find(re, text, len, 0, &m, &found) == CRE_OK && found && m.start == 4 && m.end == 11);

    /* The groups, by number and by name. */
    cre_captures *caps = NULL;
    assert(cre_captures_new(re, &caps) == CRE_OK);
    assert(cre_regex_captures(re, text, len, 0, caps, &found) == CRE_OK && found);
    assert(cre_captures_name(caps, "year", 4, &m) && m.start == 4 && m.end == 8);
    assert(cre_captures_get(caps, 2, &m) && m.start == 9 && m.end == 11);
    cre_captures_free(caps);

    /* Every match of a global search, as matchAll makes it. */
    cre_matches *all = NULL;
    assert(cre_regex_find_iter(re, text, len, &all) == CRE_OK);
    int count = 0;
    while (cre_matches_next(all, &m, &found) == CRE_OK && found) {
        count++;
    }
    assert(count == 2);
    cre_matches_free(all);

    /* replace replaces every match with the g flag, and the first without it, with every $ form of ECMA-262. */
    cre_string out;
    assert(cre_regex_replace(re, text, len, "$<month>/$1", 11, &out) == CRE_OK);
    assert(strcmp(out.ptr, "due 10/2026, then 01/2027") == 0);
    cre_string_free(&out);

    /* split puts what the groups captured between the pieces; a group that did not take part is {NULL, 0}. */
    cre_regex *sep = NULL;
    assert(cre_regex_new("\\s*(;)\\s*", 9, "", 0, &sep) == CRE_OK);
    cre_pieces pieces;
    assert(cre_regex_split(sep, "a ; b;c", 7, CRE_SPLIT_ALL, &pieces) == CRE_OK);
    assert(pieces.len == 5 && pieces.items[1].len == 1 && pieces.items[1].ptr[0] == ';');
    cre_pieces_free(&pieces);

    cre_regex_free(sep);
    cre_regex_free(re);
    return 0;
}
```

`cre_regex_find(..., start, ...)` is `exec` with `lastIndex` set to `start`, and `y` anchors it there. Replace and
split allocate what they return, which the caller frees. `cre_regex_is_match`, `cre_regex_find`,
`cre_matches_next_batch` (matches into an array the caller owns) and `cre_regex_captures_read`, which writes a match's
groups into an array the caller owns, allocate nothing once the thread has read a match with as many groups:

```c
#include <assert.h>
#include <string.h>
#include "corvus_regex_ecma262.h"

int main(void) {
    const char *pattern = "(?<year>\\d{4})-(?<month>\\d{2})";
    cre_regex *re = NULL;
    assert(cre_regex_new(pattern, strlen(pattern), "u", 1, &re) == CRE_OK);
    cre_match groups[3];
    bool found = false;
    assert(cre_regex_captures_read(re, "due 2026-10", 11, 0, groups, 3, &found) == CRE_OK && found);
    assert(groups[1].start == 4 && groups[1].end == 8);
    cre_regex_free(re);
    return 0;
}
```

A `cre_regex` is immutable, so any number of threads may use one at once; a `cre_captures` or a `cre_matches` is used
by one thread at a time.

## Timeouts

A match has no limit by default. A pattern compiled with a match timeout (in milliseconds) abandons a match that
reaches it, or that reaches the cap on backtracking memory the timeout brings, with `CRE_MATCH_LIMIT`, whatever the
pattern:

```c
#include <assert.h>
#include <string.h>
#include "corvus_regex_ecma262.h"

int main(void) {
    const char *slow = "^(?:(a+)\\1*)+$";
    cre_regex *re = NULL;
    assert(cre_regex_new_with_timeout(slow, strlen(slow), "", 0, 100, &re) == CRE_OK);
    assert(cre_regex_match_timeout(re) == 100);
    char text[61];
    memset(text, 'a', 60);
    text[60] = 'b';
    bool matched = false;
    assert(cre_regex_is_match(re, text, 61, &matched) == CRE_MATCH_LIMIT);
    cre_regex_free(re);
    return 0;
}
```

A pattern compiled without a timeout takes `cre_default_match_timeout()`, which is none (0) unless
`cre_set_default_match_timeout` sets it. A program that matches patterns or texts from outside should set one, for
example with `cre_set_default_match_timeout(1000)` when it starts. Each matching function has a `_limit` form that
gives one match a limit of its own.

A pattern without a backreference is matched in time linear in the text, and no pattern can overflow the stack, so the
timeout is what bounds the others.

## UTF-8 and UTF-16

UTF-8 text is a pointer and a length in bytes, matched as it is with no transcoding, and positions are bytes. It is
checked before it is read: text that is not UTF-8 is `CRE_INVALID_UTF8`, with the offset of the first byte that is not
in `cre_last_error_offset()`, and is never matched or read as U+FFFD. The `_unchecked` forms skip the check, for text
already known to be UTF-8. Every matching function has a `_utf16` form over a pointer to `uint16_t` and a length in
code units, lone surrogates and all, with positions in code units.

```c
#include <assert.h>
#include "corvus_regex_ecma262.h"

int main(void) {
    cre_regex *re = NULL;
    assert(cre_regex_new(".b", 2, "u", 1, &re) == CRE_OK);
    cre_match m;
    bool found = false;

    /* "a😀b" as UTF-8 and as UTF-16, where the emoji is a surrogate pair. */
    assert(cre_regex_find(re, "a\xf0\x9f\x98\x80" "b", 6, 0, &m, &found) == CRE_OK && found);
    assert(m.start == 1 && m.end == 6);  /* bytes of UTF-8 */
    const uint16_t units[] = {0x61, 0xD83D, 0xDE00, 0x62};
    assert(cre_regex_find_utf16(re, units, 4, 0, &m, &found) == CRE_OK && found);
    assert(m.start == 1 && m.end == 4);  /* UTF-16 code units */

    bool matched = false;
    assert(cre_regex_is_match(re, "\xff", 1, &matched) == CRE_INVALID_UTF8 && cre_last_error_offset() == 0);
    cre_regex_free(re);
    return 0;
}
```

Under the `u` and `v` flags a character is a code point in both encodings, so a class, a dot or a quantifier takes a
four-byte sequence as one character. With no flag ECMA-262 matches by UTF-16 code unit; over UTF-8 the library keeps
that meaning, so a match can start or end between the halves of a character beyond the Basic Multilingual Plane, at
the character's third byte.

## Handles, strings and errors

- **Handles.** A `cre_regex`, a `cre_captures` and a `cre_matches` are made by the library and freed by the caller with
  `cre_regex_free`, `cre_captures_free` and `cre_matches_free`. Each free function takes NULL and does nothing.
- **Strings in.** Every string is a pointer and a length, never NUL-terminated on the way in; a pointer may be NULL
  when its length is 0.
- **Strings out.** A borrowed string is a `cre_str`, valid as long as its function says (`cre_regex_source` and
  `cre_regex_flags` as long as the pattern). An owned string is a `cre_string`, NUL-terminated, freed with
  `cre_string_free`; the pieces of a split are a `cre_pieces`, freed with `cre_pieces_free`. The UTF-16 forms are
  `cre_str16`, `cre_string16` and `cre_pieces16`.
- **Status.** Every function that can fail returns a `cre_status`, `CRE_OK` or why it failed, with the message and the
  offset in thread-local storage (`cre_last_error_message`, `cre_last_error_offset`), valid until the next call that
  fails on the thread. On failure each out-parameter is set to its empty value. No Rust panic crosses into the caller:
  a bug in the library is `CRE_PANIC`, and the library remains usable.

## Differences from regex.h and PCRE2

The functions follow `RegExp`; the meaning follows ECMA-262. The differences a user coming from POSIX `regex.h` or
PCRE2 will meet:

- **The grammar.** ECMA-262's, not POSIX's basic or extended grammar, nor PCRE2's: lookbehind, named groups `(?<name>)`,
  modifier groups and Unicode property escapes, with no `(?i)` inline option (ECMA-262 has `(?i:...)` groups), no
  atomic groups, no possessive quantifiers, no `\A` or `\z`, no conditionals and no recursion. The flags are a
  separate string.
- **The match.** The first match `exec` finds, leftmost and by priority of alternatives, not POSIX's leftmost longest.
- **The character classes.** `\d` is `[0-9]`, `\w` is the 63 ASCII word characters, `\s` is ECMAScript's white space
  and line terminators, and `.` excludes the four line terminators (LF, CR, U+2028, U+2029), whatever the locale.
- **Replacement and split.** `cre_regex_replace` takes ECMA-262's template (`$1`, `$<name>`, `$&`, `` $` ``, `$'`),
  and `cre_regex_split` is JavaScript's; `regex.h` has neither.
- **Timeouts.** `regex.h` has no limit, and PCRE2's match limits count steps. Here a pattern without a backreference
  is matched in time linear in the text, and a timeout bounds the time and the memory of the others.
