/* The Industry benchmark from C: each workload counted by each engine, in nanoseconds per count, under the protocol of
 * the Rust harness (tools/src/bin/industry.rs, docs/benchmarks/README.md), of which this is a port:
 *
 *   industry <workloads.json> [--engines corvus-utf8,corvus-utf16,quickjs,pcre2,v8] [--v8 <industry.mjs>]
 *            [--rounds 3] [--seconds 0.7] [--passes 7] [--spread 0.05] [--filter <substring of the id>]
 *            [--out <results.json>]
 *
 * The workloads are the outer loop, each workload's rounds next and the engines innermost, their order turned by one
 * each round, so that a workload's rounds are seconds apart, as its engines are. Each figure is a child process of its
 * own (`industry --one`, or `node industry.mjs --one` for V8), which warms for two fifths of its seconds, then times
 * `passes` passes of as many counts as fit the rest and reports their median. A workload's figure for an engine is the
 * median of its rounds, with the fastest and the slowest round as its spread; a figure whose spread, (slowest -
 * fastest) / median, is over --spread is listed in the run's facts (spread_over) and on stderr. A figure that does not
 * finish in 30 s is stopped and recorded as giving no result within 30 s, and is not run in the workload's later
 * rounds. A count is the global search of ECMA-262: after an empty match the search goes on one code point later (one
 * code unit without u).
 *
 * The engines: the C library over UTF-8 and over UTF-16 (corvus_regex_ecma262.h, linked as a consumer links it), with
 * the workload's flags and u, a global search made once per text and reset for each count; QuickJS's libregexp
 * (quickjs-ng), an ECMA-262 engine in C, with the workload's flags and u over the text as QuickJS holds a string (8-bit
 * when every character is Latin-1, UTF-16 otherwise); PCRE2 with its JIT, which does not claim ECMA-262 and is given
 * the pattern as it is in UTF mode with the flags i, m and s; and V8 (--v8, the harness run by node), with gu added. An
 * engine written `name@path` is run by the harness binary at `path` (this one, or the Rust harness, whose child
 * protocol is the same), so that two builds can be timed side by side in the same rounds.
 *
 * The texts are those of the reference run, in CORVUS_REGEX_TEXTS (or ~/.local/share/corvus-regex-benchmarks); the run
 * unzips each once into a directory of its own (CORVUS_REGEX_PLAIN), which the children read. POSIX only: the
 * benchmark runs on Linux, in the image. */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <math.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "corvus_regex_ecma262.h"
#include "json.h"
#include "libregexp.h"
#define PCRE2_CODE_UNIT_WIDTH 8
#include "pcre2.h"

#ifndef QUICKJS_VERSION
#define QUICKJS_VERSION ""
#endif

static const char *DEFAULT_ENGINES[] = {"corvus-utf8", "corvus-utf16", "quickjs", "pcre2"};

typedef struct {
    char *id, *pattern, *flags, *file;
    /* V8's count, which the central run writes into the workloads once, for a run without V8 (V8=0) to check against. */
    bool has_v8_count;
    double v8_count;
} workload;

static char *dup_utf8(const json *s) {
    size_t len;
    return utf16_to_utf8_lossy(s ? s->units : NULL, s ? s->len : 0, &len);
}

static char *read_all(const char *path, size_t *len) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    size_t cap = 1 << 20, n = 0;
    char *s = malloc(cap);
    size_t got;
    while ((got = fread(s + n, 1, cap - n, f)) > 0) {
        n += got;
        if (n == cap) s = realloc(s, cap *= 2);
    }
    fclose(f);
    s = realloc(s, n + 1);
    s[n] = 0;
    *len = n;
    return s;
}

static workload *load_workloads(const char *path, size_t *count) {
    size_t len;
    char *text = read_all(path, &len);
    if (!text) {
        fprintf(stderr, "%s: %s\n", path, strerror(errno));
        exit(2);
    }
    json_arena *arena;
    const char *error;
    json *all = json_parse(text, len, &arena, &error);
    if (!all || all->kind != JSON_ARRAY) {
        fprintf(stderr, "%s: %s\n", path, all ? "not an array" : error);
        exit(2);
    }
    workload *w = calloc(all->len, sizeof *w);
    for (size_t k = 0; k < all->len; k++) {
        const json *o = &all->items[k];
        w[k].id = dup_utf8(json_get(o, "id"));
        w[k].pattern = dup_utf8(json_get(o, "pattern"));
        w[k].flags = dup_utf8(json_get(o, "flags"));
        w[k].file = dup_utf8(json_get(o, "file"));
        const json *v8_count = json_get(o, "v8Count");
        if (v8_count && v8_count->kind == JSON_NUMBER) {
            w[k].has_v8_count = true;
            w[k].v8_count = v8_count->number;
        }
    }
    *count = all->len;
    json_arena_free(arena);
    free(text);
    return w;
}

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + (double)t.tv_nsec * 1e-9;
}

/* A file name without its .gz. */
static char *plain_name(const char *file) {
    char *s = strdup(file);
    size_t n = strlen(s);
    if (n > 3 && strcmp(s + n - 3, ".gz") == 0) s[n - 3] = 0;
    return s;
}

static char *text_dir(void) {
    const char *d = getenv("CORVUS_REGEX_TEXTS");
    if (d) return strdup(d);
    char buf[4096];
    snprintf(buf, sizeof buf, "%s/.local/share/corvus-regex-benchmarks", getenv("HOME") ? getenv("HOME") : ".");
    return strdup(buf);
}

/* The text of a file: gunzipped, without a byte order mark (read as it is from CORVUS_REGEX_PLAIN when the run has
 * unzipped it there). */
static char *load_text(const char *file, size_t *len) {
    char path[8192];
    char *plain = plain_name(file);
    const char *dir = getenv("CORVUS_REGEX_PLAIN");
    char *s = NULL;
    if (dir) {
        snprintf(path, sizeof path, "%s/%s", dir, plain);
        s = read_all(path, len);
    }
    if (!s) {
        char *texts = text_dir();
        snprintf(path, sizeof path, "%s/%s", texts, file);
        struct stat st;
        if (stat(path, &st) != 0) {
            char cmd[16384];
            snprintf(cmd, sizeof cmd,
                     "mkdir -p '%s' && curl -sSfL -o '%s' "
                     "'https://raw.githubusercontent.com/dotnet/performance/main/src/benchmarks/micro/libraries/"
                     "System.Text.RegularExpressions/TestData/%s'",
                     texts, path, file);
            if (system(cmd) != 0) {
                fprintf(stderr, "could not fetch %s\n", file);
                exit(2);
            }
        }
        char cmd[16384];
        snprintf(cmd, sizeof cmd, "gzip -dc '%s'", path);
        FILE *p = popen(cmd, "r");
        size_t cap = 1 << 20, n = 0, got;
        s = malloc(cap);
        while ((got = fread(s + n, 1, cap - n, p)) > 0) {
            n += got;
            if (n == cap) s = realloc(s, cap *= 2);
        }
        pclose(p);
        s = realloc(s, n + 1);
        s[n] = 0;
        *len = n;
        free(texts);
    }
    free(plain);
    if (*len >= 3 && (unsigned char)s[0] == 0xEF && (unsigned char)s[1] == 0xBB && (unsigned char)s[2] == 0xBF) {
        memmove(s, s + 3, *len - 3 + 1);
        *len -= 3;
    }
    return s;
}

/* The workload's flags with u added, each once. */
static void flags_u(const char *flags, char *out, size_t size) {
    snprintf(out, size, "%s%s", flags, (strchr(flags, 'u') || strchr(flags, 'v')) ? "" : "u");
}

/* ------------------------------------------------------------------------------------------------------------------
 * The engines' counts */

typedef size_t (*count_fn)(void *state);

/* Times a count: warm, then passes. The median, the fastest and the slowest pass in nanoseconds per count, the count,
 * and the counts timed. */
static void measure(double seconds, int passes, count_fn count, void *state, double *median, double *min, double *max,
                    size_t *expected, size_t *reps_timed) {
    *expected = count(state);
    double warm_end = now() + seconds * 0.4;
    size_t reps = 0;
    while (now() < warm_end) {
        if (count(state) != *expected) {
            printf("{\"error\":\"the count changed\"}\n");
            exit(0);
        }
        reps++;
    }
    size_t per_pass = (size_t)llround((double)reps * 0.6 / passes);
    if (per_pass < 1) per_pass = 1;
    double *times = malloc((size_t)passes * sizeof *times);
    volatile size_t sink = 0;
    for (int p = 0; p < passes; p++) {
        double t0 = now();
        size_t s = 0;
        for (size_t k = 0; k < per_pass; k++) s += count(state);
        sink += s;
        times[p] = (now() - t0) * 1e9 / (double)per_pass;
    }
    (void)sink;
    for (int i = 1; i < passes; i++) {
        for (int j = i; j > 0 && times[j] < times[j - 1]; j--) {
            double t = times[j];
            times[j] = times[j - 1];
            times[j - 1] = t;
        }
    }
    *median = times[passes / 2];
    *min = times[0];
    *max = times[passes - 1];
    *reps_timed = per_pass * (size_t)passes;
    free(times);
}

/* The C library: a global search made once, reset for each count, its matches taken a batch at a time as a C
 * program that wants many of them takes them. */
static size_t count_corvus(void *state) {
    cre_matches *it = state;
    cre_matches_reset(it);
    size_t n = 0, got;
    cre_match batch[256];
    /* Fewer than asked for: the search has found every match. */
    do {
        if (cre_matches_next_batch(it, batch, 256, &got) != CRE_OK) break;
        n += got;
    } while (got == 256);
    return n;
}

/* QuickJS's libregexp, called as QuickJS's RegExp.prototype[Symbol.matchAll] calls it. */
static char *stack_base;

bool lre_check_stack_overflow(void *opaque, size_t alloca_size) {
    (void)opaque;
    char here;
    size_t used = (size_t)(stack_base > &here ? stack_base - &here : &here - stack_base);
    return used + alloca_size > ((size_t)6 << 20);
}

int lre_check_timeout(void *opaque) {
    (void)opaque;
    return 0;
}

void *lre_realloc(void *opaque, void *ptr, size_t size) {
    (void)opaque;
    if (size == 0) {
        free(ptr);
        return NULL;
    }
    return realloc(ptr, size);
}

typedef struct {
    uint8_t *bytecode;
    uint8_t **capture;
    const uint8_t *buf;
    int len, type; /* type: 0 for 8-bit text, 1 for 16-bit */
    const uint16_t *units;
    bool unicode;
} quickjs_state;

static size_t count_quickjs(void *state) {
    quickjs_state *q = state;
    size_t n = 0;
    int last = 0;
    for (;;) {
        if (last > q->len) return n;
        int rc = lre_exec(q->capture, q->bytecode, q->buf, last, q->len, q->type, NULL);
        if (rc != 1) return n;
        n++;
        int start = (int)((q->capture[0] - q->buf) >> q->type), end = (int)((q->capture[1] - q->buf) >> q->type);
        (void)start;
        if (end == last) {
            if (last >= q->len) return n;
            /* An empty match: on by one code point with u (a pair is two units of a 16-bit text), one unit without. */
            bool pair = q->type == 1 && q->unicode && last + 1 < q->len && q->units[last] >= 0xD800 &&
                        q->units[last] <= 0xDBFF && q->units[last + 1] >= 0xDC00 && q->units[last + 1] <= 0xDFFF;
            last += pair ? 2 : 1;
        } else {
            last = end;
        }
    }
}

typedef struct {
    pcre2_code *code;
    pcre2_match_data *data;
    const char *text;
    size_t len;
} pcre2_state;

static size_t count_pcre2(void *state) {
    pcre2_state *p = state;
    size_t n = 0, at = 0;
    for (;;) {
        if (at > p->len) return n;
        int rc = pcre2_jit_match(p->code, (PCRE2_SPTR)p->text, p->len, at, 0, p->data, NULL);
        if (rc < 0) return n;
        n++;
        PCRE2_SIZE *o = pcre2_get_ovector_pointer(p->data);
        if (o[1] == o[0]) {
            if (o[1] >= p->len) return n;
            /* An empty match: on by one character of UTF-8. */
            at = o[1] + 1;
            while (at < p->len && ((unsigned char)p->text[at] & 0xC0) == 0x80) at++;
        } else {
            at = o[1];
        }
    }
}

static void quote(FILE *out, const char *s);

/* Runs one workload on one engine in this process and prints its figures as one line of JSON. */
static int one(const char *path, size_t index, const char *engine, double seconds, int passes) {
    char base;
    stack_base = &base;
    size_t count;
    workload *all = load_workloads(path, &count);
    if (index >= count) {
        printf("{\"error\":\"no workload %zu\"}\n", index);
        return 0;
    }
    workload *w = &all[index];
    size_t len;
    char *s = load_text(w->file, &len);
    char flags[32];
    flags_u(w->flags, flags, sizeof flags);
    double median = 0, min = 0, max = 0;
    size_t expected = 0, reps = 0;
    char error[512] = "";
    if (strcmp(engine, "corvus-utf8") == 0 || strcmp(engine, "corvus-utf16") == 0) {
        cre_regex *re = NULL;
        if (cre_regex_new(w->pattern, strlen(w->pattern), flags, strlen(flags), &re) != CRE_OK) {
            cre_str m = cre_last_error_message();
            snprintf(error, sizeof error, "%.*s", (int)m.len, m.ptr);
        } else {
            cre_matches *it = NULL;
            size_t len16 = 0;
            uint16_t *s16 = NULL;
            cre_status status;
            if (strcmp(engine, "corvus-utf8") == 0) {
                status = cre_regex_find_iter(re, s, len, &it);
            } else {
                s16 = utf8_to_utf16(s, len, &len16);
                status = cre_regex_find_iter_utf16(re, s16, len16, &it);
            }
            if (status != CRE_OK) {
                cre_str m = cre_last_error_message();
                snprintf(error, sizeof error, "%.*s", (int)m.len, m.ptr);
            } else {
                measure(seconds, passes, count_corvus, it, &median, &min, &max, &expected, &reps);
            }
            cre_matches_free(it);
            free(s16);
            cre_regex_free(re);
        }
    } else if (strcmp(engine, "quickjs") == 0) {
        int re_flags = LRE_FLAG_GLOBAL | LRE_FLAG_UNICODE;
        for (const char *f = w->flags; *f; f++) {
            if (*f == 'i') re_flags |= LRE_FLAG_IGNORECASE;
            if (*f == 'm') re_flags |= LRE_FLAG_MULTILINE;
            if (*f == 's') re_flags |= LRE_FLAG_DOTALL;
            if (*f == 'y') re_flags |= LRE_FLAG_STICKY;
        }
        char message[128];
        int bytecode_len;
        uint8_t *bc = lre_compile(&bytecode_len, message, sizeof message, w->pattern, strlen(w->pattern), re_flags, NULL);
        if (!bc) {
            snprintf(error, sizeof error, "%s", message);
        } else {
            quickjs_state q = {0};
            q.bytecode = bc;
            q.unicode = true;
            int alloc = lre_get_alloc_count(bc);
            q.capture = malloc((size_t)(alloc > 0 ? alloc : 1) * sizeof *q.capture);
            size_t len16;
            uint16_t *s16 = utf8_to_utf16(s, len, &len16);
            bool latin1 = true;
            for (size_t k = 0; k < len16 && latin1; k++) latin1 = s16[k] < 0x100;
            uint8_t *s8 = NULL;
            if (latin1) {
                s8 = malloc(len16 + 1);
                for (size_t k = 0; k < len16; k++) s8[k] = (uint8_t)s16[k];
                q.buf = s8;
                q.type = 0;
            } else {
                q.buf = (const uint8_t *)s16;
                q.type = 1;
            }
            q.units = s16;
            q.len = (int)len16;
            measure(seconds, passes, count_quickjs, &q, &median, &min, &max, &expected, &reps);
            free(s8);
            free(s16);
            free(q.capture);
            free(bc);
        }
    } else if (strcmp(engine, "pcre2") == 0) {
        uint32_t options = PCRE2_UTF;
        if (strchr(w->flags, 'i')) options |= PCRE2_CASELESS;
        if (strchr(w->flags, 'm')) options |= PCRE2_MULTILINE;
        if (strchr(w->flags, 's')) options |= PCRE2_DOTALL;
        int code;
        PCRE2_SIZE offset;
        pcre2_code *re = pcre2_compile((PCRE2_SPTR)w->pattern, PCRE2_ZERO_TERMINATED, options, &code, &offset, NULL);
        if (!re) {
            PCRE2_UCHAR message[256];
            pcre2_get_error_message(code, message, sizeof message);
            snprintf(error, sizeof error, "%s", (char *)message);
        } else if ((code = pcre2_jit_compile(re, PCRE2_JIT_COMPLETE)) != 0) {
            PCRE2_UCHAR message[256];
            pcre2_get_error_message(code, message, sizeof message);
            snprintf(error, sizeof error, "JIT: %s", (char *)message);
            pcre2_code_free(re);
        } else {
            pcre2_state p = {re, pcre2_match_data_create_from_pattern(re, NULL), s, len};
            measure(seconds, passes, count_pcre2, &p, &median, &min, &max, &expected, &reps);
            pcre2_match_data_free(p.data);
            pcre2_code_free(re);
        }
    } else {
        snprintf(error, sizeof error, "unknown engine %s", engine);
    }
    if (error[0]) {
        printf("{\"error\":");
        quote(stdout, error);
        printf("}\n");
    } else {
        printf("{\"median\":%.1f,\"min\":%.1f,\"max\":%.1f,\"count\":%zu,\"reps\":%zu}\n", median, min, max, expected,
               reps);
    }
    free(s);
    return 0;
}

/* ------------------------------------------------------------------------------------------------------------------
 * The run */

/* A JSON string, everything outside printable ASCII escaped. */
static void quote(FILE *out, const char *s) {
    size_t len16;
    uint16_t *u = utf8_to_utf16(s, strlen(s), &len16);
    fputc('"', out);
    for (size_t k = 0; k < len16; k++) {
        if (u[k] == '"') {
            fputs("\\\"", out);
        } else if (u[k] == '\\') {
            fputs("\\\\", out);
        } else if (u[k] < 0x20 || u[k] > 0x7e) {
            fprintf(out, "\\u%04x", u[k]);
        } else {
            fputc((char)u[k], out);
        }
    }
    fputc('"', out);
    free(u);
}

static char *first_line(const char *cmd) {
    FILE *p = popen(cmd, "r");
    char buf[4096] = "";
    if (p) {
        if (!fgets(buf, sizeof buf, p)) buf[0] = 0;
        pclose(p);
    }
    buf[strcspn(buf, "\r\n")] = 0;
    return strdup(buf);
}

static char *file_field(const char *path, const char *prefix, char separator) {
    FILE *f = fopen(path, "r");
    char line[4096];
    char *out = strdup("");
    if (!f) return out;
    while (fgets(line, sizeof line, f)) {
        if (strncmp(line, prefix, strlen(prefix)) == 0) {
            char *v = strchr(line, separator);
            if (v) {
                v++;
                while (*v == ' ' || *v == '\t') v++;
                v[strcspn(v, "\r\n")] = 0;
                free(out);
                out = strdup(v);
            }
            break;
        }
    }
    fclose(f);
    return out;
}

typedef struct {
    double *each;
    size_t n;
    char *line;
} figures;

/* One figure: engine over workload index in a child process: the child's line of JSON, or one with the error. */
/* The line of a figure stopped at the limit, which the page shows as giving no result within 30 s. */
static const char STOPPED[] = "{\"error\":\"did not produce a result within 30 s\",\"limit\":30}";

static char *figure(const char *exe, const char *v8, const char *path, size_t index, const char *engine_spec,
                    const char *seconds, const char *passes) {
    char *spec = strdup(engine_spec);
    char *at = strchr(spec, '@');
    const char *engine = spec, *program = exe;
    if (at) {
        *at = 0;
        program = at + 1;
    }
    char index_s[32];
    snprintf(index_s, sizeof index_s, "%zu", index);
    int fds[2];
    if (pipe(fds) != 0) return strdup("{\"error\":\"pipe\"}");
    pid_t pid = fork();
    if (pid == 0) {
        dup2(fds[1], 1);
        close(fds[0]);
        close(fds[1]);
        if (strcmp(engine, "v8") == 0) {
            execlp("node", "node", v8, "--one", path, index_s, seconds, passes, (char *)NULL);
        } else {
            execl(program, program, "--one", path, index_s, engine, seconds, passes, (char *)NULL);
        }
        _exit(127);
    }
    close(fds[1]);
    char buf[65536];
    size_t n = 0;
    double deadline = now() + 30;
    bool done = false;
    while (!done && n < sizeof buf - 1) {
        int left = (int)((deadline - now()) * 1000);
        if (left <= 0) break;
        struct pollfd p = {fds[0], POLLIN, 0};
        if (poll(&p, 1, left) <= 0) break;
        ssize_t got = read(fds[0], buf + n, sizeof buf - 1 - n);
        if (got <= 0) {
            done = true;
            break;
        }
        n += (size_t)got;
        if (memchr(buf, '\n', n)) done = true;
    }
    buf[n] = 0;
    bool timed_out = !done && now() >= deadline;
    kill(pid, SIGKILL);
    waitpid(pid, NULL, 0);
    close(fds[0]);
    free(spec);
    char *nl = memchr(buf, '\n', n);
    if (nl) {
        *nl = 0;
        return strdup(buf);
    }
    if (timed_out) return strdup(STOPPED);
    return strdup("{\"error\":\"the process ended without figures\"}");
}

static int compare_doubles(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return x < y ? -1 : x > y;
}

static double median_of(const double *sorted, size_t n) {
    return n % 2 ? sorted[n / 2] : (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0;
}

static const char *opt(int argc, char **argv, const char *name) {
    for (int k = 1; k + 1 < argc; k++) {
        if (strcmp(argv[k], name) == 0) return argv[k + 1];
    }
    return NULL;
}

static double number_of(const char *line, const char *name, bool *ok) {
    json_arena *arena;
    const char *error;
    json *j = json_parse(line, strlen(line), &arena, &error);
    const json *v = j ? json_get(j, name) : NULL;
    double r = v && v->kind == JSON_NUMBER ? v->number : 0;
    *ok = v && v->kind == JSON_NUMBER;
    json_arena_free(arena);
    return r;
}

static char *string_of(const char *line, const char *name) {
    json_arena *arena;
    const char *error;
    json *j = json_parse(line, strlen(line), &arena, &error);
    const json *v = j ? json_string(json_get(j, name)) : NULL;
    char *r = v ? dup_utf8(v) : NULL;
    json_arena_free(arena);
    return r;
}

typedef struct {
    char *p;
    size_t len, cap;
} sbuf;

static void sb_printf(sbuf *b, const char *f, ...) {
    va_list a;
    va_start(a, f);
    int n = vsnprintf(NULL, 0, f, a);
    va_end(a);
    if (b->len + (size_t)n + 1 > b->cap) {
        b->cap = (b->len + (size_t)n + 1) * 2;
        b->p = realloc(b->p, b->cap);
    }
    va_start(a, f);
    vsnprintf(b->p + b->len, (size_t)n + 1, f, a);
    va_end(a);
    b->len += (size_t)n;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "--one") == 0) {
        if (argc < 7) return 2;
        return one(argv[2], (size_t)strtoull(argv[3], NULL, 10), argv[4], atof(argv[5]), atoi(argv[6]));
    }
    if (argc < 2) {
        fprintf(stderr, "usage: %s <workloads.json> [--engines ...] [--v8 industry.mjs] [--rounds R] [--seconds S] "
                        "[--passes P] [--spread L] [--filter F] [--out file]\n",
                argv[0]);
        return 2;
    }
    const char *path = argv[1];
    const char *v8 = opt(argc, argv, "--v8");
    char *engines[32];
    size_t engine_count = 0;
    const char *engines_opt = opt(argc, argv, "--engines");
    if (engines_opt) {
        char *list = strdup(engines_opt);
        for (char *e = strtok(list, ","); e && engine_count < 32; e = strtok(NULL, ",")) engines[engine_count++] = e;
    } else {
        for (size_t k = 0; k < sizeof DEFAULT_ENGINES / sizeof *DEFAULT_ENGINES; k++) {
            engines[engine_count++] = (char *)DEFAULT_ENGINES[k];
        }
        if (v8) engines[engine_count++] = "v8";
    }
    int rounds = opt(argc, argv, "--rounds") ? atoi(opt(argc, argv, "--rounds")) : 3;
    const char *seconds = opt(argc, argv, "--seconds") ? opt(argc, argv, "--seconds") : "0.7";
    const char *passes = opt(argc, argv, "--passes") ? opt(argc, argv, "--passes") : "7";
    double limit = opt(argc, argv, "--spread") ? atof(opt(argc, argv, "--spread")) : 0.05;
    const char *filter = opt(argc, argv, "--filter");
    size_t count;
    workload *all = load_workloads(path, &count);
    size_t *chosen = malloc(count * sizeof *chosen);
    size_t chosen_n = 0;
    for (size_t k = 0; k < count; k++) {
        if (!filter || strstr(all[k].id, filter)) chosen[chosen_n++] = k;
    }

    /* The texts unzipped once, for every child. */
    char plain[256];
    snprintf(plain, sizeof plain, "/tmp/corvus-regex-texts-c-%d", (int)getpid());
    mkdir(plain, 0700);
    for (size_t k = 0; k < count; k++) {
        char *name = plain_name(all[k].file);
        char file[8192];
        snprintf(file, sizeof file, "%s/%s", plain, name);
        struct stat st;
        if (stat(file, &st) != 0) {
            size_t len;
            char *s = load_text(all[k].file, &len);
            FILE *f = fopen(file, "wb");
            fwrite(s, 1, len, f);
            fclose(f);
            free(s);
        }
        free(name);
    }
    setenv("CORVUS_REGEX_PLAIN", plain, 1);
    char exe[4096];
    ssize_t exe_len = readlink("/proc/self/exe", exe, sizeof exe - 1);
    if (exe_len <= 0) {
        snprintf(exe, sizeof exe, "%s", argv[0]);
    } else {
        exe[exe_len] = 0;
    }

    figures *f = calloc(engine_count * count, sizeof *f);
    char *v8_engine = strdup("");
    for (size_t c = 0; c < chosen_n; c++) {
        size_t k = chosen[c];
        for (int round = 0; round < rounds; round++) {
            for (size_t turn = 0; turn < engine_count; turn++) {
                size_t e = (turn + (size_t)round) % engine_count;
                figures *fig = &f[e * count + k];
                /* A figure stopped at the limit in an earlier round is not run again. */
                if (fig->line && strcmp(fig->line, STOPPED) == 0) continue;
                char *line = figure(exe, v8, path, k, engines[e], seconds, passes);
                bool ok;
                double m = number_of(line, "median", &ok);
                if (ok) {
                    fig->each = realloc(fig->each, (fig->n + 1) * sizeof *fig->each);
                    fig->each[fig->n++] = m;
                } else {
                    fprintf(stderr, "%-13s %-62s %s\n", engines[e], all[k].id, line);
                }
                if (strcmp(engines[e], "v8") == 0) {
                    char *name = string_of(line, "engine");
                    if (name) {
                        free(v8_engine);
                        v8_engine = name;
                    }
                }
                free(fig->line);
                fig->line = line;
            }
        }
        fprintf(stderr, "workload %zu of %zu done\n", c + 1, chosen_n);
    }
    char rm[512];
    snprintf(rm, sizeof rm, "rm -rf '%s'", plain);
    if (system(rm) != 0) fprintf(stderr, "could not remove %s\n", plain);

    sbuf over = {0}, differ = {0}, results = {0};
    long v8_index = -1;
    for (size_t e = 0; e < engine_count; e++) {
        if (strcmp(engines[e], "v8") == 0) v8_index = (long)e;
    }
    for (size_t e = 0; e < engine_count; e++) {
        sb_printf(&results, "%s", e ? "," : "");
        FILE *mem;
        char *name_json;
        size_t name_len;
        mem = open_memstream(&name_json, &name_len);
        quote(mem, engines[e]);
        fclose(mem);
        sb_printf(&results, "%s:{", name_json);
        free(name_json);
        bool first = true;
        for (size_t c = 0; c < chosen_n; c++) {
            size_t k = chosen[c];
            figures *fig = &f[e * count + k];
            const char *line = fig->line ? fig->line : "";
            char *id_json;
            size_t id_len;
            mem = open_memstream(&id_json, &id_len);
            quote(mem, all[k].id);
            fclose(mem);
            sbuf entry = {0};
            if (fig->n < (size_t)rounds) {
                /* A round with no figure (an error, a limit) makes the figure an error. */
                if (strstr(line, "\"error\"")) {
                    sb_printf(&entry, "%s", line);
                } else {
                    sb_printf(&entry, "{\"error\":\"a round gave no figure\"}");
                }
            } else {
                sbuf in_order = {0};
                for (size_t r = 0; r < fig->n; r++) sb_printf(&in_order, "%s%.1f", r ? "," : "", fig->each[r]);
                double *sorted = malloc(fig->n * sizeof *sorted);
                memcpy(sorted, fig->each, fig->n * sizeof *sorted);
                qsort(sorted, fig->n, sizeof *sorted, compare_doubles);
                double m = median_of(sorted, fig->n), lo = sorted[0], hi = sorted[fig->n - 1];
                double spread = m > 0 ? (hi - lo) / m : 0;
                if (spread > limit) {
                    sb_printf(&over, "%s%s %s %.1f%%", over.len ? "; " : "", engines[e], all[k].id, spread * 100);
                    fprintf(stderr, "SPREAD %-13s %-62s %.1f%% over %.1f%%\n", engines[e], all[k].id, spread * 100,
                            limit * 100);
                }
                bool ok;
                double n = number_of(line, "count", &ok);
                sbuf fig_s = {0};
                sb_printf(&fig_s,
                          "\"median\":%.1f,\"min\":%.1f,\"max\":%.1f,\"count\":%.0f,\"rounds\":%zu,\"spread\":%.4f,"
                          "\"each\":[%s]",
                          m, lo, hi, n, fig->n, spread, in_order.p ? in_order.p : "");
                bool v8_ok = false;
                double oracle = 0;
                if (v8_index >= 0) {
                    figures *vf = &f[(size_t)v8_index * count + k];
                    if (vf->line) oracle = number_of(vf->line, "count", &v8_ok);
                } else if (all[k].has_v8_count) {
                    /* Without V8 in the run, its count as the workloads carry it. */
                    oracle = all[k].v8_count;
                    v8_ok = true;
                }
                if (v8_ok && oracle != n && strcmp(engines[e], "v8") != 0) {
                    sb_printf(&differ, "%s%s %s %.0f vs V8 %.0f", differ.len ? "; " : "", engines[e], all[k].id, n,
                              oracle);
                    fprintf(stderr, "COUNT  %-13s %-62s %.0f against V8's %.0f\n", engines[e], all[k].id, n, oracle);
                    if (strcmp(engines[e], "pcre2") == 0) {
                        sb_printf(&entry, "{%s,\"differs\":\"PCRE2 %.0f, V8 %.0f\"}", fig_s.p, n, oracle);
                    } else {
                        sb_printf(&entry, "{\"error\":\"counted %.0f where V8 counts %.0f\"}", n, oracle);
                    }
                } else {
                    sb_printf(&entry, "{%s}", fig_s.p);
                }
                free(fig_s.p);
                free(sorted);
                free(in_order.p);
            }
            sb_printf(&results, "%s%s:%s", first ? "" : ",", id_json, entry.p);
            first = false;
            free(entry.p);
            free(id_json);
        }
        sb_printf(&results, "}");
    }

    /* What the run is: the host, the compiler, the affinity, the load and the commit, read from the run itself. */
    struct utsname u;
    uname(&u);
    char os[512];
    snprintf(os, sizeof os, "%s %s", u.sysname, u.release);
    char *cpu = file_field("/proc/cpuinfo", "model name", ':');
    char *affinity = file_field("/proc/self/status", "Cpus_allowed_list", ':');
    char *load = first_line("cut -d' ' -f1 /proc/loadavg");
    char cores[32];
    snprintf(cores, sizeof cores, "%ld", sysconf(_SC_NPROCESSORS_ONLN));
    const char *commit_env = getenv("CORVUS_COMMIT");
    char *commit = commit_env && *commit_env ? strdup(commit_env) : first_line("git rev-parse HEAD 2>/dev/null");
    char *date = first_line("date -u +%Y-%m-%dT%H:%M:%SZ");
    cre_str version = cre_version();
    char corvus_version[64], pcre2_version[64], rounds_s[32], limit_s[32], image[256];
    snprintf(corvus_version, sizeof corvus_version, "%.*s", (int)version.len, version.ptr);
    snprintf(pcre2_version, sizeof pcre2_version, "%d.%d", PCRE2_MAJOR, PCRE2_MINOR);
    snprintf(rounds_s, sizeof rounds_s, "%d", rounds);
    snprintf(limit_s, sizeof limit_s, "%g", limit);
    snprintf(image, sizeof image, "%s %s", getenv("ImageOS") ? getenv("ImageOS") : "",
             getenv("ImageVersion") ? getenv("ImageVersion") : "");
    char *trimmed = image;
    while (*trimmed == ' ') trimmed++;
    for (size_t n = strlen(trimmed); n > 0 && trimmed[n - 1] == ' '; n--) trimmed[n - 1] = 0;
    const char *fields[][2] = {
        {"cpu", cpu},
        {"cores", cores},
        {"affinity", affinity},
        {"os", os},
        {"load", load},
        {"cc", __VERSION__},
        {"commit", commit},
        {"date", date},
        {"seconds", seconds},
        {"passes", passes},
        {"runner", getenv("RUNNER_NAME") ? getenv("RUNNER_NAME") : ""},
        {"runner_os", getenv("RUNNER_OS") ? getenv("RUNNER_OS") : ""},
        {"image", trimmed},
        {"corvus-regex-ecma262", corvus_version},
        {"quickjs", QUICKJS_VERSION},
        {"pcre2", pcre2_version},
        {"rounds", rounds_s},
        {"protocol", "interleaved"},
        {"spread_limit", limit_s},
        {"spread_over", over.p ? over.p : ""},
        {"counts_differ", differ.p ? differ.p : ""},
        {"v8", v8_engine},
    };
    const char *out_path = opt(argc, argv, "--out");
    FILE *out = out_path ? fopen(out_path, "w") : stdout;
    fputs("{\"facts\":{", out);
    for (size_t k = 0; k < sizeof fields / sizeof *fields; k++) {
        if (k) fputc(',', out);
        quote(out, fields[k][0]);
        fputc(':', out);
        quote(out, fields[k][1]);
    }
    char *end = first_line("date -u +%Y-%m-%dT%H:%M:%SZ");
    fputs("},\"end\":", out);
    quote(out, end);
    fprintf(out, ",\"results\":{%s}}\n", results.p ? results.p : "");
    if (out != stdout) fclose(out);
    return 0;
}
