use std::collections::BTreeSet;

use serde_json::{json, Value};
use uriel_ledger::{Error, Ledger, NewGoal, State, MAX_BATCH, MAX_GOALS, MAX_NEEDS};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn goal(id: &str, needs: &[&str]) -> NewGoal {
    NewGoal {
        id: id.to_string(),
        title: format!("do {id}"),
        detail: String::new(),
        needs: needs.iter().map(|s| s.to_string()).collect(),
    }
}

fn ledger(spec: &[(&str, &[&str])]) -> Ledger {
    let mut l = Ledger::new("t").unwrap();
    l.add(spec.iter().map(|(id, needs)| goal(id, needs)).collect())
        .unwrap();
    l
}

/// A random DAG over `n` goals: goal i may only need goals with a smaller index.
fn random_dag(rng: &mut Rng, n: usize) -> Vec<NewGoal> {
    (0..n)
        .map(|i| {
            let mut needs = BTreeSet::new();
            if i > 0 {
                for _ in 0..rng.below(4) {
                    needs.insert(format!("g{:03}", rng.below(i)));
                }
            }
            NewGoal {
                id: format!("g{i:03}"),
                title: format!("goal {i}"),
                detail: String::new(),
                needs: needs.into_iter().collect(),
            }
        })
        .collect()
}

fn shuffle<T>(rng: &mut Rng, v: &mut [T]) {
    for i in (1..v.len()).rev() {
        v.swap(i, rng.below(i + 1));
    }
}

fn code<T: std::fmt::Debug>(r: Result<T, Error>) -> &'static str {
    r.unwrap_err().code()
}

#[test]
fn ledger_names_use_the_one_id_grammar() {
    for ok in ["a", "A-z_0.9:x", &"x".repeat(64)] {
        assert!(Ledger::new(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "a b",
        "../x",
        "a/b",
        "a\\b",
        "..\\x",
        "é",
        &"x".repeat(65),
        "a\nb",
    ] {
        assert_eq!(code(Ledger::new(bad)), "bad_id", "{bad:?}");
    }
}

#[test]
fn layers_are_kahn_layers_with_ids_sorted_inside_each_layer() {
    let l = ledger(&[
        ("d", &["b", "c"]),
        ("c", &["a"]),
        ("b", &["a"]),
        ("z", &[]),
        ("a", &[]),
    ]);
    assert_eq!(l.layers(), vec![vec!["a", "z"], vec!["b", "c"], vec!["d"]]);
}

#[test]
fn layers_and_the_stored_form_do_not_depend_on_insertion_order() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for case in 0..40 {
        let n = 5 + rng.below(40); // at most 44, inside one batch
        let dag = random_dag(&mut rng, n);
        let mut reference = Ledger::new("r").unwrap();
        reference.add(dag.clone()).unwrap();
        let mut shuffled = dag.clone();
        shuffle(&mut rng, &mut shuffled);
        let mut other = Ledger::new("r").unwrap();
        other.add(shuffled).unwrap();
        assert_eq!(other.layers(), reference.layers(), "case {case}");
        assert_eq!(other.to_json(), reference.to_json(), "case {case}");
    }
}

#[test]
fn the_ready_frontier_is_earliest_layer_first_and_sorted_by_id() {
    let l = ledger(&[("m", &[]), ("b", &["m"]), ("a", &[]), ("c", &[])]);
    assert_eq!(l.ready(10), vec!["a", "c", "m"]);
    assert_eq!(l.ready(2), vec!["a", "c"]);
    assert!(l.ready(0).is_empty());
}

#[test]
fn issue_marks_goals_issued_takes_at_most_the_limit_and_never_issues_a_goal_twice() {
    let mut l = ledger(&[("a", &[]), ("b", &[]), ("c", &[])]);
    assert_eq!(l.issue(2), vec!["a", "b"]);
    assert_eq!(l.state_of("a"), Some(State::Issued));
    assert_eq!(l.state_of("c"), Some(State::Pending));
    assert_eq!(l.issue(5), vec!["c"]);
    assert!(l.issue(5).is_empty());
    assert!(l.issue(0).is_empty());
}

#[test]
fn a_dependent_becomes_ready_only_when_every_dependency_is_done() {
    let mut l = ledger(&[("a", &[]), ("b", &[]), ("c", &["a", "b"])]);
    l.issue(10);
    l.finish("a", true, "ok").unwrap();
    assert!(l.ready(10).is_empty(), "b is still issued");
    l.finish("b", true, "ok").unwrap();
    assert_eq!(l.ready(10), vec!["c"]);
}

#[test]
fn only_an_issued_goal_can_be_finished() {
    let mut l = ledger(&[("a", &[]), ("b", &["a"])]);
    assert_eq!(code(l.finish("a", true, "")), "not_issued");
    assert_eq!(code(l.finish("ghost", true, "")), "unknown_goal");
    l.issue(1);
    l.finish("a", true, "x").unwrap();
    assert_eq!(code(l.finish("a", true, "again")), "not_issued");
}

#[test]
fn a_failed_goal_blocks_its_whole_subtree_but_not_its_siblings() {
    let mut l = ledger(&[
        ("a", &[]),
        ("b", &["a"]),
        ("c", &["a"]),
        ("e", &["b"]),
        ("s", &[]),
        ("t", &["s"]),
    ]);
    l.issue(10);
    l.finish("a", false, "boom").unwrap();
    l.finish("s", true, "fine").unwrap();
    assert_eq!(
        l.blocked().into_iter().collect::<Vec<_>>(),
        vec!["b", "c", "e"]
    );
    assert_eq!(l.ready(10), vec!["t"]);
    let s = l.summary();
    assert_eq!(
        (
            s["blocked"].as_u64(),
            s["failed"].as_u64(),
            s["done"].as_u64()
        ),
        (Some(3), Some(1), Some(1))
    );
}

#[test]
fn a_cycle_is_rejected_and_leaves_the_ledger_unchanged() {
    let mut l = ledger(&[("a", &[])]);
    let before = l.to_json();
    assert_eq!(
        code(l.add(vec![goal("x", &["y"]), goal("y", &["x"])])),
        "cycle"
    );
    assert_eq!(
        code(l.add(vec![
            goal("p", &["q"]),
            goal("q", &["r"]),
            goal("r", &["p"]),
            goal("ok", &[])
        ])),
        "cycle"
    );
    assert_eq!(l.to_json(), before);
}

#[test]
fn self_dependencies_and_unknown_needs_are_refused_whole() {
    let mut l = ledger(&[("a", &[])]);
    let before = l.to_json();
    assert_eq!(
        code(l.add(vec![goal("fine", &[]), goal("me", &["me"])])),
        "self_need"
    );
    assert_eq!(
        code(l.add(vec![goal("fine", &[]), goal("q", &["ghost"])])),
        "unknown_need"
    );
    assert_eq!(code(l.add(vec![goal("bad id", &[])])), "bad_id");
    assert_eq!(code(l.add(vec![goal("ok", &["bad/need"])])), "bad_id");
    assert_eq!(l.to_json(), before, "no goal of a refused batch is kept");
}

#[test]
fn a_changed_redefinition_is_refused_and_an_identical_one_is_a_no_op() {
    let mut l = ledger(&[("a", &[]), ("b", &["a"])]);
    let again = l
        .add(vec![goal("a", &[]), goal("b", &["a"]), goal("c", &["b"])])
        .unwrap();
    assert_eq!((again.added, again.unchanged), (1, 2));
    let reordered = NewGoal {
        needs: vec!["a".into(), "a".into()],
        ..goal("b", &[])
    };
    assert_eq!(
        l.add(vec![reordered]).unwrap().unchanged,
        1,
        "duplicate needs collapse"
    );
    assert_eq!(code(l.add(vec![goal("b", &[])])), "changed");
    assert_eq!(
        code(l.add(vec![NewGoal {
            title: "other".into(),
            ..goal("a", &[])
        }])),
        "changed"
    );
    assert_eq!(
        code(l.add(vec![NewGoal {
            detail: "more".into(),
            ..goal("a", &[])
        }])),
        "changed"
    );
}

#[test]
fn caps_on_goals_batch_and_needs_hold() {
    let mut l = Ledger::new("caps").unwrap();
    let big: Vec<NewGoal> = (0..=MAX_BATCH)
        .map(|i| goal(&format!("b{i}"), &[]))
        .collect();
    assert_eq!(code(l.add(big)), "batch_too_large");
    assert!(l.is_empty());
    for round in 0..(MAX_GOALS / MAX_BATCH) {
        let batch: Vec<NewGoal> = (0..MAX_BATCH)
            .map(|i| goal(&format!("r{round}-{i}"), &[]))
            .collect();
        l.add(batch).unwrap();
    }
    assert_eq!(l.len(), MAX_GOALS);
    assert_eq!(
        code(l.add(vec![goal("one-too-many", &[])])),
        "too_many_goals"
    );
    assert_eq!(l.len(), MAX_GOALS);
    let mut small = Ledger::new("needs").unwrap();
    let mut batch: Vec<NewGoal> = (0..=MAX_NEEDS)
        .map(|i| goal(&format!("n{i}"), &[]))
        .collect();
    let all: Vec<String> = (0..=MAX_NEEDS).map(|i| format!("n{i}")).collect();
    batch.push(NewGoal {
        needs: all.clone(),
        ..goal("hub", &[])
    });
    assert_eq!(code(small.add(batch.clone())), "too_many_needs");
    batch.pop();
    batch.push(NewGoal {
        needs: all[..MAX_NEEDS].to_vec(),
        ..goal("hub", &[])
    });
    small.add(batch).unwrap();
}

#[test]
fn a_batch_may_name_its_own_goals_in_any_order() {
    let mut l = Ledger::new("t").unwrap();
    l.add(vec![goal("c", &["b"]), goal("b", &["a"]), goal("a", &[])])
        .unwrap();
    assert_eq!(l.layers(), vec![vec!["a"], vec!["b"], vec!["c"]]);
}

#[test]
fn text_is_sanitised_counted_and_cut_to_its_caps() {
    let mut l = Ledger::new("t").unwrap();
    let g = NewGoal {
        id: "a".into(),
        title: "go \u{1b}[31mred\u{1b}[0m sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 \u{202e}x\u{200b}y".into(),
        detail: "d".repeat(5_000),
        needs: vec![],
    };
    let r = l.add(vec![g]).unwrap();
    assert_eq!(r.sanitised.secrets.get("anthropic_key"), Some(&1));
    assert!(r.sanitised.ansi >= 2 && r.sanitised.hidden >= 2 && r.sanitised.truncated);
    let stored = l.to_json().to_string();
    for banned in ["sk-ant-api03", "\\u001b", "\\u202e", "\\u200b"] {
        assert!(!stored.contains(banned), "{banned}");
    }
    let goals = l.to_json()["goals"].clone();
    assert!(goals[0]["detail"].as_str().unwrap().len() <= 600);
    l.issue(1);
    let rr = l.finish("a", true, &"r".repeat(5_000)).unwrap();
    assert!(rr.truncated);
    assert!(l.to_json()["goals"][0]["result"].as_str().unwrap().len() <= 400);
}

#[test]
fn a_goal_needs_a_title_that_survives_sanitising() {
    let mut l = Ledger::new("t").unwrap();
    for title in ["", "   ", "\u{1b}[31m", "\u{200b}\u{200b}"] {
        let g = NewGoal {
            title: title.into(),
            ..goal("a", &[])
        };
        assert_eq!(code(l.add(vec![g])), "empty_title", "{title:?}");
    }
    assert!(l.is_empty());
}

#[test]
fn the_stored_form_round_trips_exactly_in_every_state() {
    let mut l = ledger(&[("a", &[]), ("b", &["a"]), ("c", &["a"]), ("d", &["b"])]);
    l.issue(1);
    l.finish("a", true, "result a").unwrap();
    l.issue(1);
    let json = l.to_json();
    assert_eq!(json["schema"], "uriel.ledger.v1");
    let back = Ledger::from_json(&json).unwrap();
    assert_eq!(back, l);
    assert_eq!(back.to_json(), json);
    assert_eq!(back.state_of("b"), Some(State::Issued));
}

#[test]
fn a_tampered_stored_form_is_refused_with_a_typed_error() {
    let good = ledger(&[("a", &[]), ("b", &["a"])]).to_json();
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v = good.clone();
        f(&mut v);
        Ledger::from_json(&v).unwrap_err().code()
    };
    assert_eq!(edit(&|v| v["schema"] = json!("other")), "malformed");
    assert_eq!(edit(&|v| v["name"] = json!("../x")), "malformed");
    assert_eq!(
        edit(&|v| v["goals"][1]["id"] = json!("a")),
        "malformed",
        "duplicate id"
    );
    assert_eq!(
        edit(&|v| v["goals"][0]["state"] = json!("running")),
        "malformed"
    );
    assert_eq!(
        edit(&|v| v["goals"][1]["needs"] = json!(["ghost"])),
        "malformed"
    );
    assert_eq!(
        edit(&|v| v["goals"][1]["needs"] = json!(["b"])),
        "malformed",
        "self need"
    );
    assert_eq!(edit(&|v| v["goals"][0]["needs"] = json!(["b"])), "cycle");
    assert_eq!(
        edit(&|v| v["goals"][0]["title"] = json!("x".repeat(201))),
        "malformed"
    );
    assert_eq!(
        edit(&|v| v["goals"][0]["detail"] = json!("x".repeat(601))),
        "malformed"
    );
    assert_eq!(
        edit(&|v| v["goals"][0]["result"] = json!("x".repeat(401))),
        "malformed"
    );
    for bad in [
        "a\u{1b}[31m",
        "a\u{7f}",
        "a\u{85}",
        "a\u{202e}",
        "a\u{200b}",
        "a\u{feff}",
        "a\u{0}",
    ] {
        assert_eq!(
            edit(&|v| v["goals"][0]["title"] = json!(bad)),
            "malformed",
            "{bad:?}"
        );
    }
    let mut ok = good.clone();
    ok["goals"][0]["title"] = json!("line\none\ttab");
    assert!(
        Ledger::from_json(&ok).is_ok(),
        "newline and tab are allowed"
    );
}

#[test]
fn random_dags_drain_in_waves_and_every_wave_is_one_layer() {
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    for case in 0..30 {
        let n = 1 + rng.below(60);
        let mut l = Ledger::new("d").unwrap();
        l.add(random_dag(&mut rng, n)).unwrap();
        let layers = l.layers();
        let mut waves = Vec::new();
        loop {
            let wave = l.issue(usize::MAX);
            if wave.is_empty() {
                break;
            }
            for id in &wave {
                assert!(
                    l.needs_of(id)
                        .unwrap()
                        .iter()
                        .all(|d| l.state_of(d) == Some(State::Done)),
                    "case {case}: {id} issued before its dependencies"
                );
            }
            for id in &wave {
                l.finish(id, true, "ok").unwrap();
            }
            waves.push(wave);
        }
        assert_eq!(waves, layers, "case {case}");
        assert_eq!(l.summary()["done"], n);
    }
}

#[test]
fn a_drain_with_random_failures_ends_with_every_goal_done_failed_or_blocked() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    for case in 0..30 {
        let n = 2 + rng.below(50);
        let mut l = Ledger::new("f").unwrap();
        l.add(random_dag(&mut rng, n)).unwrap();
        while !l.ready(1).is_empty() {
            for id in l.issue(1 + rng.below(4)) {
                l.finish(&id, rng.below(4) != 0, "r").unwrap();
            }
        }
        let s = l.summary();
        let total = s["done"].as_u64().unwrap()
            + s["failed"].as_u64().unwrap()
            + s["blocked"].as_u64().unwrap();
        assert_eq!(total, n as u64, "case {case}: {s}");
        assert_eq!(s["issued"], 0);
        assert_eq!(
            s["pending"], s["blocked"],
            "case {case}: only blocked goals are left pending"
        );
    }
}

#[test]
fn the_summary_counts_every_state_and_the_layers() {
    let mut l = ledger(&[("a", &[]), ("b", &["a"]), ("c", &["b"])]);
    assert_eq!(
        l.summary(),
        json!({"goals": 3, "layers": 3, "pending": 3, "issued": 0, "done": 0, "failed": 0, "blocked": 0, "ready": 1})
    );
    l.issue(1);
    l.finish("a", false, "x").unwrap();
    assert_eq!(
        l.summary(),
        json!({"goals": 3, "layers": 3, "pending": 2, "issued": 0, "done": 0, "failed": 1, "blocked": 2, "ready": 0})
    );
}
