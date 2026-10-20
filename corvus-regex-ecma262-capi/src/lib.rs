//! The C library over the `corvus-regex-ecma262` crate, in the shape of the Corvus JSON Schema C library: an opaque
//! handle created and freed by the library, UTF-8 text in as (pointer, length), strings out as borrowed `cre_str`
//! views, a `cre_status` from every fallible function with the details in thread-local storage, and no Rust panic
//! crossing into the caller.
//!
//! The comments on the public items are the C header's documentation (`include/corvus_regex_ecma262.h`).

// The names are the C API's.
#![allow(non_camel_case_types)]
// The safety contracts are the C API's documented ones (a handle from this library, a valid pointer and length).
#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use corvus::{Error, MatchError, Regex};

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
pub const CRE_VERSION_MINOR: u32 = 1;
pub const CRE_VERSION_PATCH: u32 = 0;

/// A UTF-8 string the library owns: `len` bytes from `ptr`, not NUL-terminated. Each function returning one says how
/// long it stays valid.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct cre_str {
    pub ptr: *const c_char,
    pub len: usize,
}

impl cre_str {
    fn of(s: &str) -> cre_str {
        cre_str { ptr: s.as_ptr().cast(), len: s.len() }
    }
}

/// Where a match is: byte offsets into the text, `start` to `end`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cre_match {
    pub start: usize,
    pub end: usize,
}

/// A compiled regular expression. Immutable: any number of threads may use one at once.
pub struct cre_regex {
    inner: Regex,
}

// ---------------------------------------------------------------------------------------------------------------------
// Errors

struct LastError {
    message: String,
    offset: usize,
}

thread_local! {
    static LAST_ERROR: RefCell<LastError> = const { RefCell::new(LastError { message: String::new(), offset: 0 }) };
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

/// A string argument: `len` UTF-8 bytes at `ptr` (which may be NULL when `len` is 0).
unsafe fn text<'a>(ptr: *const c_char, len: usize, what: &str) -> Result<&'a str, Failure> {
    if len == 0 {
        return Ok("");
    }
    if ptr.is_null() {
        return Err(Failure::argument(format!("{what} is NULL with a length of {len}")));
    }
    // SAFETY: the caller passes `len` readable bytes at `ptr`.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    std::str::from_utf8(bytes).map_err(|e| Failure {
        status: CRE_INVALID_UTF8,
        message: format!("{what} is not UTF-8 at byte {}", e.valid_up_to()),
        offset: e.valid_up_to(),
    })
}

unsafe fn handle<'a, T>(ptr: *const T, what: &str) -> Result<&'a T, Failure> {
    // SAFETY: a non-NULL handle is one this library created and the caller has not freed.
    unsafe { ptr.as_ref() }.ok_or_else(|| Failure::argument(format!("{what} is NULL")))
}

unsafe fn out<'a, T>(ptr: *mut T, what: &str) -> Result<&'a mut T, Failure> {
    // SAFETY: a non-NULL out-parameter points to writable storage for a T.
    unsafe { ptr.as_mut() }.ok_or_else(|| Failure::argument(format!("{what} is NULL")))
}

fn limited(e: MatchError, limit: Duration) -> Failure {
    Failure::new(CRE_MATCH_LIMIT, format!("{e} ({} ms)", limit.as_millis()))
}

// ---------------------------------------------------------------------------------------------------------------------
// The functions

/// The library's version, as "major.minor.patch". The string is static.
#[unsafe(no_mangle)]
pub extern "C" fn cre_version() -> cre_str {
    cre_str::of(env!("CARGO_PKG_VERSION"))
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

/// Compiles a pattern with the flags `new RegExp(pattern, flags)` takes (`flags` may be NULL with a length of 0 for
/// none). On success `*out` is a handle to free with `cre_regex_free`; on failure it is NULL and the status says why:
/// `CRE_INVALID_PATTERN`, `CRE_INVALID_FLAGS`, `CRE_PATTERN_TOO_LARGE`, `CRE_INVALID_UTF8` or
/// `CRE_INVALID_ARGUMENT`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_new(
    pattern: *const c_char,
    pattern_len: usize,
    flags: *const c_char,
    flags_len: usize,
    out_regex: *mut *mut cre_regex,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_regex, "out_regex") }?;
        *slot = std::ptr::null_mut();
        let pattern = unsafe { text(pattern, pattern_len, "pattern") }?;
        let flags = unsafe { text(flags, flags_len, "flags") }?;
        let regex = Regex::new(pattern, flags).map_err(|e| match &e {
            Error::Invalid(m) => Failure::new(CRE_INVALID_PATTERN, m.clone()),
            Error::Flags(m) => Failure::new(CRE_INVALID_FLAGS, m.clone()),
            Error::TooLarge(_) => Failure::new(CRE_PATTERN_TOO_LARGE, e.to_string()),
            _ => Failure::new(CRE_INVALID_PATTERN, e.to_string()),
        })?;
        *slot = Box::into_raw(Box::new(cre_regex { inner: regex }));
        Ok(())
    })
}

/// Frees a regular expression. NULL is allowed and does nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_free(regex: *mut cre_regex) {
    if !regex.is_null() {
        // SAFETY: a handle this library created, freed once.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(regex) })));
    }
}

/// Whether the pattern matches anywhere in the UTF-8 text (at its start alone with the y flag): `*out_matched`. With
/// no limit on the time the match takes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    out_matched: *mut bool,
) -> cre_status {
    unsafe { cre_regex_is_match_limit(regex, text_ptr, text_len, 0, out_matched) }
}

/// `cre_regex_is_match` with a limit on the time the match may take, in milliseconds (0 for none). A match that
/// reaches it, or whose backtracking memory reaches its cap, is `CRE_MATCH_LIMIT`, and `*out_matched` is false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_is_match_limit(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    limit_ms: u64,
    out_matched: *mut bool,
) -> cre_status {
    guard(|| {
        let slot = unsafe { out(out_matched, "out_matched") }?;
        *slot = false;
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { text(text_ptr, text_len, "text") }?;
        let limit = Duration::from_millis(limit_ms);
        *slot = regex.inner.try_is_match(text, limit).map_err(|e| limited(e, limit))?;
        Ok(())
    })
}

/// The first match at or after the byte offset `start` of the UTF-8 text, as `exec` finds it with `lastIndex` set to
/// `start`: `*out_found` says whether there is one, and `*out_match` is where it is (zeroes when there is none).
/// Without the u or v flag a match can start or end between the halves of a character beyond the Basic Multilingual
/// Plane; such an offset is that of the character's third byte.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_find(
    regex: *const cre_regex,
    text_ptr: *const c_char,
    text_len: usize,
    start: usize,
    out_match: *mut cre_match,
    out_found: *mut bool,
) -> cre_status {
    guard(|| {
        let found = unsafe { out(out_found, "out_found") }?;
        *found = false;
        let place = unsafe { out(out_match, "out_match") }?;
        *place = cre_match::default();
        let regex = unsafe { handle(regex, "regex") }?;
        let text = unsafe { text(text_ptr, text_len, "text") }?;
        if start > text.len() {
            return Err(Failure::argument(format!("start {start} is beyond the text's {} bytes", text.len())));
        }
        if let Some(m) = regex.inner.find_at(text, start) {
            *place = cre_match { start: m.start(), end: m.end() };
            *found = true;
        }
        Ok(())
    })
}

/// The number of capturing groups of the pattern.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cre_regex_group_count(regex: *const cre_regex) -> usize {
    // SAFETY: a handle this library created, or NULL.
    unsafe { regex.as_ref() }.map_or(0, |r| r.inner.captures_len() - 1)
}
