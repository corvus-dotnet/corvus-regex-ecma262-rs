//! The process-wide default of the match timeout. It is the only test of its binary, so that no other test compiles a
//! pattern while it has a default set.

use std::time::Duration;

use corvus_regex_ecma262::{Flags, MatchError, Regex, default_match_timeout, set_default_match_timeout};

#[test]
fn a_pattern_takes_the_default_timeout_when_it_is_compiled() {
    assert_eq!(default_match_timeout(), None);
    let before = Regex::new(r"^(?:(a+)\1*)+$", "u").unwrap();
    set_default_match_timeout(Some(Duration::from_millis(30)));
    assert_eq!(default_match_timeout(), Some(Duration::from_millis(30)));
    let re = Regex::new(r"^(?:(a+)\1*)+$", "u").unwrap();
    let flagged = Regex::with_flags(r"^(?:(a+)\1*)+$", Flags::parse("u").unwrap()).unwrap();
    let own = Regex::with_timeout("a", Flags::default(), Some(Duration::from_secs(5))).unwrap();
    let none = Regex::with_timeout("a", Flags::default(), None).unwrap();
    set_default_match_timeout(None);
    assert_eq!(default_match_timeout(), None);
    let text = "a".repeat(60) + "b";
    assert_eq!(re.match_timeout(), Some(Duration::from_millis(30)));
    assert_eq!(re.is_match(&text), Err(MatchError::Time));
    assert_eq!(flagged.is_match(&text), Err(MatchError::Time));
    // A pattern compiled before keeps its own, and one given a timeout, or none, keeps that.
    assert_eq!(before.match_timeout(), None);
    assert_eq!(own.match_timeout(), Some(Duration::from_secs(5)));
    assert_eq!(none.match_timeout(), None);
    // A default beyond what nanoseconds in 64 bits hold is the longest they hold.
    set_default_match_timeout(Some(Duration::MAX));
    assert_eq!(default_match_timeout(), Some(Duration::from_nanos(u64::MAX)));
    set_default_match_timeout(Some(Duration::ZERO));
    assert_eq!(default_match_timeout(), None);
}
