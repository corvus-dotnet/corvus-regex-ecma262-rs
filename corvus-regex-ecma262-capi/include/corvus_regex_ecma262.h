/*
 * corvus_regex_ecma262.h: the C library over the corvus-regex-ecma262 crate, ECMA-262 (JavaScript) regular
 * expressions with the grammar and the answers of RegExp, over UTF-8 and UTF-16 text.
 *
 * The conventions, which every function follows:
 *
 * - Handles. A cre_regex (a compiled pattern), a cre_captures (what a match captured) and a cre_matches (a global
 *   search) are opaque, created by the library and freed by the caller with their free function (cre_regex_free,
 *   cre_captures_free, cre_matches_free). Each free function takes NULL and does nothing. A cre_regex is immutable:
 *   any number of threads may use one at once. A cre_captures or a cre_matches is used by one thread at a time.
 * - Text in. UTF-8 text is a pointer and a length in bytes, checked to be UTF-8 (CRE_INVALID_UTF8 otherwise). The
 *   functions with the suffix _utf16 take UTF-16 text instead, a pointer and a length in code units, which may hold
 *   lone surrogates and is not checked. A pointer may be NULL when its length is 0. Nothing is transcoded: positions
 *   are in the units of the text, bytes of UTF-8 or UTF-16 code units.
 * - Text out. A borrowed string is a cre_str (or cre_str16), not NUL-terminated, valid for as long as its function
 *   says. An owned string is a cre_string (or cre_string16), NUL-terminated, which the caller frees with
 *   cre_string_free (or cre_string16_free); the pieces of a split are a cre_pieces (or cre_pieces16), freed with
 *   cre_pieces_free (or cre_pieces16_free).
 * - Status. Every function that can fail returns a cre_status, CRE_OK or why it failed, with a message in
 *   thread-local storage (cre_last_error_message, cre_last_error_offset). On failure each out-parameter is set to
 *   its empty value (NULL, false, CRE_UNSET or {NULL, 0}). No Rust panic crosses into the caller: a bug in the
 *   library is CRE_PANIC, and the library remains usable.
 * - Matching. A pattern is read as new RegExp(pattern, flags) reads it, and a match is the one exec finds. Without
 *   the u or v flag a pattern is matched by UTF-16 code unit, so over UTF-8 a match can start or end between the
 *   halves of a character beyond the Basic Multilingual Plane; such a position is the offset of the character's
 *   third byte. The g flag makes cre_regex_replace replace every match; the y flag makes a match start where the
 *   search starts.
 * - Limits. A pattern over a limit of size is refused (CRE_PATTERN_TOO_LARGE). A regular expression has a match
 *   timeout, as a Regex of .NET has: the time each match may take, given by cre_regex_new_with_timeout or for every
 *   regular expression by cre_set_default_match_timeout, and none unless one of them gives one. A match that reaches
 *   it, or whose backtracking memory reaches CRE_MAX_STACK entries, is CRE_MATCH_LIMIT, which is not an answer, from
 *   whichever function matched. The functions with the suffix _limit take a limit of their own in milliseconds (0
 *   for the regular expression's timeout). Without a timeout a match has no limit of time or of memory, so a program
 *   that matches patterns or texts from outside should set one. A pattern without a backreference takes time in
 *   proportion to its size times the text.
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
#define CRE_VERSION_MINOR 2
#define CRE_VERSION_PATCH 0

/* The limits of size of a pattern: its length in bytes, the depth its groups nest to, and the size counted for it as
 * it is read. A pattern over any of them is CRE_PATTERN_TOO_LARGE. */
#define CRE_MAX_PATTERN_LENGTH 1048576
#define CRE_MAX_NESTING 1000
#define CRE_MAX_SIZE 1048576
/* The entries the backtracking stack of a match with a limit may reach before the match is CRE_MATCH_LIMIT. */
#define CRE_MAX_STACK 1048576

/* The start and the end of a group that did not take part in a match, and of no match. */
#define CRE_UNSET SIZE_MAX
/* The limit of cre_regex_split that keeps every piece, as split(separator) with no limit. */
#define CRE_SPLIT_ALL 0xFFFFFFFFu

/* The outcome of a call: CRE_OK, or why it failed (the details are in cre_last_error_message). */
typedef uint32_t cre_status;
/* The call succeeded. */
#define CRE_OK 0
/* A NULL handle or pointer, a value out of range (a start beyond the text or inside a character), a misaligned
 * UTF-16 pointer, or a handle used with the wrong regular expression. */
#define CRE_INVALID_ARGUMENT 1
/* Text that is not UTF-8 (cre_last_error_offset is the offset of the first invalid byte). */
#define CRE_INVALID_UTF8 2
/* The pattern is not valid in the grammar of its flags (new RegExp would throw a SyntaxError). */
#define CRE_INVALID_PATTERN 3
/* The flags are not valid: a letter that is not one of dgimsuvy, a flag given twice, or u with v. */
#define CRE_INVALID_FLAGS 4
/* The pattern is over a limit of size (CRE_MAX_PATTERN_LENGTH, CRE_MAX_NESTING or CRE_MAX_SIZE). It is not the status
 * of a pattern that is not valid: the pattern was not read to its end. */
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

/* A UTF-16 string the library owns: len code units from ptr. */
typedef struct cre_str16 {
    const uint16_t *ptr;
    size_t len;
} cre_str16;

/* A UTF-8 string the caller owns once a function has written it: len bytes from ptr, then a NUL (not counted in
 * len). Free it with cre_string_free. */
typedef struct cre_string {
    char *ptr;
    size_t len;
} cre_string;

/* A UTF-16 string the caller owns once a function has written it: len code units from ptr, then a 0 unit (not
 * counted in len). Free it with cre_string16_free. */
typedef struct cre_string16 {
    uint16_t *ptr;
    size_t len;
} cre_string16;

/* The pieces of a split: len pieces at items, which the caller frees with cre_pieces_free. A piece whose ptr is NULL
 * is a group that did not take part (undefined in the array split returns); a piece that is present and empty has a
 * ptr that is not NULL and a len of 0. internal belongs to the library. */
typedef struct cre_pieces {
    const cre_str *items;
    size_t len;
    void *internal;
} cre_pieces;

/* The pieces of a split of UTF-16 text, freed with cre_pieces16_free; as cre_pieces. */
typedef struct cre_pieces16 {
    const cre_str16 *items;
    size_t len;
    void *internal;
} cre_pieces16;

/* Where a match or a group is: offsets into the text in its units (bytes of UTF-8, or UTF-16 code units), start to
 * end. A group that did not take part is CRE_UNSET to CRE_UNSET. */
typedef struct cre_match {
    size_t start;
    size_t end;
} cre_match;

/* A compiled regular expression. */
typedef struct cre_regex cre_regex;
/* What a match captured, by number and by name: made for one regular expression, reused from match to match. */
typedef struct cre_captures cre_captures;
/* A global search of one text, as matchAll makes it. */
typedef struct cre_matches cre_matches;

/* ------------------------------------------------------------------------------------------------------------------
 * The library */

/* The library's version, as "major.minor.patch". The string is static. */
cre_str cre_version(void);

/* The name of a status, such as "CRE_INVALID_PATTERN", or "unknown". The string is static. */
cre_str cre_status_name(cre_status status);

/* The message of the last call on this thread that failed. It stays valid until the next call that fails on this
 * thread. */
cre_str cre_last_error_message(void);

/* The offset the last failure on this thread points at: the offset of the first byte that is not UTF-8, or 0. */
size_t cre_last_error_offset(void);

/* ------------------------------------------------------------------------------------------------------------------
 * Compiling */

/* Sets the match timeout of every regular expression compiled from now on by cre_regex_new, in milliseconds (0 for
 * none, the default): the time each match may take before it is abandoned with CRE_MATCH_LIMIT. A program that
 * matches patterns or texts from outside should set it when it starts (see cre_regex_new_with_timeout). */
void cre_set_default_match_timeout(uint64_t timeout_ms);

/* The match timeout a regular expression compiled now by cre_regex_new takes, in milliseconds (0 for none). */
uint64_t cre_default_match_timeout(void);

/* Compiles a pattern with the flags new RegExp(pattern, flags) takes, any of dgimsuvy (flags may be NULL with a
 * length of 0 for none), and the match timeout of cre_default_match_timeout. On success *out_regex is a handle to
 * free with cre_regex_free; on failure it is NULL and the status says why: CRE_INVALID_PATTERN, CRE_INVALID_FLAGS,
 * CRE_PATTERN_TOO_LARGE, CRE_INVALID_UTF8 or CRE_INVALID_ARGUMENT. */
cre_status cre_regex_new(const char *pattern, size_t pattern_len, const char *flags, size_t flags_len,
                         cre_regex **out_regex);

/* cre_regex_new with a match timeout in milliseconds (0 for none): the time each match may take before it is
 * abandoned with CRE_MATCH_LIMIT, which brings a cap on the memory of its backtracking with it. Every function that
 * matches gives its match this timeout, except where a _limit function is given a limit of its own. Without one a
 * match has no limit of time or of memory. */
cre_status cre_regex_new_with_timeout(const char *pattern, size_t pattern_len, const char *flags, size_t flags_len,
                                      uint64_t timeout_ms, cre_regex **out_regex);

/* The match timeout of the regular expression, in milliseconds (0 for none). */
uint64_t cre_regex_match_timeout(const cre_regex *regex);

/* Frees a regular expression. NULL is allowed and does nothing. */
void cre_regex_free(cre_regex *regex);

/* Whether a pattern is valid with the flags, without compiling it: *out_valid. A pattern over a limit of size is not
 * valid here. The status is CRE_INVALID_UTF8 or CRE_INVALID_ARGUMENT for arguments that are not text. */
cre_status cre_regex_is_valid(const char *pattern, size_t pattern_len, const char *flags, size_t flags_len,
                              bool *out_valid);

/* The source of the pattern, as it was given. It stays valid as long as the regular expression ({NULL, 0} for NULL). */
cre_str cre_regex_source(const cre_regex *regex);

/* The flags as RegExp.prototype.flags writes them, in the order dgimsuvy. It stays valid as long as the regular
 * expression ({NULL, 0} for NULL). */
cre_str cre_regex_flags(const cre_regex *regex);

/* The number of capturing groups of the pattern (0 for NULL). A match has one more group: the whole match, group 0. */
size_t cre_regex_group_count(const cre_regex *regex);

/* The number of distinct names of the pattern's groups (0 for NULL). Two groups in separate alternatives may share a
 * name. */
size_t cre_regex_group_name_count(const cre_regex *regex);

/* The name index (from 0), in the order of the first group of each name, or {NULL, 0} when there is none. It stays
 * valid as long as the regular expression. */
cre_str cre_regex_group_name(const cre_regex *regex, size_t index);

/* ------------------------------------------------------------------------------------------------------------------
 * Matching UTF-8 text
 *
 * A start is a byte offset no greater than the text's length, on a character boundary or, without the u or v flag,
 * between the halves of a character beyond the Basic Multilingual Plane (its third byte); anything else is
 * CRE_INVALID_ARGUMENT. Each call checks that the whole text is UTF-8; a global search (cre_regex_find_iter) checks it
 * once. A caller whose strings are UTF-8 by construction can skip the check with the _unchecked functions below. */

/* Whether the pattern matches anywhere in the text (at its start alone with the y flag): *out_matched. */
cre_status cre_regex_is_match(const cre_regex *regex, const char *text, size_t text_len, bool *out_matched);

/* cre_regex_is_match with a limit on the time the match may take, in milliseconds (0 for the regular expression's match
 * timeout). A match that reaches it, or whose backtracking memory reaches its cap, is CRE_MATCH_LIMIT, and *out_matched
 * is false. */
cre_status cre_regex_is_match_limit(const cre_regex *regex, const char *text, size_t text_len, uint64_t limit_ms,
                                    bool *out_matched);

/* Whether the pattern matches at or after start (at start alone with the y flag), as RegExp.prototype.test with
 * lastIndex set to it: *out_matched. */
cre_status cre_regex_is_match_at(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                 bool *out_matched);

/* The first match at or after start, as exec finds it with lastIndex set to start: *out_found says whether there is
 * one, and *out_match is where it is (CRE_UNSET when there is none). */
cre_status cre_regex_find(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                          cre_match *out_match, bool *out_found);

/* cre_regex_find with a limit on the time the match may take, in milliseconds (0 for the regular expression's match
 * timeout): CRE_MATCH_LIMIT when it is reached. */
cre_status cre_regex_find_limit(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                uint64_t limit_ms, cre_match *out_match, bool *out_found);

/* The first match at or after start with what each group captured, read into groups, a buffer of groups_len entries
 * the caller owns: group 0 is the match, then each capturing group, CRE_UNSET for one that did not take part. A
 * buffer shorter than cre_regex_group_count() + 1 takes the first groups, and entries past the last group are
 * CRE_UNSET. Nothing is allocated once the thread has read a match with as many groups. *out_found. */
cre_status cre_regex_captures_read(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                   cre_match *groups, size_t groups_len, bool *out_found);

/* cre_regex_captures_read with a limit on the time the match may take, in milliseconds (0 for the regular expression's
 * match timeout): CRE_MATCH_LIMIT when it is reached. It allocates for each match. */
cre_status cre_regex_captures_read_limit(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                         uint64_t limit_ms, cre_match *groups, size_t groups_len, bool *out_found);

/* The first match at or after start with what each group captured, by number and by name, written into captures
 * (made by cre_captures_new for this regular expression): *out_found. */
cre_status cre_regex_captures(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                              cre_captures *captures, bool *out_found);

/* cre_regex_captures with a limit on the time the match may take, in milliseconds (0 for the regular expression's match
 * timeout): CRE_MATCH_LIMIT when it is reached. */
cre_status cre_regex_captures_limit(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                    uint64_t limit_ms, cre_captures *captures, bool *out_found);

/* A global search of the text, as matchAll makes one: cre_matches_next gives each match, and after an empty match the
 * search goes on one character later (one code point with u or v, one UTF-16 code unit without). With the y flag
 * each match must start where the one before it ended. The text is checked to be UTF-8 here, once. On success
 * *out_matches is a handle to free with cre_matches_free. The regular expression and the text must stay alive and
 * unchanged until then. */
cre_status cre_regex_find_iter(const cre_regex *regex, const char *text, size_t text_len, cre_matches **out_matches);

/* cre_regex_find_iter keeping the groups of each match, for cre_matches_next_captures. */
cre_status cre_regex_captures_iter(const cre_regex *regex, const char *text, size_t text_len,
                                   cre_matches **out_matches);

/* The text with the first match replaced, or every match with the g flag, as String.prototype.replace replaces them.
 * The replacement is a template: $& is the match, $` the text before it, $' the text after it, $1 to $99 a group,
 * $<name> a named group, and $$ a dollar sign; anything else is itself (GetSubstitution). On success *out_result is a
 * string to free with cre_string_free. A half character (an end between the halves of a character beyond the Basic
 * Multilingual Plane, without u or v) is written as U+FFFD. */
cre_status cre_regex_replace(const cre_regex *regex, const char *text, size_t text_len, const char *replacement,
                             size_t replacement_len, cre_string *out_result);

/* cre_regex_replace with every match replaced, whether or not the g flag is set (String.prototype.replaceAll). */
cre_status cre_regex_replace_all(const cre_regex *regex, const char *text, size_t text_len, const char *replacement,
                                 size_t replacement_len, cre_string *out_result);

/* The text split around the matches, as String.prototype.split(separator, limit) splits it: what each group captured
 * is spliced in after the piece before it (a NULL ptr for a group that did not take part), an empty match at the
 * start or the end of the text splits nothing, and there are at most limit pieces (CRE_SPLIT_ALL for every one). On
 * success *out_pieces holds the pieces, to free with cre_pieces_free. A half character is written as U+FFFD. */
cre_status cre_regex_split(const cre_regex *regex, const char *text, size_t text_len, uint32_t limit,
                           cre_pieces *out_pieces);

/* ------------------------------------------------------------------------------------------------------------------
 * Matching UTF-8 text without the check
 *
 * Each function is the one without the suffix, with the text taken on trust to be UTF-8: it is not checked, so a call
 * does not read the whole text once more before it searches. They are for a caller whose strings are UTF-8 by
 * construction, such as a Swift String or a Rust str, or that has checked a text once with cre_utf8_check. Text that
 * is not UTF-8 is undefined behaviour. Everything else (the arguments, the results, the statuses, CRE_INVALID_UTF8
 * aside) is as the function without the suffix. */

/* Whether the text is UTF-8: CRE_OK, or CRE_INVALID_UTF8 with cre_last_error_offset at the first byte that is not. A
 * caller whose strings carry no such guarantee checks a text once, when it makes it, and then searches it with the
 * _unchecked functions as often as it likes. */
cre_status cre_utf8_check(const char *text, size_t text_len);

cre_status cre_regex_is_match_unchecked(const cre_regex *regex, const char *text, size_t text_len, bool *out_matched);
cre_status cre_regex_is_match_limit_unchecked(const cre_regex *regex, const char *text, size_t text_len,
                                              uint64_t limit_ms, bool *out_matched);
cre_status cre_regex_is_match_at_unchecked(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                           bool *out_matched);
cre_status cre_regex_find_unchecked(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                    cre_match *out_match, bool *out_found);
cre_status cre_regex_find_limit_unchecked(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                          uint64_t limit_ms, cre_match *out_match, bool *out_found);
cre_status cre_regex_captures_read_unchecked(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                             cre_match *groups, size_t groups_len, bool *out_found);
cre_status cre_regex_captures_read_limit_unchecked(const cre_regex *regex, const char *text, size_t text_len,
                                                   size_t start, uint64_t limit_ms, cre_match *groups,
                                                   size_t groups_len, bool *out_found);
cre_status cre_regex_captures_unchecked(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                        cre_captures *captures, bool *out_found);
cre_status cre_regex_captures_limit_unchecked(const cre_regex *regex, const char *text, size_t text_len, size_t start,
                                              uint64_t limit_ms, cre_captures *captures, bool *out_found);
cre_status cre_regex_find_iter_unchecked(const cre_regex *regex, const char *text, size_t text_len,
                                         cre_matches **out_matches);
cre_status cre_regex_captures_iter_unchecked(const cre_regex *regex, const char *text, size_t text_len,
                                             cre_matches **out_matches);
cre_status cre_regex_replace_unchecked(const cre_regex *regex, const char *text, size_t text_len,
                                       const char *replacement, size_t replacement_len, cre_string *out_result);
cre_status cre_regex_replace_all_unchecked(const cre_regex *regex, const char *text, size_t text_len,
                                           const char *replacement, size_t replacement_len, cre_string *out_result);
cre_status cre_regex_split_unchecked(const cre_regex *regex, const char *text, size_t text_len, uint32_t limit,
                                     cre_pieces *out_pieces);

/* ------------------------------------------------------------------------------------------------------------------
 * Matching UTF-16 text
 *
 * As the UTF-8 functions, over text_len UTF-16 code units at text (aligned for uint16_t), which may hold lone
 * surrogates. Positions are code unit indexes, and a start may be any index no greater than text_len. */

cre_status cre_regex_is_match_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len, bool *out_matched);

cre_status cre_regex_is_match_limit_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len,
                                          uint64_t limit_ms, bool *out_matched);

cre_status cre_regex_is_match_at_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len, size_t start,
                                       bool *out_matched);

cre_status cre_regex_find_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len, size_t start,
                                cre_match *out_match, bool *out_found);

cre_status cre_regex_find_limit_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len, size_t start,
                                      uint64_t limit_ms, cre_match *out_match, bool *out_found);

cre_status cre_regex_captures_read_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len, size_t start,
                                         cre_match *groups, size_t groups_len, bool *out_found);

cre_status cre_regex_captures_read_limit_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len,
                                               size_t start, uint64_t limit_ms, cre_match *groups, size_t groups_len,
                                               bool *out_found);

cre_status cre_regex_captures_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len, size_t start,
                                    cre_captures *captures, bool *out_found);

cre_status cre_regex_captures_limit_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len, size_t start,
                                          uint64_t limit_ms, cre_captures *captures, bool *out_found);

/* The regular expression and the text must stay alive and unchanged until cre_matches_free. */
cre_status cre_regex_find_iter_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len,
                                     cre_matches **out_matches);

cre_status cre_regex_captures_iter_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len,
                                         cre_matches **out_matches);

/* The replacement is UTF-16 too; *out_result is a string to free with cre_string16_free. A lone surrogate is kept as
 * it is. */
cre_status cre_regex_replace_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len,
                                   const uint16_t *replacement, size_t replacement_len, cre_string16 *out_result);

cre_status cre_regex_replace_all_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len,
                                       const uint16_t *replacement, size_t replacement_len, cre_string16 *out_result);

/* *out_pieces holds the pieces, to free with cre_pieces16_free. */
cre_status cre_regex_split_utf16(const cre_regex *regex, const uint16_t *text, size_t text_len, uint32_t limit,
                                 cre_pieces16 *out_pieces);

/* ------------------------------------------------------------------------------------------------------------------
 * Captures */

/* A place for what a match of regex captures, for cre_regex_captures and cre_matches_next_captures, reused from match
 * to match. On success *out_captures is a handle to free with cre_captures_free. It holds no reference to the regular
 * expression or to any text, but is only used with the regular expression it was made for (CRE_INVALID_ARGUMENT
 * otherwise). */
cre_status cre_captures_new(const cre_regex *regex, cre_captures **out_captures);

/* Frees a captures handle. NULL is allowed and does nothing. */
void cre_captures_free(cre_captures *captures);

/* The number of groups of the last match, group 0 among them, or 0 when the last search found none (or for NULL). */
size_t cre_captures_len(const cre_captures *captures);

/* The group group of the last match (0 is the whole match): true with *out_match where it is, or false with
 * *out_match CRE_UNSET when it did not take part, there is no such group, or the last search found no match. */
bool cre_captures_get(const cre_captures *captures, size_t group, cre_match *out_match);

/* The group of the name in the last match (of the groups that share it, the one that took part): true with
 * *out_match where it is, or false with *out_match CRE_UNSET. The name is UTF-8, name_len bytes. */
bool cre_captures_name(const cre_captures *captures, const char *name, size_t name_len, cre_match *out_match);

/* ------------------------------------------------------------------------------------------------------------------
 * The global search */

/* The next match of a global search: *out_found says whether there is one, and *out_match is where it is (CRE_UNSET
 * when there is none). Once there is none, every later call finds none until cre_matches_reset. */
cre_status cre_matches_next(cre_matches *matches, cre_match *out_match, bool *out_found);

/* The next matches of a global search, up to capacity of them, written in order to out_matches[0] to
 * out_matches[*out_count - 1]: the matches cre_matches_next would give one call each, with one call for many, which
 * spares a call for each match where a text has very many (a buffer of some hundreds is enough). *out_count is less
 * than capacity only when the search has found every match, so a call that writes 0 ends it; a capacity of 0 writes
 * nothing and out_matches may then be NULL. It reads a search made by cre_regex_captures_iter as well, giving where
 * each match is.
 *
 *     cre_match batch[256];
 *     size_t n;
 *     while (cre_matches_next_batch(it, batch, 256, &n) == CRE_OK && n > 0) {
 *         for (size_t k = 0; k < n; k++) use(batch[k].start, batch[k].end);
 *     }
 */
cre_status cre_matches_next_batch(cre_matches *matches, cre_match *out_matches, size_t capacity, size_t *out_count);

/* The next match of a global search made by cre_regex_captures_iter (or its UTF-16 form), with its groups written
 * into captures (made for the same regular expression): *out_found. CRE_INVALID_ARGUMENT for a search made by
 * cre_regex_find_iter, which keeps no groups. */
cre_status cre_matches_next_captures(cre_matches *matches, cre_captures *captures, bool *out_found);

/* The next matches of a global search made by cre_regex_captures_iter (or its UTF-16 form) with their groups, written
 * in order to out_groups: each match as cre_regex_group_count(regex) + 1 entries (the match, then each group,
 * CRE_UNSET for one that did not take part), as many matches as capacity entries hold, and *out_count matches. It
 * spares a call for each match and allocates nothing. *out_count is less than capacity / (groups + 1) only when the
 * search has found every match. A capacity of 0 writes nothing (out_groups may then be NULL); a capacity that holds
 * no match, or a search made by cre_regex_find_iter, is CRE_INVALID_ARGUMENT. The groups are by number
 * (cre_matches_next_captures and cre_captures_name read them by name).
 *
 *     size_t width = cre_regex_group_count(re) + 1, n;
 *     cre_match groups[64 * 4];
 *     while (cre_matches_next_captures_batch(it, groups, 64 * 4, &n) == CRE_OK && n > 0) {
 *         for (size_t k = 0; k < n; k++) use(&groups[k * width]);
 *     }
 */
cre_status cre_matches_next_captures_batch(cre_matches *matches, cre_match *out_groups, size_t capacity,
                                           size_t *out_count);

/* Starts the global search again at the start of its text, which is not checked again: as setting lastIndex to 0.
 * NULL is allowed and does nothing. */
void cre_matches_reset(cre_matches *matches);

/* Frees a global search. NULL is allowed and does nothing. */
void cre_matches_free(cre_matches *matches);

/* ------------------------------------------------------------------------------------------------------------------
 * Freeing results */

/* Frees a string the library wrote, and sets it to {NULL, 0}. NULL, or a string already {NULL, 0}, does nothing. */
void cre_string_free(cre_string *string);

/* Frees a UTF-16 string the library wrote, and sets it to {NULL, 0}. NULL, or a string already {NULL, 0}, does
 * nothing. */
void cre_string16_free(cre_string16 *string);

/* Frees the pieces of a split, and empties them. NULL, or pieces already empty, does nothing. */
void cre_pieces_free(cre_pieces *pieces);

/* Frees the pieces of a split of UTF-16 text, and empties them. NULL, or pieces already empty, does nothing. */
void cre_pieces16_free(cre_pieces16 *pieces);

#ifdef __cplusplus
}
#endif

#endif /* CORVUS_REGEX_ECMA262_H */
