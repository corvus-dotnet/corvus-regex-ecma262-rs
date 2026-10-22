//! A match allocates nothing once the thread's working memory has grown to what the pattern and the text need: not
//! `is_match`, not `find`, not a global search, and not a read of the groups into a `CaptureLocations`. Only the
//! allocations of the thread that measures are counted, so a thread of the test harness cannot make it fail.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use corvus_regex_ecma262::Regex;

struct Counting;

thread_local! {
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
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
