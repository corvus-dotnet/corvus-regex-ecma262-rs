//! The C library over the `corvus-regex-ecma262` crate, in the shape of the Corvus JSON Schema C library: opaque
//! handles created and freed by the library, text in as (pointer, length) in UTF-8 or UTF-16, borrowed strings out as
//! `cre_str` views, owned results with a free function each, a `cre_status` from every fallible function with the
//! details in thread-local storage, and no Rust panic crossing into the caller.
//!
//! Every member of the crate's `Regex` has its C function, over UTF-8 and (with the suffix `_utf16`) over UTF-16:
//! `is_match`, `is_match_at`, `find_at`, `captures_at` with the groups by number and by name, `captures_read_at`
//! into a buffer the caller owns, the global search (`find_iter`, `captures_iter`), `replace`, `replace_all`, `split`
//! and `splitn`, each matching function with a limit of time where the crate has one.
//!
//! The comments on the public items are the C header's documentation (`include/corvus_regex_ecma262.h`).

// The names are the C API's.
#![allow(non_camel_case_types)]
// The safety contracts are the C API's documented ones (a handle from this library, a valid pointer and length).
#![allow(clippy::missing_safety_doc)]

use std::borrow::Cow;
use std::cell::RefCell;
use std::ffi::{c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use corvus::{
    CaptureLocations, CaptureMatches, Captures, Edition, Error, Flags, Haystack, MatchError, Matches, Regex,
    RegexBuilder, UnicodeVersion,
};

// ---------------------------------------------------------------------------------------------------------------------
// Types and constants

/// The outcome of a call: `CRE_OK`, or why it failed (the details are in `cre_last_error_message`).
pub type cre_status = u32;
/// The call succeeded.
pub const CRE_OK: cre_status = 0;
/// A NULL handle or pointer, or a value out of range.
pub const CRE_INVALID_ARGUMENT: cre_status = 1;
/// Text that is not UTF-8 (`cre_last_error_offset` is the offset of the first invalid byte).
pub const CRE_INVALID_UTF8: cre_status = 2;
/// The pattern is not valid in the grammar of its flags (`new RegExp` would throw a SyntaxError).
pub const CRE_INVALID_PATTERN: cre_status = 3;
/// The flags are not valid: a letter that is not one of `dgimsuvy`, a flag given twice, or `u` with `v`.
pub const CRE_INVALID_FLAGS: cre_status = 4;
/// The pattern is over a limit of size: 1,048,576 bytes of pattern, 1,000 levels of groups, or 1,048,576 bytes
/// counted for it as it is read. It is not the status of a pattern that is not valid: the pattern was not read to its
/// end.
pub const CRE_PATTERN_TOO_LARGE: cre_status = 5;
/// A match reached its limit of time or of backtracking memory, so there is no answer.
pub const CRE_MATCH_LIMIT: cre_status = 6;
/// A bug in the library: it was caught, and the library remains usable.
pub const CRE_PANIC: cre_status = 7;

/// The header's version: compare with `cre_version()`, the library's.
pub const CRE_VERSION_MAJOR: u32 = 0;
pub const CRE_VERSION_MINOR: u32 = 0;
pub const CRE_VERSION_PATCH: u32 = 0;

/// The start and the end of a group that did not take part in a match.
pub const CRE_UNSET: usize = usize::MAX;
/// The limit of `cre_regex_split` that keeps every piece, as `split(separator)` with no limit.
pub const CRE_SPLIT_ALL: u32 = u32::MAX;

/// The edition of ECMA-262 whose grammar a pattern is read with: today's (the default), or ECMAScript 2020's, which
/// JSON Schema 2020-12 names (see `Edition`).
pub const CRE_EDITION_CURRENT: u32 = 0;
pub const CRE_EDITION_ES2020: u32 = 1;

/// The version of Unicode whose tables a pattern is read with (see `UnicodeVersion`): the default (V8's, 17 today),
/// 13, 17, 18, or the latest the library carries (18 today).
pub const CRE_UNICODE_DEFAULT: u32 = 0;
pub const CRE_UNICODE_13: u32 = 13;
pub const CRE_UNICODE_17: u32 = 17;
pub const CRE_UNICODE_18: u32 = 18;
pub const CRE_UNICODE_LATEST: u32 = 0x7FFF_FFFF;

/// The edition of a `CRE_EDITION_` value.
fn edition_of(edition: u32) -> Result<Edition, Failure> {
    match edition {
        CRE_EDITION_CURRENT => Ok(Edition::Current),
        CRE_EDITION_ES2020 => Ok(Edition::Es2020),
        other => Err(Failure::new(CRE_INVALID_ARGUMENT, format!("{other} is not a CRE_EDITION_ value"))),
    }
}

/// The version of a `CRE_UNICODE_` value.
fn unicode_version_of(version: u32) -> Result<UnicodeVersion, Failure> {
    match version {
        CRE_UNICODE_DEFAULT => Ok(UnicodeVersion::Default),
        CRE_UNICODE_13 => Ok(UnicodeVersion::V13),
        CRE_UNICODE_17 => Ok(UnicodeVersion::V17),
        CRE_UNICODE_18 => Ok(UnicodeVersion::V18),
        CRE_UNICODE_LATEST => Ok(UnicodeVersion::Latest),
        other => Err(Failure::new(CRE_INVALID_ARGUMENT, format!("{other} is not a CRE_UNICODE_ value"))),
    }
}

/// A UTF-8 string the library owns: `len` bytes from `ptr`, not NUL-terminated. Each function returning one says how
/// long it stays valid.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct cre_str {
    pub ptr: *const c_char,
    pub len: usize,
}

impl cre_str {
    const NULL: cre_str = cre_str { ptr: std::ptr::null(), len: 0 };

    fn of(s: &str) -> cre_str {
        cre_str { ptr: s.as_ptr().cast(), len: s.len() }
    }
}

/// A UTF-16 string the library owns: `len` code units from `ptr`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct cre_str16 {
    pub ptr: *const u16,
    pub len: usize,
}

/// A UTF-8 string the caller owns once a function has written it, NUL-terminated (`len` does not count the NUL), and
/// frees with `cre_string_free`.
#[repr(C)]
pub struct cre_string {
    pub ptr: *mut c_char,
    pub len: usize,
}

/// A UTF-16 string the caller owns once a function has written it, with a terminating 0 unit (`len` does not count
/// it), and frees with `cre_string16_free`.
#[repr(C)]
pub struct cre_string16 {
    pub ptr: *mut u16,
    pub len: usize,
}

/// The pieces of a split, which the caller frees with `cre_pieces_free`. `items[i].ptr` is NULL for a group that did
/// not take part (`undefined` in the array `split` returns); a piece that is present and empty has a non-NULL `ptr`
/// and a `len` of 0.
#[repr(C)]
pub struct cre_pieces {
    pub items: *const cre_str,
    pub len: usize,
    pub internal: *mut c_void,
}

/// The pieces of a split of UTF-16 text, which the caller frees with `cre_pieces16_free`; as `cre_pieces`.
#[repr(C)]
pub struct cre_pieces16 {
    pub items: *const cre_str16,
    pub len: usize,
    pub internal: *mut c_void,
}

/// Where a match or a group is, in the units of the text (bytes of UTF-8, or UTF-16 code units), start to end. A
/// group that did not take part is `CRE_UNSET` to `CRE_UNSET`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct cre_match {
    pub start: usize,
    pub end: usize,
}

impl From<Option<(usize, usize)>> for cre_match {
    fn from(span: Option<(usize, usize)>) -> cre_match {
        cre_match::of(span)
    }
}

impl From<(usize, usize)> for cre_match {
    fn from((start, end): (usize, usize)) -> cre_match {
        cre_match { start, end }
    }
}

impl cre_match {
    const UNSET: cre_match = cre_match { start: CRE_UNSET, end: CRE_UNSET };

    fn of(span: Option<(usize, usize)>) -> cre_match {
        span.map_or(cre_match::UNSET, |(start, end)| cre_match { start, end })
    }
}

/// A compiled regular expression. Immutable: any number of threads may use one at once.
pub struct cre_regex {
    inner: Regex,
    flags: String,
    names: Arc<[String]>,
}

/// What a match captured, by number and by name: made for one regular expression, and reused from match to match.
pub struct cre_captures {
    regex: *const cre_regex,
    names: Arc<[String]>,
    matched: bool,
    groups: Vec<Option<(usize, usize)>>,
    named: Vec<Option<(usize, usize)>>,
}

impl cre_captures {
    fn fill<H: Haystack + ?Sized>(&mut self, caps: Option<&Captures<'_, H>>) {
        self.groups.clear();
        self.named.clear();
        self.matched = caps.is_some();
        if let Some(caps) = caps {
            self.groups.extend(caps.iter().map(|m| m.map(|m| (m.start(), m.end()))));
            self.named.extend(self.names.iter().map(|n| caps.name(n).map(|m| (m.start(), m.end()))));
        }
    }
}

/// The global search of one text: the matches as `matchAll` finds them, one at a time.
pub struct cre_matches {
    regex: *const cre_regex,
    text: Text,
    iter: Iter,
}

/// The text of a global search, which the caller keeps alive and unchanged while the search lives.
#[derive(Clone, Copy)]
enum Text {
    Utf8(*const u8, usize),
    Utf16(*const u16, usize),
}

enum Iter {
    Find8(Matches<'static, 'static, str>),
    Find16(Matches<'static, 'static, [u16]>),
    Captures8(CaptureMatches<'static, 'static, str>),
    Captures16(CaptureMatches<'static, 'static, [u16]>),
}

impl cre_matches {
    /// The iterator from the start of the text. The regular expression and the text are the caller's, which the
    /// header says must outlive the search; the lifetimes are unbounded on that contract.
    unsafe fn start(regex: *const cre_regex, text: Text, captures: bool) -> Iter {
        // SAFETY: a handle this library created that the caller keeps alive while the search lives.
        let re: &'static Regex = unsafe { &(*regex).inner };
        match text {
            Text::Utf8(ptr, len) => {
                // SAFETY: the text was checked to be UTF-8 when the search was made, and the caller keeps it.
                let s: &'static str = unsafe { std::str::from_utf8_unchecked(raw_slice(ptr, len)) };
                if captures { Iter::Captures8(re.captures_iter(s)) } else { Iter::Find8(re.find_iter(s)) }
            }
            Text::Utf16(ptr, len) => {
                // SAFETY: the caller passes `len` readable units at `ptr` and keeps them.
                let s: &'static [u16] = unsafe { raw_slice(ptr, len) };
                if captures { Iter::Captures16(re.captures_iter_utf16(s)) } else { Iter::Find16(re.find_iter_utf16(s)) }
            }
        }
    }

    fn captures(&self) -> bool {
        matches!(self.iter, Iter::Captures8(_) | Iter::Captures16(_))
    }

    /// The failure of a match of the search that reached the pattern's timeout.
    fn limited(&self, e: MatchError) -> Failure {
        // SAFETY: a handle this library created that the caller keeps alive while the search lives.
        limited(e, unsafe { &(*self.regex).inner }, 0)
    }
}

/// A slice of `len` items at `ptr`, which may be dangling when `len` is 0.
unsafe fn raw_slice<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
    if len == 0 {
        &[]
    } else {
        // SAFETY: the caller passes `len` readable items at `ptr`.
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Errors

struct LastError {
    message: String,
    offset: usize,
}

thread_local! {
    static LAST_ERROR: RefCell<LastError> = const { RefCell::new(LastError { message: String::new(), offset: 0 }) };
    /// The thread's place to read groups into for `cre_regex_captures_read`, which grows once and is reused.
    static LOCATIONS: RefCell<Option<CaptureLocations>> = const { RefCell::new(None) };
}

struct Failure {
    status: cre_status,
    message: String,
    offset: usize,
}

impl Failure {
    fn new(status: cre_status, message: impl Into<String>) -> Failure {
        Failure { status, message: message.into(), offset: 0 }
    }

    fn argument(message: impl Into<String>) -> Failure {
        Failure::new(CRE_INVALID_ARGUMENT, message)
    }
}

fn set_last_error(message: String, offset: usize) {
    LAST_ERROR.with(|e| {
        let mut e = e.borrow_mut();
        e.message = message;
        e.offset = offset;
    });
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    let what = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown".into());
    format!("internal error (please report it): {what}")
}

/// Runs a call: a failure or a panic becomes its status, with the details stored for `cre_last_error_message`.
fn guard(f: impl FnOnce() -> Result<(), Failure>) -> cre_status {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => CRE_OK,
        Ok(Err(failure)) => {
            set_last_error(failure.message, failure.offset);
            failure.status
        }
        Err(payload) => {
            set_last_error(panic_message(&*payload), 0);
            CRE_PANIC
        }
    }
}

/// Runs a call that has no status to return: a panic gives `fallback`, and is stored for `cre_last_error_message`.
fn quietly<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        set_last_error(panic_message(&*payload), 0);
        fallback
    })
}

/// A UTF-8 argument: `len` bytes at `ptr` (which may be NULL when `len` is 0).
unsafe fn text<'a>(ptr: *const c_char, len: usize, what: &str) -> Result<&'a str, Failure> {
    if len == 0 {
        return Ok("");
    }
    if ptr.is_null() {
        return Err(Failure::argument(format!("{what} is NULL with a length of {len}")));
    }
    // SAFETY: the caller passes `len` readable bytes at `ptr`.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    // simdutf8 checks a large text in about half the time std does; std then says where a text that fails stops being
    // UTF-8.
    if let Ok(s) = simdutf8::basic::from_utf8(bytes) {
        return Ok(s);
    }
    std::str::from_utf8(bytes).map_err(|e| Failure {
        status: CRE_INVALID_UTF8,
        message: format!("{what} is not UTF-8 at byte {}", e.valid_up_to()),
        offset: e.valid_up_to(),
    })
}

/// How a function takes the UTF-8 text it searches.
#[derive(Clone, Copy)]
enum Utf8 {
    /// Checked to be UTF-8 on each call (`CRE_INVALID_UTF8` when it is not): the functions without a suffix.
    Checked,
    /// Trusted to be UTF-8, not checked: the `_unchecked` functions, for a caller whose strings are UTF-8 by
    /// construction (a Swift `String`, a Rust `str`), so that a large text is not read once more on every call. Text
    /// that is not UTF-8 is undefined behaviour.
    Unchecked,
}

/// The UTF-8 text a function searches: `len` bytes at `ptr` (which may be NULL when `len` is 0), taken as `mode` says.
unsafe fn haystack<'a>(ptr: *const c_char, len: usize, mode: Utf8) -> Result<&'a str, Failure> {
    match mode {
        Utf8::Checked => unsafe { text(ptr, len, "text") },
        Utf8::Unchecked => {
            if len == 0 {
                return Ok("");
            }
            if ptr.is_null() {
                return Err(Failure::argument(format!("text is NULL with a length of {len}")));
            }
            // SAFETY: the caller passes `len` readable bytes at `ptr`, and promises they are UTF-8.
            Ok(unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr.cast::<u8>(), len)) })
        }
    }
}

/// A UTF-16 argument: `len` code units at `ptr` (which may be NULL when `len` is 0), lone surrogates and all.
unsafe fn units<'a>(ptr: *const u16, len: usize, what: &str) -> Result<&'a [u16], Failure> {
    if len == 0 {
        return Ok(&[]);
    }
    if ptr.is_null() {
        return Err(Failure::argument(format!("{what} is NULL with a length of {len}")));
    }
    if !ptr.is_aligned() {
        return Err(Failure::argument(format!("{what} is not aligned for uint16_t")));
    }
    // SAFETY: the caller passes `len` readable units at `ptr`.
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}

unsafe fn handle<'a, T>(ptr: *const T, what: &str) -> Result<&'a T, Failure> {
    // SAFETY: a non-NULL handle is one this library created and the caller has not freed.
    unsafe { ptr.as_ref() }.ok_or_else(|| Failure::argument(format!("{what} is NULL")))
}

unsafe fn handle_mut<'a, T>(ptr: *mut T, what: &str) -> Result<&'a mut T, Failure> {
    // SAFETY: a non-NULL handle is one this library created and the caller has not freed, used by one thread.
    unsafe { ptr.as_mut() }.ok_or_else(|| Failure::argument(format!("{what} is NULL")))
}

unsafe fn out<'a, T>(ptr: *mut T, what: &str) -> Result<&'a mut T, Failure> {
    // SAFETY: a non-NULL out-parameter points to writable storage for a T.
    unsafe { ptr.as_mut() }.ok_or_else(|| Failure::argument(format!("{what} is NULL")))
}

/// A buffer of groups the caller owns: `len` entries at `ptr`, which may be NULL when `len` is 0.
unsafe fn groups_out<'a>(ptr: *mut cre_match, len: usize) -> Result<&'a mut [cre_match], Failure> {
    if len == 0 {
        return Ok(&mut []);
    }
    if ptr.is_null() {
        return Err(Failure::argument(format!("groups is NULL with a length of {len}")));
    }
    // SAFETY: the caller passes `len` writable entries at `ptr`.
    let groups = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
    groups.fill(cre_match::UNSET);
    Ok(groups)
}

/// The failure of a match that reached its limit: `limit_ms`, or with 0 the pattern's timeout.
fn limited(e: MatchError, re: &Regex, limit_ms: u64) -> Failure {
    let ms = if limit_ms == 0 { re.match_timeout().map_or(0, |t| t.as_millis()) } else { u128::from(limit_ms) };
    Failure::new(CRE_MATCH_LIMIT, format!("{e} ({ms} ms)"))
}

/// A match with a limit of `limit_ms` milliseconds, or with 0 the pattern's timeout: the member `$plain` with the
/// arguments, or `$within` with them and the limit.
macro_rules! within {
    ($re:expr, $plain:ident, $within:ident, $ms:expr, $($arg:expr),+) => {{
        let re: &Regex = &$re;
        let found = if $ms == 0 { re.$plain($($arg),+) } else { re.$within($($arg),+, Duration::from_millis($ms)) };
        found.map_err(|e| limited(e, re, $ms))
    }};
}

/// A start position: within the text, and over UTF-8 on a character boundary or, without the u or v flag, between
/// the halves of a character beyond the Basic Multilingual Plane (the offset of its third byte).
fn start_in(re: &cre_regex, text: &str, start: usize) -> Result<(), Failure> {
    if start > text.len() {
        return Err(Failure::argument(format!("start {start} is beyond the text's {} bytes", text.len())));
    }
    let flags = re.inner.flags();
    let between_halves = !(flags.unicode || flags.unicode_sets)
        && start >= 2
        && text.is_char_boundary(start - 2)
        && text.as_bytes()[start - 2] >= 0xF0;
    if text.is_char_boundary(start) || between_halves {
        Ok(())
    } else {
        Err(Failure::argument(format!("start {start} is inside a character")))
    }
}

fn start_in16(text: &[u16], start: usize) -> Result<(), Failure> {
    if start > text.len() {
        return Err(Failure::argument(format!("start {start} is beyond the text's {} code units", text.len())));
    }
    Ok(())
}

/// The captures handle, checked to be one made for the regular expression.
unsafe fn captures_for<'a>(
    captures: *mut cre_captures,
    regex: *const cre_regex,
) -> Result<&'a mut cre_captures, Failure> {
    let caps = unsafe { handle_mut(captures, "captures") }?;
    if !std::ptr::eq(caps.regex, regex) {
        return Err(Failure::argument("captures was made for another regular expression"));
    }
    Ok(caps)
}

fn string_of(s: &[u8]) -> cre_string {
    let mut bytes = Vec::with_capacity(s.len() + 1);
    bytes.extend_from_slice(s);
    bytes.push(0);
    let len = s.len();
    let ptr = Box::into_raw(bytes.into_boxed_slice()).cast::<c_char>();
    cre_string { ptr, len }
}

fn string16_of(s: &[u16]) -> cre_string16 {
    let mut units = Vec::with_capacity(s.len() + 1);
    units.extend_from_slice(s);
    units.push(0);
    let len = s.len();
    let ptr = Box::into_raw(units.into_boxed_slice()).cast::<u16>();
    cre_string16 { ptr, len }
}

/// What a split's pieces own: one buffer with every piece, and the views into it.
struct PiecesOwner<U, V> {
    _buffer: Vec<U>,
    items: Vec<V>,
}

fn pieces_of<U: Copy>(parts: &[Option<Cow<'_, [U]>>]) -> (Vec<U>, Vec<Option<(usize, usize)>>) {
    let mut buffer = Vec::with_capacity(parts.iter().flatten().map(|p| p.len()).sum::<usize>().max(1));
    let spans = parts
        .iter()
        .map(|p| {
            p.as_ref().map(|p| {
                let from = buffer.len();
                buffer.extend_from_slice(p);
                (from, p.len())
            })
        })
        .collect();
    (buffer, spans)
}

// ---------------------------------------------------------------------------------------------------------------------
// The library

/// The library's version, as "major.minor.patch". The string is static.
#[unsafe(no_mangle)]
pub extern "C" fn cre_version() -> cre_str {
    cre_str::of(env!("CARGO_PKG_VERSION"))
}

/// The name of a status, such as "CRE_INVALID_PATTERN", or "unknown". The string is static.
#[unsafe(no_mangle)]
pub extern "C" fn cre_status_name(status: cre_status) -> cre_str {
    cre_str::of(match status {
        CRE_OK => "CRE_OK",
        CRE_INVALID_ARGUMENT => "CRE_INVALID_ARGUMENT",
        CRE_INVALID_UTF8 => "CRE_INVALID_UTF8",
        CRE_INVALID_PATTERN => "CRE_INVALID_PATTERN",
        CRE_INVALID_FLAGS => "CRE_INVALID_FLAGS",
        CRE_PATTERN_TOO_LARGE => "CRE_PATTERN_TOO_LARGE",
        CRE_MATCH_LIMIT => "CRE_MATCH_LIMIT",
        CRE_PANIC => "CRE_PANIC",
        _ => "unknown",
    })
}

/// The message of the last call on this thread that failed. It stays valid until the next call that fails on this
/// thread.
#[unsafe(no_mangle)]
pub extern "C" fn cre_last_error_message() -> cre_str {
    LAST_ERROR.with(|e| {
        let e = e.borrow();
        cre_str { ptr: e.message.as_ptr().cast(), len: e.message.len() }
    })
}

/// The offset the last failure on this thread points at: the offset of the first byte that is not UTF-8, or 0.
#[unsafe(no_mangle)]
pub extern "C" fn cre_last_error_offset() -> usize {
    LAST_ERROR.with(|e| e.borrow().offset)
}

// ---------------------------------------------------------------------------------------------------------------------
// Compiling

/// Sets the match timeout of every regular expression compiled from now on by `cre_regex_new`, in milliseconds (0 for
/// none, the default): the time each match may take before it is abandoned with `CRE_MATCH_LIMIT`. A program that
/// matches patterns or texts from outside should set it when it starts (see `cre_regex_new_with_timeout`).
#[unsafe(no_mangle)]
pub extern "C" fn cre_set_default_match_timeout(timeout_ms: u64) {
    corvus::set_default_match_timeout(Some(Duration::from_millis(timeout_ms)));
}

/// The match timeout a regular expression compiled now by `cre_regex_new` takes, in milliseconds (0 for none).
#[unsafe(no_mangle)]
pub extern "C" fn cre_default_match_timeout() -> u64 {
    millis(corvus::default_match_timeout())
}

/// A timeout in whole milliseconds, 0 for none.
fn millis(timeout: Option<Duration>) -> u64 {
    timeout.map_or(0, |t| u64::try_from(t.as_millis()).unwrap_or(u64::MAX))
}

/// Compiles a pattern with the flags `new RegExp(pattern, flags)` takes (`flags` may be NULL with a length of 0 for
/// none), and the match timeout of `cre_default_match_timeout`. On success `*out_regex` is a handle to free with
/// `cre_regex_free`; on failure it is NULL and the status says why: `CRE_INVALID_PATTERN`, `CRE_INVALID_FLAGS`,
/// `CRE_PATTERN_TOO_LARGE`, `CRE_INVALID_UTF8` or `CRE_INVALID_ARGUMENT`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_new(
    pattern: *const c_char,
    pattern_len: usize,
    flags: *const c_char,
    flags_len: usize,
    out_regex: *mut *mut cre_regex,
) -> cre_status {
    let timeout_ms = cre_default_match_timeout();
    unsafe { cre_regex_new_with_timeout(pattern, pattern_len, flags, flags_len, timeout_ms, out_regex) }
}

/// `cre_regex_new` with a match timeout in milliseconds (0 for none): the time each match may take before it is
/// abandoned with `CRE_MATCH_LIMIT`, which brings a cap on the memory of its backtracking with it. Every function that
/// matches gives its match this timeout, except where a `_limit` function is given a limit of its own. Without one a
/// match has no limit of time or of memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_new_with_timeout(
    pattern: *const c_char,
    pattern_len: usize,
    flags: *const c_char,
    flags_len: usize,
    timeout_ms: u64,
    out_regex: *mut *mut cre_regex,
) -> cre_status {
    unsafe {
        cre_regex_new_with_options(
            pattern,
            pattern_len,
            flags,
            flags_len,
            timeout_ms,
            CRE_EDITION_CURRENT,
            CRE_UNICODE_DEFAULT,
            out_regex,
        )
    }
}

/// `cre_regex_new_with_timeout` with the edition of ECMA-262 whose grammar the pattern is read with (a `CRE_EDITION_`
/// value) and the version of Unicode whose tables it is read with (a `CRE_UNICODE_` value), each independent of the
/// other. Under `CRE_EDITION_ES2020` the d and v flags are `CRE_INVALID_FLAGS`, and a modifier group, a group name
/// used twice or a property name outside the tables of ECMAScript 2020 is `CRE_INVALID_PATTERN`. A value that is
/// not one of the constants is `CRE_INVALID_ARGUMENT`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn cre_regex_new_with_options(
    pattern: *const c_char,
    pattern_len: usize,
    flags: *const c_char,
    flags_len: usize,
    timeout_ms: u64,
    edition: u32,
    unicode_version: u32,
    out_regex: *mut *mut cre_regex,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_regex, "out_regex") }?;
        *slot = std::ptr::null_mut();
        let pattern = unsafe { text(pattern, pattern_len, "pattern") }?;
        let flags = unsafe { text(flags, flags_len, "flags") }?;
        let flags = Flags::parse(flags).map_err(|e| Failure::new(CRE_INVALID_FLAGS, e.to_string()))?;
        let (edition, unicode_version) = (edition_of(edition)?, unicode_version_of(unicode_version)?);
        let timeout = Some(Duration::from_millis(timeout_ms));
        let regex = RegexBuilder::new(pattern)
            .with_flags(flags)
            .match_timeout(timeout)
            .edition(edition)
            .unicode_version(unicode_version)
            .build()
            .map_err(|e| match &e {
                Error::Invalid(m) => Failure::new(CRE_INVALID_PATTERN, m.clone()),
                Error::Flags(m) => Failure::new(CRE_INVALID_FLAGS, m.clone()),
                Error::TooLarge(_) => Failure::new(CRE_PATTERN_TOO_LARGE, e.to_string()),
                _ => Failure::new(CRE_INVALID_PATTERN, e.to_string()),
            })?;
        let flags = regex.flags().to_string();
        let names = regex.group_names().map(String::from).collect();
        *slot = Box::into_raw(Box::new(cre_regex { inner: regex, flags, names }));
        Ok(())
    })
}

/// The match timeout of the regular expression, in milliseconds (0 for none).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_match_timeout(regex: *const cre_regex) -> u64 {
    // SAFETY: a handle this library created, or NULL.
    quietly(0, || unsafe { regex.as_ref() }.map_or(0, |r| millis(r.inner.match_timeout())))
}

/// Frees a regular expression. NULL is allowed and does nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_free(regex: *mut cre_regex) {
    if !regex.is_null() {
        // SAFETY: a handle this library created, freed once.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(regex) })));
    }
}

/// Whether a pattern is valid with the flags, without compiling it: `*out_valid`. A pattern over a limit of size is
/// not valid here. The status is `CRE_INVALID_UTF8` or `CRE_INVALID_ARGUMENT` for arguments that are not text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_valid(
    pattern: *const c_char,
    pattern_len: usize,
    flags: *const c_char,
    flags_len: usize,
    out_valid: *mut bool,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_valid, "out_valid") }?;
        *slot = false;
        let pattern = unsafe { text(pattern, pattern_len, "pattern") }?;
        let flags = unsafe { text(flags, flags_len, "flags") }?;
        *slot = Regex::is_valid(pattern, flags);
        Ok(())
    })
}

/// `cre_regex_is_valid` in the grammar of an edition (a `CRE_EDITION_` value) and under a version of Unicode (a
/// `CRE_UNICODE_` value). A value that is not one of the constants is `CRE_INVALID_ARGUMENT`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_valid_with_options(
    pattern: *const c_char,
    pattern_len: usize,
    flags: *const c_char,
    flags_len: usize,
    edition: u32,
    unicode_version: u32,
    out_valid: *mut bool,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_valid, "out_valid") }?;
        *slot = false;
        let pattern = unsafe { text(pattern, pattern_len, "pattern") }?;
        let flags = unsafe { text(flags, flags_len, "flags") }?;
        let (edition, unicode_version) = (edition_of(edition)?, unicode_version_of(unicode_version)?);
        *slot = Flags::parse(flags).is_ok_and(|f| Regex::is_valid_with(pattern, f, edition, unicode_version));
        Ok(())
    })
}

/// The edition the regular expression was read with, a `CRE_EDITION_` value (`CRE_EDITION_CURRENT` for NULL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_edition(regex: *const cre_regex) -> u32 {
    // SAFETY: a handle this library created, or NULL.
    quietly(CRE_EDITION_CURRENT, || {
        unsafe { regex.as_ref() }.map_or(CRE_EDITION_CURRENT, |r| match r.inner.edition() {
            Edition::Es2020 => CRE_EDITION_ES2020,
            _ => CRE_EDITION_CURRENT,
        })
    })
}

/// The version of Unicode the regular expression was read with, resolved: 13, 17 or 18 (0 for NULL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_unicode_version(regex: *const cre_regex) -> u32 {
    // SAFETY: a handle this library created, or NULL.
    quietly(0, || unsafe { regex.as_ref() }.map_or(0, |r| r.inner.unicode_version().major()))
}

/// The source of the pattern, as it was given. It stays valid as long as the regular expression ({NULL, 0} for
/// NULL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_source(regex: *const cre_regex) -> cre_str {
    // SAFETY: a handle this library created, or NULL.
    unsafe { regex.as_ref() }.map_or(cre_str::NULL, |r| cre_str::of(r.inner.as_str()))
}

/// The flags as `RegExp.prototype.flags` writes them, in the order `dgimsuvy`. It stays valid as long as the regular
/// expression ({NULL, 0} for NULL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_flags(regex: *const cre_regex) -> cre_str {
    // SAFETY: a handle this library created, or NULL.
    unsafe { regex.as_ref() }.map_or(cre_str::NULL, |r| cre_str::of(&r.flags))
}

/// The number of capturing groups of the pattern (0 for NULL). A match has one more group: the whole match, group 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_group_count(regex: *const cre_regex) -> usize {
    // SAFETY: a handle this library created, or NULL.
    unsafe { regex.as_ref() }.map_or(0, |r| r.inner.captures_len() - 1)
}

/// The number of distinct names of the pattern's groups (0 for NULL). Two groups in separate alternatives may share a
/// name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_group_name_count(regex: *const cre_regex) -> usize {
    // SAFETY: a handle this library created, or NULL.
    unsafe { regex.as_ref() }.map_or(0, |r| r.names.len())
}

/// The name `index` (from 0), in the order of the first group of each name, or {NULL, 0} when there is none. It stays
/// valid as long as the regular expression.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_group_name(regex: *const cre_regex, index: usize) -> cre_str {
    // SAFETY: a handle this library created, or NULL.
    unsafe { regex.as_ref() }.and_then(|r| r.names.get(index)).map_or(cre_str::NULL, |n| cre_str::of(n))
}

// ---------------------------------------------------------------------------------------------------------------------
// Matching UTF-8

/// Whether `text_len` bytes at `text_ptr` are UTF-8: `CRE_OK`, or `CRE_INVALID_UTF8` with `cre_last_error_offset` at
/// the first byte that is not. A caller whose strings carry no such guarantee checks a text once, when it makes it,
/// and then searches it with the `_unchecked` functions as often as it likes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_utf8_check(text_ptr: *const c_char, text_len: usize) -> cre_status {
    guard(|| unsafe { text(text_ptr, text_len, "text") }.map(|_| ()))
}

/// Whether the pattern matches anywhere in the UTF-8 text (at its start alone with the y flag): `*out_matched`. With
/// the regular expression's match timeout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    out_matched: *mut bool,
) -> cre_status {
    unsafe { cre_regex_is_match_limit(regex, text_ptr, text_len, 0, out_matched) }
}

/// `cre_regex_is_match` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    out_matched: *mut bool,
) -> cre_status {
    unsafe { cre_regex_is_match_limit_unchecked(regex, text_ptr, text_len, 0, out_matched) }
}

/// `cre_regex_is_match` with a limit on the time the match may take, in milliseconds (0 for the regular expression's
/// match timeout). A match that reaches it, or whose backtracking memory reaches its cap, is `CRE_MATCH_LIMIT`, and
/// `*out_matched` is false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match_limit(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    limit_ms: u64,
    out_matched: *mut bool,
) -> cre_status {
    unsafe { is_match_limit8(regex, text_ptr, text_len, limit_ms, out_matched, Utf8::Checked) }
}

/// `cre_regex_is_match_limit` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match_limit_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    limit_ms: u64,
    out_matched: *mut bool,
) -> cre_status {
    unsafe { is_match_limit8(regex, text_ptr, text_len, limit_ms, out_matched, Utf8::Unchecked) }
}

unsafe fn is_match_limit8(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    limit_ms: u64,
    out_matched: *mut bool,
    mode: Utf8,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_matched, "out_matched") }?;
        *slot = false;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { haystack(text_ptr, text_len, mode) }?;
        *slot = within!(regex.inner, is_match, is_match_within, limit_ms, text)?;
        Ok(())
    })
}

/// Whether the pattern matches at or after the byte offset `start` (at `start` alone with the y flag), as
/// `RegExp.prototype.test` with `lastIndex` set to it: `*out_matched`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match_at(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    out_matched: *mut bool,
) -> cre_status {
    unsafe { is_match_at8(regex, text_ptr, text_len, start, out_matched, Utf8::Checked) }
}

/// `cre_regex_is_match_at` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match_at_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    out_matched: *mut bool,
) -> cre_status {
    unsafe { is_match_at8(regex, text_ptr, text_len, start, out_matched, Utf8::Unchecked) }
}

unsafe fn is_match_at8(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    out_matched: *mut bool,
    mode: Utf8,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_matched, "out_matched") }?;
        *slot = false;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { haystack(text_ptr, text_len, mode) }?;
        start_in(regex, text, start)?;
        *slot = regex.inner.is_match_at(text, start).map_err(|e| limited(e, &regex.inner, 0))?;
        Ok(())
    })
}

/// The first match at or after the byte offset `start` of the UTF-8 text, as `exec` finds it with `lastIndex` set to
/// `start`: `*out_found` says whether there is one, and `*out_match` is where it is (`CRE_UNSET` when there is none).
/// Without the u or v flag a match can start or end between the halves of a character beyond the Basic Multilingual
/// Plane; such an offset is that of the character's third byte, and `start` may be one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    out_match: *mut cre_match,
    out_found: *mut bool,
) -> cre_status {
    unsafe { cre_regex_find_limit(regex, text_ptr, text_len, start, 0, out_match, out_found) }
}

/// `cre_regex_find` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    out_match: *mut cre_match,
    out_found: *mut bool,
) -> cre_status {
    unsafe { cre_regex_find_limit_unchecked(regex, text_ptr, text_len, start, 0, out_match, out_found) }
}

/// `cre_regex_find` with a limit on the time the match may take, in milliseconds (0 for the regular expression's match
/// timeout): `CRE_MATCH_LIMIT` when it is reached.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find_limit(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    out_match: *mut cre_match,
    out_found: *mut bool,
) -> cre_status {
    unsafe { find_limit8(regex, text_ptr, text_len, start, limit_ms, out_match, out_found, Utf8::Checked) }
}

/// `cre_regex_find_limit` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find_limit_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    out_match: *mut cre_match,
    out_found: *mut bool,
) -> cre_status {
    unsafe { find_limit8(regex, text_ptr, text_len, start, limit_ms, out_match, out_found, Utf8::Unchecked) }
}

// The C function's arguments and how it takes its text.
#[allow(clippy::too_many_arguments)]
unsafe fn find_limit8(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    out_match: *mut cre_match,
    out_found: *mut bool,
    mode: Utf8,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let place = unsafe { out(out_match, "out_match") }?;
        *place = cre_match::UNSET;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { haystack(text_ptr, text_len, mode) }?;
        start_in(regex, text, start)?;
        if let Some(m) = within!(regex.inner, find_at, find_at_within, limit_ms, text, start)? {
            *place = cre_match { start: m.start(), end: m.end() };
            *found = true;
        }
        Ok(())
    })
}

/// The first match at or after `start` with what each group captured, read into `groups`, a buffer of `groups_len`
/// entries the caller owns: group 0 is the match, then each capturing group, `CRE_UNSET` for one that did not take
/// part. A buffer shorter than `cre_regex_group_count() + 1` takes the first groups; entries past the last group are
/// `CRE_UNSET`. Nothing is allocated once the thread has read a match with as many groups.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_read(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    groups: *mut cre_match,
    groups_len: usize,
    out_found: *mut bool,
) -> cre_status {
    unsafe { captures_read8(regex, text_ptr, text_len, start, groups, groups_len, out_found, Utf8::Checked) }
}

/// `cre_regex_captures_read` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_read_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    groups: *mut cre_match,
    groups_len: usize,
    out_found: *mut bool,
) -> cre_status {
    unsafe { captures_read8(regex, text_ptr, text_len, start, groups, groups_len, out_found, Utf8::Unchecked) }
}

// The C function's arguments and how it takes its text.
#[allow(clippy::too_many_arguments)]
unsafe fn captures_read8(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    groups: *mut cre_match,
    groups_len: usize,
    out_found: *mut bool,
    mode: Utf8,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let groups = unsafe { groups_out(groups, groups_len) }?;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { haystack(text_ptr, text_len, mode) }?;
        start_in(regex, text, start)?;
        LOCATIONS.with(|cell| {
            let mut cell = cell.borrow_mut();
            let locs = cell.get_or_insert_with(|| regex.inner.capture_locations());
            let got = regex.inner.captures_read_at(locs, text, start).map_err(|e| limited(e, &regex.inner, 0))?;
            if got.is_some() {
                *found = true;
                for (k, g) in groups.iter_mut().enumerate() {
                    *g = cre_match::of(locs.get(k));
                }
            }
            Ok(())
        })
    })
}

/// `cre_regex_captures_read` with a limit on the time the match may take, in milliseconds (0 for the regular
/// expression's match timeout): `CRE_MATCH_LIMIT` when it is reached. It allocates for each match.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_read_limit(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    groups: *mut cre_match,
    groups_len: usize,
    out_found: *mut bool,
) -> cre_status {
    unsafe {
        captures_read_limit8(regex, text_ptr, text_len, start, limit_ms, groups, groups_len, out_found, Utf8::Checked)
    }
}

/// `cre_regex_captures_read_limit` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_read_limit_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    groups: *mut cre_match,
    groups_len: usize,
    out_found: *mut bool,
) -> cre_status {
    unsafe {
        captures_read_limit8(regex, text_ptr, text_len, start, limit_ms, groups, groups_len, out_found, Utf8::Unchecked)
    }
}

// The C function's arguments and how it takes its text.
#[allow(clippy::too_many_arguments)]
unsafe fn captures_read_limit8(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    groups: *mut cre_match,
    groups_len: usize,
    out_found: *mut bool,
    mode: Utf8,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let groups = unsafe { groups_out(groups, groups_len) }?;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { haystack(text_ptr, text_len, mode) }?;
        start_in(regex, text, start)?;
        if let Some(caps) = within!(regex.inner, captures_at, captures_at_within, limit_ms, text, start)? {
            *found = true;
            for (g, m) in groups.iter_mut().zip(caps.iter()) {
                *g = cre_match::of(m.map(|m| (m.start(), m.end())));
            }
        }
        Ok(())
    })
}

/// The first match at or after `start` with what each group captured, by number and by name, written into
/// `captures` (made by `cre_captures_new` for this regular expression): `*out_found`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    captures: *mut cre_captures,
    out_found: *mut bool,
) -> cre_status {
    unsafe { cre_regex_captures_limit(regex, text_ptr, text_len, start, 0, captures, out_found) }
}

/// `cre_regex_captures` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    captures: *mut cre_captures,
    out_found: *mut bool,
) -> cre_status {
    unsafe { cre_regex_captures_limit_unchecked(regex, text_ptr, text_len, start, 0, captures, out_found) }
}

/// `cre_regex_captures` with a limit on the time the match may take, in milliseconds (0 for the regular expression's
/// match timeout): `CRE_MATCH_LIMIT` when it is reached.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_limit(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    captures: *mut cre_captures,
    out_found: *mut bool,
) -> cre_status {
    unsafe { captures_limit8(regex, text_ptr, text_len, start, limit_ms, captures, out_found, Utf8::Checked) }
}

/// `cre_regex_captures_limit` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_limit_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    captures: *mut cre_captures,
    out_found: *mut bool,
) -> cre_status {
    unsafe { captures_limit8(regex, text_ptr, text_len, start, limit_ms, captures, out_found, Utf8::Unchecked) }
}

// The C function's arguments and how it takes its text.
#[allow(clippy::too_many_arguments)]
unsafe fn captures_limit8(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    captures: *mut cre_captures,
    out_found: *mut bool,
    mode: Utf8,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let caps = unsafe { captures_for(captures, regex) }?;
        caps.fill::<str>(None);
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { haystack(text_ptr, text_len, mode) }?;
        start_in(regex, text, start)?;
        let got = within!(regex.inner, captures_at, captures_at_within, limit_ms, text, start)?;
        caps.fill(got.as_ref());
        *found = got.is_some();
        Ok(())
    })
}

/// A global search of the UTF-8 text, as `matchAll` makes one: `cre_matches_next` gives each match, and after an
/// empty match the search goes on one character later. The text is checked to be UTF-8 here, once. The regular
/// expression and the text must stay alive and unchanged until `cre_matches_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find_iter(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    out_matches: *mut *mut cre_matches,
) -> cre_status {
    unsafe { new_matches8(regex, text_ptr, text_len, false, out_matches, Utf8::Checked) }
}

/// `cre_regex_find_iter` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find_iter_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    out_matches: *mut *mut cre_matches,
) -> cre_status {
    unsafe { new_matches8(regex, text_ptr, text_len, false, out_matches, Utf8::Unchecked) }
}

/// `cre_regex_find_iter` with the groups of each match, for `cre_matches_next_captures`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_iter(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    out_matches: *mut *mut cre_matches,
) -> cre_status {
    unsafe { new_matches8(regex, text_ptr, text_len, true, out_matches, Utf8::Checked) }
}

/// `cre_regex_captures_iter` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_iter_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    out_matches: *mut *mut cre_matches,
) -> cre_status {
    unsafe { new_matches8(regex, text_ptr, text_len, true, out_matches, Utf8::Unchecked) }
}

unsafe fn new_matches8(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    captures: bool,
    out_matches: *mut *mut cre_matches,
    mode: Utf8,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_matches, "out_matches") }?;
        *slot = std::ptr::null_mut();
        unsafe { handle(regex, "regex") }?;
        let s = unsafe { haystack(text_ptr, text_len, mode) }?;
        let text = Text::Utf8(s.as_ptr(), s.len());
        let iter = unsafe { cre_matches::start(regex, text, captures) };
        *slot = Box::into_raw(Box::new(cre_matches { regex, text, iter }));
        Ok(())
    })
}

/// The UTF-8 text with the first match replaced, or every match with the g flag, as `String.prototype.replace`
/// replaces them. The replacement is a template: `$&` is the match, `` $` `` the text before it, `$'` the text after
/// it, `$1` to `$99` a group, `$<name>` a named group, and `$$` a dollar sign; anything else is itself
/// (`GetSubstitution`). On success `*out_result` is a string to free with `cre_string_free`; on failure it is {NULL,
/// 0}. A half character (an end between the halves of a character beyond the Basic Multilingual Plane, without u or
/// v) is written as U+FFFD.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_replace(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    replacement: *const c_char,
    replacement_len: usize,
    out_result: *mut cre_string,
) -> cre_status {
    unsafe { replace8(regex, text_ptr, text_len, replacement, replacement_len, false, out_result, Utf8::Checked) }
}

/// `cre_regex_replace` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_replace_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    replacement: *const c_char,
    replacement_len: usize,
    out_result: *mut cre_string,
) -> cre_status {
    unsafe { replace8(regex, text_ptr, text_len, replacement, replacement_len, false, out_result, Utf8::Unchecked) }
}

/// `cre_regex_replace` with every match replaced, whether or not the g flag is set (`String.prototype.replaceAll`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_replace_all(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    replacement: *const c_char,
    replacement_len: usize,
    out_result: *mut cre_string,
) -> cre_status {
    unsafe { replace8(regex, text_ptr, text_len, replacement, replacement_len, true, out_result, Utf8::Checked) }
}

/// `cre_regex_replace_all` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_replace_all_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    replacement: *const c_char,
    replacement_len: usize,
    out_result: *mut cre_string,
) -> cre_status {
    unsafe { replace8(regex, text_ptr, text_len, replacement, replacement_len, true, out_result, Utf8::Unchecked) }
}

// The C function's arguments and how it takes its text.
#[allow(clippy::too_many_arguments)]
unsafe fn replace8(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    replacement: *const c_char,
    replacement_len: usize,
    all: bool,
    out_result: *mut cre_string,
    mode: Utf8,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_result, "out_result") }?;
        *slot = cre_string { ptr: std::ptr::null_mut(), len: 0 };
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { haystack(text_ptr, text_len, mode) }?;
        let replacement = unsafe { self::text(replacement, replacement_len, "replacement") }?;
        let result =
            if all { regex.inner.replace_all(text, replacement) } else { regex.inner.replace(text, replacement) };
        let result = result.map_err(|e| limited(e, &regex.inner, 0))?;
        *slot = string_of(result.as_bytes());
        Ok(())
    })
}

/// The UTF-8 text split around the matches, as `String.prototype.split(separator, limit)` splits it: what each group
/// captured is spliced in after the piece before it (with a NULL `ptr` for a group that did not take part), an empty
/// match at the start or the end of the text splits nothing, and there are at most `limit` pieces (`CRE_SPLIT_ALL`
/// for every one). On success `*out_pieces` holds the pieces, to free with `cre_pieces_free`; on failure it is empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_split(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    limit: u32,
    out_pieces: *mut cre_pieces,
) -> cre_status {
    unsafe { split8(regex, text_ptr, text_len, limit, out_pieces, Utf8::Checked) }
}

/// `cre_regex_split` with the text trusted to be UTF-8, unchecked: see `Utf8::Unchecked`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_split_unchecked(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    limit: u32,
    out_pieces: *mut cre_pieces,
) -> cre_status {
    unsafe { split8(regex, text_ptr, text_len, limit, out_pieces, Utf8::Unchecked) }
}

unsafe fn split8(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    limit: u32,
    out_pieces: *mut cre_pieces,
    mode: Utf8,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_pieces, "out_pieces") }?;
        *slot = cre_pieces { items: std::ptr::null(), len: 0, internal: std::ptr::null_mut() };
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { haystack(text_ptr, text_len, mode) }?;
        let parts = regex.inner.splitn(text, limit as usize).map_err(|e| limited(e, &regex.inner, 0))?;
        let parts: Vec<Option<Cow<'_, [u8]>>> = parts
            .into_iter()
            .map(|p| {
                p.map(|p| match p {
                    Cow::Borrowed(s) => Cow::Borrowed(s.as_bytes()),
                    Cow::Owned(s) => Cow::Owned(s.into_bytes()),
                })
            })
            .collect();
        let (buffer, spans) = pieces_of(&parts);
        let items: Vec<cre_str> = spans
            .iter()
            .map(|s| {
                s.map_or(cre_str::NULL, |(from, len)| {
                    // SAFETY: within the buffer, or at its start (a non-NULL pointer) for an empty piece.
                    cre_str { ptr: unsafe { buffer.as_ptr().add(from) }.cast(), len }
                })
            })
            .collect();
        let owner = Box::new(PiecesOwner { _buffer: buffer, items });
        *slot =
            cre_pieces { items: owner.items.as_ptr(), len: owner.items.len(), internal: Box::into_raw(owner).cast() };
        Ok(())
    })
}

// ---------------------------------------------------------------------------------------------------------------------
// Matching UTF-16

/// `cre_regex_is_match` over UTF-16 text: `text_len` code units, which may hold lone surrogates.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    out_matched: *mut bool,
) -> cre_status {
    unsafe { cre_regex_is_match_limit_utf16(regex, text_ptr, text_len, 0, out_matched) }
}

/// `cre_regex_is_match_limit` over UTF-16 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match_limit_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    limit_ms: u64,
    out_matched: *mut bool,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_matched, "out_matched") }?;
        *slot = false;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { units(text_ptr, text_len, "text") }?;
        *slot = within!(regex.inner, is_match_utf16, is_match_within_utf16, limit_ms, text)?;
        Ok(())
    })
}

/// `cre_regex_is_match_at` over UTF-16 text, `start` a code unit index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match_at_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    start: usize,
    out_matched: *mut bool,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_matched, "out_matched") }?;
        *slot = false;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { units(text_ptr, text_len, "text") }?;
        start_in16(text, start)?;
        *slot = regex.inner.is_match_at_utf16(text, start).map_err(|e| limited(e, &regex.inner, 0))?;
        Ok(())
    })
}

/// `cre_regex_find` over UTF-16 text, positions in code units.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    start: usize,
    out_match: *mut cre_match,
    out_found: *mut bool,
) -> cre_status {
    unsafe { cre_regex_find_limit_utf16(regex, text_ptr, text_len, start, 0, out_match, out_found) }
}

/// `cre_regex_find_limit` over UTF-16 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find_limit_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    out_match: *mut cre_match,
    out_found: *mut bool,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let place = unsafe { out(out_match, "out_match") }?;
        *place = cre_match::UNSET;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { units(text_ptr, text_len, "text") }?;
        start_in16(text, start)?;
        if let Some(m) = within!(regex.inner, find_at_utf16, find_at_within_utf16, limit_ms, text, start)? {
            *place = cre_match { start: m.start(), end: m.end() };
            *found = true;
        }
        Ok(())
    })
}

/// `cre_regex_captures_read` over UTF-16 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_read_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    start: usize,
    groups: *mut cre_match,
    groups_len: usize,
    out_found: *mut bool,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let groups = unsafe { groups_out(groups, groups_len) }?;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { units(text_ptr, text_len, "text") }?;
        start_in16(text, start)?;
        LOCATIONS.with(|cell| {
            let mut cell = cell.borrow_mut();
            let locs = cell.get_or_insert_with(|| regex.inner.capture_locations());
            let got = regex.inner.captures_read_at_utf16(locs, text, start).map_err(|e| limited(e, &regex.inner, 0))?;
            if got.is_some() {
                *found = true;
                for (k, g) in groups.iter_mut().enumerate() {
                    *g = cre_match::of(locs.get(k));
                }
            }
            Ok(())
        })
    })
}

/// `cre_regex_captures_read_limit` over UTF-16 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_read_limit_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    groups: *mut cre_match,
    groups_len: usize,
    out_found: *mut bool,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let groups = unsafe { groups_out(groups, groups_len) }?;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { units(text_ptr, text_len, "text") }?;
        start_in16(text, start)?;
        if let Some(caps) = within!(regex.inner, captures_at_utf16, captures_at_within_utf16, limit_ms, text, start)? {
            *found = true;
            for (g, m) in groups.iter_mut().zip(caps.iter()) {
                *g = cre_match::of(m.map(|m| (m.start(), m.end())));
            }
        }
        Ok(())
    })
}

/// `cre_regex_captures` over UTF-16 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    start: usize,
    captures: *mut cre_captures,
    out_found: *mut bool,
) -> cre_status {
    unsafe { cre_regex_captures_limit_utf16(regex, text_ptr, text_len, start, 0, captures, out_found) }
}

/// `cre_regex_captures_limit` over UTF-16 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_limit_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    start: usize,
    limit_ms: u64,
    captures: *mut cre_captures,
    out_found: *mut bool,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let caps = unsafe { captures_for(captures, regex) }?;
        caps.fill::<[u16]>(None);
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { units(text_ptr, text_len, "text") }?;
        start_in16(text, start)?;
        let got = within!(regex.inner, captures_at_utf16, captures_at_within_utf16, limit_ms, text, start)?;
        caps.fill(got.as_ref());
        *found = got.is_some();
        Ok(())
    })
}

/// `cre_regex_find_iter` over UTF-16 text. The regular expression and the text must stay alive and unchanged until
/// `cre_matches_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find_iter_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    out_matches: *mut *mut cre_matches,
) -> cre_status {
    unsafe { new_matches16(regex, text_ptr, text_len, false, out_matches) }
}

/// `cre_regex_captures_iter` over UTF-16 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_captures_iter_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    out_matches: *mut *mut cre_matches,
) -> cre_status {
    unsafe { new_matches16(regex, text_ptr, text_len, true, out_matches) }
}

unsafe fn new_matches16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    captures: bool,
    out_matches: *mut *mut cre_matches,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_matches, "out_matches") }?;
        *slot = std::ptr::null_mut();
        unsafe { handle(regex, "regex") }?;
        let s = unsafe { units(text_ptr, text_len, "text") }?;
        let text = Text::Utf16(s.as_ptr(), s.len());
        let iter = unsafe { cre_matches::start(regex, text, captures) };
        *slot = Box::into_raw(Box::new(cre_matches { regex, text, iter }));
        Ok(())
    })
}

/// `cre_regex_replace` over UTF-16 text, with a UTF-16 replacement: `*out_result` is a string to free with
/// `cre_string16_free`. A lone surrogate is kept as it is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_replace_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    replacement: *const u16,
    replacement_len: usize,
    out_result: *mut cre_string16,
) -> cre_status {
    unsafe { replace16(regex, text_ptr, text_len, replacement, replacement_len, false, out_result) }
}

/// `cre_regex_replace_all` over UTF-16 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_replace_all_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    replacement: *const u16,
    replacement_len: usize,
    out_result: *mut cre_string16,
) -> cre_status {
    unsafe { replace16(regex, text_ptr, text_len, replacement, replacement_len, true, out_result) }
}

unsafe fn replace16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    replacement: *const u16,
    replacement_len: usize,
    all: bool,
    out_result: *mut cre_string16,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_result, "out_result") }?;
        *slot = cre_string16 { ptr: std::ptr::null_mut(), len: 0 };
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { units(text_ptr, text_len, "text") }?;
        let replacement = unsafe { units(replacement, replacement_len, "replacement") }?;
        let result = if all {
            regex.inner.replace_all_utf16(text, replacement)
        } else {
            regex.inner.replace_utf16(text, replacement)
        };
        let result = result.map_err(|e| limited(e, &regex.inner, 0))?;
        *slot = string16_of(&result);
        Ok(())
    })
}

/// `cre_regex_split` over UTF-16 text: `*out_pieces` holds the pieces, to free with `cre_pieces16_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_split_utf16(
    regex: *const cre_regex,
    text_ptr: *const u16,
    text_len: usize,
    limit: u32,
    out_pieces: *mut cre_pieces16,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_pieces, "out_pieces") }?;
        *slot = cre_pieces16 { items: std::ptr::null(), len: 0, internal: std::ptr::null_mut() };
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { units(text_ptr, text_len, "text") }?;
        let parts = regex.inner.splitn_utf16(text, limit as usize).map_err(|e| limited(e, &regex.inner, 0))?;
        let (buffer, spans) = pieces_of(&parts);
        let items: Vec<cre_str16> = spans
            .iter()
            .map(|s| match s {
                None => cre_str16 { ptr: std::ptr::null(), len: 0 },
                // SAFETY: within the buffer, or at its start (a non-NULL pointer) for an empty piece.
                Some((from, len)) => cre_str16 { ptr: unsafe { buffer.as_ptr().add(*from) }, len: *len },
            })
            .collect();
        let owner = Box::new(PiecesOwner { _buffer: buffer, items });
        *slot =
            cre_pieces16 { items: owner.items.as_ptr(), len: owner.items.len(), internal: Box::into_raw(owner).cast() };
        Ok(())
    })
}

// ---------------------------------------------------------------------------------------------------------------------
// Captures

/// A place for what a match of `regex` captures, for `cre_regex_captures` and `cre_matches_next_captures`, reused
/// from match to match. On success `*out_captures` is a handle to free with `cre_captures_free`; it holds no
/// reference to the regular expression or to any text, and is used by one thread at a time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_captures_new(regex: *const cre_regex, out_captures: *mut *mut cre_captures) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_captures, "out_captures") }?;
        *slot = std::ptr::null_mut();
        let re = unsafe { handle(regex, "regex") }?;
        let groups = re.inner.captures_len();
        *slot = Box::into_raw(Box::new(cre_captures {
            regex,
            names: re.names.clone(),
            matched: false,
            groups: Vec::with_capacity(groups),
            named: Vec::with_capacity(re.names.len()),
        }));
        Ok(())
    })
}

/// Frees a captures handle. NULL is allowed and does nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_captures_free(captures: *mut cre_captures) {
    if !captures.is_null() {
        // SAFETY: a handle this library created, freed once.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(captures) })));
    }
}

/// The number of groups of the last match, group 0 among them, or 0 when the last search found none (or for NULL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_captures_len(captures: *const cre_captures) -> usize {
    // SAFETY: a handle this library created, or NULL.
    unsafe { captures.as_ref() }.map_or(0, |c| c.groups.len())
}

/// The group `group` of the last match (0 is the whole match): true with `*out_match` where it is, or false with
/// `*out_match` `CRE_UNSET` when it did not take part, there is no such group, or the last search found no match.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_captures_get(
    captures: *const cre_captures,
    group: usize,
    out_match: *mut cre_match,
) -> bool {
    quietly(false, || {
        // SAFETY: a handle this library created, or NULL; an out-parameter, or NULL.
        let (Some(c), Some(slot)) = (unsafe { captures.as_ref() }, unsafe { out_match.as_mut() }) else {
            return false;
        };
        *slot = cre_match::of(c.groups.get(group).copied().flatten());
        slot.start != CRE_UNSET
    })
}

/// The group of the name that took part in the last match (of the groups that share it, the one that took part):
/// true with `*out_match` where it is, or false with `*out_match` `CRE_UNSET`. The name is UTF-8, `name_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_captures_name(
    captures: *const cre_captures,
    name: *const c_char,
    name_len: usize,
    out_match: *mut cre_match,
) -> bool {
    quietly(false, || {
        // SAFETY: a handle this library created, or NULL; an out-parameter, or NULL.
        let (Some(c), Some(slot)) = (unsafe { captures.as_ref() }, unsafe { out_match.as_mut() }) else {
            return false;
        };
        *slot = cre_match::UNSET;
        if name.is_null() && name_len > 0 {
            return false;
        }
        // SAFETY: the caller passes `name_len` readable bytes at `name`.
        let name = unsafe { raw_slice(name.cast::<u8>(), name_len) };
        let Some(k) = c.names.iter().position(|n| n.as_bytes() == name) else { return false };
        *slot = cre_match::of(c.named.get(k).copied().flatten());
        slot.start != CRE_UNSET
    })
}

// ---------------------------------------------------------------------------------------------------------------------
// The global search

/// The next match of a global search: `*out_found` says whether there is one, and `*out_match` is where it is
/// (`CRE_UNSET` when there is none). Once there is none, every later call finds none until `cre_matches_reset`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_matches_next(
    matches: *mut cre_matches,
    out_match: *mut cre_match,
    out_found: *mut bool,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let place = unsafe { out(out_match, "out_match") }?;
        *place = cre_match::UNSET;
        let m = unsafe { handle_mut(matches, "matches") }?;
        let span = match &mut m.iter {
            Iter::Find8(it) => it.next().map(|r| r.map(|m| (m.start(), m.end()))),
            Iter::Find16(it) => it.next().map(|r| r.map(|m| (m.start(), m.end()))),
            Iter::Captures8(it) => it.next().map(|r| r.map(|c| (c.get_match().start(), c.get_match().end()))),
            Iter::Captures16(it) => it.next().map(|r| r.map(|c| (c.get_match().start(), c.get_match().end()))),
        };
        let span = span.transpose().map_err(|e| m.limited(e))?;
        if let Some(span) = span {
            *place = cre_match::of(Some(span));
            *found = true;
        }
        Ok(())
    })
}

/// The next matches of a global search, up to `capacity` of them, written in order to `out_matches[0]` to
/// `out_matches[*out_count - 1]`: the matches `cre_matches_next` would give one call each, with one call for many.
/// `*out_count` is less than `capacity` only when the search has found every match, so a call that writes 0 ends it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_matches_next_batch(
    matches: *mut cre_matches,
    out_matches: *mut cre_match,
    capacity: usize,
    out_count: *mut usize,
) -> cre_status {
    guard(|| {
        let count = unsafe { out(out_count, "out_count") }?;
        *count = 0;
        let m = unsafe { handle_mut(matches, "matches") }?;
        if capacity == 0 {
            return Ok(());
        }
        if out_matches.is_null() {
            return Err(Failure::argument(format!("out_matches is NULL with a capacity of {capacity}")));
        }
        // SAFETY: the caller passes `capacity` writable entries at `out_matches`.
        let place = unsafe { std::slice::from_raw_parts_mut(out_matches, capacity) };
        let got = match &mut m.iter {
            Iter::Find8(it) => it.next_batch(place),
            Iter::Find16(it) => it.next_batch(place),
            Iter::Captures8(it) => fill(place, it.map(|c| c.map(|c| (c.get_match().start(), c.get_match().end())))),
            Iter::Captures16(it) => fill(place, it.map(|c| c.map(|c| (c.get_match().start(), c.get_match().end())))),
        };
        *count = got.map_err(|e| m.limited(e))?;
        Ok(())
    })
}

/// The next matches of a global search made by `cre_regex_captures_iter` (or its UTF-16 form) with their groups,
/// written in order to `out_groups`: each match as `cre_regex_group_count(regex) + 1` entries (the match, then each
/// group, `CRE_UNSET` for one that did not take part), as many matches as `capacity` entries hold, and
/// `*out_count` matches. `*out_count` is less than that only when the search has found every match. A capacity of 0
/// writes nothing (`out_groups` may then be NULL); a capacity that holds no match, or a search made by
/// `cre_regex_find_iter`, is `CRE_INVALID_ARGUMENT`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_matches_next_captures_batch(
    matches: *mut cre_matches,
    out_groups: *mut cre_match,
    capacity: usize,
    out_count: *mut usize,
) -> cre_status {
    guard(|| {
        let count = unsafe { out(out_count, "out_count") }?;
        *count = 0;
        let m = unsafe { handle_mut(matches, "matches") }?;
        if !m.captures() {
            return Err(Failure::argument("the search was made by cre_regex_find_iter, which keeps no groups"));
        }
        if capacity == 0 {
            return Ok(());
        }
        // SAFETY: a handle this library created that the caller keeps alive while the search lives.
        let width = unsafe { &*m.regex }.inner.captures_len();
        if capacity < width {
            return Err(Failure::argument(format!("a capacity of {capacity} holds no match of {width} groups")));
        }
        if out_groups.is_null() {
            return Err(Failure::argument(format!("out_groups is NULL with a capacity of {capacity}")));
        }
        // SAFETY: the caller passes `capacity` writable entries at `out_groups`.
        let place = unsafe { std::slice::from_raw_parts_mut(out_groups, capacity) };
        let got = match &mut m.iter {
            Iter::Captures8(it) => it.next_batch(place),
            Iter::Captures16(it) => it.next_batch(place),
            _ => unreachable!(),
        };
        *count = got.map_err(|e| m.limited(e))?;
        Ok(())
    })
}

/// Writes the spans `spans` gives into `place` in order, asking for no more than `place` holds: how many, or the
/// error of a match that reached its limit.
fn fill(
    place: &mut [cre_match],
    spans: impl Iterator<Item = Result<(usize, usize), MatchError>>,
) -> Result<usize, MatchError> {
    let mut n = 0;
    // `zip` asks `place` first, so no match is taken from the search once `place` is full.
    for (slot, span) in place.iter_mut().zip(spans) {
        let (start, end) = span?;
        *slot = cre_match { start, end };
        n += 1;
    }
    Ok(n)
}

/// The next match of a global search made by `cre_regex_captures_iter` (or its UTF-16 form), with its groups written
/// into `captures` (made for the same regular expression): `*out_found`. `CRE_INVALID_ARGUMENT` for a search made by
/// `cre_regex_find_iter`, which keeps no groups.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_matches_next_captures(
    matches: *mut cre_matches,
    captures: *mut cre_captures,
    out_found: *mut bool,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let m = unsafe { handle_mut(matches, "matches") }?;
        let caps = unsafe { captures_for(captures, m.regex) }?;
        if !m.captures() {
            return Err(Failure::argument("the search was made by cre_regex_find_iter, which keeps no groups"));
        }
        match &mut m.iter {
            Iter::Captures8(it) => {
                let got = it.next().transpose().map_err(|e| limited(e, unsafe { &(*m.regex).inner }, 0))?;
                caps.fill(got.as_ref());
                *found = got.is_some();
            }
            Iter::Captures16(it) => {
                let got = it.next().transpose().map_err(|e| limited(e, unsafe { &(*m.regex).inner }, 0))?;
                caps.fill(got.as_ref());
                *found = got.is_some();
            }
            _ => unreachable!(),
        }
        Ok(())
    })
}

/// Starts the global search again at the start of its text, which is not checked again: as setting `lastIndex` to
/// 0. NULL is allowed and does nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_matches_reset(matches: *mut cre_matches) {
    quietly((), || {
        // SAFETY: a handle this library created, or NULL.
        if let Some(m) = unsafe { matches.as_mut() } {
            let captures = m.captures();
            m.iter = unsafe { cre_matches::start(m.regex, m.text, captures) };
        }
    })
}

/// Frees a global search. NULL is allowed and does nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_matches_free(matches: *mut cre_matches) {
    if !matches.is_null() {
        // SAFETY: a handle this library created, freed once.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(matches) })));
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Freeing results

/// Frees a string the library wrote, and sets it to {NULL, 0}. NULL, or a string already {NULL, 0}, does nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_string_free(string: *mut cre_string) {
    // SAFETY: a string this library wrote, or NULL.
    if let Some(s) = unsafe { string.as_mut() }
        && !s.ptr.is_null()
    {
        // SAFETY: the boxed slice of `len + 1` bytes `string_of` made, freed once.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(s.ptr.cast::<u8>(), s.len + 1)) });
        *s = cre_string { ptr: std::ptr::null_mut(), len: 0 };
    }
}

/// Frees a UTF-16 string the library wrote, and sets it to {NULL, 0}. NULL, or a string already {NULL, 0}, does
/// nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_string16_free(string: *mut cre_string16) {
    // SAFETY: a string this library wrote, or NULL.
    if let Some(s) = unsafe { string.as_mut() }
        && !s.ptr.is_null()
    {
        // SAFETY: the boxed slice of `len + 1` units `string16_of` made, freed once.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(s.ptr, s.len + 1)) });
        *s = cre_string16 { ptr: std::ptr::null_mut(), len: 0 };
    }
}

/// Frees the pieces of a split, and empties them. NULL, or pieces already empty, does nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_pieces_free(pieces: *mut cre_pieces) {
    // SAFETY: pieces this library wrote, or NULL.
    if let Some(p) = unsafe { pieces.as_mut() }
        && !p.internal.is_null()
    {
        // SAFETY: the owner `cre_regex_split` boxed, freed once.
        drop(unsafe { Box::from_raw(p.internal.cast::<PiecesOwner<u8, cre_str>>()) });
        *p = cre_pieces { items: std::ptr::null(), len: 0, internal: std::ptr::null_mut() };
    }
}

/// Frees the pieces of a split of UTF-16 text, and empties them. NULL, or pieces already empty, does nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_pieces16_free(pieces: *mut cre_pieces16) {
    // SAFETY: pieces this library wrote, or NULL.
    if let Some(p) = unsafe { pieces.as_mut() }
        && !p.internal.is_null()
    {
        // SAFETY: the owner `cre_regex_split_utf16` boxed, freed once.
        drop(unsafe { Box::from_raw(p.internal.cast::<PiecesOwner<u16, cre_str16>>()) });
        *p = cre_pieces16 { items: std::ptr::null(), len: 0, internal: std::ptr::null_mut() };
    }
}
