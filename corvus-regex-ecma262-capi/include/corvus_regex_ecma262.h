/*
 * corvus_regex_ecma262.h: the C library over the corvus-regex-ecma262 crate, ECMA-262 (JavaScript) regular
 * expressions with the grammar and the answers of RegExp.
 *
 * An opaque handle is created and freed by the library. Text goes in as UTF-8 (a pointer and a length; a pointer may
 * be NULL when its length is 0), and strings come out as borrowed cre_str views. Every fallible function returns a
 * cre_status, with the details in thread-local storage (cre_last_error_message, cre_last_error_offset). No Rust panic
 * crosses into the caller. A cre_regex is immutable: any number of threads may use one at once.
 *
 * Licensed under the Apache License, Version 2.0.
 */
#ifndef CORVUS_REGEX_ECMA262_H
#define CORVUS_REGEX_ECMA262_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The header's version: compare with cre_version(), the library's. */
#define CRE_VERSION_MAJOR 0
#define CRE_VERSION_MINOR 1
#define CRE_VERSION_PATCH 0

/* The outcome of a call: CRE_OK, or why it failed (the details are in cre_last_error_message). */
typedef uint32_t cre_status;
/* The call succeeded. */
#define CRE_OK 0
/* A NULL handle or pointer, or a value out of range. */
#define CRE_INVALID_ARGUMENT 1
/* Text that is not UTF-8 (cre_last_error_offset is the offset of the first invalid byte). */
#define CRE_INVALID_UTF8 2
/* The pattern is not valid in the grammar of its flags (new RegExp would throw a SyntaxError). */
#define CRE_INVALID_PATTERN 3
/* The flags are not valid: a letter that is not one of dgimsuvy, a flag given twice, or u with v. */
#define CRE_INVALID_FLAGS 4
/* The pattern is over a limit of size (1,048,576 bytes, 1,000 levels of groups, or 1,048,576 bytes counted for it as
 * it is read). It is not the status of a pattern that is not valid: the pattern was not read to its end. */
#define CRE_PATTERN_TOO_LARGE 5
/* A match reached its limit of time or of backtracking memory, so there is no answer. */
#define CRE_MATCH_LIMIT 6
/* A bug in the library: it was caught, and the library remains usable. */
#define CRE_PANIC 7

/* A UTF-8 string the library owns: len bytes from ptr, not NUL-terminated. Each function returning one says how long
 * it stays valid. */
typedef struct cre_str {
    const char *ptr;
    size_t len;
} cre_str;

/* Where a match is: byte offsets into the text, start to end. */
typedef struct cre_match {
    size_t start;
    size_t end;
} cre_match;

/* A compiled regular expression. */
typedef struct cre_regex cre_regex;

/* The library's version, as "major.minor.patch". The string is static. */
cre_str cre_version(void);

/* The message of the last call on this thread that failed. It stays valid until the next call that fails on this
 * thread. */
cre_str cre_last_error_message(void);

/* The offset the last failure on this thread points at: the offset of the first byte that is not UTF-8, or 0. */
size_t cre_last_error_offset(void);

/* Compiles a pattern with the flags new RegExp(pattern, flags) takes (flags may be NULL with a length of 0 for none).
 * On success *out_regex is a handle to free with cre_regex_free; on failure it is NULL and the status says why:
 * CRE_INVALID_PATTERN, CRE_INVALID_FLAGS, CRE_PATTERN_TOO_LARGE, CRE_INVALID_UTF8 or CRE_INVALID_ARGUMENT. */
cre_status cre_regex_new(const char *pattern, size_t pattern_len, const char *flags, size_t flags_len,
                         cre_regex **out_regex);

/* Frees a regular expression. NULL is allowed and does nothing. */
void cre_regex_free(cre_regex *regex);

/* Whether the pattern matches anywhere in the UTF-8 text (at its start alone with the y flag): *out_matched. With no
 * limit on the time the match takes. */
cre_status cre_regex_is_match(const cre_regex *regex, const char *text, size_t text_len, bool *out_matched);

/* cre_regex_is_match with a limit on the time the match may take, in milliseconds (0 for none). A match that reaches
 * it, or whose backtracking memory reaches its cap, is CRE_MATCH_LIMIT, and *out_matched is false. */
cre_status cre_regex_is_match_limit(const cre_regex *regex, const char *text, size_t text_len, uint64_t limit_ms,
                                    bool *out_matched);

/* The first match at or after the byte offset start of the UTF-8 text, as exec finds it with lastIndex set to start:
 * *out_found says whether there is one, and *out_match is where it is (zeroes when there is none). Without the u or v
 * flag a match can start or end between the halves of a character beyond the Basic Multilingual Plane; such an offset
 * is that of the character's third byte. */
cre_status cre_regex_find(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                          cre_match *out_match, bool *out_found);

/* The number of capturing groups of the pattern (0 for NULL). */
size_t cre_regex_group_count(const cre_regex *regex);

#ifdef __cplusplus
}
#endif

#endif /* CORVUS_REGEX_ECMA262_H */
