use uriel_mcp::prefix_tree::{agreement, dominant_prefix_len, PrefixLearner};

fn refs(v: &[Vec<u8>]) -> Vec<&[u8]> {
    v.iter().map(Vec::as_slice).collect()
}

fn rotated(samples: &[Vec<u8>], by: usize) -> Vec<Vec<u8>> {
    let mut v = samples.to_vec();
    v.rotate_left(by % samples.len());
    if by % 2 == 1 {
        v.reverse();
    }
    v
}

#[test]
fn an_empty_window_has_prefix_length_zero() {
    assert_eq!(dominant_prefix_len(&[], 0.95), 0);
}

#[test]
fn a_single_sample_is_its_own_prefix() {
    assert_eq!(dominant_prefix_len(&[b"scaffold"], 0.95), 8);
}

#[test]
fn identical_samples_share_their_full_length() {
    assert_eq!(dominant_prefix_len(&[b"abcd", b"abcd", b"abcd"], 0.95), 4);
}

#[test]
fn a_two_two_tie_stops_at_the_divergence_under_the_strict_majority() {
    let s: [&[u8]; 4] = [b"abXq", b"abXr", b"abYq", b"abYr"];
    assert_eq!(dominant_prefix_len(&s, 0.5), 2);
}

#[test]
fn a_plurality_that_is_not_a_strict_majority_does_not_descend() {
    let s: [&[u8]; 4] = [b"abXq", b"abXr", b"abYq", b"abZr"];
    assert_eq!(dominant_prefix_len(&s, 0.5), 2);
}

#[test]
fn a_strict_majority_child_is_followed_even_below_the_threshold() {
    let s: [&[u8]; 5] = [b"abXq", b"abXr", b"abXs", b"abYq", b"abZr"];
    assert_eq!(dominant_prefix_len(&s, 0.1), 3);
}

#[test]
fn one_short_sample_blocks_the_ninety_five_percent_descent_but_not_the_majority_one() {
    let s: [&[u8]; 4] = [b"abc", b"abcd", b"abcd", b"abcd"];
    assert_eq!(dominant_prefix_len(&s, 0.95), 3);
    assert_eq!(dominant_prefix_len(&s, 0.5), 4);
}

#[test]
fn nineteen_of_twenty_share_a_six_block_prefix_and_eighteen_do_not() {
    let mut s: Vec<Vec<u8>> = (0..19).map(|i| format!("sysabc{i:02}").into_bytes()).collect();
    s.push(b"sXsabc99".to_vec());
    assert_eq!(dominant_prefix_len(&refs(&s), 0.95), 6);
    s[18] = b"sYsabc18".to_vec();
    assert_eq!(dominant_prefix_len(&refs(&s), 0.95), 1);
}

#[test]
fn the_learned_length_does_not_depend_on_sample_order() {
    let s: Vec<Vec<u8>> = (0..20)
        .map(|i| format!("scaffold-v1|task-{}|turn", "x".repeat(i % 7)).into_bytes())
        .collect();
    let lens: Vec<usize> = (0..20)
        .map(|k| dominant_prefix_len(&refs(&rotated(&s, k)), 0.95))
        .collect();
    assert_eq!(agreement(&lens), 1.0, "{lens:?}");
}

#[test]
fn agreement_is_the_pairwise_probability_of_equal_outcomes() {
    assert_eq!(agreement(&[3, 3, 3, 3]), 1.0);
    assert_eq!(agreement(&[1, 2, 3, 4]), 0.0);
    assert!((agreement(&[1, 1, 2, 2]) - 4.0 / 12.0).abs() < 1e-12);
    assert_eq!(agreement(&[7]), 1.0);
}

const SCAFFOLD: &str = "{\"prefix_id\":\"0123456789abcdef\",\"preamble\":\"uriel scaffold contract 1\",";

fn response(task: usize, turn: usize) -> Vec<u8> {
    format!("{SCAFFOLD}\"task\":\"{}\",\"turn\":{turn}}}", "t".repeat(3 + task % 9)).into_bytes()
}

#[test]
fn twenty_processes_sharing_a_scaffold_agree_on_the_prefix_where_first_task_keying_does_not() {
    let tasks = 20;
    let turns = 5;
    let pooled: Vec<Vec<u8>> = (0..tasks)
        .flat_map(|t| (0..turns).map(move |u| response(t, u)))
        .collect();
    let scaffold_keyed: Vec<usize> = (0..tasks)
        .map(|p| dominant_prefix_len(&refs(&rotated(&pooled, p * 7 + 1)), 0.95))
        .collect();
    let first_task_keyed: Vec<usize> = (0..tasks)
        .map(|p| {
            let own: Vec<Vec<u8>> = (0..turns).map(|u| response(p, u)).collect();
            dominant_prefix_len(&refs(&own), 0.95)
        })
        .collect();
    let a_scaffold = agreement(&scaffold_keyed);
    let a_first_task = agreement(&first_task_keyed);
    println!("PREFIX_AGREEMENT scaffold-keyed {a_scaffold:.3} first-task-keyed {a_first_task:.3}");
    assert_eq!(a_scaffold, 1.0, "{scaffold_keyed:?}");
    assert!(scaffold_keyed[0] >= SCAFFOLD.len(), "{scaffold_keyed:?}");
    assert!(a_first_task < 0.5, "{a_first_task} {first_task_keyed:?}");
}

#[test]
fn a_changed_fingerprint_reopens_learning_from_the_new_scaffold_only() {
    let mut learner = PrefixLearner::new(0.95);
    for u in 0..3 {
        assert!(!learner.observe("fp-a", &response(0, u)));
    }
    let before = learner.stable_len();
    assert!(before > SCAFFOLD.len());
    assert!(learner.observe("fp-b", b"other-scaffold|1"));
    assert_eq!(learner.len(), 1);
    assert_eq!(learner.stable_len(), 16);
    assert!(!learner.observe("fp-b", b"other-scaffold|2"));
    assert_eq!(learner.stable_len(), 15);
}

#[test]
fn the_learner_window_is_bounded() {
    let mut learner = PrefixLearner::new(0.95);
    for i in 0..200u32 {
        learner.observe("fp", &i.to_le_bytes());
    }
    assert!(learner.len() <= 64);
}
