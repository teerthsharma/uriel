use serde_json::Value;
use uriel_terse::{count_tokens, estimate_tokens};

fn samples() -> Vec<(String, usize)> {
    let v: Value = serde_json::from_str(include_str!("fixtures/cl100k_samples.json")).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| (s["text"].as_str().unwrap().to_string(), s["real"].as_u64().unwrap() as usize))
        .collect()
}

fn mape(f: impl Fn(&str) -> usize) -> f64 {
    let s = samples();
    s.iter().map(|(t, r)| (f(t) as f64 - *r as f64).abs() / *r as f64).sum::<f64>() / s.len() as f64
}

#[test]
fn the_counter_is_within_five_percent_of_real_cl100k_counts_on_recorded_outputs() {
    let e = mape(count_tokens);
    assert!(e < 0.05, "mean error {e:.3}");
}

#[test]
fn it_is_far_closer_than_the_four_characters_rule_it_replaces() {
    let old = mape(estimate_tokens);
    let new = mape(count_tokens);
    assert!(old > 0.2, "the old rule is off by {old:.3}");
    assert!(new * 4.0 < old, "{new:.3} vs {old:.3}");
}

#[test]
fn the_total_over_all_samples_is_unbiased_within_three_percent() {
    let s = samples();
    let est: usize = s.iter().map(|(t, _)| count_tokens(t)).sum();
    let real: usize = s.iter().map(|(_, r)| *r).sum();
    let ratio = est as f64 / real as f64;
    assert!((0.97..=1.03).contains(&ratio), "{ratio:.3}");
}

#[test]
fn edge_inputs_are_total_and_monotone_in_length() {
    assert_eq!(count_tokens(""), 0);
    assert_eq!(count_tokens("   \t "), 0);
    assert_eq!(count_tokens("a"), 1);
    assert!(count_tokens("\u{4e2d}\u{6587}") >= 2);
    assert!(count_tokens("\u{1f600}") >= 1);
    let base = "token ".repeat(10);
    for n in 1..8 {
        assert!(count_tokens(&base.repeat(n + 1)) > count_tokens(&base.repeat(n)));
    }
    let hostile = "\u{0}\u{7f}\r\n\u{2028}".repeat(1000);
    let _ = count_tokens(&hostile);
}

#[test]
fn counting_is_deterministic_and_splits_additively_at_whitespace() {
    let t = "fn main() { let x = 1234567; }\n";
    assert_eq!(count_tokens(t), count_tokens(t));
    let a = count_tokens("alpha beta");
    let b = count_tokens("alpha") + count_tokens("beta");
    assert!(a.abs_diff(b) <= 1);
}
