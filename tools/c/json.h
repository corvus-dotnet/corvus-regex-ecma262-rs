/* A small JSON reader for the C tools, as tools/src/json.rs is for the Rust ones: one value from a buffer, its
 * strings as UTF-16 code units (lone surrogates kept, as the test data writes them), everything in one arena that is
 * freed at once. */
#ifndef CRE_TOOLS_JSON_H
#define CRE_TOOLS_JSON_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef enum { JSON_NULL, JSON_BOOL, JSON_NUMBER, JSON_STRING, JSON_ARRAY, JSON_OBJECT } json_kind;

typedef struct json json;
typedef struct json_member json_member;

struct json {
    json_kind kind;
    bool boolean;
    double number;
    /* A string: len code units. An array: len items. An object: len members. */
    size_t len;
    uint16_t *units;
    json *items;
    json_member *members;
};

struct json_member {
    char *name; /* ASCII names only are looked up; others are kept as their UTF-8. */
    json value;
};

typedef struct json_arena json_arena;

/* Reads the one value that is all of text[0..len) but white space. NULL on an error, with a message in *error (static
 * storage, or the arena's). Free the arena (and so the value) with json_arena_free. */
json *json_parse(const char *text, size_t len, json_arena **arena, const char **error);
void json_arena_free(json_arena *arena);

/* The member of an object, or NULL. */
const json *json_get(const json *object, const char *name);
/* Accessors that answer false (or NULL) for another kind of value. */
bool json_is_true(const json *value);
bool json_as_bool(const json *value, bool *out);
const json *json_string(const json *value);

/* UTF-16 to UTF-8, lone surrogates written as U+FFFD (String::from_utf16_lossy): a NUL-terminated string from
 * malloc, *out_len its length. */
char *utf16_to_utf8_lossy(const uint16_t *units, size_t len, size_t *out_len);
/* Whether the units hold a lone surrogate. */
bool has_lone_surrogate(const uint16_t *units, size_t len);
/* UTF-8 (valid) to UTF-16: an array from malloc, *out_len its length. */
uint16_t *utf8_to_utf16(const char *s, size_t len, size_t *out_len);

#endif
