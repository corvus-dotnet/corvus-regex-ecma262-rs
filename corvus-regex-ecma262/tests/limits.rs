//! The limits on a regular expression (from the tests of the Go module's `pattern_match_limit_test.go`,
//! `pattern_size_test.go` and `pattern_bounds_test.go`, by way of the JSON Schema evaluator for Rust).
//!
//! A pattern and a text may both come from outside, so neither may be able to make a program hang or hold memory
//! out of proportion to its input. A match asked for with a limit of time ends at that limit, and with it at a cap
//! on the memory of its backtracking. A match that reaches it is an error, never a match and never no match. A
//! pattern has limits of size, and one over them is refused with an error that is not the one of a pattern that
//! is not valid.
//!
//! No test here depends on a quiet machine: the matches that are to reach a limit would run for longer than anyone
//! waits, and the upper bounds on time are generous.

use std::time::{Duration, Instant};

use corvus_regex_ecma262::{Error, Limit, MatchError, Regex};

/// A time no test should come near.
const GENEROUS: Duration = Duration::from_secs(20);

/// A pattern whose match on [`never_answered`] takes longer than any test waits: a group read back inside a
/// repetition, which nothing but the limit ends.
const NEVER_ANSWERS: &str = r"^(?:(a+)\1*)+$";

fn never_answered() -> String {
    "a".repeat(60) + "b"
}

#[test]
fn a_match_that_reaches_its_limit_is_an_error() {
    let limit = Duration::from_millis(50);
    let re = Regex::new(NEVER_ANSWERS, "u").unwrap();
    let text = never_answered();
    let start = Instant::now();
    let result = re.try_is_match(&text, limit);
    let took = start.elapsed();
    assert_eq!(result, Err(MatchError::Time));
    assert!(took >= limit && took < limit + GENEROUS, "abandoned after {took:?}");
}

#[test]
fn the_match_limit_is_for_each_match() {
    let re = Regex::new(NEVER_ANSWERS, "u").unwrap();
    for _ in 0..3 {
        assert_eq!(re.try_is_match("aaaa", Duration::from_millis(50)), Ok(true));
    }
}

#[test]
fn a_match_that_reaches_its_memory_limit_is_an_error() {
    let hour = Duration::from_secs(3600);
    let re = Regex::new("(?:a?){2147483647}", "u").unwrap();
    let start = Instant::now();
    assert_eq!(re.try_is_match("", hour), Err(MatchError::Memory));
    assert!(start.elapsed() < GENEROUS, "abandoned after {:?}", start.elapsed());
    // With no limit of time there is none of memory either, and the match is the engine's to finish: this one
    // gives up at once, because the text is shorter than the least the pattern consumes.
    let re = Regex::new("(?:a){2147483647}", "u").unwrap();
    assert!(!re.is_match("aaa"));
    assert_eq!(re.try_is_match("aaa", Duration::ZERO), Ok(false));
}

#[test]
fn an_ordinary_pattern_is_not_affected_by_the_limit() {
    let long = "ab".repeat(20000) + "\u{e9}";
    for (pattern, text, want) in [
        (r"^[a-z]+$", "abc", true),
        (r"^[a-z]+$", "abc1", false),
        (r"^x-", "x-a", true),
        (r"^\p{L}+$", "caf\u{e9}", true),
        (r"^\p{L}+$", "caf\u{e9} ", false),
        (r"^(?=.*\d)\w+$", "abc1", true),
        (r"^(?=.*\d)\w+$", "abcd", false),
        (r"^(a+)\1$", "aaaa", true),
        (r"^(a+)\1$", "aaa", false),
        (r"(?<=\$)\d+", "cost $42", true),
        (r"^(?:[a-z]+\.)+[a-z]+$", "a.b.c", true),
        ("^[ab\u{e9}]*c$", &long, false),
    ] {
        let re = Regex::new(pattern, "u").unwrap();
        for limit in [Duration::from_secs(1), Duration::from_secs(10), Duration::ZERO] {
            assert_eq!(re.try_is_match(text, limit), Ok(want), "{pattern} with the limit {limit:?}");
        }
        assert_eq!(re.is_match(text), want, "{pattern}");
    }
}

/// A search may not read a text from every position for each position. Each of these answers, or ends at the limit.
#[test]
fn no_search_reads_the_text_from_every_position() {
    let items = "ab,".repeat(33333);
    let limit = Duration::from_millis(200);
    for (pattern, text) in [
        (r"[a-z]{50000}[0-9]|x", "a".repeat(100000)),
        (r"^(?:[a-z]+,)*[a-z,]+[0-9]$", items.clone()),
        (r"^(?:[a-z]+,)*[a-z]{2}[0-9]$", items.clone()),
        ("^(?:[a-z\u{e9}]+,)*[a-z]{2}[0-9]$", format!("\u{e9},{items}")),
    ] {
        let re = Regex::new(pattern, "u").unwrap();
        let start = Instant::now();
        let result = re.try_is_match(&text, limit);
        let took = start.elapsed();
        assert!(
            matches!(result, Ok(false) | Err(_)) && took < limit + GENEROUS,
            "{pattern}: {result:?} after {took:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The limits of size.

fn nest(depth: usize) -> String {
    "(?:".repeat(depth) + "a" + &")".repeat(depth)
}

#[test]
fn a_pattern_over_a_limit_of_size_is_refused() {
    let escape = |zeros: usize| format!("\\u{{{}41}}", "0".repeat(zeros));
    let class = |members: usize| format!("[{}]", r"ĀĂ".repeat(members));
    for (name, at, over, limit, value, reason) in [
        (
            "length",
            escape((1 << 20) - 6),
            escape((1 << 20) - 5),
            Limit::Length,
            1 << 20,
            "the pattern is longer than 1048576 bytes",
        ),
        ("nesting", nest(999), nest(1000), Limit::Nesting, 1000, "the groups of the pattern nest more than 1000 deep"),
        (
            "size",
            "a".repeat(16384),
            "a".repeat(16385),
            Limit::Size,
            1 << 20,
            "the pattern has a size of more than 1048576 bytes when it is read",
        ),
        (
            "size of a class",
            class(60000),
            class(70000),
            Limit::Size,
            1 << 20,
            "the pattern has a size of more than 1048576 bytes when it is read",
        ),
        (
            "nesting, and not valid",
            nest(999),
            nest(1000) + "*+)",
            Limit::Nesting,
            1000,
            "the groups of the pattern nest more than 1000 deep",
        ),
    ] {
        let start = Instant::now();
        if let Err(e) = Regex::new(&at, "u") {
            panic!("{name}: the pattern at the limit: {e}");
        }
        let e = Regex::new(&over, "u").expect_err("the pattern over the limit compiles");
        assert!(start.elapsed() < GENEROUS, "{name}: compiling took {:?}", start.elapsed());
        let Error::TooLarge(large) = &e else {
            panic!("{name}: {e:.200}, want the error of a pattern that is too large")
        };
        assert_eq!((large.limit, large.value), (limit, value), "{name}");
        assert_eq!(e.to_string(), reason, "{name}");
    }
}

#[test]
fn a_pattern_that_is_not_valid_is_not_too_large() {
    for pattern in [
        "(".to_string(),
        "a**".into(),
        "[b-a]".into(),
        r"(?<n>a)\k<m>".into(),
        "(?:".repeat(999) + "a" + &")".repeat(1000),
    ] {
        for flags in ["", "u"] {
            let e = Regex::new(&pattern, flags).expect_err("a pattern that is not valid compiles");
            assert!(matches!(e, Error::Invalid(_)), "{pattern:.20}: {e}");
        }
    }
}
