/* The C library from C99: every function once, over UTF-8 and UTF-16, linked as a consumer links it (the shared or
 * the static library). */

#include <stdio.h>
#include <stdlib.h>
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

static bool is(cre_str s, const char *want) {
    return s.len == strlen(want) && (s.len == 0 || memcmp(s.ptr, want, s.len) == 0);
}

/* An ASCII string as UTF-16, into a buffer of at least strlen(s) units. */
static size_t to16(const char *s, uint16_t *out) {
    size_t n = strlen(s);
    for (size_t k = 0; k < n; k++) {
        out[k] = (uint16_t)(unsigned char)s[k];
    }
    return n;
}

static void the_library(void) {
    char version[32];
    snprintf(version, sizeof version, "%d.%d.%d", CRE_VERSION_MAJOR, CRE_VERSION_MINOR, CRE_VERSION_PATCH);
    CHECK(is(cre_version(), version));
    CHECK(is(cre_status_name(CRE_MATCH_LIMIT), "CRE_MATCH_LIMIT"));
}

static void compiling(void) {
    cre_regex *re = compile("(?<word>\\p{Lu}\\w+)|(?<n>\\d)", "ug");
    CHECK(is(cre_regex_source(re), "(?<word>\\p{Lu}\\w+)|(?<n>\\d)"));
    CHECK(is(cre_regex_flags(re), "gu"));
    CHECK(cre_regex_group_count(re) == 2);
    CHECK(cre_regex_group_name_count(re) == 2);
    CHECK(is(cre_regex_group_name(re, 0), "word") && is(cre_regex_group_name(re, 1), "n"));
    CHECK(cre_regex_group_name(re, 2).ptr == NULL);
    cre_regex_free(re);
    bool valid = true;
    CHECK(cre_regex_is_valid("a{2,1}", 6, "", 0, &valid) == CRE_OK && !valid);
    CHECK(cre_regex_is_valid("a{1,2}", 6, NULL, 0, &valid) == CRE_OK && valid);
}

static void matching_utf8(void) {
    cre_regex *re = compile("(?<word>\\p{Lu}\\w+)", "u");
    bool matched = false;
    const char *text = "say \xc3\x89t\xc3\xa9 Hello";
    size_t len = strlen(text);
    CHECK(cre_regex_is_match(re, text, len, &matched) == CRE_OK && matched);
    CHECK(cre_regex_is_match_at(re, text, len, 10, &matched) == CRE_OK && matched);
    CHECK(cre_regex_is_match_at(re, text, len, 11, &matched) == CRE_OK && !matched);
    cre_match m;
    bool found = false;
    CHECK(cre_regex_find(re, text, len, 0, &m, &found) == CRE_OK && found);
    /* \w is ASCII in ECMA-262: the first match is the capital and the "t" after it, and not the accented letter. */
    CHECK(m.start == 4 && m.end == 7);
    CHECK(cre_regex_find_limit(re, text, len, 7, 1000, &m, &found) == CRE_OK && found && m.start == 10 && m.end == 15);
    CHECK(cre_regex_find(re, text, len, 15, &m, &found) == CRE_OK && !found && m.start == CRE_UNSET);

    cre_match groups[3];
    CHECK(cre_regex_captures_read(re, text, len, 0, groups, 3, &found) == CRE_OK && found);
    CHECK(groups[0].start == 4 && groups[1].start == 4 && groups[1].end == 7 && groups[2].start == CRE_UNSET);
    CHECK(cre_regex_captures_read_limit(re, text, len, 7, 1000, groups, 1, &found) == CRE_OK && found);
    CHECK(groups[0].start == 10);

    cre_captures *caps = NULL;
    CHECK(cre_captures_new(re, &caps) == CRE_OK && caps != NULL);
    CHECK(cre_regex_captures(re, text, len, 0, caps, &found) == CRE_OK && found);
    CHECK(cre_captures_len(caps) == 2);
    CHECK(cre_captures_get(caps, 1, &m) && m.start == 4 && m.end == 7);
    CHECK(cre_captures_name(caps, "word", 4, &m) && m.start == 4 && m.end == 7);
    CHECK(!cre_captures_name(caps, "other", 5, &m) && m.start == CRE_UNSET);
    CHECK(cre_regex_captures_limit(re, text, len, 15, 1000, caps, &found) == CRE_OK && !found);
    CHECK(cre_captures_len(caps) == 0);
    cre_captures_free(caps);
    cre_regex_free(re);

    /* The Kelvin sign folds to k under i and u. */
    re = compile("k", "iu");
    const char *kelvin = "\xe2\x84\xaa";
    CHECK(cre_regex_is_match(re, kelvin, strlen(kelvin), &matched) == CRE_OK && matched);
    cre_regex_free(re);
}

static void searching_replacing_and_splitting_utf8(void) {
    cre_regex *re = compile("(\\d)(?<a>\\d)?", "g");
    const char *text = "a1b23c";
    size_t len = strlen(text);
    cre_matches *it = NULL;
    CHECK(cre_regex_find_iter(re, text, len, &it) == CRE_OK);
    cre_match m;
    bool found = false;
    size_t n = 0;
    while (cre_matches_next(it, &m, &found) == CRE_OK && found) {
        n++;
    }
    CHECK(n == 2);
    cre_matches_reset(it);
    CHECK(cre_matches_next(it, &m, &found) == CRE_OK && found && m.start == 1 && m.end == 2);
    /* The rest a batch at a time: fewer than asked for ends the search. */
    cre_match batch[4];
    CHECK(cre_matches_next_batch(it, batch, 4, &n) == CRE_OK && n == 1 && batch[0].start == 3 && batch[0].end == 5);
    CHECK(cre_matches_next_batch(it, batch, 4, &n) == CRE_OK && n == 0);
    cre_matches_reset(it);
    CHECK(cre_matches_next_batch(it, batch, 1, &n) == CRE_OK && n == 1 && batch[0].start == 1);
    CHECK(cre_matches_next_batch(it, NULL, 0, &n) == CRE_OK && n == 0);
    CHECK(cre_matches_next_batch(it, batch, 4, &n) == CRE_OK && n == 1 && batch[0].start == 3);
    cre_matches_free(it);

    cre_captures *caps = NULL;
    CHECK(cre_captures_new(re, &caps) == CRE_OK);
    CHECK(cre_regex_captures_iter(re, text, len, &it) == CRE_OK);
    CHECK(cre_matches_next_captures(it, caps, &found) == CRE_OK && found && !cre_captures_name(caps, "a", 1, &m));
    CHECK(cre_matches_next_captures(it, caps, &found) == CRE_OK && found && cre_captures_name(caps, "a", 1, &m));
    CHECK(m.start == 4 && m.end == 5);
    CHECK(cre_matches_next_captures(it, caps, &found) == CRE_OK && !found);
    /* The groups a batch at a time, each match as its three groups. */
    cre_matches_reset(it);
    cre_match groups[7];
    CHECK(cre_matches_next_captures_batch(it, groups, 7, &n) == CRE_OK && n == 2);
    CHECK(groups[0].start == 1 && groups[1].start == 1 && groups[2].start == CRE_UNSET);
    CHECK(groups[3].start == 3 && groups[3].end == 5 && groups[5].start == 4 && groups[5].end == 5);
    CHECK(cre_matches_next_captures_batch(it, groups, 2, &n) == CRE_INVALID_ARGUMENT && n == 0);
    cre_matches_free(it);
    cre_captures_free(caps);

    const char *template = "<$&|$1|$<a>|$$>";
    cre_string out = {NULL, 0};
    CHECK(cre_regex_replace(re, text, len, template, strlen(template), &out) == CRE_OK);
    CHECK(strcmp(out.ptr, "a<1|1||$>b<23|2|3|$>c") == 0 && out.len == strlen(out.ptr));
    cre_string_free(&out);
    CHECK(out.ptr == NULL);
    cre_regex_free(re);

    re = compile("\\d", "");
    CHECK(cre_regex_replace(re, text, len, "#", 1, &out) == CRE_OK && strcmp(out.ptr, "a#b23c") == 0);
    cre_string_free(&out);
    CHECK(cre_regex_replace_all(re, text, len, "#", 1, &out) == CRE_OK && strcmp(out.ptr, "a#b##c") == 0);
    cre_string_free(&out);
    cre_regex_free(re);

    re = compile("\\s*(,)?\\s*;", "");
    const char *list = "a ;b,;c";
    cre_pieces pieces = {NULL, 0, NULL};
    CHECK(cre_regex_split(re, list, strlen(list), CRE_SPLIT_ALL, &pieces) == CRE_OK && pieces.len == 5);
    CHECK(is(pieces.items[0], "a") && pieces.items[1].ptr == NULL && is(pieces.items[2], "b"));
    CHECK(is(pieces.items[3], ",") && is(pieces.items[4], "c"));
    cre_pieces_free(&pieces);
    CHECK(pieces.items == NULL && pieces.len == 0);
    CHECK(cre_regex_split(re, list, strlen(list), 1, &pieces) == CRE_OK && pieces.len == 1 && is(pieces.items[0], "a"));
    cre_pieces_free(&pieces);
    cre_regex_free(re);
}

static void utf16(void) {
    /* U+1F600 as a surrogate pair, then a lone high surrogate. */
    uint16_t text[] = {'a', 0xD83D, 0xDE00, 'b', 0xD83D, '1', '2'};
    size_t len = sizeof text / sizeof text[0];
    cre_regex *re = compile("\\ud83d", "");
    bool matched = false;
    CHECK(cre_regex_is_match_utf16(re, text, len, &matched) == CRE_OK && matched);
    CHECK(cre_regex_is_match_limit_utf16(re, text, len, 1000, &matched) == CRE_OK && matched);
    CHECK(cre_regex_is_match_at_utf16(re, text, len, 5, &matched) == CRE_OK && !matched);
    cre_match m;
    bool found = false;
    CHECK(cre_regex_find_utf16(re, text, len, 2, &m, &found) == CRE_OK && found && m.start == 4 && m.end == 5);
    CHECK(cre_regex_find_limit_utf16(re, text, len, 0, 1000, &m, &found) == CRE_OK && found && m.start == 1);
    cre_regex_free(re);

    re = compile("(?<d>\\d)", "gu");
    cre_match groups[2];
    CHECK(cre_regex_captures_read_utf16(re, text, len, 0, groups, 2, &found) == CRE_OK && found && groups[1].start == 5);
    CHECK(cre_regex_captures_read_limit_utf16(re, text, len, 6, 1000, groups, 2, &found) == CRE_OK && found);
    CHECK(groups[1].start == 6);
    cre_captures *caps = NULL;
    CHECK(cre_captures_new(re, &caps) == CRE_OK);
    CHECK(cre_regex_captures_utf16(re, text, len, 0, caps, &found) == CRE_OK && found);
    CHECK(cre_captures_name(caps, "d", 1, &m) && m.start == 5);
    CHECK(cre_regex_captures_limit_utf16(re, text, len, 6, 1000, caps, &found) == CRE_OK && found);
    cre_matches *it = NULL;
    size_t n = 0;
    CHECK(cre_regex_find_iter_utf16(re, text, len, &it) == CRE_OK);
    while (cre_matches_next(it, &m, &found) == CRE_OK && found) {
        n++;
    }
    CHECK(n == 2);
    cre_matches_reset(it);
    cre_match batch[8];
    CHECK(cre_matches_next_batch(it, batch, 8, &n) == CRE_OK && n == 2 && batch[0].start == 5 && batch[1].start == 6);
    cre_matches_free(it);
    CHECK(cre_regex_captures_iter_utf16(re, text, len, &it) == CRE_OK);
    CHECK(cre_matches_next_captures(it, caps, &found) == CRE_OK && found && cre_captures_get(caps, 1, &m));
    cre_matches_free(it);
    cre_captures_free(caps);

    /* Without the g flag, replace replaces the first match and replace_all every one. */
    cre_regex *first = compile("(?<d>\\d)", "u");
    uint16_t template[8];
    size_t template_len = to16("<$<d>>", template);
    cre_string16 out = {NULL, 0};
    CHECK(cre_regex_replace_utf16(first, text, len, template, template_len, &out) == CRE_OK && out.len == 9);
    CHECK(out.ptr[1] == 0xD83D && out.ptr[4] == 0xD83D && out.ptr[5] == '<' && out.ptr[8] == '2' && out.ptr[9] == 0);
    cre_string16_free(&out);
    CHECK(cre_regex_replace_all_utf16(first, text, len, template, template_len, &out) == CRE_OK && out.len == 11);
    cre_regex_free(first);
    cre_string16_free(&out);
    CHECK(out.ptr == NULL);

    cre_pieces16 pieces = {NULL, 0, NULL};
    CHECK(cre_regex_split_utf16(re, text, len, CRE_SPLIT_ALL, &pieces) == CRE_OK && pieces.len == 5);
    CHECK(pieces.items[0].len == 5 && pieces.items[1].len == 1 && pieces.items[1].ptr[0] == '1');
    CHECK(pieces.items[4].ptr != NULL && pieces.items[4].len == 0);
    cre_pieces16_free(&pieces);
    cre_regex_free(re);
}

static void errors_and_limits(void) {
    cre_regex *bad = NULL;
    bool matched = false;
    CHECK(cre_regex_new("a**", 3, "", 0, &bad) == CRE_INVALID_PATTERN && bad == NULL);
    CHECK(cre_last_error_message().len > 0);
    CHECK(cre_regex_new("a", 1, "uv", 2, &bad) == CRE_INVALID_FLAGS && bad == NULL);
    CHECK(cre_regex_new("\\&", 2, NULL, 0, &bad) == CRE_OK && bad != NULL);
    CHECK(cre_regex_is_match(bad, "a\xff", 2, &matched) == CRE_INVALID_UTF8 && cre_last_error_offset() == 1);
    cre_match m;
    bool found = true;
    CHECK(cre_regex_find(bad, "a", 1, 2, &m, &found) == CRE_INVALID_ARGUMENT && !found);
    cre_regex_free(bad);
    CHECK(cre_regex_is_match(NULL, "a", 1, &matched) == CRE_INVALID_ARGUMENT);

    /* A pattern nested deeper than CRE_MAX_NESTING. */
    size_t depth = CRE_MAX_NESTING + 1;
    char *deep = malloc(4 * depth + 2);
    size_t k = 0;
    for (size_t d = 0; d < depth; d++) {
        memcpy(deep + k, "(?:", 3);
        k += 3;
    }
    deep[k++] = 'a';
    memset(deep + k, ')', depth);
    k += depth;
    CHECK(cre_regex_new(deep, k, "u", 1, &bad) == CRE_PATTERN_TOO_LARGE && bad == NULL);
    free(deep);

    /* A limit on a match that takes longer than anyone waits. */
    cre_regex *re = compile("^(?:(a+)\\1*)+$", "u");
    char slow[64];
    memset(slow, 'a', 60);
    slow[60] = 'b';
    CHECK(cre_regex_is_match_limit(re, slow, 61, 20, &matched) == CRE_MATCH_LIMIT && !matched);
    cre_regex_free(re);

    cre_regex_free(NULL);
    cre_captures_free(NULL);
    cre_matches_free(NULL);
    cre_matches_reset(NULL);
    cre_string_free(NULL);
    cre_string16_free(NULL);
    cre_pieces_free(NULL);
    cre_pieces16_free(NULL);
}

int main(void) {
    the_library();
    compiling();
    matching_utf8();
    searching_replacing_and_splitting_utf8();
    utf16();
    errors_and_limits();
    if (failures == 0) {
        printf("smoke: all checks passed (library %.*s)\n", (int)cre_version().len, cre_version().ptr);
    }
    return failures == 0 ? 0 : 1;
}
