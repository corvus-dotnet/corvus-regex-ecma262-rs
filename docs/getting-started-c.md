# Getting started with C

The C library is the Rust engine behind a C ABI: `libcorvus_regex_ecma262`, shared and static, with one header,
`corvus_regex_ecma262.h`, which documents every function. Its patterns have the grammar and the answers of
JavaScript's `RegExp`, held to V8's answers on the ECMA-262 test data, over UTF-8 and UTF-16.

## Install

Each [release](https://github.com/corvus-dotnet/corvus-regex-ecma262-rs/releases) has an archive for each platform
(Linux with glibc or musl, macOS and Windows, each on x64 and arm64) holding the header, the shared and static
libraries, a CMake package and a pkg-config file. Unpack it and point CMake at it:

```cmake
find_package(corvus_regex_ecma262 CONFIG REQUIRED)
target_link_libraries(app PRIVATE corvus_regex_ecma262::corvus_regex_ecma262)        # or ..._static
```

or use `pkg-config --cflags --libs corvus_regex_ecma262`. The header is C99.

Every function that can fail returns a `cre_status`: `CRE_OK`, or why it failed, with the message in
`cre_last_error_message()`. Strings are pointer and length, never NUL-terminated on the way in. Text that is not UTF-8
is refused with `CRE_INVALID_UTF8`, never read as U+FFFD.

## Compile a pattern

`cre_regex_new` takes a pattern and its flags as `new RegExp(pattern, flags)` takes them: `d`, `g`, `i`, `m`, `s`, `u`,
`v` and `y`. A pattern that is not valid is `CRE_INVALID_PATTERN`, the `SyntaxError` of `new RegExp`.

```c
#include <assert.h>
#include <string.h>
#include "corvus_regex_ecma262.h"

int main(void) {
    const char *pattern = "(?<year>\\d{4})-(?<month>\\d{2})";
    cre_regex *re = NULL;
    assert(cre_regex_new(pattern, strlen(pattern), "gu", 2, &re) == CRE_OK);
    assert(cre_regex_group_count(re) == 2);
    cre_regex_free(re);

    // Not valid: the status says why, and the message is ECMA-262's.
    cre_regex *bad = NULL;
    assert(cre_regex_new("a{2,1}", 6, "u", 1, &bad) == CRE_INVALID_PATTERN && bad == NULL);
    assert(cre_last_error_message().len > 0);
    assert(cre_regex_new("a", 1, "uv", 2, &bad) == CRE_INVALID_FLAGS);
    return 0;
}
```

## Match

Positions are byte offsets into the UTF-8 text. A group that did not take part is `CRE_UNSET` to `CRE_UNSET`.

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

    // The first match, and its groups into an array the caller owns: the match, then each group.
    cre_match groups[3];
    bool found = false;
    assert(cre_regex_captures_read(re, text, len, 0, groups, 3, &found) == CRE_OK && found);
    assert(groups[0].start == 4 && groups[0].end == 11);
    assert(groups[1].start == 4 && groups[1].end == 8);          // "2026"

    // Every match of a global search.
    cre_matches *all = NULL;
    assert(cre_regex_find_iter(re, text, len, &all) == CRE_OK);
    int count = 0;
    cre_match m;
    while (cre_matches_next(all, &m, &found) == CRE_OK && found) {
        count++;
    }
    assert(count == 2);
    cre_matches_free(all);

    // replace_all, with every $ form of ECMA-262; the result is the caller's to free.
    cre_string out;
    assert(cre_regex_replace_all(re, text, len, "$<month>/$1", 11, &out) == CRE_OK);
    assert(strcmp(out.ptr, "due 10/2026, then 01/2027") == 0);
    cre_string_free(&out);

    // split splices in what the groups captured.
    cre_regex *sep = NULL;
    assert(cre_regex_new("\\s*(;)\\s*", 9, "", 0, &sep) == CRE_OK);
    cre_pieces pieces;
    assert(cre_regex_split(sep, "a ; b;c", 7, CRE_SPLIT_ALL, &pieces) == CRE_OK);
    assert(pieces.len == 5);
    cre_pieces_free(&pieces);

    cre_regex_free(sep);
    cre_regex_free(re);
    return 0;
}
```

## UTF-16

Every matching function has a `_utf16` form over `const uint16_t *`, lone surrogates and all, with positions in code
units.

```c
#include <assert.h>
#include "corvus_regex_ecma262.h"

int main(void) {
    // "a😀b": the emoji is a surrogate pair.
    const uint16_t text[] = {0x61, 0xD83D, 0xDE00, 0x62};
    cre_regex *re = NULL;
    assert(cre_regex_new(".b", 2, "u", 1, &re) == CRE_OK);
    cre_match m;
    bool found = false;
    assert(cre_regex_find_utf16(re, text, 4, 0, &m, &found) == CRE_OK && found);
    assert(m.start == 1 && m.end == 4);
    cre_regex_free(re);
    return 0;
}
```

## Limits, the edition and the version of Unicode

A match has no limit by default. A program that matches a pattern or a text it did not write gives the pattern a match
timeout in milliseconds: a match that runs past it, or whose backtracking memory reaches its cap, is
`CRE_MATCH_LIMIT`. No pattern can overflow the stack. `cre_set_default_match_timeout` sets the timeout of every pattern
compiled after it, and each matching function has a `_limit` form that takes a limit of its own.

`cre_regex_new_with_options` also takes the edition (`CRE_EDITION_ES2020` is the grammar JSON Schema 2020-12 names,
which refuses what later editions added) and the version of Unicode (`CRE_UNICODE_13`, `_17`, `_18`, `_LATEST`, or
`CRE_UNICODE_DEFAULT`, V8's). Both are read when the pattern is compiled, so a match costs the same whatever they are.

```c
#include <assert.h>
#include <string.h>
#include "corvus_regex_ecma262.h"

int main(void) {
    // A pattern that takes time exponential in the text, with a timeout of 100 ms.
    const char *slow = "^(?:(a+)\\1*)+$";
    cre_regex *re = NULL;
    assert(cre_regex_new_with_timeout(slow, strlen(slow), "", 0, 100, &re) == CRE_OK);
    char text[62];
    memset(text, 'a', 60);
    text[60] = 'b';
    text[61] = '\0';
    bool matched = false;
    assert(cre_regex_is_match(re, text, 61, &matched) == CRE_MATCH_LIMIT);
    cre_regex_free(re);

    // ECMAScript 2020 with the Unicode of its year: Vithkuqi came in Unicode 14.
    const char *greek = "^\\p{Script=Greek}+$";
    assert(cre_regex_new_with_options(greek, strlen(greek), "u", 1, 1000, CRE_EDITION_ES2020, CRE_UNICODE_13, &re) == CRE_OK);
    assert(cre_regex_is_match(re, "\xce\xb1\xce\xb2\xce\xb3", 6, &matched) == CRE_OK && matched);
    cre_regex_free(re);
    const char *vith = "\\p{Script=Vithkuqi}";
    assert(cre_regex_new_with_options(vith, strlen(vith), "u", 1, 0, CRE_EDITION_CURRENT, CRE_UNICODE_13, &re) == CRE_INVALID_PATTERN);
    return 0;
}
```

## Where next

- [The header](../corvus-regex-ecma262-capi/include/corvus_regex_ecma262.h), which documents every function: the
  groups by name (`cre_captures`), batches of matches (`cre_matches_next_batch`), and the `_unchecked` forms for text
  already known to be UTF-8.
- [Getting started with Rust](getting-started.md): the engine itself.
- The bindings over this library: C++, Python, Ruby, PHP, Swift, Fortran and R, each with its own page.
- [The benchmark page](https://corvus-oss.org/corvus-regex-ecma262-benchmarks/), with every engine measured nightly.
