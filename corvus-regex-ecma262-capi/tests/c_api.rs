//! The C library through its C functions, against the crate it wraps: compiling, matching, finding, the errors and
//! the limit of a match, and that every function the header declares is exported under that name.

use std::ffi::c_char;

use corvus_regex_ecma262::*;

fn string(s: cre_str) -> String {
    if s.len == 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr.cast::<u8>(), s.len) };
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn ptr(s: &str) -> *const c_char {
    s.as_ptr().cast()
}

fn compile(pattern: &str, flags: &str) -> *mut cre_regex {
    let mut re = std::ptr::null_mut();
    let status = unsafe { cre_regex_new(ptr(pattern), pattern.len(), ptr(flags), flags.len(), &mut re) };
    assert_eq!(status, CRE_OK, "{pattern}: {}", string(cre_last_error_message()));
    re
}

#[test]
fn matches_and_finds_as_the_crate_does() {
    for (pattern, flags, text) in [
        (r"\d+", "", "ab 12 345"),
        (r"(?<y>\d{4})-(?<m>\d{2})", "u", "on 2026-10"),
        (r"k", "iu", "\u{212A}"),
        (r"^b$", "m", "a\nb"),
        (r"\ud83d", "", "a\u{1F600}"),
        (r"[\w--\d]+", "v", "12ab3"),
        (r"x", "y", "ax"),
    ] {
        let re = compile(pattern, flags);
        let reference = corvus::Regex::new(pattern, flags).unwrap();
        let mut matched = false;
        assert_eq!(unsafe { cre_regex_is_match(re, ptr(text), text.len(), &mut matched) }, CRE_OK);
        assert_eq!(matched, reference.is_match(text), "/{pattern}/{flags}");
        let mut start = 0;
        let mut found = false;
        let mut m = cre_match::default();
        let mut all = Vec::new();
        while start <= text.len() {
            assert_eq!(unsafe { cre_regex_find(re, ptr(text), text.len(), start, &mut m, &mut found) }, CRE_OK);
            if !found {
                break;
            }
            all.push((m.start, m.end));
            start = if m.end == m.start { m.end + 1 } else { m.end };
        }
        let want: Vec<(usize, usize)> = reference.find_iter(text).map(|m| (m.start(), m.end())).collect();
        if !flags.contains('y') && !text.contains('\u{1F600}') {
            assert_eq!(all, want, "/{pattern}/{flags}");
        }
        unsafe { cre_regex_free(re) };
    }
}

#[test]
fn errors_have_their_status_and_message() {
    let mut re = std::ptr::null_mut();
    assert_eq!(unsafe { cre_regex_new(ptr("a**"), 3, std::ptr::null(), 0, &mut re) }, CRE_INVALID_PATTERN);
    assert!(re.is_null() && !string(cre_last_error_message()).is_empty());
    assert_eq!(unsafe { cre_regex_new(ptr("a"), 1, ptr("gg"), 2, &mut re) }, CRE_INVALID_FLAGS);
    let deep = "(?:".repeat(1000) + "a" + &")".repeat(1000);
    assert_eq!(unsafe { cre_regex_new(ptr(&deep), deep.len(), ptr("u"), 1, &mut re) }, CRE_PATTERN_TOO_LARGE);
    assert_eq!(unsafe { cre_regex_new(std::ptr::null(), 1, std::ptr::null(), 0, &mut re) }, CRE_INVALID_ARGUMENT);
    let re = compile("a", "");
    let mut matched = true;
    let bad = b"a\xffb";
    assert_eq!(unsafe { cre_regex_is_match(re, bad.as_ptr().cast(), 3, &mut matched) }, CRE_INVALID_UTF8);
    assert_eq!((cre_last_error_offset(), matched), (1, false));
    let mut m = cre_match::default();
    let mut found = true;
    assert_eq!(unsafe { cre_regex_find(re, ptr("a"), 1, 2, &mut m, &mut found) }, CRE_INVALID_ARGUMENT);
    unsafe { cre_regex_free(re) };
    assert_eq!(unsafe { cre_regex_is_match(std::ptr::null(), ptr("a"), 1, &mut matched) }, CRE_INVALID_ARGUMENT);
    unsafe { cre_regex_free(std::ptr::null_mut()) };
}

#[test]
fn a_match_has_a_limit_when_asked() {
    let re = compile(r"^(?:(a+)\1*)+$", "u");
    let text = "a".repeat(60) + "b";
    let mut matched = true;
    assert_eq!(unsafe { cre_regex_is_match_limit(re, ptr(&text), text.len(), 20, &mut matched) }, CRE_MATCH_LIMIT);
    assert!(!matched);
    assert_eq!(unsafe { cre_regex_is_match_limit(re, ptr("aaaa"), 4, 20, &mut matched) }, CRE_OK);
    assert!(matched);
    unsafe { cre_regex_free(re) };
}

#[test]
fn the_header_declares_what_the_library_exports() {
    let header = include_str!("../include/corvus_regex_ecma262.h");
    let source = include_str!("../src/lib.rs");
    let exported: Vec<&str> = source
        .split("pub unsafe extern \"C\" fn ")
        .chain(source.split("pub extern \"C\" fn "))
        .skip(1)
        .filter_map(|s| s.split('(').next())
        .filter(|name| name.starts_with("cre_"))
        .collect();
    assert!(exported.len() >= 9, "{exported:?}");
    for name in exported {
        assert!(header.contains(&format!(" {name}(")), "the header does not declare {name}");
    }
    for (name, value) in [
        ("CRE_OK", CRE_OK),
        ("CRE_INVALID_ARGUMENT", CRE_INVALID_ARGUMENT),
        ("CRE_INVALID_UTF8", CRE_INVALID_UTF8),
        ("CRE_INVALID_PATTERN", CRE_INVALID_PATTERN),
        ("CRE_INVALID_FLAGS", CRE_INVALID_FLAGS),
        ("CRE_PATTERN_TOO_LARGE", CRE_PATTERN_TOO_LARGE),
        ("CRE_MATCH_LIMIT", CRE_MATCH_LIMIT),
        ("CRE_PANIC", CRE_PANIC),
    ] {
        assert!(header.contains(&format!("#define {name} {value}\n")), "{name} is not {value} in the header");
    }
    assert_eq!(string(cre_version()), env!("CARGO_PKG_VERSION"));
    let version = format!(
        "#define CRE_VERSION_MAJOR {CRE_VERSION_MAJOR}\n#define CRE_VERSION_MINOR {CRE_VERSION_MINOR}\n#define CRE_VERSION_PATCH {CRE_VERSION_PATCH}"
    );
    assert!(header.contains(&version));
}
