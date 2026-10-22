//! The C library through its C functions, against the crate it wraps: compiling, matching, finding, the groups by
//! number and by name, the global search, replace and split over UTF-8 and UTF-16, the errors and the limit of a
//! match, that every function the header declares is exported under that name, and that the versions agree.

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

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn compile(pattern: &str, flags: &str) -> *mut cre_regex {
    let mut re = std::ptr::null_mut();
    let status = unsafe { cre_regex_new(ptr(pattern), pattern.len(), ptr(flags), flags.len(), &mut re) };
    assert_eq!(status, CRE_OK, "{pattern}: {}", string(cre_last_error_message()));
    re
}

fn unset() -> cre_match {
    cre_match { start: CRE_UNSET, end: CRE_UNSET }
}

fn span(m: cre_match) -> Option<(usize, usize)> {
    (m.start != CRE_UNSET).then_some((m.start, m.end))
}

const CASES: [(&str, &str, &str); 10] = [
    (r"\d+", "", "ab 12 345"),
    (r"(?<y>\d{4})-(?<m>\d{2})", "gu", "on 2026-10 and 2027-01"),
    (r"(?:(?<a>x)|(?<a>y))(z)?", "g", "x yz"),
    (r"k", "iu", "\u{212A}"),
    (r"^b$", "gm", "a\nb\nb"),
    (r"\ud83d", "", "a\u{1F600}"),
    (r"", "g", "a\u{1F600}b"),
    (r"", "gu", "a\u{1F600}b"),
    (r"[\w--\d]+", "v", "12ab3"),
    (r"x", "y", "ax"),
];

#[test]
fn matches_and_finds_as_the_crate_does() {
    for (pattern, flags, text) in CASES {
        let re = compile(pattern, flags);
        let reference = corvus::Regex::new(pattern, flags).unwrap();
        let t16 = utf16(text);
        let mut matched = false;
        assert_eq!(unsafe { cre_regex_is_match(re, ptr(text), text.len(), &mut matched) }, CRE_OK);
        assert_eq!(matched, reference.is_match(text), "/{pattern}/{flags}");
        assert_eq!(unsafe { cre_regex_is_match_utf16(re, t16.as_ptr(), t16.len(), &mut matched) }, CRE_OK);
        assert_eq!(matched, reference.is_match_utf16(&t16), "/{pattern}/{flags}");
        let at = (1..=text.len()).find(|&k| text.is_char_boundary(k)).unwrap_or(0);
        assert_eq!(unsafe { cre_regex_is_match_at(re, ptr(text), text.len(), at, &mut matched) }, CRE_OK);
        assert_eq!(matched, reference.is_match_at(text, at), "/{pattern}/{flags}");
        assert_eq!(unsafe { cre_regex_is_match_at_utf16(re, t16.as_ptr(), t16.len(), 1, &mut matched) }, CRE_OK);
        assert_eq!(matched, reference.is_match_at_utf16(&t16, 1), "/{pattern}/{flags}");
        for start in (0..=text.len()).filter(|&k| text.is_char_boundary(k)) {
            let (mut m, mut found) = (unset(), false);
            assert_eq!(unsafe { cre_regex_find(re, ptr(text), text.len(), start, &mut m, &mut found) }, CRE_OK);
            let want = reference.find_at(text, start).map(|m| (m.start(), m.end()));
            assert_eq!((found, span(m)), (want.is_some(), want), "/{pattern}/{flags} at {start}");
        }
        for start in 0..=t16.len() {
            let (mut m, mut found) = (unset(), false);
            let status =
                unsafe { cre_regex_find_limit_utf16(re, t16.as_ptr(), t16.len(), start, 1000, &mut m, &mut found) };
            assert_eq!(status, CRE_OK);
            let want = reference.find_at_utf16(&t16, start).map(|m| (m.start(), m.end()));
            assert_eq!((found, span(m)), (want.is_some(), want), "/{pattern}/{flags} at {start}");
        }
        unsafe { cre_regex_free(re) };
    }
}

#[test]
fn groups_by_number_and_by_name_are_the_crate_s() {
    for (pattern, flags, text) in CASES {
        let re = compile(pattern, flags);
        let reference = corvus::Regex::new(pattern, flags).unwrap();
        let t16 = utf16(text);
        let n = unsafe { cre_regex_group_count(re) } + 1;
        assert_eq!(n, reference.captures_len());
        let names: Vec<String> = (0..unsafe { cre_regex_group_name_count(re) })
            .map(|k| string(unsafe { cre_regex_group_name(re, k) }))
            .collect();
        assert_eq!(names, reference.group_names().collect::<Vec<_>>());
        assert!(unsafe { cre_regex_group_name(re, names.len()) }.ptr.is_null());
        let mut caps = std::ptr::null_mut();
        assert_eq!(unsafe { cre_captures_new(re, &mut caps) }, CRE_OK);
        // A buffer one longer than the groups: the last entry is unset.
        let mut groups = vec![unset(); n + 1];
        let mut found = false;
        let status = unsafe { cre_regex_captures(re, ptr(text), text.len(), 0, caps, &mut found) };
        assert_eq!(status, CRE_OK);
        let want = reference.captures(text);
        assert_eq!(found, want.is_some());
        let status = unsafe {
            cre_regex_captures_read(re, ptr(text), text.len(), 0, groups.as_mut_ptr(), groups.len(), &mut found)
        };
        assert_eq!((status, found), (CRE_OK, want.is_some()));
        if let Some(want) = &want {
            assert_eq!(unsafe { cre_captures_len(caps) }, n);
            for (k, &g) in groups.iter().enumerate().take(n) {
                let mut m = unset();
                let got = unsafe { cre_captures_get(caps, k, &mut m) };
                let w = want.get(k).map(|m| (m.start(), m.end()));
                assert_eq!((got, span(m), span(g)), (w.is_some(), w, w), "/{pattern}/{flags} group {k}");
            }
            assert!(span(groups[n]).is_none());
            for name in &names {
                let mut m = unset();
                let got = unsafe { cre_captures_name(caps, ptr(name), name.len(), &mut m) };
                let w = want.name(name).map(|m| (m.start(), m.end()));
                assert_eq!((got, span(m)), (w.is_some(), w), "/{pattern}/{flags} <{name}>");
            }
        }
        let mut m = unset();
        assert!(!unsafe { cre_captures_name(caps, ptr("nope"), 4, &mut m) });
        // Over UTF-16, with a limit, and into a short buffer.
        let status = unsafe { cre_regex_captures_limit_utf16(re, t16.as_ptr(), t16.len(), 0, 1000, caps, &mut found) };
        assert_eq!(status, CRE_OK);
        let want = reference.captures_utf16(&t16);
        assert_eq!(found, want.is_some());
        let mut short = [unset()];
        let status = unsafe {
            cre_regex_captures_read_limit_utf16(re, t16.as_ptr(), t16.len(), 0, 1000, short.as_mut_ptr(), 1, &mut found)
        };
        assert_eq!(status, CRE_OK);
        if let Some(want) = &want {
            for k in 0..n {
                let mut m = unset();
                unsafe { cre_captures_get(caps, k, &mut m) };
                assert_eq!(span(m), want.get(k).map(|m| (m.start(), m.end())));
            }
            assert_eq!(span(short[0]), Some((want.get_match().start(), want.get_match().end())));
        }
        unsafe { cre_captures_free(caps) };
        unsafe { cre_regex_free(re) };
    }
}

#[test]
fn the_global_search_is_find_iter() {
    for (pattern, flags, text) in CASES {
        let re = compile(pattern, flags);
        let reference = corvus::Regex::new(pattern, flags).unwrap();
        let t16 = utf16(text);
        let collect = |it: *mut cre_matches| -> Vec<(usize, usize)> {
            let mut all = Vec::new();
            loop {
                let (mut m, mut found) = (unset(), false);
                assert_eq!(unsafe { cre_matches_next(it, &mut m, &mut found) }, CRE_OK);
                if !found {
                    break;
                }
                all.push((m.start, m.end));
            }
            all
        };
        // The same matches a batch at a time, until a batch is short.
        let batches = |it: *mut cre_matches, size: usize| -> Vec<(usize, usize)> {
            let (mut all, mut place) = (Vec::new(), vec![unset(); size]);
            loop {
                let mut n = usize::MAX;
                assert_eq!(unsafe { cre_matches_next_batch(it, place.as_mut_ptr(), size, &mut n) }, CRE_OK);
                all.extend(place[..n].iter().map(|m| (m.start, m.end)));
                if n < size {
                    break;
                }
            }
            all
        };
        let mut it = std::ptr::null_mut();
        assert_eq!(unsafe { cre_regex_find_iter(re, ptr(text), text.len(), &mut it) }, CRE_OK);
        let want: Vec<(usize, usize)> = reference.find_iter(text).map(|m| (m.start(), m.end())).collect();
        for size in [1, 2, 5] {
            unsafe { cre_matches_reset(it) };
            assert_eq!(batches(it, size), want, "/{pattern}/{flags} in batches of {size}");
        }
        unsafe { cre_matches_reset(it) };
        assert_eq!(collect(it), want, "/{pattern}/{flags}");
        assert!(collect(it).is_empty());
        unsafe { cre_matches_reset(it) };
        assert_eq!(collect(it), want, "/{pattern}/{flags} after a reset");
        unsafe { cre_matches_free(it) };
        assert_eq!(unsafe { cre_regex_find_iter_utf16(re, t16.as_ptr(), t16.len(), &mut it) }, CRE_OK);
        let want16: Vec<(usize, usize)> = reference.find_iter_utf16(&t16).map(|m| (m.start(), m.end())).collect();
        assert_eq!(batches(it, 2), want16, "/{pattern}/{flags} in batches");
        unsafe { cre_matches_reset(it) };
        assert_eq!(collect(it), want16, "/{pattern}/{flags}");
        // A search without groups cannot give them.
        let mut caps = std::ptr::null_mut();
        assert_eq!(unsafe { cre_captures_new(re, &mut caps) }, CRE_OK);
        let mut found = true;
        assert_eq!(unsafe { cre_matches_next_captures(it, caps, &mut found) }, CRE_INVALID_ARGUMENT);
        unsafe { cre_matches_free(it) };
        // With the groups of every match.
        assert_eq!(unsafe { cre_regex_captures_iter_utf16(re, t16.as_ptr(), t16.len(), &mut it) }, CRE_OK);
        for want in reference.captures_iter_utf16(&t16) {
            assert_eq!(unsafe { cre_matches_next_captures(it, caps, &mut found) }, CRE_OK);
            assert!(found);
            for k in 0..want.len() {
                let mut m = unset();
                unsafe { cre_captures_get(caps, k, &mut m) };
                assert_eq!(span(m), want.get(k).map(|m| (m.start(), m.end())));
            }
        }
        assert_eq!(unsafe { cre_matches_next_captures(it, caps, &mut found) }, CRE_OK);
        assert!(!found && unsafe { cre_captures_len(caps) } == 0);
        unsafe { cre_matches_free(it) };
        assert_eq!(unsafe { cre_regex_captures_iter(re, ptr(text), text.len(), &mut it) }, CRE_OK);
        assert_eq!(collect(it), want, "/{pattern}/{flags}");
        unsafe { cre_matches_reset(it) };
        assert_eq!(batches(it, 3), want, "/{pattern}/{flags} with groups, in batches");
        // The groups a batch at a time, two matches a batch, as the crate's captures_iter gives them.
        let width = reference.captures_len();
        let want_groups: Vec<Option<(usize, usize)>> = reference
            .captures_iter(text)
            .flat_map(|c| (0..width).map(|k| c.get(k).map(|m| (m.start(), m.end()))).collect::<Vec<_>>())
            .collect();
        unsafe { cre_matches_reset(it) };
        let (mut got, mut place) = (Vec::new(), vec![unset(); 2 * width + 1]);
        loop {
            let mut n = usize::MAX;
            assert_eq!(unsafe { cre_matches_next_captures_batch(it, place.as_mut_ptr(), place.len(), &mut n) }, CRE_OK);
            got.extend(place[..n * width].iter().map(|&m| span(m)));
            if n < 2 {
                break;
            }
        }
        assert_eq!(got, want_groups, "/{pattern}/{flags}, the groups in batches");
        unsafe { cre_matches_free(it) };
        unsafe { cre_captures_free(caps) };
        unsafe { cre_regex_free(re) };
    }
}

#[test]
fn replace_and_split_are_the_crate_s() {
    let template = "<$&|$1|$2|$10|$<a>|$$|$`|$'>";
    let t16 = utf16(template);
    for (pattern, flags, text) in CASES {
        let re = compile(pattern, flags);
        let reference = corvus::Regex::new(pattern, flags).unwrap();
        let s16 = utf16(text);
        let mut out = cre_string { ptr: std::ptr::null_mut(), len: 0 };
        let status = unsafe { cre_regex_replace(re, ptr(text), text.len(), ptr(template), template.len(), &mut out) };
        assert_eq!(status, CRE_OK);
        let got = unsafe { std::slice::from_raw_parts(out.ptr.cast::<u8>(), out.len + 1) };
        assert_eq!(got[out.len], 0, "NUL-terminated");
        assert_eq!(std::str::from_utf8(&got[..out.len]).unwrap(), reference.replace(text, template));
        unsafe { cre_string_free(&mut out) };
        assert!(out.ptr.is_null());
        unsafe { cre_string_free(&mut out) };
        let status =
            unsafe { cre_regex_replace_all(re, ptr(text), text.len(), ptr(template), template.len(), &mut out) };
        assert_eq!(status, CRE_OK);
        assert_eq!(string(cre_str { ptr: out.ptr, len: out.len }), reference.replace_all(text, template));
        unsafe { cre_string_free(&mut out) };
        let mut out16 = cre_string16 { ptr: std::ptr::null_mut(), len: 0 };
        for all in [false, true] {
            let status = unsafe {
                if all {
                    cre_regex_replace_all_utf16(re, s16.as_ptr(), s16.len(), t16.as_ptr(), t16.len(), &mut out16)
                } else {
                    cre_regex_replace_utf16(re, s16.as_ptr(), s16.len(), t16.as_ptr(), t16.len(), &mut out16)
                }
            };
            assert_eq!(status, CRE_OK);
            let got = unsafe { std::slice::from_raw_parts(out16.ptr, out16.len + 1) };
            let want = if all { reference.replace_all_utf16(&s16, &t16) } else { reference.replace_utf16(&s16, &t16) };
            assert_eq!((&got[..out16.len], got[out16.len]), (&*want, 0));
            unsafe { cre_string16_free(&mut out16) };
        }
        for limit in [CRE_SPLIT_ALL, 0, 1, 3] {
            let mut pieces = cre_pieces { items: std::ptr::null(), len: 0, internal: std::ptr::null_mut() };
            assert_eq!(unsafe { cre_regex_split(re, ptr(text), text.len(), limit, &mut pieces) }, CRE_OK);
            let items = unsafe { std::slice::from_raw_parts(pieces.items, pieces.len) };
            let got: Vec<Option<String>> = items.iter().map(|p| (!p.ptr.is_null()).then(|| string(*p))).collect();
            let want: Vec<Option<String>> =
                reference.splitn(text, limit as usize).into_iter().map(|p| p.map(|p| p.into_owned())).collect();
            assert_eq!(got, want, "/{pattern}/{flags} split {limit}");
            unsafe { cre_pieces_free(&mut pieces) };
            assert!(pieces.items.is_null() && pieces.internal.is_null());
            let mut pieces = cre_pieces16 { items: std::ptr::null(), len: 0, internal: std::ptr::null_mut() };
            assert_eq!(unsafe { cre_regex_split_utf16(re, s16.as_ptr(), s16.len(), limit, &mut pieces) }, CRE_OK);
            let items = unsafe { std::slice::from_raw_parts(pieces.items, pieces.len) };
            let got: Vec<Option<Vec<u16>>> = items
                .iter()
                .map(|p| (!p.ptr.is_null()).then(|| unsafe { std::slice::from_raw_parts(p.ptr, p.len) }.to_vec()))
                .collect();
            let want: Vec<Option<Vec<u16>>> =
                reference.splitn_utf16(&s16, limit as usize).into_iter().map(|p| p.map(|p| p.into_owned())).collect();
            assert_eq!(got, want, "/{pattern}/{flags} split16 {limit}");
            unsafe { cre_pieces16_free(&mut pieces) };
        }
        unsafe { cre_regex_free(re) };
    }
}

#[test]
fn the_pattern_s_facts() {
    let re = compile(r"(?<x>a)|b", "ugi");
    assert_eq!(string(unsafe { cre_regex_source(re) }), r"(?<x>a)|b");
    assert_eq!(string(unsafe { cre_regex_flags(re) }), "giu");
    unsafe { cre_regex_free(re) };
    let mut valid = false;
    assert_eq!(unsafe { cre_regex_is_valid(ptr("a{2,1}"), 6, ptr("u"), 1, &mut valid) }, CRE_OK);
    assert!(!valid);
    assert_eq!(unsafe { cre_regex_is_valid(ptr("a{2}"), 4, std::ptr::null(), 0, &mut valid) }, CRE_OK);
    assert!(valid);
    assert_eq!(string(cre_status_name(CRE_INVALID_PATTERN)), "CRE_INVALID_PATTERN");
    assert_eq!(string(cre_status_name(99)), "unknown");
    assert!(unsafe { cre_regex_source(std::ptr::null()) }.ptr.is_null());
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
    let mut m = unset();
    let mut found = true;
    assert_eq!(unsafe { cre_regex_find(re, ptr("a"), 1, 2, &mut m, &mut found) }, CRE_INVALID_ARGUMENT);
    assert!(!found && span(m).is_none());
    // A start inside a character, and one between the halves of a character beyond the BMP without u.
    let s = "\u{1F600}";
    assert_eq!(unsafe { cre_regex_find(re, ptr(s), s.len(), 1, &mut m, &mut found) }, CRE_INVALID_ARGUMENT);
    assert_eq!(unsafe { cre_regex_find(re, ptr(s), s.len(), 2, &mut m, &mut found) }, CRE_OK);
    let u = compile("a", "u");
    assert_eq!(unsafe { cre_regex_find(u, ptr(s), s.len(), 2, &mut m, &mut found) }, CRE_INVALID_ARGUMENT);
    // A captures handle made for another regular expression.
    let mut caps = std::ptr::null_mut();
    assert_eq!(unsafe { cre_captures_new(u, &mut caps) }, CRE_OK);
    assert_eq!(unsafe { cre_regex_captures(re, ptr("a"), 1, 0, caps, &mut found) }, CRE_INVALID_ARGUMENT);
    unsafe { cre_captures_free(caps) };
    unsafe { cre_regex_free(u) };
    // A batch: a capacity of 0 writes nothing and takes NULL; a NULL place with a capacity, or no count, fails.
    let mut it = std::ptr::null_mut();
    assert_eq!(unsafe { cre_regex_find_iter(re, ptr("aa"), 2, &mut it) }, CRE_OK);
    let mut n = usize::MAX;
    assert_eq!(unsafe { cre_matches_next_batch(it, std::ptr::null_mut(), 0, &mut n) }, CRE_OK);
    assert_eq!(n, 0);
    n = usize::MAX;
    assert_eq!(unsafe { cre_matches_next_batch(it, std::ptr::null_mut(), 4, &mut n) }, CRE_INVALID_ARGUMENT);
    assert_eq!(n, 0);
    let mut place = [unset(); 4];
    assert_eq!(
        unsafe { cre_matches_next_batch(it, place.as_mut_ptr(), 4, std::ptr::null_mut()) },
        CRE_INVALID_ARGUMENT
    );
    assert_eq!(unsafe { cre_matches_next_batch(it, place.as_mut_ptr(), 4, &mut n) }, CRE_OK);
    assert_eq!((n, place[1].start), (2, 1));
    unsafe { cre_matches_free(it) };
    let status = unsafe { cre_matches_next_batch(std::ptr::null_mut(), place.as_mut_ptr(), 4, &mut n) };
    assert_eq!((status, n), (CRE_INVALID_ARGUMENT, 0));
    // The groups a batch at a time: only from a search that keeps them, and room for one match at least.
    let groups = compile("(a)(b)?", "g");
    assert_eq!(unsafe { cre_regex_find_iter(groups, ptr("aab"), 3, &mut it) }, CRE_OK);
    let status = unsafe { cre_matches_next_captures_batch(it, place.as_mut_ptr(), 4, &mut n) };
    assert_eq!((status, n), (CRE_INVALID_ARGUMENT, 0));
    unsafe { cre_matches_free(it) };
    assert_eq!(unsafe { cre_regex_captures_iter(groups, ptr("aab"), 3, &mut it) }, CRE_OK);
    assert_eq!(unsafe { cre_matches_next_captures_batch(it, place.as_mut_ptr(), 2, &mut n) }, CRE_INVALID_ARGUMENT);
    assert_eq!(unsafe { cre_matches_next_captures_batch(it, std::ptr::null_mut(), 0, &mut n) }, CRE_OK);
    assert_eq!(n, 0);
    let status = unsafe { cre_matches_next_captures_batch(it, std::ptr::null_mut(), 3, &mut n) };
    assert_eq!(status, CRE_INVALID_ARGUMENT);
    assert_eq!(unsafe { cre_matches_next_captures_batch(it, place.as_mut_ptr(), 4, &mut n) }, CRE_OK);
    assert_eq!((n, span(place[0]), span(place[2])), (1, Some((0, 1)), None));
    assert_eq!(unsafe { cre_matches_next_captures_batch(it, place.as_mut_ptr(), 3, &mut n) }, CRE_OK);
    assert_eq!((n, span(place[2])), (1, Some((2, 3))));
    assert_eq!(unsafe { cre_matches_next_captures_batch(it, place.as_mut_ptr(), 3, &mut n) }, CRE_OK);
    assert_eq!(n, 0);
    unsafe { cre_matches_free(it) };
    unsafe { cre_regex_free(groups) };
    // A NULL buffer with a length, and a misaligned UTF-16 text.
    let status = unsafe { cre_regex_captures_read(re, ptr("a"), 1, 0, std::ptr::null_mut(), 2, &mut found) };
    assert_eq!(status, CRE_INVALID_ARGUMENT);
    let units = [0u16; 4];
    let odd = unsafe { units.as_ptr().cast::<u8>().add(1) }.cast::<u16>();
    assert_eq!(unsafe { cre_regex_is_match_utf16(re, odd, 1, &mut matched) }, CRE_INVALID_ARGUMENT);
    unsafe { cre_regex_free(re) };
    assert_eq!(unsafe { cre_regex_is_match(std::ptr::null(), ptr("a"), 1, &mut matched) }, CRE_INVALID_ARGUMENT);
    unsafe { cre_regex_free(std::ptr::null_mut()) };
    unsafe { cre_captures_free(std::ptr::null_mut()) };
    unsafe { cre_matches_free(std::ptr::null_mut()) };
    unsafe { cre_matches_reset(std::ptr::null_mut()) };
    unsafe { cre_pieces_free(std::ptr::null_mut()) };
    unsafe { cre_pieces16_free(std::ptr::null_mut()) };
    unsafe { cre_string_free(std::ptr::null_mut()) };
    unsafe { cre_string16_free(std::ptr::null_mut()) };
}

#[test]
fn a_match_has_a_limit_when_asked() {
    let re = compile(r"^(?:(a+)\1*)+$", "u");
    let text = "a".repeat(60) + "b";
    let t16 = utf16(&text);
    let mut matched = true;
    assert_eq!(unsafe { cre_regex_is_match_limit(re, ptr(&text), text.len(), 20, &mut matched) }, CRE_MATCH_LIMIT);
    assert!(!matched);
    assert_eq!(unsafe { cre_regex_is_match_limit(re, ptr("aaaa"), 4, 20, &mut matched) }, CRE_OK);
    assert!(matched);
    let status = unsafe { cre_regex_is_match_limit_utf16(re, t16.as_ptr(), t16.len(), 20, &mut matched) };
    assert_eq!(status, CRE_MATCH_LIMIT);
    let (mut m, mut found) = (unset(), true);
    let status = unsafe { cre_regex_find_limit(re, ptr(&text), text.len(), 0, 20, &mut m, &mut found) };
    assert_eq!((status, found), (CRE_MATCH_LIMIT, false));
    let mut caps = std::ptr::null_mut();
    assert_eq!(unsafe { cre_captures_new(re, &mut caps) }, CRE_OK);
    let status = unsafe { cre_regex_captures_limit(re, ptr(&text), text.len(), 0, 20, caps, &mut found) };
    assert_eq!((status, found), (CRE_MATCH_LIMIT, false));
    let mut groups = [unset(); 2];
    let status =
        unsafe { cre_regex_captures_read_limit(re, ptr(&text), text.len(), 0, 20, groups.as_mut_ptr(), 2, &mut found) };
    assert_eq!((status, found), (CRE_MATCH_LIMIT, false));
    unsafe { cre_captures_free(caps) };
    unsafe { cre_regex_free(re) };
}

#[test]
fn the_unchecked_functions_find_what_the_checked_ones_do() {
    for (pattern, flags, text) in CASES {
        let re = compile(pattern, flags);
        let (p, n) = (ptr(text), text.len());
        let (mut a, mut b) = (false, true);
        assert_eq!(unsafe { cre_regex_is_match(re, p, n, &mut a) }, CRE_OK);
        assert_eq!(unsafe { cre_regex_is_match_unchecked(re, p, n, &mut b) }, CRE_OK);
        assert_eq!(a, b, "/{pattern}/{flags}");
        assert_eq!(unsafe { cre_regex_is_match_limit_unchecked(re, p, n, 1000, &mut b) }, CRE_OK);
        assert_eq!(a, b, "/{pattern}/{flags}");
        for start in (0..=n).filter(|&k| text.is_char_boundary(k)) {
            assert_eq!(unsafe { cre_regex_is_match_at(re, p, n, start, &mut a) }, CRE_OK);
            assert_eq!(unsafe { cre_regex_is_match_at_unchecked(re, p, n, start, &mut b) }, CRE_OK);
            assert_eq!(a, b, "/{pattern}/{flags} at {start}");
            let (mut m, mut found, mut m2, mut found2) = (unset(), false, unset(), false);
            assert_eq!(unsafe { cre_regex_find(re, p, n, start, &mut m, &mut found) }, CRE_OK);
            assert_eq!(unsafe { cre_regex_find_unchecked(re, p, n, start, &mut m2, &mut found2) }, CRE_OK);
            assert_eq!((found, span(m)), (found2, span(m2)), "/{pattern}/{flags} at {start}");
            let status = unsafe { cre_regex_find_limit_unchecked(re, p, n, start, 1000, &mut m2, &mut found2) };
            assert_eq!(status, CRE_OK);
            assert_eq!((found, span(m)), (found2, span(m2)), "/{pattern}/{flags} at {start}");
            let width = unsafe { cre_regex_group_count(re) } + 1;
            let (mut g, mut g2) = (vec![unset(); width], vec![unset(); width]);
            let read = |groups: &mut Vec<cre_match>, found: &mut bool, unchecked: bool, limit: bool| unsafe {
                match (unchecked, limit) {
                    (false, _) => cre_regex_captures_read(re, p, n, start, groups.as_mut_ptr(), width, found),
                    (true, false) => {
                        cre_regex_captures_read_unchecked(re, p, n, start, groups.as_mut_ptr(), width, found)
                    }
                    (true, true) => cre_regex_captures_read_limit_unchecked(
                        re,
                        p,
                        n,
                        start,
                        1000,
                        groups.as_mut_ptr(),
                        width,
                        found,
                    ),
                }
            };
            assert_eq!(read(&mut g, &mut found, false, false), CRE_OK);
            for limit in [false, true] {
                assert_eq!(read(&mut g2, &mut found2, true, limit), CRE_OK);
                let spans = |g: &[cre_match]| g.iter().map(|&m| span(m)).collect::<Vec<_>>();
                assert_eq!((found, spans(&g)), (found2, spans(&g2)), "/{pattern}/{flags} at {start}");
            }
            let (mut caps, mut caps2) = (std::ptr::null_mut(), std::ptr::null_mut());
            assert_eq!(unsafe { cre_captures_new(re, &mut caps) }, CRE_OK);
            assert_eq!(unsafe { cre_captures_new(re, &mut caps2) }, CRE_OK);
            let all = |c: *mut cre_captures| -> Vec<Option<(usize, usize)>> {
                (0..unsafe { cre_captures_len(c) })
                    .map(|k| {
                        let mut m = unset();
                        unsafe { cre_captures_get(c, k, &mut m) }.then_some((m.start, m.end))
                    })
                    .collect()
            };
            assert_eq!(unsafe { cre_regex_captures(re, p, n, start, caps, &mut found) }, CRE_OK);
            assert_eq!(unsafe { cre_regex_captures_unchecked(re, p, n, start, caps2, &mut found2) }, CRE_OK);
            assert_eq!((found, all(caps)), (found2, all(caps2)), "/{pattern}/{flags} at {start}");
            assert_eq!(
                unsafe { cre_regex_captures_limit_unchecked(re, p, n, start, 1000, caps2, &mut found2) },
                CRE_OK
            );
            assert_eq!((found, all(caps)), (found2, all(caps2)), "/{pattern}/{flags} at {start}");
            unsafe { cre_captures_free(caps) };
            unsafe { cre_captures_free(caps2) };
        }
        // The global searches, each match with its groups.
        for captures in [false, true] {
            let run = |unchecked: bool| -> Vec<(usize, usize)> {
                let mut it = std::ptr::null_mut();
                let status = unsafe {
                    match (captures, unchecked) {
                        (false, false) => cre_regex_find_iter(re, p, n, &mut it),
                        (false, true) => cre_regex_find_iter_unchecked(re, p, n, &mut it),
                        (true, false) => cre_regex_captures_iter(re, p, n, &mut it),
                        (true, true) => cre_regex_captures_iter_unchecked(re, p, n, &mut it),
                    }
                };
                assert_eq!(status, CRE_OK);
                let mut all = Vec::new();
                loop {
                    let (mut m, mut found) = (unset(), false);
                    assert_eq!(unsafe { cre_matches_next(it, &mut m, &mut found) }, CRE_OK);
                    if !found {
                        break;
                    }
                    all.push((m.start, m.end));
                }
                unsafe { cre_matches_free(it) };
                all
            };
            assert_eq!(run(false), run(true), "/{pattern}/{flags}");
        }
        let template = "<$&|$1|$`|$'>";
        for all in [false, true] {
            let replace = |unchecked: bool| -> String {
                let mut out = cre_string { ptr: std::ptr::null_mut(), len: 0 };
                let (t, tn) = (ptr(template), template.len());
                let status = unsafe {
                    match (all, unchecked) {
                        (false, false) => cre_regex_replace(re, p, n, t, tn, &mut out),
                        (false, true) => cre_regex_replace_unchecked(re, p, n, t, tn, &mut out),
                        (true, false) => cre_regex_replace_all(re, p, n, t, tn, &mut out),
                        (true, true) => cre_regex_replace_all_unchecked(re, p, n, t, tn, &mut out),
                    }
                };
                assert_eq!(status, CRE_OK);
                let got = string(cre_str { ptr: out.ptr, len: out.len });
                unsafe { cre_string_free(&mut out) };
                got
            };
            assert_eq!(replace(false), replace(true), "/{pattern}/{flags}");
        }
        let split = |unchecked: bool| -> Vec<Option<String>> {
            let mut out = cre_pieces { items: std::ptr::null(), len: 0, internal: std::ptr::null_mut() };
            let status = unsafe {
                if unchecked {
                    cre_regex_split_unchecked(re, p, n, CRE_SPLIT_ALL, &mut out)
                } else {
                    cre_regex_split(re, p, n, CRE_SPLIT_ALL, &mut out)
                }
            };
            assert_eq!(status, CRE_OK);
            let items = unsafe { std::slice::from_raw_parts(out.items, out.len) };
            let got = items.iter().map(|&s| (!s.ptr.is_null()).then(|| string(s))).collect();
            unsafe { cre_pieces_free(&mut out) };
            got
        };
        assert_eq!(split(false), split(true), "/{pattern}/{flags}");
        unsafe { cre_regex_free(re) };
    }
    // An empty text may be NULL; a NULL text with a length is still refused.
    let re = compile("a", "");
    let mut matched = true;
    assert_eq!(unsafe { cre_regex_is_match_unchecked(re, std::ptr::null(), 0, &mut matched) }, CRE_OK);
    assert!(!matched);
    assert_eq!(unsafe { cre_regex_is_match_unchecked(re, std::ptr::null(), 1, &mut matched) }, CRE_INVALID_ARGUMENT);
    unsafe { cre_regex_free(re) };
}

#[test]
fn a_text_that_is_not_utf8_far_in_is_refused_where_it_stops_being_utf8() {
    // Past the first blocks the fast check reads, so the offset comes from the second, exact check.
    let re = compile("a", "");
    let mut text = "a".repeat(1000).into_bytes();
    text.extend_from_slice(b"\xe2\x82");
    text.extend_from_slice("b".repeat(100).as_bytes());
    let mut matched = true;
    assert_eq!(unsafe { cre_regex_is_match(re, text.as_ptr().cast(), text.len(), &mut matched) }, CRE_INVALID_UTF8);
    assert_eq!((cre_last_error_offset(), matched), (1000, false));
    unsafe { cre_regex_free(re) };
}

#[test]
fn the_header_declares_what_the_library_exports() {
    let header = include_str!("../include/corvus_regex_ecma262.h").replace("\r\n", "\n");
    let source = include_str!("../src/lib.rs");
    let exported: Vec<&str> = source
        .split("pub unsafe extern \"C\" fn ")
        .chain(source.split("pub extern \"C\" fn "))
        .skip(1)
        .filter_map(|s| s.split('(').next())
        .filter(|name| name.starts_with("cre_"))
        .collect();
    assert!(exported.len() >= 50, "{exported:?}");
    for name in &exported {
        assert!(header.contains(&format!(" {name}(")), "the header does not declare {name}");
    }
    // And the header declares nothing the library does not export.
    for line in header.lines().filter(|l| !l.starts_with(' ') && !l.starts_with('#') && !l.starts_with('/')) {
        if let Some(name) = line.split('(').next().and_then(|s| s.split([' ', '*']).next_back())
            && name.starts_with("cre_")
            && line.contains('(')
        {
            assert!(exported.contains(&name), "the library does not export {name}");
        }
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
    for (name, value) in [
        ("CRE_MAX_PATTERN_LENGTH", corvus::MAX_PATTERN_LENGTH),
        ("CRE_MAX_NESTING", corvus::MAX_NESTING),
        ("CRE_MAX_SIZE", corvus::MAX_SIZE),
        ("CRE_MAX_STACK", corvus::MAX_STACK),
    ] {
        assert!(header.contains(&format!("#define {name} {value}\n")), "{name} is not {value} in the header");
    }
    assert!(header.contains("#define CRE_UNSET SIZE_MAX\n") && CRE_UNSET == usize::MAX);
    assert!(header.contains("#define CRE_SPLIT_ALL 0xFFFFFFFFu\n") && CRE_SPLIT_ALL == u32::MAX);
}

#[test]
fn the_versions_agree() {
    // The library's version is the crate's (Cargo.toml), and so are the header's macros.
    assert_eq!(string(cre_version()), env!("CARGO_PKG_VERSION"));
    let parts = [env!("CARGO_PKG_VERSION_MAJOR"), env!("CARGO_PKG_VERSION_MINOR"), env!("CARGO_PKG_VERSION_PATCH")];
    let parts: Vec<u32> = parts.iter().map(|p| p.parse().unwrap()).collect();
    assert_eq!([CRE_VERSION_MAJOR, CRE_VERSION_MINOR, CRE_VERSION_PATCH], parts[..], "CRE_VERSION_* in src/lib.rs");
    let header = include_str!("../include/corvus_regex_ecma262.h").replace("\r\n", "\n");
    let version = format!(
        "#define CRE_VERSION_MAJOR {}\n#define CRE_VERSION_MINOR {}\n#define CRE_VERSION_PATCH {}\n",
        parts[0], parts[1], parts[2]
    );
    assert!(header.contains(&version), "CRE_VERSION_* in the header");
    // The C library's version follows the engine's.
    let engine = include_str!("../../corvus-regex-ecma262/Cargo.toml");
    let engine_version = engine.lines().find_map(|l| l.strip_prefix("version = \"")).map(|v| v.trim_end_matches('"'));
    assert_eq!(engine_version, Some(env!("CARGO_PKG_VERSION")), "the engine's version in its Cargo.toml");
    let manifest = include_str!("../Cargo.toml");
    let dependency = format!("version = \"{}\" }}", env!("CARGO_PKG_VERSION"));
    assert!(manifest.lines().any(|l| l.starts_with("corvus = ") && l.ends_with(&dependency)), "the engine dependency");
}
