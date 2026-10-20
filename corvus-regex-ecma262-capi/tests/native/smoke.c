/* The C library from C99: every function once, linked against the shared library. */

#include <stdio.h>
#include <string.h>

#include "corvus_regex_ecma262.h"

static int failures = 0;

#define CHECK(condition)                                                                   \
    do {                                                                                   \
        if (!(condition)) {                                                                \
            fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #condition); \
            failures++;                                                                    \
        }                                                                                  \
    } while (0)

static cre_regex *compile(const char *pattern, const char *flags) {
    cre_regex *re = NULL;
    cre_status status = cre_regex_new(pattern, strlen(pattern), flags, strlen(flags), &re);
    if (status != CRE_OK) {
        cre_str m = cre_last_error_message();
        fprintf(stderr, "compile failed (%u): %.*s\n", (unsigned)status, (int)m.len, m.ptr);
        failures++;
    }
    return re;
}

int main(void) {
    cre_str v = cre_version();
    CHECK(v.len > 0);

    /* Matching, and the grammar of the flags. */
    cre_regex *re = compile("(?<word>\\p{Lu}\\w+)", "u");
    bool matched = false;
    const char *text = "say \xc3\x89t\xc3\xa9 Hello";
    CHECK(cre_regex_is_match(re, text, strlen(text), &matched) == CRE_OK && matched);
    CHECK(cre_regex_group_count(re) == 1);
    cre_match m;
    bool found = false;
    CHECK(cre_regex_find(re, text, strlen(text), 0, &m, &found) == CRE_OK && found);
    /* \w is ASCII in ECMA-262: the first match is the capital and the "t" after it, and not the accented letter. */
    CHECK(m.start == 4 && m.end == 7);
    CHECK(cre_regex_find(re, text, strlen(text), 7, &m, &found) == CRE_OK && found && m.start == 10 && m.end == 15);
    CHECK(cre_regex_find(re, text, strlen(text), 15, &m, &found) == CRE_OK && !found);
    cre_regex_free(re);

    re = compile("k", "iu");
    const char *kelvin = "\xe2\x84\xaa"; /* U+212A, the Kelvin sign */
    CHECK(cre_regex_is_match(re, kelvin, strlen(kelvin), &matched) == CRE_OK && matched);
    cre_regex_free(re);

    /* Errors: a pattern that is not valid, flags that are not, text that is not UTF-8, a NULL handle. */
    cre_regex *bad = NULL;
    CHECK(cre_regex_new("a**", 3, "", 0, &bad) == CRE_INVALID_PATTERN && bad == NULL);
    CHECK(cre_last_error_message().len > 0);
    CHECK(cre_regex_new("a", 1, "uv", 2, &bad) == CRE_INVALID_FLAGS && bad == NULL);
    CHECK(cre_regex_new("\\&", 2, NULL, 0, &bad) == CRE_OK && bad != NULL);
    CHECK(cre_regex_is_match(bad, "a\xff", 2, &matched) == CRE_INVALID_UTF8 && cre_last_error_offset() == 1);
    cre_regex_free(bad);
    CHECK(cre_regex_is_match(NULL, "a", 1, &matched) == CRE_INVALID_ARGUMENT);
    cre_regex_free(NULL);

    /* A limit on a match that takes longer than anyone waits. */
    re = compile("^(?:(a+)\\1*)+$", "u");
    char slow[64];
    memset(slow, 'a', 60);
    slow[60] = 'b';
    CHECK(cre_regex_is_match_limit(re, slow, 61, 20, &matched) == CRE_MATCH_LIMIT && !matched);
    cre_regex_free(re);

    if (failures == 0) {
        printf("smoke: all checks passed\n");
    }
    return failures == 0 ? 0 : 1;
}
