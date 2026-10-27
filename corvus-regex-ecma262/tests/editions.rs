//! The edition of the grammar and the version of Unicode: ECMAScript 2020 refuses what later editions added and
//! nothing else, and each version of Unicode has its own scripts and case folding, in the matcher as in the parser.

use std::time::Duration;

use corvus_regex_ecma262::{Edition, Error, Flags, Regex, RegexBuilder, UnicodeVersion};

// U+10570 and U+10597 are a capital and a small letter of Vithkuqi, a script of Unicode 14. U+A7C0 and U+A7C1 are the
// capital and the small Old Polish O, also of Unicode 14.
const VITHKUQI_PAIR: &str = "\u{10570}\u{10597}";
const OLD_POLISH_PAIR: &str = "\u{A7C0}\u{A7C1}";

fn build(pattern: &str, flags: &str, edition: Edition, version: UnicodeVersion) -> Result<Regex, Error> {
    RegexBuilder::new(pattern).flags(flags).edition(edition).unicode_version(version).build()
}

fn under(pattern: &str, flags: &str, version: UnicodeVersion) -> Regex {
    build(pattern, flags, Edition::Current, version).unwrap()
}

#[test]
fn the_defaults_are_todays_grammar_and_v8s_unicode() {
    let re = Regex::new("a", "").unwrap();
    assert_eq!(re.edition(), Edition::Current);
    assert_eq!(re.unicode_version(), UnicodeVersion::V17);
    assert_eq!(RegexBuilder::new("a").build().unwrap().unicode_version(), UnicodeVersion::V17);
}

#[test]
fn a_version_is_resolved_when_the_pattern_is_compiled() {
    for (asked, resolved) in [
        (UnicodeVersion::Default, UnicodeVersion::V17),
        (UnicodeVersion::V13, UnicodeVersion::V13),
        (UnicodeVersion::V17, UnicodeVersion::V17),
        (UnicodeVersion::V18, UnicodeVersion::V18),
        (UnicodeVersion::Latest, UnicodeVersion::V18),
    ] {
        assert_eq!(under("a", "", asked).unicode_version(), resolved, "{asked:?}");
        assert_eq!(asked.resolve(), resolved);
    }
    assert_eq!(UnicodeVersion::Latest.major(), 18);
}

#[test]
fn each_version_has_its_own_scripts() {
    for (pattern, version, valid) in [
        (r"\p{Script=Vithkuqi}", UnicodeVersion::V13, false),
        (r"\p{Script=Vithkuqi}", UnicodeVersion::Default, true),
        (r"\p{Script=Yezidi}", UnicodeVersion::V13, true),
        (r"\p{Script=Jurchen}", UnicodeVersion::Default, false),
        (r"\p{scx=Jurc}", UnicodeVersion::V18, true),
        (r"\p{Script=Jurchen}", UnicodeVersion::Latest, true),
    ] {
        let flags = Flags::parse("u").unwrap();
        assert_eq!(Regex::is_valid_with(pattern, flags, Edition::Current, version), valid, "{pattern} {version:?}");
        assert_eq!(build(pattern, "u", Edition::Current, version).is_ok(), valid, "{pattern} {version:?}");
    }
}

#[test]
fn a_property_set_is_its_versions_however_the_cache_was_filled() {
    // The same expression under two versions, in both orders: the cache of property sets is one for each version.
    let letter = "\u{10597}";
    for _ in 0..2 {
        assert!(under(r"^\p{Ll}$", "u", UnicodeVersion::V17).is_match(letter).unwrap());
        assert!(!under(r"^\p{Ll}$", "u", UnicodeVersion::V13).is_match(letter).unwrap());
    }
}

#[test]
fn a_literal_in_a_case_insensitive_pattern_folds_as_its_version_folds() {
    assert!(under(r"\u{10570}", "iu", UnicodeVersion::V17).is_match("\u{10597}").unwrap());
    assert!(!under(r"\u{10570}", "iu", UnicodeVersion::V13).is_match("\u{10597}").unwrap());
    assert!(under(r"Ꟁ", "i", UnicodeVersion::V17).is_match("\u{A7C1}").unwrap());
    assert!(!under(r"Ꟁ", "i", UnicodeVersion::V13).is_match("\u{A7C1}").unwrap());
}

#[test]
fn a_backreference_in_a_case_insensitive_pattern_folds_as_its_version_folds() {
    for (flags, text) in [("iu", VITHKUQI_PAIR), ("i", OLD_POLISH_PAIR)] {
        let first: String = text.chars().take(1).collect();
        let twice = first.repeat(2);
        let t16: Vec<u16> = text.encode_utf16().collect();
        for pattern in [r"(.)\1", r"(?<=\1(.))", r"(?:x|(.))+\1$"] {
            for version in [UnicodeVersion::V17, UnicodeVersion::V18] {
                let re = under(pattern, flags, version);
                assert!(re.is_match(text).unwrap(), "{pattern} /{flags} under {version:?}");
                assert!(re.is_match_utf16(&t16).unwrap(), "{pattern} /{flags} over UTF-16 under {version:?}");
            }
            let re = under(pattern, flags, UnicodeVersion::V13);
            assert!(!re.is_match(text).unwrap(), "{pattern} /{flags} under 13");
            assert!(!re.is_match_utf16(&t16).unwrap(), "{pattern} /{flags} over UTF-16 under 13");
            assert!(re.is_match(&twice).unwrap(), "{pattern} /{flags} under 13, the same letter twice");
            // The program that records every group folds as the one that finds the match.
            assert!(re.captures(&twice).unwrap().is_some(), "{pattern} /{flags} captures under 13");
            assert!(re.captures(text).unwrap().is_none(), "{pattern} /{flags} captures under 13");
        }
    }
}

#[test]
fn the_grammar_of_2020_refuses_what_later_editions_added() {
    for (pattern, flags) in [
        ("(?i:a)", ""),
        ("(?-i:a)", "i"),
        ("(?s:.)", "u"),
        ("(?<a>x)|(?<a>y)", ""),
        ("(?<a>x)|(?<a>y)", "u"),
        (r"\p{Script=Yezidi}", "u"),
        (r"\p{sc=Vith}", "u"),
        (r"\p{Script_Extensions=Toto}", "u"),
    ] {
        assert!(
            matches!(build(pattern, flags, Edition::Es2020, UnicodeVersion::Latest), Err(Error::Invalid(_))),
            "{pattern} /{flags} in 2020"
        );
        assert!(build(pattern, flags, Edition::Current, UnicodeVersion::Latest).is_ok(), "{pattern} /{flags} today");
    }
}

#[test]
fn the_grammar_of_2020_accepts_what_it_had() {
    for (pattern, flags) in [
        (r"\p{Script=Greek}", "u"),
        (r"\p{L}\p{gc=Lu}\p{Lowercase}", "u"),
        (r"\P{scx=Latn}", "iu"),
        (r"\p{Script=Yezidi}", ""),
        (r"(?<a>x)(?<b>y)\k<a>", "u"),
        (r"(?<=a)(?<!b)", "s"),
        (r"(?:i)", ""),
    ] {
        assert!(build(pattern, flags, Edition::Es2020, UnicodeVersion::V13).is_ok(), "{pattern} /{flags}");
    }
}

#[test]
fn the_grammar_of_2020_has_no_d_or_v_flag() {
    for flags in ["d", "v"] {
        assert!(matches!(build("a", flags, Edition::Es2020, UnicodeVersion::Default), Err(Error::Flags(_))), "{flags}");
        assert!(!Regex::is_valid_with("a", Flags::parse(flags).unwrap(), Edition::Es2020, UnicodeVersion::Default));
        assert!(build("a", flags, Edition::Current, UnicodeVersion::Default).is_ok(), "{flags}");
    }
}

#[test]
fn the_builder_takes_a_match_timeout_and_the_default_without_one() {
    let re = RegexBuilder::new("a+").flags("u").match_timeout(Some(Duration::from_millis(50))).build().unwrap();
    assert_eq!(re.match_timeout(), Some(Duration::from_millis(50)));
    assert_eq!(re.edition(), Edition::Current);
    assert_eq!(RegexBuilder::new("a").build().unwrap().match_timeout(), None);
    assert!(matches!(RegexBuilder::new("a").flags("gg").build(), Err(Error::Flags(_))));
    assert!(matches!(RegexBuilder::new("a{2,1}").flags("u").build(), Err(Error::Invalid(_))));
}
