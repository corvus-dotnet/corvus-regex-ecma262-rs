/* Runs an engine over the ECMA-262 regular expression test data (github.com/corvus-dotnet/ecma262-regex-test-data),
 * from C, judged by the data's own rule ("How a runner judges an engine" in its README) exactly as the Rust runner
 * (tools/src/bin/dataset.rs) judges the crate:
 *
 *   dataset <data directory> <wrong-cases output file> [file filter] [utf16|utf8|both] [--compliance <file>]
 *
 * Built against the C library (corvus_regex_ecma262.h, linked as a consumer links it), it judges the library. Built
 * with DATASET_QUICKJS against QuickJS's libregexp (quickjs-ng), it judges that engine instead, as `dataset-quickjs`.
 *
 * Each record is asked about under each grammar V8 found it valid in: the record's flags with u added (the unicode
 * answers), the record's flags alone (the legacy answers) and, in the file that has them, the flags with v added (the
 * unicode_sets answers). A text is matched as UTF-16 and as UTF-8; a text with a lone surrogate has no UTF-8 form and
 * is matched as UTF-16 alone. Records marked v8_deviation are left out. Any difference from V8 is a wrong answer; a
 * refusal of a valid pattern, an acceptance of an invalid one, and a match that reaches its limit are counted apart
 * and listed too. captures.jsonl is judged on exec, the global search, the sticky flag, replace and split.
 * ill-formed-utf8.jsonl holds bytes that are not UTF-8, which the C library must refuse with CRE_INVALID_UTF8.
 *
 * The C library is judged through its own functions for each of them, over UTF-16 and UTF-8. libregexp has exec
 * alone, over UTF-16 alone (it has no UTF-8 interface), so for it the runner is the caller, as QuickJS's own
 * String.prototype methods are: the global search, replace (GetSubstitution) and split are written over its exec as
 * the Rust runner writes them over regress's, and on captures.jsonl it gives the seven answers over UTF-16. A match is
 * given 2 seconds and, with libregexp, 256 MB of backtracking memory; reaching either is an answer never given.
 *
 * exec/test262.jsonl, the exec calls of test262 (in a directory of its own), is judged last: each call one exec with
 * the record's flags from its lastIndex, right when the positions of the match and of every group, the named groups
 * and, with g or y, the lastIndex after are V8's; one answer over UTF-16 and, when the text holds no lone surrogate and
 * lastIndex is not between the halves of a pair, one over UTF-8 (the C library alone) with positions in bytes.
 *
 * With --compliance <file>, the engine's compliance object is written to the file, as the benchmark page reads it
 * (docs/implementing-a-source.md in corvus-regex-ecma262-benchmarks): the counts, this runner and the calls it makes
 * in each encoding, and what it leaves out (nothing for the C library; for libregexp, UTF-8 and the operations
 * written over its exec). */

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "json.h"
#include "platform.h"

#ifdef DATASET_QUICKJS
#include "libregexp.h"
#else
#include "corvus_regex_ecma262.h"
#endif

#define LIMIT_MS 2000
#define THREADS 4
static const char REPLACEMENT[] = "<$&|$1|$2|$10|$<a>|$$|$`|$'>";

typedef struct {
    long long right, wrong, refused, accepted, limited, errors;
    /* The answers of the grammar-only reader that agree with V8's on whether a pattern is valid, and those that do
     * not. */
    long long reader_right, reader_wrong;
} counts;

static void counts_add(counts *a, const counts *b) {
    a->right += b->right;
    a->wrong += b->wrong;
    a->refused += b->refused;
    a->accepted += b->accepted;
    a->limited += b->limited;
    a->errors += b->errors;
    a->reader_right += b->reader_right;
    a->reader_wrong += b->reader_wrong;
}

typedef struct {
    bool utf16, utf8;
} options;

/* ------------------------------------------------------------------------------------------------------------------
 * Strings */

typedef struct {
    char *p;
    size_t len, cap;
} sbuf;

static void sb_grow(sbuf *b, size_t more) {
    if (b->len + more + 1 > b->cap) {
        b->cap = (b->len + more + 1) * 2;
        b->p = realloc(b->p, b->cap);
        if (!b->p) abort();
    }
}

static void sb_put(sbuf *b, const char *s, size_t n) {
    sb_grow(b, n);
    memcpy(b->p + b->len, s, n);
    b->len += n;
    b->p[b->len] = 0;
}

#ifdef DATASET_QUICKJS
static void sb_puts(sbuf *b, const char *s) { sb_put(b, s, strlen(s)); }
#endif

static void sb_printf(sbuf *b, const char *f, ...) {
    va_list a;
    va_start(a, f);
    int n = vsnprintf(NULL, 0, f, a);
    va_end(a);
    sb_grow(b, (size_t)n);
    va_start(a, f);
    vsnprintf(b->p + b->len, (size_t)n + 1, f, a);
    va_end(a);
    b->len += (size_t)n;
}

/* The units as the Rust runner's escape() writes them: printable ASCII but the backslash as itself, the rest \uXXXX. */
static void sb_escape(sbuf *b, const uint16_t *u, size_t n) {
    for (size_t k = 0; k < n; k++) {
        if (u[k] >= 0x20 && u[k] < 0x7f && u[k] != '\\') {
            char c = (char)u[k];
            sb_put(b, &c, 1);
        } else {
            sb_printf(b, "\\u%04x", u[k]);
        }
    }
}

/* The first line of a message. */
static void sb_first_line(sbuf *b, const char *s) {
    size_t n = strcspn(s, "\r\n");
    sb_put(b, s, n);
}

typedef struct {
    char **items;
    size_t len, cap;
} lines;

static void lines_push(lines *l, sbuf *b) {
    if (l->len == l->cap) {
        l->cap = l->cap ? l->cap * 2 : 64;
        l->items = realloc(l->items, l->cap * sizeof *l->items);
        if (!l->items) abort();
    }
    l->items[l->len++] = b->p;
    b->p = NULL;
    b->len = b->cap = 0;
}

static bool units_equal(const uint16_t *a, size_t an, const uint16_t *b, size_t bn) {
    return an == bn && (an == 0 || memcmp(a, b, an * sizeof *a) == 0);
}

#ifndef DATASET_QUICKJS
/* For each UTF-16 code unit index of a text (and the index past its end), the byte offset in UTF-8; the low half of a
 * pair is its character's third byte. */
static size_t *byte_offsets(const uint16_t *s, size_t n) {
    size_t *at = malloc((n + 1) * sizeof *at);
    if (!at) abort();
    size_t b = 0, k = 0;
    while (k < n) {
        at[k] = b;
        uint16_t c = s[k];
        if (c >= 0xD800 && c <= 0xDBFF && k + 1 < n && s[k + 1] >= 0xDC00 && s[k + 1] <= 0xDFFF) {
            at[k + 1] = b + 2;
            b += 4;
            k += 2;
        } else {
            b += c < 0x80 ? 1 : c < 0x800 ? 2 : 3;
            k += 1;
        }
    }
    at[n] = b;
    return at;
}
#endif

/* ------------------------------------------------------------------------------------------------------------------
 * exec answers: null, or [start, end] for the match then [start, end] or null for each group. */

typedef struct {
    bool matched;
    size_t len;  /* groups, the match among them */
    long *spans; /* 2 per group, -1 for a group that did not take part */
} exec_answer;

static exec_answer exec_of(const json *e) {
    exec_answer a = {false, 0, NULL};
    if (!e || e->kind != JSON_ARRAY) return a;
    a.matched = true;
    a.len = e->len;
    a.spans = malloc((e->len ? e->len : 1) * 2 * sizeof *a.spans);
    for (size_t k = 0; k < e->len; k++) {
        const json *g = &e->items[k];
        if (g->kind == JSON_ARRAY && g->len == 2) {
            a.spans[2 * k] = (long)g->items[0].number;
            a.spans[2 * k + 1] = (long)g->items[1].number;
        } else {
            a.spans[2 * k] = a.spans[2 * k + 1] = -1;
        }
    }
    return a;
}

/* Whether the groups found (CRE_UNSET-style, (size_t)-1 for none) are the expected ones, positions mapped by `map`
 * (NULL for none). */
static bool same_slots(const exec_answer *e, bool found, const size_t *got, size_t got_len, const size_t *map) {
    if (!e->matched || !found) return !e->matched && !found;
    if (e->len != got_len) return false;
    for (size_t k = 0; k < e->len; k++) {
        long s = e->spans[2 * k], t = e->spans[2 * k + 1];
        size_t gs = got[2 * k], gt = got[2 * k + 1];
        if (s < 0) {
            if (gs != (size_t)-1) return false;
        } else {
            if (gs == (size_t)-1) return false;
            size_t ms = map ? map[s] : (size_t)s, mt = map ? map[t] : (size_t)t;
            if (ms != gs || mt != gt) return false;
        }
    }
    return true;
}

/* Whether the pieces of a split are the expected array of strings and nulls. */
typedef struct {
    bool present;
    const uint16_t *units;
    size_t len;
} piece16;

static bool same_split(const json *expected, const piece16 *got, size_t n) {
    if (!expected || expected->kind != JSON_ARRAY || expected->len != n) return false;
    for (size_t k = 0; k < n; k++) {
        const json *e = &expected->items[k];
        if (e->kind == JSON_NULL) {
            if (got[k].present) return false;
        } else if (e->kind == JSON_STRING) {
            if (!got[k].present || !units_equal(e->units, e->len, got[k].units, got[k].len)) return false;
        } else {
            return false;
        }
    }
    return true;
}

/* ------------------------------------------------------------------------------------------------------------------
 * The engine judged on the answers of test: compile, and test over UTF-16 and (where it has it) UTF-8. */

/* TEST_REFUSED: the text is not UTF-8, and the engine said so (the right answer in ill-formed-utf8.jsonl). */
typedef enum { TEST_NO = 0, TEST_YES = 1, TEST_LIMIT = -1, TEST_ERROR = -2, TEST_REFUSED = -3 } test_result;

#ifndef DATASET_QUICKJS

static const bool ENGINE_HAS_UTF8 = true;
typedef cre_regex engine_re;

static char *message_copy(void) {
    cre_str m = cre_last_error_message();
    char *s = malloc(m.len + 1);
    memcpy(s, m.ptr, m.len);
    s[m.len] = 0;
    return s;
}

/* The pattern as the Rust runner gives it to the crate: UTF-8, a lone surrogate as U+FFFD. NULL on a refusal, with
 * *is_error for a refusal that is not of the pattern (the flags, or a bug) and the message. */
static engine_re *engine_compile(const uint16_t *pattern, size_t pattern_len, const char *flags, bool *is_error,
                                 char **message) {
    size_t len;
    char *p = utf16_to_utf8_lossy(pattern, pattern_len, &len);
    cre_regex *re = NULL;
    cre_status status = cre_regex_new(p, len, flags, strlen(flags), &re);
    free(p);
    *is_error = false;
    *message = NULL;
    if (status == CRE_OK) return re;
    sbuf b = {0};
    char *m = message_copy();
    if (status == CRE_INVALID_PATTERN) {
        sb_printf(&b, "PARSE: %s", m);
    } else if (status == CRE_PATTERN_TOO_LARGE) {
        sb_printf(&b, "TOO-LARGE: %s", m);
    } else {
        *is_error = true;
        sb_printf(&b, "ERROR: %s", m);
    }
    free(m);
    *message = b.p;
    return NULL;
}

static void engine_free(engine_re *re) { cre_regex_free(re); }

static test_result test_status(cre_status status, bool matched, const char **why) {
    if (status == CRE_OK) return matched ? TEST_YES : TEST_NO;
    if (status == CRE_INVALID_UTF8) return TEST_REFUSED;
    if (status == CRE_MATCH_LIMIT) {
        char *m = message_copy();
        *why = strstr(m, "memory") ? "memory" : "time";
        free(m);
        return TEST_LIMIT;
    }
    *why = "panic";
    return TEST_ERROR;
}

static test_result engine_test16(engine_re *re, const uint16_t *text, size_t len, const char **why) {
    bool matched = false;
    /* The call first: the order in which a function's arguments are evaluated is unspecified (MSVC goes from the
     * right). */
    cre_status status = cre_regex_is_match_limit_utf16(re, text, len, LIMIT_MS, &matched);
    return test_status(status, matched, why);
}

static test_result engine_test8(engine_re *re, const char *text, size_t len, const char **why) {
    bool matched = false;
    /* The call first: the order in which a function's arguments are evaluated is unspecified (MSVC goes from the
     * right). */
    cre_status status = cre_regex_is_match_limit(re, text, len, LIMIT_MS, &matched);
    return test_status(status, matched, why);
}

#else /* DATASET_QUICKJS */

static const bool ENGINE_HAS_UTF8 = false;

/* The thread's deadline for the match running, its stack's base, and the bytes of backtracking memory it holds. */
static THREAD_LOCAL double deadline;
static THREAD_LOCAL char *stack_base;
#define WORKER_STACK ((size_t)256 << 20)
#define STACK_MARGIN ((size_t)8 << 20)
#define MEMORY_CAP ((size_t)256 << 20)

bool lre_check_stack_overflow(void *opaque, size_t alloca_size) {
    (void)opaque;
    char here;
    if (!stack_base) return false;
    size_t used = (size_t)(stack_base > &here ? stack_base - &here : &here - stack_base);
    return used + alloca_size > WORKER_STACK - STACK_MARGIN;
}

int lre_check_timeout(void *opaque) {
    (void)opaque;
    return deadline > 0 && now_seconds() > deadline;
}

void *lre_realloc(void *opaque, void *ptr, size_t size) {
    (void)opaque;
    if (size == 0) {
        free(ptr);
        return NULL;
    }
    if (size > MEMORY_CAP) return NULL;
    return realloc(ptr, size);
}

typedef struct {
    uint8_t *bytecode;
    int flags;
    int capture_count;
    int alloc_count;
    bool unicode;
    /* The names of groups 1 to capture_count - 1, as UTF-16 (NULL for a group with none), when it has any. */
    uint16_t **names;
    size_t *name_lens;
} engine_re;

/* The flags as js_compile_regexp reads them: each of dgimsuvy once, and not u with v. -1 for flags it refuses. */
static int lre_flags_of(const char *flags) {
    int re_flags = 0;
    for (const char *f = flags; *f; f++) {
        int mask;
        switch (*f) {
        case 'd': mask = LRE_FLAG_INDICES; break;
        case 'g': mask = LRE_FLAG_GLOBAL; break;
        case 'i': mask = LRE_FLAG_IGNORECASE; break;
        case 'm': mask = LRE_FLAG_MULTILINE; break;
        case 's': mask = LRE_FLAG_DOTALL; break;
        case 'u': mask = LRE_FLAG_UNICODE; break;
        case 'v': mask = LRE_FLAG_UNICODE_SETS; break;
        case 'y': mask = LRE_FLAG_STICKY; break;
        default: return -1;
        }
        if (re_flags & mask) return -1;
        re_flags |= mask;
    }
    if ((re_flags & LRE_FLAG_UNICODE) && (re_flags & LRE_FLAG_UNICODE_SETS)) return -1;
    return re_flags;
}

/* The pattern as QuickJS hands it to lre_compile (JS_ToCStringLen2): CESU-8 without u or v, each code unit on its own;
 * UTF-8 with them, a pair as one character and a lone surrogate as three bytes. */
static char *pattern_bytes(const uint16_t *s, size_t n, bool cesu8, size_t *out_len) {
    char *out = malloc(3 * n + 1);
    size_t b = 0;
    for (size_t k = 0; k < n; k++) {
        uint32_t c = s[k];
        if (!cesu8 && c >= 0xD800 && c <= 0xDBFF && k + 1 < n && s[k + 1] >= 0xDC00 && s[k + 1] <= 0xDFFF) {
            c = 0x10000 + ((c - 0xD800) << 10) + (s[k + 1] - 0xDC00);
            k++;
        }
        if (c < 0x80) {
            out[b++] = (char)c;
        } else if (c < 0x800) {
            out[b++] = (char)(0xC0 | (c >> 6));
            out[b++] = (char)(0x80 | (c & 63));
        } else if (c < 0x10000) {
            out[b++] = (char)(0xE0 | (c >> 12));
            out[b++] = (char)(0x80 | ((c >> 6) & 63));
            out[b++] = (char)(0x80 | (c & 63));
        } else {
            out[b++] = (char)(0xF0 | (c >> 18));
            out[b++] = (char)(0x80 | ((c >> 12) & 63));
            out[b++] = (char)(0x80 | ((c >> 6) & 63));
            out[b++] = (char)(0x80 | (c & 63));
        }
    }
    out[b] = 0;
    *out_len = b;
    return out;
}

static engine_re *engine_compile(const uint16_t *pattern, size_t pattern_len, const char *flags, bool *is_error,
                                 char **message) {
    *is_error = false;
    *message = NULL;
    sbuf b = {0};
    int re_flags = lre_flags_of(flags);
    if (re_flags < 0) {
        sb_puts(&b, "PARSE: invalid regular expression flags");
        *message = b.p;
        return NULL;
    }
    bool unicode = (re_flags & (LRE_FLAG_UNICODE | LRE_FLAG_UNICODE_SETS)) != 0;
    size_t len;
    char *p = pattern_bytes(pattern, pattern_len, !unicode, &len);
    char error[128];
    int bytecode_len = 0;
    deadline = now_seconds() + LIMIT_MS / 1000.0;
    uint8_t *bc = lre_compile(&bytecode_len, error, sizeof error, p, len, re_flags, NULL);
    deadline = 0;
    free(p);
    if (!bc) {
        sb_printf(&b, "PARSE: %s", error);
        *message = b.p;
        return NULL;
    }
    engine_re *re = calloc(1, sizeof *re);
    re->bytecode = bc;
    re->flags = re_flags;
    re->capture_count = lre_get_capture_count(bc);
    re->alloc_count = lre_get_alloc_count(bc);
    re->unicode = unicode;
    const char *names = lre_get_groupnames(bc);
    if (names) {
        re->names = calloc((size_t)re->capture_count, sizeof *re->names);
        re->name_lens = calloc((size_t)re->capture_count, sizeof *re->name_lens);
        for (int g = 1; g < re->capture_count; g++) {
            size_t n = strlen(names);
            if (n > 0) re->names[g] = utf8_to_utf16(names, n, &re->name_lens[g]);
            names += n + LRE_GROUP_NAME_TRAILER_LEN;
        }
    }
    return re;
}

static void engine_free(engine_re *re) {
    if (!re) return;
    if (re->names) {
        for (int g = 0; g < re->capture_count; g++) free(re->names[g]);
        free(re->names);
        free(re->name_lens);
    }
    free(re->bytecode);
    free(re);
}

/* exec at `from`: 1 with groups[2 * capture_count] filled ((size_t)-1 for a group that did not take part), 0 for no
 * match, TEST_LIMIT or TEST_ERROR. */
static int engine_exec16(engine_re *re, const uint16_t *text, size_t len, size_t from, size_t *groups,
                         const char **why) {
    if (from > len) return 0;
    int alloc = re->alloc_count > 0 ? re->alloc_count : 1;
    uint8_t **capture = malloc((size_t)alloc * sizeof *capture);
    deadline = now_seconds() + LIMIT_MS / 1000.0;
    int rc = lre_exec(capture, re->bytecode, (const uint8_t *)text, (int)from, (int)len, 1, NULL);
    deadline = 0;
    if (rc == 1 && groups) {
        for (int g = 0; g < re->capture_count; g++) {
            uint8_t *s = capture[2 * g], *e = capture[2 * g + 1];
            if (s && e) {
                groups[2 * g] = (size_t)(s - (const uint8_t *)text) / 2;
                groups[2 * g + 1] = (size_t)(e - (const uint8_t *)text) / 2;
            } else {
                groups[2 * g] = groups[2 * g + 1] = (size_t)-1;
            }
        }
    }
    free(capture);
    if (rc == 1 || rc == 0) return rc;
    if (rc == LRE_RET_TIMEOUT) {
        *why = "time";
        return TEST_LIMIT;
    }
    if (rc == LRE_RET_MEMORY_ERROR) {
        *why = "memory";
        return TEST_LIMIT;
    }
    *why = "error";
    return TEST_ERROR;
}

static test_result engine_test16(engine_re *re, const uint16_t *text, size_t len, const char **why) {
    return (test_result)engine_exec16(re, text, len, 0, NULL, why);
}

static test_result engine_test8(engine_re *re, const char *text, size_t len, const char **why) {
    (void)re, (void)text, (void)len, (void)why;
    return TEST_ERROR;
}

#endif

/* ------------------------------------------------------------------------------------------------------------------
 * The test answers */

typedef struct {
    const char *suffix, *answer;
    bool valid;
} mode;

static size_t modes_of(const json *r, mode out[3]) {
    const json *valid = json_get(r, "valid");
    size_t n = 0;
    bool v = false;
    json_as_bool(json_get(valid, "unicode"), &v);
    out[n++] = (mode){"u", "unicode", v};
    v = false;
    json_as_bool(json_get(valid, "legacy"), &v);
    out[n++] = (mode){"", "legacy", v};
    const json *sets = json_get(valid, "unicode_sets");
    if (sets) {
        v = false;
        json_as_bool(sets, &v);
        out[n++] = (mode){"v", "unicode_sets", v};
    }
    return n;
}

/* The pattern as the Rust runner shows it: through UTF-8 (a lone surrogate as U+FFFD) and back, escaped. */
static void sb_shown(sbuf *b, const json *pattern) {
    size_t len, len16;
    char *p8 = utf16_to_utf8_lossy(pattern->units, pattern->len, &len);
    uint16_t *p16 = utf8_to_utf16(p8, len, &len16);
    sb_escape(b, p16, len16);
    free(p8);
    free(p16);
}

static void judge_tests(const char *name, const json *r, const json *pattern, const options *o, counts *c, lines *w) {
    const json *flags_j = json_string(json_get(r, "flags"));
    size_t flags_len;
    char *flags = utf16_to_utf8_lossy(flags_j ? flags_j->units : NULL, flags_j ? flags_j->len : 0, &flags_len);
    sbuf shown = {0};
    sb_shown(&shown, pattern);
    mode modes[3];
    size_t mode_count = modes_of(r, modes);
    const json *tests = json_get(r, "tests");
    for (size_t m = 0; m < mode_count; m++) {
        char mode_flags[32];
        snprintf(mode_flags, sizeof mode_flags, "%s%s", flags, modes[m].suffix);
        bool is_error;
        char *message;
        engine_re *re = engine_compile(pattern->units, pattern->len, mode_flags, &is_error, &message);
        sbuf b = {0};
        if (!re) {
            if (is_error) c->errors++;
            if (modes[m].valid) {
                c->refused++;
                sb_printf(&b, "REFUSED\t%s\t/%s/%s\t", name, shown.p ? shown.p : "", mode_flags);
                sb_first_line(&b, message);
                lines_push(w, &b);
            }
            free(message);
            continue;
        }
        if (!modes[m].valid) {
            c->accepted++;
            sb_printf(&b, "ACCEPTED-INVALID\t%s\t/%s/%s", name, shown.p ? shown.p : "", mode_flags);
            lines_push(w, &b);
            engine_free(re);
            continue;
        }
        for (size_t t = 0; tests && tests->kind == JSON_ARRAY && t < tests->len; t++) {
            const json *test = &tests->items[t];
            bool expected;
            if (!json_as_bool(json_get(test, modes[m].answer), &expected)) continue;
            const json *text = json_string(json_get(test, "text"));
            if (!text) continue;
            for (int encoding = 0; encoding < 2; encoding++) {
                const char *why = "";
                test_result result;
                if (encoding == 0) {
                    if (!o->utf16) continue;
                    result = engine_test16(re, text->units, text->len, &why);
                } else {
                    if (!o->utf8 || !ENGINE_HAS_UTF8 || has_lone_surrogate(text->units, text->len)) continue;
                    size_t len;
                    char *s = utf16_to_utf8_lossy(text->units, text->len, &len);
                    result = engine_test8(re, s, len, &why);
                    free(s);
                }
                const char *enc = encoding == 0 ? "utf16" : "utf8";
                if (result == TEST_YES || result == TEST_NO) {
                    if ((result == TEST_YES) == expected) {
                        c->right++;
                        continue;
                    }
                    c->wrong++;
                    sb_printf(&b, "WRONG\t%s\t/%s/%s\t", name, shown.p ? shown.p : "", mode_flags);
                    sb_escape(&b, text->units, text->len);
                    sb_printf(&b, "\t%s\texpected %s", enc, expected ? "true" : "false");
                } else if (result == TEST_ERROR) {
                    c->errors++;
                    sb_printf(&b, "ERROR\t%s\t/%s/%s\t", name, shown.p ? shown.p : "", mode_flags);
                    sb_escape(&b, text->units, text->len);
                    sb_printf(&b, "\t%s\t%s", enc, why);
                } else {
                    c->limited++;
                    sb_printf(&b, "LIMIT\t%s\t/%s/%s\t", name, shown.p ? shown.p : "", mode_flags);
                    sb_escape(&b, text->units, text->len);
                    sb_printf(&b, "\t%s\t%s", enc, why);
                }
                lines_push(w, &b);
            }
        }
        engine_free(re);
    }
    free(shown.p);
    free(flags);
}

/* A record of ill-formed-utf8.jsonl: under each grammar the pattern is valid in, each test's bytes, which are not
 * UTF-8, are one answer over UTF-8, and the right one is a refusal (CRE_INVALID_UTF8). An engine with no UTF-8
 * interface is asked nothing. */
static void judge_ill_formed(const json *r, const json *pattern, const options *o, counts *c, lines *w) {
    if (!o->utf8 || !ENGINE_HAS_UTF8) return;
    const char *name = "ill-formed-utf8.jsonl";
    const json *flags_j = json_string(json_get(r, "flags"));
    size_t flags_len;
    char *flags = utf16_to_utf8_lossy(flags_j ? flags_j->units : NULL, flags_j ? flags_j->len : 0, &flags_len);
    sbuf shown = {0};
    sb_shown(&shown, pattern);
    mode modes[3];
    size_t mode_count = modes_of(r, modes);
    const json *tests = json_get(r, "tests");
    for (size_t m = 0; m < mode_count; m++) {
        char mode_flags[32];
        snprintf(mode_flags, sizeof mode_flags, "%s%s", flags, modes[m].suffix);
        bool is_error;
        char *message;
        engine_re *re = engine_compile(pattern->units, pattern->len, mode_flags, &is_error, &message);
        sbuf b = {0};
        if (!re) {
            if (is_error) c->errors++;
            if (modes[m].valid) {
                c->refused++;
                sb_printf(&b, "REFUSED\t%s\t/%s/%s\t", name, shown.p ? shown.p : "", mode_flags);
                sb_first_line(&b, message);
                lines_push(w, &b);
            }
            free(message);
            continue;
        }
        if (!modes[m].valid) {
            c->accepted++;
            sb_printf(&b, "ACCEPTED-INVALID\t%s\t/%s/%s", name, shown.p ? shown.p : "", mode_flags);
            lines_push(w, &b);
            engine_free(re);
            continue;
        }
        for (size_t t = 0; tests && tests->kind == JSON_ARRAY && t < tests->len; t++) {
            const json *hex = json_string(json_get(&tests->items[t], "bytes"));
            if (!hex) continue;
            size_t len = hex->len / 2;
            char *bytes = malloc(len + 1);
            for (size_t k = 0; k < len; k++) {
                char pair[3] = {(char)hex->units[2 * k], (char)hex->units[2 * k + 1], 0};
                bytes[k] = (char)strtol(pair, NULL, 16);
            }
            const char *why = "";
            test_result result = engine_test8(re, bytes, len, &why);
            free(bytes);
            if (result == TEST_REFUSED) {
                c->right++;
                continue;
            }
            if (result == TEST_YES || result == TEST_NO) {
                c->wrong++;
                sb_printf(&b, "WRONG\t%s\t/%s/%s\t", name, shown.p ? shown.p : "", mode_flags);
                sb_escape(&b, hex->units, hex->len);
                sb_printf(&b, "\tutf8\tread as UTF-8 (%s), not refused", result == TEST_YES ? "matched" : "no match");
            } else if (result == TEST_ERROR) {
                c->errors++;
                sb_printf(&b, "ERROR\t%s\t/%s/%s\t", name, shown.p ? shown.p : "", mode_flags);
                sb_escape(&b, hex->units, hex->len);
                sb_printf(&b, "\tutf8\t%s", why);
            } else {
                c->limited++;
                sb_printf(&b, "LIMIT\t%s\t/%s/%s\t", name, shown.p ? shown.p : "", mode_flags);
                sb_escape(&b, hex->units, hex->len);
                sb_printf(&b, "\tutf8\t%s", why);
            }
            lines_push(w, &b);
        }
        engine_free(re);
    }
    free(shown.p);
    free(flags);
}

/* ------------------------------------------------------------------------------------------------------------------
 * captures.jsonl */

typedef struct {
    counts *c;
    lines *w;
    sbuf where;
} judge;

static void check(judge *j, const char *what, bool ok) {
    if (ok) {
        j->c->right++;
        return;
    }
    j->c->wrong++;
    sbuf b = {0};
    sb_printf(&b, "WRONG\tcaptures.jsonl\t%s\t%s", j->where.p, what);
    lines_push(j->w, &b);
}

/* The compiled forms of a record's pattern, by flags, as the Rust runner caches them. */
typedef struct {
    char flags[48];
    engine_re *re;
} cached;

static engine_re *make(cached *cache, size_t *n, const json *pattern, const char *flags) {
    for (size_t k = 0; k < *n; k++) {
        if (strcmp(cache[k].flags, flags) == 0) return cache[k].re;
    }
    cached *e = &cache[(*n)++];
    snprintf(e->flags, sizeof e->flags, "%s", flags);
    bool is_error;
    char *message;
    e->re = engine_compile(pattern->units, pattern->len, flags, &is_error, &message);
    free(message);
    return e->re;
}

#ifndef DATASET_QUICKJS

/* The groups of a captures handle as spans, (size_t)-1 for a group that did not take part. */
static size_t groups_of(cre_captures *caps, size_t *spans) {
    size_t n = cre_captures_len(caps);
    for (size_t k = 0; k < n; k++) {
        cre_match m;
        cre_captures_get(caps, k, &m);
        spans[2 * k] = m.start;
        spans[2 * k + 1] = m.end;
    }
    return n;
}

/* The global search's matches against all (at most 20 of them), positions mapped by map: the Rust runner's
 * judge_all. */
static bool judge_all(cre_matches *it, const exec_answer *all, size_t all_len, const size_t *map) {
    size_t n = 0;
    cre_match m;
    bool found;
    while (cre_matches_next(it, &m, &found) == CRE_OK && found) {
        /* One match more than the data has: wrong, unless the data kept only the first twenty. */
        if (n >= all_len) return all_len == 20;
        if (!all[n].matched) return false;
        long s = all[n].spans[0], t = all[n].spans[1];
        size_t ms = map ? map[s] : (size_t)s, mt = map ? map[t] : (size_t)t;
        if (ms != m.start || mt != m.end) return false;
        n++;
    }
    return !((all_len < 20 && n != all_len) || (all_len == 20 && n < 20));
}

static bool same_pieces16(const json *expected, const cre_pieces16 *got) {
    piece16 *p = calloc(got->len ? got->len : 1, sizeof *p);
    for (size_t k = 0; k < got->len; k++) {
        p[k].present = got->items[k].ptr != NULL;
        p[k].units = got->items[k].ptr;
        p[k].len = got->items[k].len;
    }
    bool ok = same_split(expected, p, got->len);
    free(p);
    return ok;
}

static void judge_mode(judge *j, const json *a, const uint16_t *text, size_t text_len, cre_regex *re, cre_regex *g,
                       cre_regex *y) {
    bool lone = has_lone_surrogate(text, text_len);
    size_t utf8_len = 0;
    char *utf8 = lone ? NULL : utf16_to_utf8_lossy(text, text_len, &utf8_len);
    size_t *byte_at = lone ? NULL : byte_offsets(text, text_len);
    size_t groups = cre_regex_group_count(re) + 1;
    size_t *spans = malloc(2 * groups * sizeof *spans);
    cre_captures *caps = NULL, *gcaps = NULL, *ycaps = NULL;
    cre_captures_new(re, &caps);
    cre_captures_new(g, &gcaps);
    cre_captures_new(y, &ycaps);
    bool found;

    /* exec */
    exec_answer exec = exec_of(json_get(a, "exec"));
    cre_status status = cre_regex_captures_limit_utf16(re, text, text_len, 0, LIMIT_MS, caps, &found);
    if (status != CRE_OK) {
        sbuf b = {0};
        if (status == CRE_MATCH_LIMIT) {
            j->c->limited++;
            sb_printf(&b, "LIMIT\tcaptures.jsonl\t%s", j->where.p);
        } else {
            j->c->errors++;
            sb_printf(&b, "ERROR\tcaptures.jsonl\t%s\t%s", j->where.p, cre_status_name(status).ptr);
        }
        lines_push(j->w, &b);
        goto done;
    }
    check(j, "exec", same_slots(&exec, found, spans, groups_of(caps, spans), NULL));
    if (!lone) {
        cre_regex_captures(re, utf8, utf8_len, 0, caps, &found);
        check(j, "exec-utf8", same_slots(&exec, found, spans, groups_of(caps, spans), byte_at));
    }

    /* all */
    {
        const json *all_j = json_get(a, "all");
        size_t all_len = all_j && all_j->kind == JSON_ARRAY ? all_j->len : 0;
        exec_answer *all = calloc(all_len ? all_len : 1, sizeof *all);
        for (size_t k = 0; k < all_len; k++) all[k] = exec_of(&all_j->items[k]);
        /* The groups of each match too, over UTF-16. */
        bool all_ok = true;
        size_t n = 0;
        cre_matches *it = NULL;
        cre_regex_captures_iter_utf16(g, text, text_len, &it);
        while (cre_matches_next_captures(it, gcaps, &found) == CRE_OK && found) {
            if (n >= all_len) {
                all_ok = all_ok && all_len == 20;
                break;
            }
            if (!same_slots(&all[n], true, spans, groups_of(gcaps, spans), NULL)) {
                all_ok = false;
                break;
            }
            n++;
        }
        cre_matches_free(it);
        if ((all_len < 20 && n != all_len) || (all_len == 20 && n < 20)) all_ok = false;
        cre_regex_find_iter_utf16(g, text, text_len, &it);
        check(j, "all", all_ok && judge_all(it, all, all_len, NULL));
        cre_matches_free(it);
        if (!lone) {
            cre_regex_find_iter(g, utf8, utf8_len, &it);
            check(j, "all-utf8", judge_all(it, all, all_len, byte_at));
            cre_matches_free(it);
        }
        for (size_t k = 0; k < all_len; k++) free(all[k].spans);
        free(all);
    }

    /* sticky at 1 */
    if (text_len > 0) {
        exec_answer sticky = exec_of(json_get(a, "sticky"));
        cre_regex_captures_utf16(y, text, text_len, 1, ycaps, &found);
        check(j, "sticky", same_slots(&sticky, found, spans, groups_of(ycaps, spans), NULL));
        if (!lone && (byte_at[1] >= utf8_len || ((unsigned char)utf8[byte_at[1]] & 0xC0) != 0x80)) {
            cre_regex_captures(y, utf8, utf8_len, byte_at[1], ycaps, &found);
            check(j, "sticky-utf8", same_slots(&sticky, found, spans, groups_of(ycaps, spans), byte_at));
        }
        free(sticky.spans);
    }

    /* replace */
    {
        size_t template_len;
        uint16_t *template = utf8_to_utf16(REPLACEMENT, strlen(REPLACEMENT), &template_len);
        const json *replace = json_string(json_get(a, "replace"));
        const json *replace_all = json_string(json_get(a, "replaceAll"));
        cre_string16 out16 = {NULL, 0};
        cre_regex_replace_utf16(re, text, text_len, template, template_len, &out16);
        check(j, "replace", replace && units_equal(out16.ptr, out16.len, replace->units, replace->len));
        cre_string16_free(&out16);
        cre_regex_replace_utf16(g, text, text_len, template, template_len, &out16);
        check(j, "replaceAll", replace_all && units_equal(out16.ptr, out16.len, replace_all->units, replace_all->len));
        cre_string16_free(&out16);
        if (!lone) {
            /* Over UTF-8 a half character is written as U+FFFD, which is what a lossy conversion of V8's answer writes
             * for it. */
            for (int k = 0; k < 2; k++) {
                const json *want = k == 0 ? replace : replace_all;
                size_t want_len = 0;
                char *want8 = want ? utf16_to_utf8_lossy(want->units, want->len, &want_len) : NULL;
                cre_string out = {NULL, 0};
                cre_regex_replace(k == 0 ? re : g, utf8, utf8_len, REPLACEMENT, strlen(REPLACEMENT), &out);
                check(j, k == 0 ? "replace-utf8" : "replaceAll-utf8",
                      want8 && out.ptr && out.len == want_len && memcmp(out.ptr, want8, want_len) == 0);
                cre_string_free(&out);
                free(want8);
            }
        }
        free(template);
    }

    /* split */
    {
        cre_pieces16 pieces = {NULL, 0, NULL};
        cre_regex_split_utf16(re, text, text_len, CRE_SPLIT_ALL, &pieces);
        check(j, "split", same_pieces16(json_get(a, "split"), &pieces));
        cre_pieces16_free(&pieces);
        cre_regex_split_utf16(re, text, text_len, 3, &pieces);
        check(j, "split3", same_pieces16(json_get(a, "split3"), &pieces));
        cre_pieces16_free(&pieces);
    }
    free(exec.spans);
done:
    cre_captures_free(caps);
    cre_captures_free(gcaps);
    cre_captures_free(ycaps);
    free(spans);
    free(byte_at);
    free(utf8);
}

#else /* DATASET_QUICKJS */

/* After an empty match at pos, the next position: a code point with the u or v flag, a code unit without. */
static size_t advance16(const uint16_t *text, size_t len, size_t pos, bool unicode) {
    if (unicode && pos + 1 < len && text[pos] >= 0xD800 && text[pos] <= 0xDBFF && text[pos + 1] >= 0xDC00 &&
        text[pos + 1] <= 0xDFFF) {
        return pos + 2;
    }
    return pos + 1;
}

typedef struct {
    uint16_t *u;
    size_t len, cap;
} ubuf;

static void ub_put(ubuf *b, const uint16_t *s, size_t n) {
    if (b->len + n > b->cap) {
        b->cap = (b->len + n) * 2 + 16;
        b->u = realloc(b->u, b->cap * sizeof *b->u);
    }
    if (n) memcpy(b->u + b->len, s, n * sizeof *s);
    b->len += n;
}

static void ub_group(ubuf *b, const uint16_t *text, const size_t *groups, size_t g) {
    if (groups[2 * g] != (size_t)-1) ub_put(b, text + groups[2 * g], groups[2 * g + 1] - groups[2 * g]);
}

/* GetSubstitution over UTF-16, as the Rust runner writes it for regress. */
static void substitute16(ubuf *out, const uint16_t *text, size_t text_len, const size_t *groups, const engine_re *re,
                         const uint16_t *template, size_t tlen) {
    size_t from = groups[0], to = groups[1];
    size_t m = (size_t)re->capture_count - 1;
    size_t k = 0;
#define DIGIT(i) ((i) < tlen && template[i] >= '0' && template[i] <= '9' ? (long)(template[i] - '0') : -1L)
    while (k < tlen) {
        if (template[k] != '$' || k + 1 >= tlen) {
            ub_put(out, &template[k], 1);
            k++;
            continue;
        }
        uint16_t c = template[k + 1];
        if (c == '$') {
            ub_put(out, &template[k], 1);
            k += 2;
        } else if (c == '&') {
            ub_put(out, text + from, to - from);
            k += 2;
        } else if (c == '`') {
            ub_put(out, text, from);
            k += 2;
        } else if (c == '\'') {
            ub_put(out, text + to, text_len - to);
            k += 2;
        } else if (c >= '0' && c <= '9') {
            long d1 = DIGIT(k + 1), d2 = DIGIT(k + 2);
            if (d2 >= 0 && d1 * 10 + d2 >= 1 && (size_t)(d1 * 10 + d2) <= m) {
                ub_group(out, text, groups, (size_t)(d1 * 10 + d2));
                k += 3;
            } else if (d1 >= 1 && (size_t)d1 <= m) {
                ub_group(out, text, groups, (size_t)d1);
                k += 2;
            } else {
                ub_put(out, &template[k], 1);
                k++;
            }
        } else if (c == '<') {
            size_t n = 0;
            bool closed = false;
            for (size_t i = k + 2; i < tlen; i++, n++) {
                if (template[i] == '>') {
                    closed = true;
                    break;
                }
            }
            if (closed && re->names) {
                for (size_t g = 1; g <= m; g++) {
                    if (re->names[g] && units_equal(re->names[g], re->name_lens[g], template + k + 2, n) &&
                        groups[2 * g] != (size_t)-1) {
                        ub_group(out, text, groups, g);
                        break;
                    }
                }
                k += n + 3;
            } else {
                ub_put(out, &template[k], 1);
                k++;
            }
        } else {
            ub_put(out, &template[k], 1);
            k++;
        }
    }
#undef DIGIT
}

static bool same_exec(const exec_answer *e, int rc, const size_t *groups, size_t n) {
    return same_slots(e, rc == 1, groups, n, NULL);
}

static void judge_mode(judge *j, const json *a, const uint16_t *text, size_t text_len, engine_re *re, engine_re *g,
                       engine_re *y) {
    (void)g;
    size_t n = (size_t)re->capture_count;
    size_t *groups = malloc(2 * n * sizeof *groups);
    const char *why = "";
    bool unicode = re->unicode;
    /* An answer never given (a limit or an error) leaves every answer of the test and grammar after it unanswered
     * too, since each is built on the matches before it. */
    int limited = 0;
    long long before = j->c->right + j->c->wrong;

    exec_answer exec = exec_of(json_get(a, "exec"));
    int rc = engine_exec16(re, text, text_len, 0, groups, &why);
    if (rc < 0) {
        limited = rc;
        goto unanswered;
    }
    check(j, "exec", same_exec(&exec, rc, groups, n));

    /* The global search. */
    {
        const json *all_j = json_get(a, "all");
        size_t all_len = all_j && all_j->kind == JSON_ARRAY ? all_j->len : 0;
        size_t found = 0, pos = 0;
        bool ok = true;
        while (found < 20) {
            rc = engine_exec16(re, text, text_len, pos, groups, &why);
            if (rc < 0) {
                limited = rc;
                goto unanswered;
            }
            if (rc != 1) break;
            size_t s = groups[0], e = groups[1];
            pos = s == e ? advance16(text, text_len, e, unicode) : e;
            if (found < all_len) {
                exec_answer want = exec_of(&all_j->items[found]);
                if (!same_exec(&want, 1, groups, n)) ok = false;
                free(want.spans);
            }
            found++;
        }
        bool ok_all = (all_len < 20 ? found == all_len : found >= 20) && ok;
        check(j, "all", ok_all);
    }

    /* sticky at 1, by the y flag of libregexp. */
    if (text_len > 0) {
        exec_answer sticky = exec_of(json_get(a, "sticky"));
        size_t *ygroups = malloc(2 * (size_t)y->capture_count * sizeof *ygroups);
        rc = engine_exec16(y, text, text_len, 1, ygroups, &why);
        if (rc < 0) {
            free(ygroups);
            free(sticky.spans);
            limited = rc;
            goto unanswered;
        }
        check(j, "sticky", same_slots(&sticky, rc == 1, ygroups, (size_t)y->capture_count, NULL));
        free(ygroups);
        free(sticky.spans);
    }

    /* replace and replaceAll. */
    {
        size_t tlen;
        uint16_t *template = utf8_to_utf16(REPLACEMENT, strlen(REPLACEMENT), &tlen);
        for (int global = 0; global < 2; global++) {
            ubuf out = {0};
            size_t last = 0, pos = 0;
            for (;;) {
                rc = engine_exec16(re, text, text_len, pos, groups, &why);
                if (rc < 0) {
                    free(out.u);
                    free(template);
                    limited = rc;
                    goto unanswered;
                }
                if (rc != 1) break;
                size_t s = groups[0], e = groups[1];
                ub_put(&out, text + last, s - last);
                substitute16(&out, text, text_len, groups, re, template, tlen);
                last = e;
                if (!global) break;
                pos = s == e ? advance16(text, text_len, e, unicode) : e;
            }
            ub_put(&out, text + last, text_len - last);
            const json *want = json_string(json_get(a, global ? "replaceAll" : "replace"));
            check(j, global ? "replaceAll" : "replace", want && units_equal(want->units, want->len, out.u, out.len));
            free(out.u);
        }
        free(template);
    }

    /* split and split3. */
    for (int k = 0; k < 2; k++) {
        size_t limit = k == 0 ? 0xFFFFFFFFu : 3;
        piece16 *out = malloc((text_len + n + 2) * (n + 1) * sizeof *out);
        size_t count = 0;
        if (text_len == 0) {
            rc = engine_exec16(re, text, text_len, 0, groups, &why);
            if (rc < 0) {
                free(out);
                limited = rc;
                goto unanswered;
            }
            if (rc != 1) out[count++] = (piece16){true, text, 0};
        } else {
            size_t p = 0, q = 0;
            bool stop = false;
            while (q < text_len && !stop) {
                rc = engine_exec16(re, text, text_len, q, groups, &why);
                if (rc < 0) {
                    free(out);
                    limited = rc;
                    goto unanswered;
                }
                if (rc != 1) break;
                size_t s = groups[0], e = groups[1];
                if (s >= text_len) break;
                size_t end = e < text_len ? e : text_len;
                if (end == p) {
                    q = advance16(text, text_len, s, unicode);
                    continue;
                }
                out[count++] = (piece16){true, text + p, s - p};
                if (count == limit) {
                    stop = true;
                    break;
                }
                p = end;
                for (size_t gi = 1; gi < n; gi++) {
                    if (groups[2 * gi] == (size_t)-1) {
                        out[count++] = (piece16){false, NULL, 0};
                    } else {
                        out[count++] = (piece16){true, text + groups[2 * gi], groups[2 * gi + 1] - groups[2 * gi]};
                    }
                    if (count == limit) {
                        stop = true;
                        break;
                    }
                }
                q = p;
            }
            if (count < limit) out[count++] = (piece16){true, text + p, text_len - p};
        }
        check(j, k == 0 ? "split" : "split3", same_split(json_get(a, k == 0 ? "split" : "split3"), out, count));
        free(out);
    }
    free(exec.spans);
    free(groups);
    return;

unanswered:
    /* Each of the seven answers (six for an empty text, which has no sticky answer) not given. */
    free(exec.spans);
    free(groups);
    {
        long long missing = (text_len > 0 ? 7 : 6) - (j->c->right + j->c->wrong - before);
        sbuf b = {0};
        if (limited == TEST_LIMIT) {
            j->c->limited += missing;
            sb_printf(&b, "LIMIT\tcaptures.jsonl\t%s\t%s", j->where.p, why);
        } else {
            j->c->errors += missing;
            sb_printf(&b, "ERROR\tcaptures.jsonl\t%s\t%s", j->where.p, why);
        }
        lines_push(j->w, &b);
    }
}

#endif

static void judge_captures(const json *r, const json *pattern, counts *c, lines *w) {
    const json *flags_j = json_string(json_get(r, "flags"));
    size_t flags_len;
    char *flags = utf16_to_utf8_lossy(flags_j ? flags_j->units : NULL, flags_j ? flags_j->len : 0, &flags_len);
    const json *valid = json_get(r, "valid");
    const json *tests = json_get(r, "tests");
    sbuf shown = {0};
    sb_shown(&shown, pattern);
    cached cache[8];
    size_t cached_n = 0;
    for (size_t t = 0; tests && tests->kind == JSON_ARRAY && t < tests->len; t++) {
        const json *test = &tests->items[t];
        if (json_is_true(json_get(test, "v8_deviation"))) continue;
        const json *text = json_string(json_get(test, "text"));
        if (!text) continue;
        for (int m = 0; m < 2; m++) {
            const char *mode = m == 0 ? "unicode" : "legacy";
            if (!json_is_true(json_get(valid, mode))) continue;
            const json *a = json_get(test, mode);
            if (!a) continue;
            char mode_flags[32], gflags[40], yflags[40];
            snprintf(mode_flags, sizeof mode_flags, "%s%s", flags, m == 0 ? "u" : "");
            snprintf(gflags, sizeof gflags, "%sg", mode_flags);
            snprintf(yflags, sizeof yflags, "%sy", mode_flags);
            judge j = {c, w, {0}};
            sb_printf(&j.where, "/%s/%s\t", shown.p ? shown.p : "", mode_flags);
            sb_escape(&j.where, text->units, text->len);
            engine_re *re = make(cache, &cached_n, pattern, mode_flags);
            engine_re *g = make(cache, &cached_n, pattern, gflags);
            engine_re *y = make(cache, &cached_n, pattern, yflags);
            if (!re || !g || !y) {
                c->refused++;
                sbuf b = {0};
                sb_printf(&b, "REFUSED\tcaptures.jsonl\t%s", j.where.p);
                lines_push(w, &b);
            } else {
                judge_mode(&j, a, text->units, text->len, re, g, y);
            }
            free(j.where.p);
        }
    }
    for (size_t k = 0; k < cached_n; k++) engine_free(cache[k].re);
    free(shown.p);
    free(flags);
}

/* ------------------------------------------------------------------------------------------------------------------
 * exec/test262.jsonl */

static const char EXEC[] = "exec/test262.jsonl";

/* An engine's match for a call: the bounds of the match and of each group ((size_t)-1 for a group that did not take
 * part), and the bounds of each name's group (the one of that name that took part), the names as UTF-16. */
typedef struct {
    size_t groups_len;
    size_t *groups;
    size_t names_len;
    const uint16_t **names;
    size_t *name_lens;
    size_t *named; /* 2 per name */
} exec_got;

static void exec_got_free(exec_got *g) {
    free(g->groups);
    free(g->names);
    free(g->name_lens);
    free(g->named);
}

/* A bound of the answer (a UTF-16 position) against the engine's, through map (NULL for none). */
static bool same_bound(const json *b, size_t got, const size_t *map) {
    size_t k = (size_t)b->number;
    return (map ? map[k] : k) == got;
}

/* A span of the answer (null, or [start, end]) against the engine's. */
static bool same_span(const json *e, size_t s, size_t t, const size_t *map) {
    if (e->kind == JSON_NULL) return s == (size_t)-1;
    if (e->kind != JSON_ARRAY || e->len != 2 || s == (size_t)-1) return false;
    return same_bound(&e->items[0], s, map) && same_bound(&e->items[1], t, map);
}

/* Whether an engine's match (NULL for none) is the call's answer: both null, or the same groups, the same named groups
 * (present exactly when the pattern has names) and, with g or y, a lastIndex after at the match's end. */
static bool same_exec_answer(const json *answer, const exec_got *g, bool carries, const size_t *map) {
    if (!g || answer->kind == JSON_NULL) return !g && answer->kind == JSON_NULL;
    const json *captures = json_get(answer, "captures");
    if (!captures || captures->kind != JSON_ARRAY || captures->len != g->groups_len) return false;
    for (size_t k = 0; k < g->groups_len; k++) {
        if (!same_span(&captures->items[k], g->groups[2 * k], g->groups[2 * k + 1], map)) return false;
    }
    const json *groups = json_get(answer, "groups");
    if (!groups) {
        if (g->names_len) return false;
    } else {
        if (groups->kind != JSON_OBJECT || groups->len != g->names_len) return false;
        for (size_t k = 0; k < groups->len; k++) {
            const json_member *m = &groups->members[k];
            size_t name_len;
            uint16_t *name = utf8_to_utf16(m->name, strlen(m->name), &name_len);
            size_t found = g->names_len;
            for (size_t n = 0; n < g->names_len; n++) {
                if (units_equal(name, name_len, g->names[n], g->name_lens[n])) found = n;
            }
            free(name);
            if (found == g->names_len || !same_span(&m->value, g->named[2 * found], g->named[2 * found + 1], map))
                return false;
        }
    }
    if (!carries) return true;
    const json *last = json_get(answer, "lastIndex");
    return last && last->kind == JSON_NUMBER && same_bound(last, g->groups[1], map);
}

/* A call's lastIndex. Past the end of the text exec gives null; test262 sets it as far as 2^53 - 1 and
 * Number.MAX_VALUE, so anything past the end reads as the text's length plus one. */
static size_t last_index_of(const json *t, size_t len) {
    const json *v = json_get(t, "lastIndex");
    double d = v && v->kind == JSON_NUMBER ? v->number : 0;
    return d > (double)len ? len + 1 : (size_t)d;
}

#ifndef DATASET_QUICKJS
/* Whether a position is between the halves of a surrogate pair, where UTF-8 has no position (libregexp has no UTF-8). */
static bool inside_pair(const uint16_t *s, size_t len, size_t at) {
    return at > 0 && at < len && s[at] >= 0xDC00 && s[at] <= 0xDFFF && s[at - 1] >= 0xD800 && s[at - 1] <= 0xDBFF;
}
#endif

/* Counts one answer of a call: 1 right, 0 wrong, TEST_LIMIT or TEST_ERROR. */
static void count_exec(counts *c, lines *w, const char *where, const char *encoding, int outcome, const char *why) {
    sbuf b = {0};
    switch (outcome) {
    case 1: c->right++; return;
    case 0:
        c->wrong++;
        sb_printf(&b, "WRONG\t%s\t%s\t%s", EXEC, where, encoding);
        break;
    case TEST_LIMIT:
        c->limited++;
        sb_printf(&b, "LIMIT\t%s\t%s\t%s\t%s", EXEC, where, encoding, why ? why : "limit");
        break;
    default:
        c->errors++;
        sb_printf(&b, "ERROR\t%s\t%s\t%s\t%s", EXEC, where, encoding, why ? why : "error");
        break;
    }
    lines_push(w, &b);
}

#ifndef DATASET_QUICKJS

/* The C library's exec at `start` over UTF-16 (utf8 NULL) or UTF-8: 1 with *g filled, 0 for no match, TEST_LIMIT or
 * TEST_ERROR. A y flag makes it sticky, as the crate's own captures are. */
static int exec_call(cre_regex *re, const uint16_t *text, size_t len, const char *utf8, size_t utf8_len, size_t start,
                     exec_got *g, const char **why) {
    cre_captures *caps = NULL;
    cre_captures_new(re, &caps);
    bool found = false;
    cre_status status = utf8 ? cre_regex_captures_limit(re, utf8, utf8_len, start, LIMIT_MS, caps, &found)
                             : cre_regex_captures_limit_utf16(re, text, len, start, LIMIT_MS, caps, &found);
    int rc = status == CRE_OK ? (found ? 1 : 0) : status == CRE_MATCH_LIMIT ? TEST_LIMIT : TEST_ERROR;
    if (rc == TEST_LIMIT) *why = "time";
    if (rc == TEST_ERROR) *why = "error";
    if (rc == 1) {
        g->groups_len = cre_captures_len(caps);
        g->groups = malloc(2 * g->groups_len * sizeof *g->groups);
        groups_of(caps, g->groups);
        g->names_len = cre_regex_group_name_count(re);
        g->names = calloc(g->names_len + 1, sizeof *g->names);
        g->name_lens = calloc(g->names_len + 1, sizeof *g->name_lens);
        g->named = malloc((2 * g->names_len + 1) * sizeof *g->named);
        for (size_t n = 0; n < g->names_len; n++) {
            cre_str name = cre_regex_group_name(re, n);
            g->names[n] = utf8_to_utf16(name.ptr, name.len, &g->name_lens[n]);
            cre_match m;
            cre_captures_name(caps, name.ptr, name.len, &m);
            g->named[2 * n] = m.start;
            g->named[2 * n + 1] = m.end;
        }
    }
    cre_captures_free(caps);
    return rc;
}

#else

/* libregexp's exec at `start` over UTF-16: 1 with *g filled (a name's group the one of that name that took part), 0
 * for no match, TEST_LIMIT or TEST_ERROR. libregexp reads the y flag from its bytecode, as QuickJS's exec does. */
static int exec_call(engine_re *re, const uint16_t *text, size_t len, const char *utf8, size_t utf8_len, size_t start,
                     exec_got *g, const char **why) {
    (void)utf8, (void)utf8_len;
    size_t n = (size_t)re->capture_count;
    size_t *groups = malloc(2 * n * sizeof *groups);
    int rc = engine_exec16(re, text, len, start, groups, why);
    if (rc != 1) {
        free(groups);
        return rc;
    }
    g->groups_len = n;
    g->groups = groups;
    g->names = calloc(n, sizeof *g->names);
    g->name_lens = calloc(n, sizeof *g->name_lens);
    g->named = malloc(2 * n * sizeof *g->named);
    for (size_t k = 1; re->names && k < n; k++) {
        if (!re->names[k]) continue;
        size_t at = g->names_len;
        for (size_t m = 0; m < g->names_len; m++) {
            if (units_equal(g->names[m], g->name_lens[m], re->names[k], re->name_lens[k])) at = m;
        }
        if (at == g->names_len) {
            g->names[at] = re->names[k];
            g->name_lens[at] = re->name_lens[k];
            g->named[2 * at] = g->named[2 * at + 1] = (size_t)-1;
            g->names_len++;
        }
        if (g->named[2 * at] == (size_t)-1 && groups[2 * k] != (size_t)-1) {
            g->named[2 * at] = groups[2 * k];
            g->named[2 * at + 1] = groups[2 * k + 1];
        }
    }
    return 1;
}

#endif

/* One answer of a call: exec from start (null past the end of the text), against the answer. */
static int exec_answer_of(engine_re *re, const json *answer, const uint16_t *text, size_t len, const char *utf8,
                          size_t utf8_len, const size_t *byte_at, size_t start, bool carries, const char **why) {
    if (start > len) return answer->kind == JSON_NULL ? 1 : 0;
    exec_got g = {0};
    int rc = exec_call(re, text, len, utf8, utf8_len, utf8 ? byte_at[start] : start, &g, why);
    if (rc < 0) return rc;
    bool ok = same_exec_answer(answer, rc == 1 ? &g : NULL, carries, utf8 ? byte_at : NULL);
    exec_got_free(&g);
    return ok ? 1 : 0;
}

/* Judges a record of exec/test262.jsonl: the pattern with its own flags (g, y and d among them), from each call's
 * lastIndex. */
static void judge_exec(const json *r, const json *pattern, const options *o, counts *c, lines *w) {
    const json *flags_j = json_string(json_get(r, "flags"));
    size_t flags_len;
    char *flags = utf16_to_utf8_lossy(flags_j ? flags_j->units : NULL, flags_j ? flags_j->len : 0, &flags_len);
    sbuf shown = {0};
    sb_shown(&shown, pattern);
    bool is_error;
    char *message;
    engine_re *re = engine_compile(pattern->units, pattern->len, flags, &is_error, &message);
    free(message);
    const json *tests = json_get(r, "tests");
    if (!re) {
        c->refused++;
        sbuf b = {0};
        sb_printf(&b, "REFUSED\t%s\t/%s/%s", EXEC, shown.p ? shown.p : "", flags);
        lines_push(w, &b);
    } else {
        bool carries = strchr(flags, 'g') || strchr(flags, 'y');
        for (size_t t = 0; tests && tests->kind == JSON_ARRAY && t < tests->len; t++) {
            const json *test = &tests->items[t];
            if (json_is_true(json_get(test, "v8_deviation"))) continue;
            const json *text = json_string(json_get(test, "text"));
            const json *answer = json_get(test, "answer");
            if (!text || !answer) continue;
            size_t start = carries ? last_index_of(test, text->len) : 0;
            sbuf where = {0};
            sb_printf(&where, "/%s/%s\t", shown.p ? shown.p : "", flags);
            sb_escape(&where, text->units, text->len);
            sb_printf(&where, "\t%.17g", json_get(test, "lastIndex")->number);
            const char *why = NULL;
            if (o->utf16) {
                int rc = exec_answer_of(re, answer, text->units, text->len, NULL, 0, NULL, start, carries, &why);
                count_exec(c, w, where.p, "utf16", rc, why);
            }
#ifndef DATASET_QUICKJS
            if (o->utf8 && !has_lone_surrogate(text->units, text->len) && !inside_pair(text->units, text->len, start)) {
                size_t utf8_len;
                char *utf8 = utf16_to_utf8_lossy(text->units, text->len, &utf8_len);
                size_t *byte_at = byte_offsets(text->units, text->len);
                int rc = exec_answer_of(re, answer, text->units, text->len, utf8, utf8_len, byte_at, start, carries, &why);
                count_exec(c, w, where.p, "utf8", rc, why);
                free(byte_at);
                free(utf8);
            }
#endif
            free(where.p);
        }
        engine_free(re);
    }
    free(shown.p);
    free(flags);
}

/* Writes the engine's compliance as the benchmark page reads it: the counts, this runner and the calls it makes in
 * each encoding it asked in, and what it leaves out. */
static void write_compliance(const char *path, const options *o, const counts *t) {
    FILE *f = fopen(path, "wb");
    if (!f) {
        fprintf(stderr, "cannot write %s\n", path);
        exit(2);
    }
    fprintf(f, "{\"right\":%lld,\"wrong\":%lld,\"refused\":%lld,\"acceptedInvalid\":%lld,\"limited\":%lld,", t->right,
            t->wrong, t->refused, t->accepted, t->limited + t->errors);
    fprintf(f, "\"judge\":{\"runner\":\"https://github.com/corvus-dotnet/corvus-regex-ecma262-rs/blob/main/tools/c/dataset.c\",\"calls\":{");
#ifdef DATASET_QUICKJS
    (void)o;
    fprintf(f, "\"utf16\":[\"lre_compile\",\"lre_exec\"]}},");
    fprintf(f, "\"excludes\":[{\"class\":\"encoding\",\"leaves\":\"utf8\"},"
               "{\"class\":\"operations\",\"leaves\":[\"global\",\"replace\",\"split\"]}]}\n");
#else
    if (o->utf16)
        fprintf(f, "\"utf16\":[\"cre_regex_is_match_limit_utf16\",\"cre_regex_captures_limit_utf16\","
                   "\"cre_regex_find_iter_utf16\",\"cre_regex_replace_utf16\",\"cre_regex_split_utf16\"]%s",
                o->utf8 ? "," : "");
    if (o->utf8)
        fprintf(f, "\"utf8\":[\"cre_regex_is_match_limit\",\"cre_regex_captures_limit\",\"cre_regex_find_iter\","
                   "\"cre_regex_replace\"]");
    fprintf(f, "}},\"excludes\":[]}\n");
#endif
    fclose(f);
}

#ifndef DATASET_QUICKJS
/* Asks the grammar-only reader (cre_regex_is_valid_pattern) whether the pattern is valid in each grammar of the record,
 * as V8 answered. It reads no limit of size, so its answer is the grammar's for a pattern of any size. */
static void judge_reader(const char *name, const json *r, const json *pattern, counts *c, lines *w) {
    const json *flags_j = json_string(json_get(r, "flags"));
    size_t flags_len, pattern_len;
    char *flags = utf16_to_utf8_lossy(flags_j ? flags_j->units : NULL, flags_j ? flags_j->len : 0, &flags_len);
    char *p8 = utf16_to_utf8_lossy(pattern->units, pattern->len, &pattern_len);
    mode modes[3];
    size_t mode_count = modes_of(r, modes);
    for (size_t m = 0; m < mode_count; m++) {
        char mode_flags[32];
        snprintf(mode_flags, sizeof mode_flags, "%s%s", flags, modes[m].suffix);
        bool valid = false;
        if (cre_regex_is_valid_pattern(p8, pattern_len, mode_flags, strlen(mode_flags), &valid) != CRE_OK) valid = false;
        if (valid == modes[m].valid) {
            c->reader_right++;
            continue;
        }
        c->reader_wrong++;
        sbuf b = {0};
        sb_printf(&b, "READER\t%s\t/", name);
        sb_shown(&b, pattern);
        sb_printf(&b, "/%s\tV8 says %s", mode_flags, modes[m].valid ? "valid" : "not valid");
        lines_push(w, &b);
    }
    free(flags);
    free(p8);
}
#endif

static void judge_record(const char *name, const json *r, const options *o, counts *c, lines *w) {
    const json *pattern = json_string(json_get(r, "pattern"));
    if (!pattern) return;
#ifndef DATASET_QUICKJS
    if (strcmp(name, EXEC) != 0) judge_reader(name, r, pattern, c, w);
#endif
    if (strcmp(name, EXEC) == 0) {
        judge_exec(r, pattern, o, c, w);
        return;
    }
    if (strcmp(name, "captures.jsonl") == 0) {
        if (o->utf16) judge_captures(r, pattern, c, w);
        return;
    }
    if (strcmp(name, "ill-formed-utf8.jsonl") == 0) {
        judge_ill_formed(r, pattern, o, c, w);
        return;
    }
    judge_tests(name, r, pattern, o, c, w);
}

/* ------------------------------------------------------------------------------------------------------------------
 * The run */

typedef struct {
    const char *name;
    char **line_starts;
    size_t *line_lens;
    size_t line_count;
    size_t next;
    mutex_t lock;
    const options *o;
    counts total;
    lines wrong;
    long long records, skipped;
} file_work;

static void *worker(void *p) {
    file_work *f = p;
#ifdef DATASET_QUICKJS
    char base;
    stack_base = &base;
#endif
    counts c = {0};
    lines w = {0};
    long long records = 0, skipped = 0;
    for (;;) {
        mutex_lock(&f->lock);
        size_t k = f->next++;
        mutex_unlock(&f->lock);
        if (k >= f->line_count) break;
        json_arena *arena;
        const char *error;
        json *r = json_parse(f->line_starts[k], f->line_lens[k], &arena, &error);
        if (!r) {
            fprintf(stderr, "%s:%zu: %s\n", f->name, k + 1, error);
            exit(2);
        }
        if (json_is_true(json_get(r, "v8_deviation"))) {
            skipped++;
        } else {
            records++;
            judge_record(f->name, r, f->o, &c, &w);
        }
        json_arena_free(arena);
    }
    mutex_lock(&f->lock);
    counts_add(&f->total, &c);
    for (size_t k = 0; k < w.len; k++) {
        sbuf b = {w.items[k], strlen(w.items[k]), strlen(w.items[k]) + 1};
        lines_push(&f->wrong, &b);
    }
    f->records += records;
    f->skipped += skipped;
    mutex_unlock(&f->lock);
    free(w.items);
    return NULL;
}

/* Whether a file name matches a filter with * wildcards. */
static bool glob(const char *pattern, const char *name) {
    if (!strchr(pattern, '*')) return strcmp(pattern, name) == 0;
    const char *star = strchr(pattern, '*');
    size_t head = (size_t)(star - pattern);
    if (strncmp(pattern, name, head) != 0) return false;
    const char *rest = name + head;
    const char *p = star + 1;
    for (;;) {
        const char *next = strchr(p, '*');
        if (!next) {
            size_t tail = strlen(p), have = strlen(rest);
            return have >= tail && strcmp(rest + have - tail, p) == 0;
        }
        size_t part = (size_t)(next - p);
        char *piece = malloc(part + 1);
        memcpy(piece, p, part);
        piece[part] = 0;
        const char *at = strstr(rest, piece);
        free(piece);
        if (!at) return false;
        rest = at + part;
        p = next + 1;
    }
}

static int compare_strings(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

static char *read_file(const char *path, size_t *len) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *s = malloc((size_t)n + 1);
    if (fread(s, 1, (size_t)n, f) != (size_t)n) {
        fclose(f);
        free(s);
        return NULL;
    }
    fclose(f);
    s[n] = 0;
    *len = (size_t)n;
    return s;
}

int main(int argc, char **argv) {
    const char *compliance = NULL;
    for (int k = 1; k + 1 < argc; k++) {
        if (strcmp(argv[k], "--compliance") == 0) {
            compliance = argv[k + 1];
            for (int m = k; m + 2 <= argc; m++) argv[m] = argv[m + 2];
            argc -= 2;
            break;
        }
    }
    if (argc < 3) {
        fprintf(stderr, "usage: %s <data directory> <wrong-cases file> [file filter] [utf16|utf8|both] [--compliance <file>]\n",
                argv[0]);
        return 2;
    }
    const char *directory = argv[1], *wrong_path = argv[2];
    const char *filter = argc > 3 ? argv[3] : "*.jsonl";
    const char *encodings = argc > 4 ? argv[4] : "both";
    options o = {strcmp(encodings, "both") == 0 || strcmp(encodings, "utf16") == 0,
                 strcmp(encodings, "both") == 0 || strcmp(encodings, "utf8") == 0};
    size_t file_count;
    char **files = list_directory(directory, &file_count);
    size_t kept = 0;
    for (size_t k = 0; k < file_count; k++) {
        size_t n = strlen(files[k]);
        if (n > 6 && strcmp(files[k] + n - 6, ".jsonl") == 0 && glob(filter, files[k])) {
            files[kept++] = files[k];
        } else {
            free(files[k]);
        }
    }
    qsort(files, kept, sizeof *files, compare_strings);
    /* The exec calls of test262, in a directory of their own: with the default filter, or one that names them. */
    if (strcmp(filter, "*.jsonl") == 0 || strncmp(filter, "exec", 4) == 0) {
        char path[4096];
        snprintf(path, sizeof path, "%s/%s", directory, EXEC);
        FILE *probe = fopen(path, "rb");
        if (probe) {
            fclose(probe);
            files = realloc(files, (kept + 1) * sizeof *files);
            files[kept++] = strdup(EXEC);
        }
    }
    counts total = {0};
    lines wrong = {0};
    long long records = 0, skipped = 0;
    double start = now_seconds();
    for (size_t k = 0; k < kept; k++) {
        char path[4096];
        snprintf(path, sizeof path, "%s/%s", directory, files[k]);
        size_t len;
        char *text = read_file(path, &len);
        if (!text) {
            fprintf(stderr, "cannot read %s\n", path);
            return 2;
        }
        file_work f = {0};
        f.name = files[k];
        f.o = &o;
        mutex_init(&f.lock);
        size_t cap = 1024;
        f.line_starts = malloc(cap * sizeof *f.line_starts);
        f.line_lens = malloc(cap * sizeof *f.line_lens);
        for (char *p = text; p < text + len;) {
            char *end = memchr(p, '\n', (size_t)(text + len - p));
            if (!end) end = text + len;
            size_t n = (size_t)(end - p);
            if (n > 0 && p[n - 1] == '\r') n--;
            if (f.line_count == cap) {
                cap *= 2;
                f.line_starts = realloc(f.line_starts, cap * sizeof *f.line_starts);
                f.line_lens = realloc(f.line_lens, cap * sizeof *f.line_lens);
            }
            f.line_starts[f.line_count] = p;
            f.line_lens[f.line_count++] = n;
            p = end + 1;
        }
        thread_t threads[THREADS];
#ifdef DATASET_QUICKJS
        size_t stack = (size_t)256 << 20;
#else
        size_t stack = (size_t)16 << 20;
#endif
        for (int t = 0; t < THREADS; t++) thread_start(&threads[t], worker, &f, stack);
        for (int t = 0; t < THREADS; t++) thread_join(threads[t]);
        printf("%-24s right %10lld  wrong %6lld  refused-valid %4lld  accepted-invalid %4lld  limit %4lld  %6.1fs\n",
               files[k], f.total.right, f.total.wrong, f.total.refused, f.total.accepted, f.total.limited,
               now_seconds() - start);
        fflush(stdout);
        counts_add(&total, &f.total);
        for (size_t i = 0; i < f.wrong.len; i++) {
            sbuf b = {f.wrong.items[i], strlen(f.wrong.items[i]), strlen(f.wrong.items[i]) + 1};
            lines_push(&wrong, &b);
        }
        free(f.wrong.items);
        records += f.records;
        skipped += f.skipped;
        free(f.line_starts);
        free(f.line_lens);
        free(text);
    }
    qsort(wrong.items, wrong.len, sizeof *wrong.items, compare_strings);
    FILE *out = fopen(wrong_path, "wb");
    if (out) {
        for (size_t k = 0; k < wrong.len; k++) fprintf(out, "%s\n", wrong.items[k]);
        fclose(out);
    }
    if (compliance) write_compliance(compliance, &o, &total);
    printf("\nrecords %lld (skipped %lld deviations)  right %lld  wrong %lld  refused-valid %lld  accepted-invalid %lld  "
           "limit %lld  errors %lld  in %.1fs\n",
           records, skipped, total.right, total.wrong, total.refused, total.accepted, total.limited, total.errors,
           now_seconds() - start);
#ifndef DATASET_QUICKJS
    printf("grammar-only reader: right %lld  wrong %lld\n", total.reader_right, total.reader_wrong);
#endif
    return total.wrong + total.refused + total.accepted + total.errors + total.limited + total.reader_wrong == 0 ? 0 : 1;
}
