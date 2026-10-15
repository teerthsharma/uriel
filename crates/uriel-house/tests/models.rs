//! Model slots and the Dr House leap.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::*;
use uriel_core::Graph;
use uriel_house::*;
use serde_json::Value;

fn with_models(tweak: impl FnOnce(&mut ModelSlots)) -> Env {
    env_with(16_000, 16_000, |c| tweak(&mut c.models))
}

fn refute(e: &Env, goal: &str) {
    e.house
        .add_fact(Fact {
            id: "F1".into(),
            by: "Wilson".into(),
            refutes: Some(gid(goal)),
            source: "src/a.rs:3".into(),
            text: "the migration is innocent".into(),
        })
        .expect("fact");
}

/// G2 refuted twice with no replacement: a leap candidate.
fn leap_ready(e: &Env) {
    merged(e, base_goals());
    refute(e, "G2");
    for _ in 0..2 {
        e.house
            .submit_chart(Fellow::Foreman, chart(Fellow::Foreman, base_goals()))
            .expect("rechart");
        e.house.merge().expect("merge");
    }
    assert_eq!(e.house.status().expect("status")["leap_candidate"], true);
}

// ---------------------------------------------------------------- slots

#[test]
fn default_slots_are_the_documented_tiers() {
    let m = ModelSlots::default();
    assert_eq!(m.fellows, "opus");
    assert_eq!(m.residents, "sonnet");
    assert_eq!(m.nurses, "haiku");
    assert_eq!(m.wilson, "opus");
    assert_eq!(m.inspector, "opus");
    assert_eq!(m.dr_house, "opus");
    assert_eq!(m.dr_house_fallback, "opus");
    assert_eq!(HouseConfig::default().models, m);
}

#[test]
fn env_overrides_only_the_dr_house_slot() {
    let mut cfg = HouseConfig::default();
    let env: BTreeMap<&str, &str> = BTreeMap::from([("SEAL_DR_HOUSE_MODEL", "fable")]);
    cfg.apply_env_from(|k| env.get(k).map(|v| v.to_string()));
    assert_eq!(cfg.models.dr_house, "fable");
    assert_eq!(cfg.models.dr_house_fallback, "opus");
    assert_eq!(cfg.models.residents, "sonnet");
}

#[test]
fn empty_env_value_is_ignored() {
    let mut cfg = HouseConfig::default();
    cfg.apply_env_from(|_| Some("  ".to_string()));
    assert_eq!(cfg.models.dr_house, "opus");
}

#[test]
fn from_file_reads_models_and_governor_settings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"models":{"dr_house":"fable","dr_house_fallback":"opus-4"},"heavy_cap":3,"unknown_key":1}"#,
    )
    .expect("write");
    let cfg = HouseConfig::from_file(&path).expect("load");
    assert_eq!(cfg.models.dr_house, "fable");
    assert_eq!(cfg.models.dr_house_fallback, "opus-4");
    // unspecified slots and settings keep their defaults
    assert_eq!(cfg.models.residents, "sonnet");
    assert_eq!(cfg.heavy_cap, 3);
    assert_eq!(cfg.reserve_pct, 15);
}

#[test]
fn from_file_rejects_missing_and_malformed_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(HouseConfig::from_file(dir.path().join("absent.json")).is_err());
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "{ not json").expect("write");
    assert!(HouseConfig::from_file(&bad).is_err());
    let wrong = dir.path().join("wrong.json");
    std::fs::write(&wrong, r#"{"models":{"dr_house":7}}"#).expect("write");
    assert!(HouseConfig::from_file(&wrong).is_err());
}

#[test]
fn project_root_is_not_read_from_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"project_root":"/elsewhere"}"#).expect("write");
    let cfg = HouseConfig::from_file(&path).expect("load");
    assert_eq!(cfg.project_root, HouseConfig::default().project_root);
}

// ---------------------------------------------------------------- dispatch follows the slots

#[test]
fn dispatch_models_come_from_the_slots() {
    let e = with_models(|m| {
        m.residents = "sonnet-x".into();
        m.nurses = "haiku-x".into();
    });
    let mut goals = base_goals();
    goals.push(nurse("G2.3", "G2"));
    merged(&e, goals);
    let batch = e.house.next().expect("next");
    let model_of = |id: &str| {
        batch
            .dispatches
            .iter()
            .find(|d| d.goal.as_str() == id)
            .unwrap_or_else(|| panic!("{id} dispatched"))
            .model
            .clone()
    };
    assert_eq!(model_of("G1.1"), "sonnet-x");
    assert_eq!(model_of("G2.3"), "haiku-x");
    // the board events carry the same model names
    let dispatched = e.sink.of_kind("dispatch");
    assert!(dispatched.iter().all(|d| d["model"] == "sonnet-x"));
    let nurses = e.sink.of_kind("nurse");
    assert_eq!(nurses.len(), 1);
    assert_eq!(nurses[0]["model"], "haiku-x");
}

#[test]
fn default_dispatch_keeps_sonnet_and_haiku() {
    let e = env();
    let mut goals = base_goals();
    goals.push(nurse("G2.3", "G2"));
    merged(&e, goals);
    let batch = e.house.next().expect("next");
    let models: Vec<&str> = batch.dispatches.iter().map(|d| d.model.as_str()).collect();
    assert!(models.contains(&"sonnet") && models.contains(&"haiku"));
}

// ---------------------------------------------------------------- leap

#[test]
fn leap_on_a_candidate_dispatches_dr_house() {
    let e = with_models(|m| {
        m.dr_house = "fable".into();
        m.dr_house_fallback = "opus".into();
    });
    leap_ready(&e);
    let l = e
        .house
        .leap(&gid("G2"), "needs a lock-free ordering nobody has")
        .expect("leap")
        .expect("candidate");
    assert_eq!(l.goal, gid("G2"));
    assert_eq!(l.model, "fable");
    assert_eq!(l.fallback_model, "opus");
    assert_eq!(l.effort, "max");
    assert_eq!(l.timeout_secs, 300);
    assert_eq!(l.nurses, 0);
}

#[test]
fn leap_input_carries_theory_cause_and_facts_of_the_subtree() {
    let e = env();
    leap_ready(&e);
    let l = e
        .house
        .leap(&gid("G2"), "missing innovation: ordering")
        .expect("leap")
        .expect("candidate");
    assert!(l.input.contains("part G2"), "{}", l.input);
    assert!(l.input.contains("do G2.1"), "subtree text: {}", l.input);
    assert!(l.input.contains("missing innovation: ordering"));
    assert!(l.input.contains("the migration is innocent"));
    assert!(l.input.contains("src/a.rs:3"));
    // text of the live subtree G1 is not part of it
    assert!(!l.input.contains("do G1.1"), "{}", l.input);
}

#[test]
fn leap_is_none_for_a_goal_that_is_not_a_candidate() {
    let e = env();
    leap_ready(&e);
    assert!(e.house.leap(&gid("G1"), "cause").expect("leap").is_none());
    assert!(e.sink.of_kind("leap").is_empty());
}

#[test]
fn leap_is_none_before_anything_died() {
    let e = env();
    merged(&e, base_goals());
    assert!(e.house.leap(&gid("G2"), "cause").expect("leap").is_none());
}

#[test]
fn leap_on_an_unknown_goal_is_an_error() {
    let e = env();
    leap_ready(&e);
    assert!(matches!(
        e.house.leap(&gid("G77"), "cause"),
        Err(HouseError::UnknownGoal(_))
    ));
}

#[test]
fn a_goal_gets_one_leap() {
    let e = env();
    leap_ready(&e);
    assert!(e.house.leap(&gid("G2"), "c").expect("first").is_some());
    let second = e.house.leap(&gid("G2"), "c");
    assert!(
        matches!(second, Err(HouseError::LeapAlreadyUsed(ref g)) if *g == gid("G2")),
        "{second:?}"
    );
    assert_eq!(e.sink.of_kind("leap").len(), 1);
}

#[test]
fn leap_event_has_the_gates_py_shape() {
    let e = with_models(|m| m.dr_house = "fable".into());
    leap_ready(&e);
    e.house.leap(&gid("G2"), "c").expect("leap");
    let events = e.sink.of_kind("leap");
    assert_eq!(events.len(), 1);
    let ev = &events[0];
    assert_eq!(ev["id"], "L1");
    assert_eq!(ev["goal"], "G2");
    assert_eq!(ev["model"], "fable");
    assert_eq!(ev["minutes"], 5);
    assert!(ev["text"].as_str().is_some_and(|t| t.contains("part G2")));
    // unbound until a RED test is bound to it
    assert!(ev.get("test").is_none_or(Value::is_null));
}

#[test]
fn leap_ids_are_sequential_per_house() {
    let e = env();
    merged(&e, base_goals());
    // two independent refuted goals
    for (n, g) in ["G1", "G2"].into_iter().enumerate() {
        e.house
            .add_fact(Fact {
                id: format!("F{}", n + 1),
                by: "Wilson".into(),
                refutes: Some(gid(g)),
                source: "src/a.rs:1".into(),
                text: format!("{g} is dead"),
            })
            .expect("fact");
    }
    for _ in 0..2 {
        e.house
            .submit_chart(Fellow::Foreman, chart(Fellow::Foreman, base_goals()))
            .expect("rechart");
        e.house.merge().expect("merge");
    }
    e.house.leap(&gid("G1"), "c").expect("l1");
    e.house.leap(&gid("G2"), "c").expect("l2");
    let ids: Vec<String> = e
        .sink
        .of_kind("leap")
        .iter()
        .map(|v| v["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(ids, ["L1", "L2"]);
}

#[test]
fn leap_use_survives_a_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("h.seal");
    let project = project();
    let open = || {
        House::open(
            Graph::open(&db).expect("graph"),
            HouseConfig::at(project.path()),
            Box::new(FakeProbe::new(16_000, 16_000)),
            Box::new(Arc::new(VecSink::new())),
        )
        .expect("house")
    };
    {
        let house = open();
        house.need("p", "g", &["a".to_string()]).expect("need");
        house
            .submit_chart(Fellow::Foreman, chart(Fellow::Foreman, base_goals()))
            .expect("chart");
        house.merge().expect("merge");
        house
            .add_fact(Fact {
                id: "F1".into(),
                by: "Wilson".into(),
                refutes: Some(gid("G2")),
                source: "s".into(),
                text: "dead".into(),
            })
            .expect("fact");
        for _ in 0..2 {
            house
                .submit_chart(Fellow::Foreman, chart(Fellow::Foreman, base_goals()))
                .expect("rechart");
            house.merge().expect("merge");
        }
        assert!(house.leap(&gid("G2"), "c").expect("leap").is_some());
    }
    let again = open();
    assert!(matches!(
        again.leap(&gid("G2"), "c"),
        Err(HouseError::LeapAlreadyUsed(_))
    ));
}
