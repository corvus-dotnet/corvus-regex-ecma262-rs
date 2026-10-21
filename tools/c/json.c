/* See json.h. */
#include "json.h"

#include <stdlib.h>
#include <string.h>

/* A block of the arena: the header, then its data (the header is a multiple of 16 bytes, so the data is aligned as
 * malloc aligns). */
typedef struct chunk {
    struct chunk *next;
    size_t used, size;
    char *data;
} chunk;

struct json_arena {
    chunk *head;
};

static void *arena_alloc(json_arena *a, size_t n) {
    n = (n + 15) & ~(size_t)15;
    if (!a->head || a->head->size - a->head->used < n) {
        size_t size = n > 65536 ? n : 65536;
        chunk *c = malloc(sizeof(chunk) + size);
        if (!c) abort();
        c->data = (char *)(c + 1);
        c->next = a->head;
        c->used = 0;
        c->size = size;
        a->head = c;
    }
    void *p = a->head->data + a->head->used;
    a->head->used += n;
    return p;
}

void json_arena_free(json_arena *a) {
    if (!a) return;
    for (chunk *c = a->head; c;) {
        chunk *next = c->next;
        free(c);
        c = next;
    }
    free(a);
}

typedef struct {
    const unsigned char *s;
    size_t i, n;
    json_arena *arena;
    const char *error;
} reader;

static void space(reader *r) {
    while (r->i < r->n && (r->s[r->i] == ' ' || r->s[r->i] == '\t' || r->s[r->i] == '\n' || r->s[r->i] == '\r')) r->i++;
}

static bool value(reader *r, json *out, int depth);

static int hex(int c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

/* A string at r->i (after its opening quote): its code units into the arena. */
static bool string(reader *r, uint16_t **units, size_t *len) {
    size_t cap = 64, n = 0;
    uint16_t *buf = malloc(cap * sizeof *buf);
    if (!buf) abort();
    for (;;) {
        if (r->i >= r->n) {
            free(buf);
            r->error = "unterminated string";
            return false;
        }
        if (n + 2 > cap) {
            cap *= 2;
            buf = realloc(buf, cap * sizeof *buf);
            if (!buf) abort();
        }
        unsigned c = r->s[r->i++];
        if (c == '"') break;
        if (c == '\\') {
            if (r->i >= r->n) break;
            unsigned e = r->s[r->i++];
            switch (e) {
            case '"': buf[n++] = '"'; break;
            case '\\': buf[n++] = '\\'; break;
            case '/': buf[n++] = '/'; break;
            case 'b': buf[n++] = 8; break;
            case 'f': buf[n++] = 12; break;
            case 'n': buf[n++] = 10; break;
            case 'r': buf[n++] = 13; break;
            case 't': buf[n++] = 9; break;
            case 'u': {
                if (r->i + 4 > r->n) {
                    free(buf);
                    r->error = "short \\u escape";
                    return false;
                }
                int v = 0;
                for (int k = 0; k < 4; k++) {
                    int h = hex(r->s[r->i + k]);
                    if (h < 0) {
                        free(buf);
                        r->error = "bad \\u escape";
                        return false;
                    }
                    v = v * 16 + h;
                }
                r->i += 4;
                buf[n++] = (uint16_t)v;
                break;
            }
            default:
                free(buf);
                r->error = "bad escape";
                return false;
            }
            continue;
        }
        /* A UTF-8 sequence: its code point as one or two units. */
        uint32_t cp = c;
        int more = 0;
        if (c >= 0xF0) {
            cp = c & 7;
            more = 3;
        } else if (c >= 0xE0) {
            cp = c & 15;
            more = 2;
        } else if (c >= 0xC0) {
            cp = c & 31;
            more = 1;
        }
        for (int k = 0; k < more && r->i < r->n; k++) cp = (cp << 6) | (r->s[r->i++] & 63);
        if (cp >= 0x10000) {
            cp -= 0x10000;
            buf[n++] = (uint16_t)(0xD800 + (cp >> 10));
            buf[n++] = (uint16_t)(0xDC00 + (cp & 0x3FF));
        } else {
            buf[n++] = (uint16_t)cp;
        }
    }
    *units = arena_alloc(r->arena, (n ? n : 1) * sizeof **units);
    memcpy(*units, buf, n * sizeof *buf);
    *len = n;
    free(buf);
    return true;
}

static bool expect(reader *r, const char *word) {
    size_t k = strlen(word);
    if (r->i + k > r->n || memcmp(r->s + r->i, word, k) != 0) {
        r->error = "unexpected text";
        return false;
    }
    r->i += k;
    return true;
}

static bool value(reader *r, json *out, int depth) {
    memset(out, 0, sizeof *out);
    if (depth > 512) {
        r->error = "nested too deeply";
        return false;
    }
    space(r);
    if (r->i >= r->n) {
        r->error = "unexpected end";
        return false;
    }
    unsigned char c = r->s[r->i];
    if (c == '{') {
        r->i++;
        size_t cap = 8, n = 0;
        json_member *m = malloc(cap * sizeof *m);
        if (!m) abort();
        space(r);
        if (r->i < r->n && r->s[r->i] == '}') {
            r->i++;
        } else {
            for (;;) {
                space(r);
                if (r->i >= r->n || r->s[r->i] != '"') {
                    free(m);
                    r->error = "expected a name";
                    return false;
                }
                r->i++;
                uint16_t *units;
                size_t len;
                if (!string(r, &units, &len)) {
                    free(m);
                    return false;
                }
                if (n == cap) {
                    cap *= 2;
                    m = realloc(m, cap * sizeof *m);
                    if (!m) abort();
                }
                size_t name_len;
                char *name8 = utf16_to_utf8_lossy(units, len, &name_len);
                m[n].name = arena_alloc(r->arena, name_len + 1);
                memcpy(m[n].name, name8, name_len + 1);
                free(name8);
                space(r);
                if (r->i >= r->n || r->s[r->i] != ':') {
                    free(m);
                    r->error = "expected a colon";
                    return false;
                }
                r->i++;
                if (!value(r, &m[n].value, depth + 1)) {
                    free(m);
                    return false;
                }
                n++;
                space(r);
                if (r->i < r->n && r->s[r->i] == ',') {
                    r->i++;
                } else if (r->i < r->n && r->s[r->i] == '}') {
                    r->i++;
                    break;
                } else {
                    free(m);
                    r->error = "expected , or }";
                    return false;
                }
            }
        }
        out->kind = JSON_OBJECT;
        out->len = n;
        out->members = arena_alloc(r->arena, (n ? n : 1) * sizeof *m);
        memcpy(out->members, m, n * sizeof *m);
        free(m);
        return true;
    }
    if (c == '[') {
        r->i++;
        size_t cap = 8, n = 0;
        json *items = malloc(cap * sizeof *items);
        if (!items) abort();
        space(r);
        if (r->i < r->n && r->s[r->i] == ']') {
            r->i++;
        } else {
            for (;;) {
                if (n == cap) {
                    cap *= 2;
                    items = realloc(items, cap * sizeof *items);
                    if (!items) abort();
                }
                if (!value(r, &items[n], depth + 1)) {
                    free(items);
                    return false;
                }
                n++;
                space(r);
                if (r->i < r->n && r->s[r->i] == ',') {
                    r->i++;
                } else if (r->i < r->n && r->s[r->i] == ']') {
                    r->i++;
                    break;
                } else {
                    free(items);
                    r->error = "expected , or ]";
                    return false;
                }
            }
        }
        out->kind = JSON_ARRAY;
        out->len = n;
        out->items = arena_alloc(r->arena, (n ? n : 1) * sizeof *items);
        memcpy(out->items, items, n * sizeof *items);
        free(items);
        return true;
    }
    if (c == '"') {
        r->i++;
        out->kind = JSON_STRING;
        return string(r, &out->units, &out->len);
    }
    if (c == 't') {
        out->kind = JSON_BOOL;
        out->boolean = true;
        return expect(r, "true");
    }
    if (c == 'f') {
        out->kind = JSON_BOOL;
        return expect(r, "false");
    }
    if (c == 'n') {
        out->kind = JSON_NULL;
        return expect(r, "null");
    }
    char number[64];
    size_t k = 0;
    while (r->i < r->n && k < sizeof number - 1 && strchr("+-0123456789.eE", r->s[r->i]) && r->s[r->i]) {
        number[k++] = (char)r->s[r->i++];
    }
    number[k] = 0;
    if (k == 0) {
        r->error = "unexpected character";
        return false;
    }
    out->kind = JSON_NUMBER;
    out->number = strtod(number, NULL);
    return true;
}

json *json_parse(const char *text, size_t len, json_arena **arena, const char **error) {
    reader r = {(const unsigned char *)text, 0, len, calloc(1, sizeof(json_arena)), NULL};
    if (!r.arena) abort();
    json *v = arena_alloc(r.arena, sizeof *v);
    *arena = r.arena;
    if (!value(&r, v, 0)) {
        *error = r.error;
        return NULL;
    }
    space(&r);
    if (r.i != r.n) {
        *error = "text after the value";
        return NULL;
    }
    return v;
}

const json *json_get(const json *o, const char *name) {
    if (!o || o->kind != JSON_OBJECT) return NULL;
    for (size_t k = 0; k < o->len; k++) {
        if (strcmp(o->members[k].name, name) == 0) return &o->members[k].value;
    }
    return NULL;
}

bool json_is_true(const json *v) {
    return v && v->kind == JSON_BOOL && v->boolean;
}

bool json_as_bool(const json *v, bool *out) {
    if (!v || v->kind != JSON_BOOL) return false;
    *out = v->boolean;
    return true;
}

const json *json_string(const json *v) {
    return v && v->kind == JSON_STRING ? v : NULL;
}

bool has_lone_surrogate(const uint16_t *s, size_t n) {
    for (size_t k = 0; k < n; k++) {
        uint16_t u = s[k];
        if (u >= 0xD800 && u <= 0xDBFF) {
            if (k + 1 < n && s[k + 1] >= 0xDC00 && s[k + 1] <= 0xDFFF) {
                k++;
                continue;
            }
            return true;
        }
        if (u >= 0xDC00 && u <= 0xDFFF) return true;
    }
    return false;
}

char *utf16_to_utf8_lossy(const uint16_t *s, size_t n, size_t *out_len) {
    char *out = malloc(3 * n + 1);
    if (!out) abort();
    size_t b = 0;
    for (size_t k = 0; k < n; k++) {
        uint32_t c = s[k];
        if (c >= 0xD800 && c <= 0xDBFF && k + 1 < n && s[k + 1] >= 0xDC00 && s[k + 1] <= 0xDFFF) {
            c = 0x10000 + ((c - 0xD800) << 10) + (s[k + 1] - 0xDC00);
            k++;
        } else if (c >= 0xD800 && c <= 0xDFFF) {
            c = 0xFFFD;
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

uint16_t *utf8_to_utf16(const char *str, size_t n, size_t *out_len) {
    const unsigned char *s = (const unsigned char *)str;
    uint16_t *out = malloc((n + 1) * sizeof *out);
    if (!out) abort();
    size_t k = 0, u = 0;
    while (k < n) {
        uint32_t c = s[k];
        int more = c >= 0xF0 ? 3 : c >= 0xE0 ? 2 : c >= 0xC0 ? 1 : 0;
        c = more == 3 ? c & 7 : more == 2 ? c & 15 : more == 1 ? c & 31 : c;
        k++;
        for (int j = 0; j < more && k < n; j++) c = (c << 6) | (s[k++] & 63);
        if (c >= 0x10000) {
            c -= 0x10000;
            out[u++] = (uint16_t)(0xD800 + (c >> 10));
            out[u++] = (uint16_t)(0xDC00 + (c & 0x3FF));
        } else {
            out[u++] = (uint16_t)c;
        }
    }
    *out_len = u;
    return out;
}
