//! The process-wide default of the match timeout through the C library. It is the only test of its binary, so that no
//! other test compiles a regular expression while it has a default set.

use corvus_regex_ecma262::*;

#[test]
fn a_regular_expression_takes_the_default_timeout_when_it_is_compiled() {
    assert_eq!(cre_default_match_timeout(), 0);
    cre_set_default_match_timeout(25);
    assert_eq!(cre_default_match_timeout(), 25);
    let pattern = r"^(?:(a+)\1*)+$";
    let mut re = std::ptr::null_mut();
    let status = unsafe { cre_regex_new(pattern.as_ptr().cast(), pattern.len(), "u".as_ptr().cast(), 1, &mut re) };
    cre_set_default_match_timeout(0);
    assert_eq!(cre_default_match_timeout(), 0);
    assert_eq!(status, CRE_OK);
    assert_eq!(unsafe { cre_regex_match_timeout(re) }, 25);
    let text = "a".repeat(60) + "b";
    let mut matched = true;
    assert_eq!(unsafe { cre_regex_is_match(re, text.as_ptr().cast(), text.len(), &mut matched) }, CRE_MATCH_LIMIT);
    unsafe { cre_regex_free(re) };
}
