//! The public surface: what each member gives, over UTF-8 and UTF-16, as `RegExp` and `String.prototype` give it.

use corvus_regex_ecma262::{Error, Flags, Regex};

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[test]
fn flags_are_read_as_new_regexp_reads_them() {
    let f = Flags::parse("dgimsuy").unwrap();
    assert!(f.has_indices && f.global && f.ignore_case && f.multiline && f.dot_all && f.unicode && f.sticky);
    assert_eq!(f.to_string(), "dgimsuy");
    assert_eq!(Flags::parse("v").unwrap().to_string(), "v");
    for bad in ["uv", "gg", "x", "u "] {
        assert!(matches!(Regex::new("a", bad), Err(Error::Flags(_))), "{bad:?}");
    }
    assert_eq!(Regex::new("a+", "gi").unwrap().to_string(), "/a+/gi");
}

#[test]
fn a_pattern_is_read_by_the_grammar_of_its_flags() {
    // An identity escape is Annex B's alone, and set notation the v flag's alone.
    assert!(Regex::new(r"\&", "").is_ok());
    assert!(matches!(Regex::new(r"\&", "u"), Err(Error::Invalid(_))));
    assert!(Regex::new(r"[\w--\d]", "v").unwrap().is_match("a"));
    assert!(!Regex::new(r"^[\w--\d]$", "v").unwrap().is_match("1"));
    assert!(Regex::new(r"^[\w--\d]$", "u").is_err());
    assert!(Regex::is_valid(r"\p{RGI_Emoji}", "v"));
    assert!(!Regex::is_valid(r"\p{RGI_Emoji}", "u"));
    assert!(!Regex::is_valid(r"[^\p{RGI_Emoji}]", "v"));
}

#[test]
fn find_and_find_at() {
    let re = Regex::new(r"\d+", "").unwrap();
    let m = re.find("ab 12 345").unwrap();
    assert_eq!((m.start(), m.end(), m.as_str()), (3, 5, "12"));
    assert_eq!(re.find_at("ab 12 345", 5).unwrap().as_str(), "345");
    assert!(re.find_at("ab 12 345", 10).is_none());
    assert!(re.find("none").is_none());
}

#[test]
fn the_sticky_flag_anchors_each_match_at_its_start() {
    let re = Regex::new("a", "y").unwrap();
    assert!(re.find("ba").is_none());
    assert_eq!(re.find_at("ba", 1).unwrap().start(), 1);
    assert!(!re.is_match("ba"));
    let all: Vec<usize> = re.find_iter("aab").map(|m| m.start()).collect();
    assert_eq!(all, [0, 1]);
}

#[test]
fn a_global_search_steps_over_an_empty_match_by_a_character() {
    let re = Regex::new("", "u").unwrap();
    let starts: Vec<usize> = re.find_iter("a\u{1F600}b").map(|m| m.start()).collect();
    assert_eq!(starts, [0, 1, 5, 6]);
    // Without the u flag the step is a code unit, which over UTF-8 stands between the halves at the third byte.
    let re = Regex::new("", "").unwrap();
    let starts: Vec<usize> = re.find_iter("a\u{1F600}b").map(|m| m.start()).collect();
    assert_eq!(starts, [0, 1, 3, 5, 6]);
    let starts: Vec<usize> = re.find_iter_utf16(&utf16("a\u{1F600}b")).map(|m| m.start()).collect();
    assert_eq!(starts, [0, 1, 2, 3, 4]);
}

#[test]
fn captures_and_names() {
    let re = Regex::new(r"(?<y>\d{4})-(?<m>\d{2})(?:-(?<d>\d{2}))?", "").unwrap();
    assert_eq!(re.captures_len(), 4);
    assert_eq!(re.group_names().collect::<Vec<_>>(), ["y", "m", "d"]);
    let caps = re.captures("on 2026-10!").unwrap();
    assert_eq!(caps.get(0).unwrap().as_str(), "2026-10");
    assert_eq!(caps.name("y").unwrap().as_str(), "2026");
    assert_eq!(caps.name("m").unwrap().range(), 8..10);
    assert!(caps.name("d").is_none());
    assert!(caps.get(3).is_none() && caps.get(4).is_none());
    // Two groups of one name in separate alternatives: the one that took part.
    let re = Regex::new(r"(?<x>a)|(?<x>b)", "").unwrap();
    assert_eq!(re.captures("b").unwrap().name("x").unwrap().start(), 0);
}

#[test]
fn the_groups_of_a_repetition_are_those_of_its_last_iteration() {
    let re = Regex::new(r"(?:(a)|b)+", "").unwrap();
    let caps = re.captures("ab").unwrap();
    assert!(caps.get(1).is_none(), "the last iteration took b, so group 1 is cleared");
    let mut locs = re.capture_locations();
    let m = re.captures_read_at(&mut locs, "xba", 0).unwrap();
    assert_eq!((m.start(), m.end()), (1, 3));
    assert_eq!(locs.get(1), Some((2, 3)));
}

#[test]
fn replace_substitutes_as_get_substitution_does() {
    let re = Regex::new(r"(\w)(\w)?", "").unwrap();
    assert_eq!(re.replace("ab cd", "[$2$1]"), "[ba] cd");
    assert_eq!(re.replace_all("ab c", "[$2$1]"), "[ba] [c]");
    assert_eq!(Regex::new(r"(\w)(\w)?", "g").unwrap().replace("ab c", "[$2$1]"), "[ba] [c]");
    let re = Regex::new("b", "").unwrap();
    assert_eq!(re.replace("abc", "<$&|$`|$'|$$|$1|$<n>>"), "a<b|a|c|$|$1|$<n>>c");
    let re = Regex::new("(?<n>b)", "").unwrap();
    assert_eq!(re.replace("abc", "<$<n>|$<m>|$01|$10|$<n"), "a<b||b|b0|$<nc");
    // The name runs to the next `>`, wherever it is.
    assert_eq!(re.replace("abc", "<$<n|$1>"), "a<c");
    let re = Regex::new(r"(?<first>\w+) (?<last>\w+)", "").unwrap();
    assert_eq!(re.replace_with("Ada Lovelace", |c| c.name("last").unwrap().as_str().to_uppercase()), "LOVELACE");
    // Nothing matched: the text itself.
    assert!(matches!(re.replace("x", "y"), std::borrow::Cow::Borrowed("x")));
}

#[test]
fn split_splices_the_groups_in() {
    let re = Regex::new(r"(-)|(\+)", "").unwrap();
    let parts: Vec<Option<String>> = re.split("a-b+c").into_iter().map(|p| p.map(|s| s.into_owned())).collect();
    let want = [Some("a"), Some("-"), None, Some("b"), None, Some("+"), Some("c")];
    assert_eq!(parts, want.map(|p| p.map(String::from)));
    assert_eq!(re.splitn("a-b+c", 2).len(), 2);
    // An empty match at the start or the end of the text splits nothing, and the empty text is one piece unless the
    // pattern matches it.
    let re = Regex::new("", "").unwrap();
    assert_eq!(re.split("ab").len(), 2);
    assert_eq!(re.split("").len(), 0);
    assert_eq!(Regex::new("x", "").unwrap().split("").len(), 1);
}

#[test]
fn utf16_text_with_lone_surrogates() {
    let text = [0x61, 0xD800, 0x62, 0xD83D, 0xDE00];
    // With the u flag a lone surrogate is a code point of its own, and a pair is one character.
    let re = Regex::new(r"^a\ud800b.$", "u").unwrap();
    assert!(re.is_match_utf16(&text));
    let re = Regex::new(r"^a.b..$", "").unwrap();
    assert!(re.is_match_utf16(&text), "without the u flag a pair is two units");
    let re = Regex::new(r"\udc00", "u").unwrap();
    assert!(!re.is_match_utf16(&utf16("\u{10400}")), "the half of a whole pair is not a lone surrogate");
    let re = Regex::new(r"(?<x>\w)", "").unwrap();
    let caps = re.captures_utf16(&text).unwrap();
    assert_eq!(caps.name("x").unwrap().as_slice(), &[0x61]);
    assert_eq!(
        re.replace_all_utf16(&text, &utf16("<$1>")).into_owned(),
        [0x3C, 0x61, 0x3E, 0xD800, 0x3C, 0x62, 0x3E, 0xD83D, 0xDE00]
    );
}

#[test]
fn a_backreference_does_not_end_inside_a_surrogate_pair_with_the_u_flag() {
    // The group holds a lone high surrogate; what follows is a pair, one code point that is not it.
    let re = Regex::new(r"^(.)\1", "u").unwrap();
    assert!(!re.is_match_utf16(&[0xD800, 0xD800, 0xDC00]));
    assert!(Regex::new(r"^(.)\1", "").unwrap().is_match_utf16(&[0xD800, 0xD800, 0xDC00]));
}

#[test]
fn a_half_character_over_utf8_is_written_as_a_replacement_character() {
    // Without the u flag the pattern matches the high half of the character, which UTF-8 cannot hold.
    let re = Regex::new(r"\ud83d", "").unwrap();
    let m = re.find("a\u{1F600}").unwrap();
    assert_eq!((m.start(), m.end()), (1, 3));
    assert_eq!(m.as_str(), "");
    assert_eq!(m.as_cow(), "\u{FFFD}");
    assert_eq!(re.replace("a\u{1F600}", "x"), "ax\u{FFFD}");
}

#[test]
fn modifiers_and_flags_mean_the_same() {
    for (flags, pattern) in [("i", "k"), ("", "(?i:k)")] {
        let re = Regex::new(&format!("^{pattern}$"), &format!("{flags}u")).unwrap();
        assert!(re.is_match("K") && re.is_match("\u{212A}"), "/{pattern}/{flags}u");
    }
    assert!(Regex::new("^b$", "m").unwrap().is_match("a\nb\nc"));
    assert!(Regex::new("a.b", "s").unwrap().is_match("a\nb"));
    assert!(!Regex::new("a.b", "").unwrap().is_match("a\nb"));
}

#[test]
fn a_run_of_the_dot_with_the_s_flag_is_the_rest_of_the_text() {
    // `.*` with the s flag is answered without reading the text: from where the search starts (a code point's start
    // with the u or v flag, so a start between the halves of a pair moves back to the pair's start; any code unit
    // without them) to the end. Each answer is compared with the matcher's, which a group in the pattern makes the
    // search use, and with that rule written out, over UTF-16 text with lone halves anywhere, the last unit among
    // them, and every start.
    let texts: [&[u16]; 6] = [
        &[],
        &[0x61, 0xD800],
        &[0x61, 0xDC00],
        &[0xDC00, 0x61, 0xD83D, 0xDE00],
        &[0xD83D, 0xDE00],
        &[0xDC00, 0x62, 0xD83D, 0xDE00, 0xD800, 0x63, 0xDBFF],
    ];
    for flags in ["s", "su", "sv", "sy", "suy"] {
        let unicode = flags.contains('u') || flags.contains('v');
        let shortcut = Regex::new(".*", flags).unwrap();
        let matcher = Regex::new("(.*)", flags).unwrap();
        let plus = Regex::new(".+", flags).unwrap();
        let plus_matcher = Regex::new("(.+)", flags).unwrap();
        for text in texts {
            for start in 0..=text.len() {
                let mid_pair = start > 0
                    && start < text.len()
                    && (0xDC00..=0xDFFF).contains(&text[start])
                    && (0xD800..=0xDBFF).contains(&text[start - 1]);
                let from = if unicode && mid_pair { start - 1 } else { start };
                let got = shortcut.find_at_utf16(text, start).map(|m| (m.start(), m.end()));
                let want = matcher.find_at_utf16(text, start).map(|m| (m.start(), m.end()));
                assert_eq!(got, want, "/.*/{flags} over {text:x?} from {start}");
                assert_eq!(got, Some((from, text.len())), "/.*/{flags} over {text:x?} from {start}");
                let got = plus.find_at_utf16(text, start).map(|m| (m.start(), m.end()));
                let want = plus_matcher.find_at_utf16(text, start).map(|m| (m.start(), m.end()));
                assert_eq!(got, want, "/.+/{flags} over {text:x?} from {start}");
                assert_eq!(got, (from < text.len()).then_some((from, text.len())), "/.+/{flags} {text:x?} {start}");
            }
            // The global search: the rest of the text, then the empty match at its end.
            let all: Vec<(usize, usize)> = shortcut.find_iter_utf16(text).map(|m| (m.start(), m.end())).collect();
            let want: Vec<(usize, usize)> = matcher.find_iter_utf16(text).map(|m| (m.start(), m.end())).collect();
            assert_eq!(all, want, "/.*/{flags} global over {text:x?}");
            if !flags.contains('y') {
                let expected: &[(usize, usize)] =
                    if text.is_empty() { &[(0, 0)] } else { &[(0, text.len()), (text.len(), text.len())] };
                assert_eq!(all, expected, "/.*/{flags} global over {text:x?}");
            }
        }
    }
}

#[test]
fn a_batch_of_matches_is_what_find_iter_gives() {
    // Each shape the batch has a loop for (one class, one loop of a class with each way of finding a run's end, the
    // literals, the literals with forms), the matcher, the sticky flag and an anchored pattern, with empty matches
    // between the halves of a pair and lone halves in the UTF-16 text.
    let patterns: [(&str, &str); 22] = [
        (r"\p{L}", ""),
        (r"\p{L}", "u"),
        (r"[^\n]*", ""),
        (r".*", ""),
        (r".*", "u"),
        (r".*", "s"),
        (r".+", "u"),
        (r"[a-z]*", ""),
        (r"\w+", ""),
        (r"x*", ""),
        (r"x*", "u"),
        (r"", "u"),
        (r"", ""),
        (r"ab", ""),
        (r"ab|cd", ""),
        (r"ab", "i"),
        (r"the\s+\w+", ""),
        (r"(a)\1", ""),
        (r"a", "y"),
        (r"^a", "m"),
        (r"\b\w", ""),
        (r"\ud83d", ""),
    ];
    let texts = ["", "a", "ab cd AB\nthe  cat x\u{1F600}x\r\naa a\u{2028}é\u{1F600}", "\n\n", "aaa ab"];
    for (pattern, flags) in patterns {
        let re = Regex::new(pattern, flags).unwrap();
        for text in texts {
            let want: Vec<(usize, usize)> = re.find_iter(text).map(|m| (m.start(), m.end())).collect();
            let mut t16 = utf16(text);
            t16.push(0xD800);
            t16.insert(0, 0xDC00);
            let want16: Vec<(usize, usize)> = re.find_iter_utf16(&t16).map(|m| (m.start(), m.end())).collect();
            for size in [1, 2, 3, 7, 64] {
                let mut it = re.find_iter(text);
                let (mut got, mut spans) = (Vec::new(), vec![(0, 0); size]);
                loop {
                    let n = it.next_batch(&mut spans);
                    got.extend_from_slice(&spans[..n]);
                    if n < size {
                        break;
                    }
                }
                assert_eq!(got, want, "/{pattern}/{flags} over {text:?} in batches of {size}");
                assert_eq!(it.next_batch(&mut spans), 0);
                assert!(it.next().is_none());
                let mut it = re.find_iter_utf16(&t16);
                let mut got = Vec::new();
                // Batches and single matches mixed.
                loop {
                    let n = it.next_batch(&mut spans);
                    got.extend_from_slice(&spans[..n]);
                    if n < size {
                        break;
                    }
                    got.extend(it.next().map(|m| (m.start(), m.end())));
                }
                assert_eq!(got, want16, "/{pattern}/{flags} over UTF-16 {t16:?} in batches of {size}");
            }
            assert_eq!(re.find_iter(text).next_batch::<(usize, usize)>(&mut []), 0);
        }
    }
}

#[test]
fn a_batch_of_captures_is_what_captures_iter_gives() {
    let patterns: [(&str, &str); 9] = [
        (r"(\d)(x)?", ""),
        (r"(?<y>\d{4})-(?<m>\d{2})", "u"),
        (r"(?:(?<a>x)|(?<a>y))(z)?", ""),
        (r"(a)\1", ""),
        (r"(?<=(a))b", ""),
        (r"(x*)", "u"),
        (r"(x*)", ""),
        (r"\w+", ""),
        (r"(\d)", "y"),
    ];
    let texts = ["", "1x 2 2026-10 x yz aab ab\u{1F600}", "12x"];
    for (pattern, flags) in patterns {
        let re = Regex::new(pattern, flags).unwrap();
        let width = re.captures_len();
        for text in texts {
            let span = |m: Option<corvus_regex_ecma262::Match<'_, _>>| m.map(|m| (m.start(), m.end()));
            let want: Vec<Option<(usize, usize)>> =
                re.captures_iter(text).flat_map(|c| (0..width).map(|k| span(c.get(k))).collect::<Vec<_>>()).collect();
            let mut t16 = utf16(text);
            t16.push(0xDC00);
            let span16 = |m: Option<corvus_regex_ecma262::Match<'_, [u16]>>| m.map(|m| (m.start(), m.end()));
            let want16: Vec<Option<(usize, usize)>> = re
                .captures_iter_utf16(&t16)
                .flat_map(|c| (0..width).map(|k| span16(c.get(k))).collect::<Vec<_>>())
                .collect();
            for size in [1, 2, 5, 11] {
                // Room for `size` entries: as many matches as that holds, and what is left over is left alone.
                let mut groups = vec![Some((7, 7)); size.max(width)];
                let matches = groups.len() / width;
                let mut it = re.captures_iter(text);
                let mut got = Vec::new();
                loop {
                    let n = it.next_batch(&mut groups);
                    got.extend_from_slice(&groups[..n * width]);
                    if n < matches {
                        break;
                    }
                }
                assert_eq!(got, want, "/{pattern}/{flags} over {text:?}, {matches} a batch");
                assert!(groups[matches * width..].iter().all(|g| *g == Some((7, 7))));
                let mut it = re.captures_iter_utf16(&t16);
                let mut got = Vec::new();
                loop {
                    let n = it.next_batch(&mut groups);
                    got.extend_from_slice(&groups[..n * width]);
                    if n < matches {
                        break;
                    }
                }
                assert_eq!(got, want16, "/{pattern}/{flags} over UTF-16 {t16:?}, {matches} a batch");
            }
            // Too little room for one match.
            let mut small = vec![None::<(usize, usize)>; width - 1];
            assert_eq!(re.captures_iter(text).next_batch(&mut small), 0);
        }
    }
}
