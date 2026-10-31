//! A match allocates nothing once the thread's working memory has grown to what the pattern and the text need: not
//! `is_match`, not `find`, not a global search, and not a read of the groups into a `CaptureLocations`. Only the
//! allocations of the thread that measures are counted, so a thread of the test harness cannot make it fail.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use corvus_regex_ecma262::Regex;

struct Counting;

thread_local! {
    static COUNT: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        let _ = BYTES.try_with(|c| c.set(c.get() + layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        let _ = BYTES.try_with(|c| c.set(c.get() + new_size));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations(mut f: impl FnMut()) -> usize {
    // Twice to warm: the automaton, the program that records the groups and the working memory are built on use.
    f();
    f();
    let before = COUNT.with(Cell::get);
    f();
    COUNT.with(Cell::get) - before
}

#[test]
fn a_warm_match_allocates_nothing() {
    let text = "The year 2026, the month 10; Ünïcödé text, \u{1F600} and more letters after it. ".repeat(20);
    let text16: Vec<u16> = text.encode_utf16().collect();
    for (pattern, flags) in [
        (r"\d+", ""),
        (r"(?<y>\d{4})\D+(\d+)", "u"),
        (r"text|month", "i"),
        (r"(?<=the )\w+", "u"),
        (r"(t|e)\1", ""),
        (r"\p{L}+", "u"),
        (r"[\p{L}--[a-z]]+", "v"),
        (r"^.*$", "m"),
        (r"\bt\w*\b", "gi"),
    ] {
        let re = Regex::new(pattern, flags).unwrap();
        let mut locs = re.capture_locations();
        let n = allocations(|| {
            assert!(re.is_match(&text).unwrap());
            assert!(re.find(&text).unwrap().is_some());
            assert!(re.find_iter(&text).map(Result::unwrap).count() > 0);
            assert!(re.is_match_utf16(&text16).unwrap());
            assert!(re.find_iter_utf16(&text16).map(Result::unwrap).count() > 0);
        });
        assert_eq!(n, 0, "/{pattern}/{flags}: {n} allocations");
        let n = allocations(|| {
            let mut at = 0;
            while let Some(m) = re.captures_read_at(&mut locs, &text, at).unwrap() {
                at = if m.is_empty() { m.end() + 1 } else { m.end() };
                if at > text.len() {
                    break;
                }
            }
        });
        assert_eq!(n, 0, "/{pattern}/{flags}: {n} allocations reading the groups");
        // The global search a batch at a time, the groups too.
        let (mut spans, mut groups) = ([(0, 0); 16], [None::<(usize, usize)>; 48]);
        let n = allocations(|| {
            for _ in 0..2 {
                let mut it = re.find_iter(&text);
                while it.next_batch(&mut spans).unwrap() == spans.len() {}
                let mut it = re.find_iter_utf16(&text16);
                while it.next_batch(&mut spans).unwrap() == spans.len() {}
                let mut it = re.captures_iter(&text);
                while it.next_batch(&mut groups).unwrap() > 0 {}
                let mut it = re.captures_iter_utf16(&text16);
                while it.next_batch(&mut groups).unwrap() > 0 {}
            }
        });
        assert_eq!(n, 0, "/{pattern}/{flags}: {n} allocations in batches");
    }
}

/// The grammar-only reader allocates nothing for a pattern of ordinary size, valid or not, in any grammar: it keeps
/// its stacks in place, and a property name as its place in the pattern.
#[test]
fn the_grammar_only_reader_allocates_nothing() {
    let names = (0..12).map(|n| format!("(?<n{n}>a)")).collect::<String>() + r"\k<n11>";
    let nested = "(?:".repeat(60) + "a" + &")".repeat(60);
    let groups = "(a)".repeat(60);
    for (pattern, flags) in [
        (r"^[a-z][a-z0-9_]{0,29}$", "u"),
        (r"^(?<year>\d{4})-(?<month>\d{2})\k<year>$", "u"),
        (r"(?<a>x)|(?<a>y)", "u"),
        (r"^\p{Script=Greek}+\P{L}$", "u"),
        (r"[\p{L}--[a-z]&&\q{abc|d}]", "v"),
        (r"[^\p{RGI_Emoji}]", "v"),
        (r"(?i:a)(?<=b)(?!c)", ""),
        (r"\8[\c_]\k{2}", ""),
        (r"[z-a]", "u"),
        (r"(?<a>x)(?<a>y)", "u"),
        (names.as_str(), "u"),
        (nested.as_str(), "u"),
        (groups.as_str(), "u"),
    ] {
        let n = allocations(|| {
            std::hint::black_box(Regex::is_valid_pattern(pattern, flags));
        });
        assert_eq!(n, 0, "/{pattern}/{flags}: {n} allocations");
    }
}

/// Each named group keeps the disjunction and the alternative it lies in, not a copy of the path to it, so nested named
/// groups take memory in proportion to their number, not to its square. A name kept deep inside one alternative and
/// used again in the next walks out of the closed groups to the disjunction around both, and the walk is shortened as
/// it goes, so each name does not walk the whole depth again.
#[test]
fn named_groups_are_read_in_memory_and_time_in_proportion_to_the_pattern() {
    use corvus_regex_ecma262::{Edition, Flags, UnicodeVersion};
    let nested = (0..100_000).map(|k| format!("(?<g{k}>")).collect::<String>() + "a" + &")".repeat(100_000);
    let names = |text: &str| (0..20_000).map(|k| format!("(?<n{k}>{text})")).collect::<String>();
    let reused = "(?:".repeat(50_000) + &names("a") + &")".repeat(50_000) + "|" + &names("b");
    let u = Flags { unicode: true, ..Flags::default() };
    for (pattern, valid) in [(nested, true), (reused.clone(), true), (reused + "(?<n0>c)", false)] {
        let (before, clock) = (BYTES.with(Cell::get), std::time::Instant::now());
        assert_eq!(Regex::is_valid_pattern_with(&pattern, u, Edition::Current, UnicodeVersion::Default), valid);
        let (bytes, took) = (BYTES.with(Cell::get) - before, clock.elapsed());
        assert!(bytes < 64 * pattern.len(), "{bytes} bytes for a pattern of {}", pattern.len());
        assert!(took < std::time::Duration::from_secs(10), "{took:?} for a pattern of {}", pattern.len());
    }
}
