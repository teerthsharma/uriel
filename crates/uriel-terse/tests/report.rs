use uriel_terse::{estimate_tokens, Report, CHARS_PER_TOKEN};

#[test]
fn proxy_is_ceil_chars_over_four() {
    assert_eq!(CHARS_PER_TOKEN, 4);
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("a"), 1);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcde"), 2);
    assert_eq!(estimate_tokens("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}"), 2);
}

#[test]
fn report_ratio() {
    let r = Report::measure(&"a".repeat(400), &"a".repeat(100));
    assert_eq!((r.before, r.after), (100, 25));
    assert!((r.saved_ratio - 0.75).abs() < 1e-12);
    assert_eq!(Report::from_counts(0, 0).saved_ratio, 0.0);
    assert!(Report::from_counts(10, 12).saved_ratio < 0.0);
}
