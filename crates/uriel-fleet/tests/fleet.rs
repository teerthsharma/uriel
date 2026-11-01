use std::collections::BTreeMap;

use uriel_fleet::{Deny, Error, Fleet, Kind, Limits, Obs, Status, Want};

fn limits(concurrent: usize, depth: usize, total: usize) -> Limits {
    Limits {
        max_concurrent: concurrent,
        max_depth: depth,
        max_total: total,
        ..Limits::default()
    }
}

fn admit(f: &mut Fleet, want: Want) -> uriel_fleet::Admission {
    f.admit("r1", want, None, "lens", "small").unwrap()
}

#[test]
fn max_grants_exactly_the_free_concurrency_and_no_more() {
    let mut f = Fleet::new(limits(6, 2, 64));
    let a = admit(&mut f, Want::Max);
    assert_eq!(a.granted, vec!["a1", "a2", "a3", "a4", "a5", "a6"]);
    assert_eq!(a.free_after, 0);
    assert_eq!(a.deny, None);
    let b = admit(&mut f, Want::Max);
    assert!(b.granted.is_empty());
    assert_eq!(b.deny, Some(Deny::Concurrency));
}

#[test]
fn a_counted_request_over_the_cap_is_granted_partly_and_the_rest_is_deferred() {
    let mut f = Fleet::new(limits(4, 2, 64));
    let a = admit(&mut f, Want::Count(7));
    assert_eq!(a.granted.len(), 4);
    assert_eq!(a.deferred, 3);
    assert_eq!(a.requested, Some(7));
    assert_eq!(a.deny, Some(Deny::Concurrency));
}

#[test]
fn finishing_an_agent_frees_its_slot_and_ids_are_never_reused() {
    let mut f = Fleet::new(limits(2, 2, 64));
    admit(&mut f, Want::Count(2));
    f.end("r1", "a1", Status::Done, "").unwrap();
    let a = admit(&mut f, Want::Count(2));
    assert_eq!(a.granted, vec!["a3"]);
    assert_eq!(a.deferred, 1);
}

#[test]
fn the_lifetime_total_binds_even_when_slots_are_free() {
    let mut f = Fleet::new(limits(8, 2, 5));
    admit(&mut f, Want::Count(5));
    for id in ["a1", "a2", "a3", "a4", "a5"] {
        f.end("r1", id, Status::Done, "").unwrap();
    }
    let a = admit(&mut f, Want::Count(1));
    assert!(a.granted.is_empty());
    assert_eq!(a.deny, Some(Deny::Total));
}

#[test]
fn children_of_a_running_agent_are_admitted_until_the_depth_cap() {
    let mut f = Fleet::new(limits(8, 2, 64));
    admit(&mut f, Want::Count(1));
    let child = f.admit("r1", Want::Count(1), Some("a1"), "sub", "small").unwrap();
    assert_eq!(child.granted, vec!["a2"]);
    let grandchild = f.admit("r1", Want::Count(1), Some("a2"), "sub", "small").unwrap();
    assert!(grandchild.granted.is_empty());
    assert_eq!(grandchild.deny, Some(Deny::Depth));
}

#[test]
fn a_fork_bomb_of_recursive_spawns_stops_at_depth_two_by_default() {
    let mut f = Fleet::new(Limits::default());
    let mut parent: Option<String> = None;
    let mut spawned = 0;
    for _ in 0..50 {
        let a = f.admit("r1", Want::Count(1), parent.as_deref(), "x", "x").unwrap();
        match a.granted.first() {
            Some(id) => {
                spawned += 1;
                parent = Some(id.clone());
            }
            None => break,
        }
    }
    assert_eq!(spawned, 2);
}

#[test]
fn a_child_of_a_finished_or_unknown_parent_is_refused() {
    let mut f = Fleet::new(Limits::default());
    admit(&mut f, Want::Count(1));
    f.end("r1", "a1", Status::Done, "").unwrap();
    assert_eq!(
        f.admit("r1", Want::Count(1), Some("a1"), "x", "x").unwrap_err(),
        Error::NotRunning("a1".into())
    );
    assert_eq!(
        f.admit("r1", Want::Count(1), Some("zz"), "x", "x").unwrap_err(),
        Error::UnknownAgent("zz".into())
    );
}

#[test]
fn run_ids_outside_the_safe_alphabet_are_rejected() {
    let mut f = Fleet::new(Limits::default());
    for bad in ["", "has space", "a/b", "x\ny", &"a".repeat(65)] {
        assert!(matches!(f.admit(bad, Want::Max, None, "x", "x"), Err(Error::BadId(_))), "{bad:?}");
    }
}

#[test]
fn the_token_budget_stops_further_admission_and_marks_the_overrun() {
    let mut f = Fleet::new(Limits { max_tokens: 1000, ..Limits::default() });
    admit(&mut f, Want::Count(2));
    let obs = Obs { kind: Some(Kind::Llm), name: "call".into(), tokens_in: 900, tokens_out: 300, ..Obs::default() };
    f.observe("r1", "a1", obs).unwrap();
    let a = admit(&mut f, Want::Count(1));
    assert!(a.granted.is_empty());
    assert_eq!(a.deny, Some(Deny::TokenBudget));
    let jsonl = f.trace_jsonl("r1").unwrap();
    assert!(jsonl.contains("token_budget_exceeded"));
}

#[test]
fn observations_on_a_finished_agent_are_rejected_and_do_not_enter_the_trace() {
    let mut f = Fleet::new(Limits::default());
    admit(&mut f, Want::Count(1));
    f.end("r1", "a1", Status::Done, "").unwrap();
    let before = f.trace("r1").unwrap().len();
    let err = f.observe("r1", "a1", Obs { name: "late".into(), ..Obs::default() }).unwrap_err();
    assert_eq!(err, Error::NotRunning("a1".into()));
    assert_eq!(f.trace("r1").unwrap().len(), before);
}

#[test]
fn secrets_and_control_bytes_in_observation_attributes_never_reach_the_trace() {
    let mut f = Fleet::new(Limits::default());
    admit(&mut f, Want::Count(1));
    let mut attrs = BTreeMap::new();
    attrs.insert("cmd".to_string(), "curl -H 'Authorization: Bearer abcdef0123456789ABCDEF' \u{1b}[31m".to_string());
    attrs.insert("env".to_string(), "ANTHROPIC=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123".to_string());
    let (_, report) = f
        .observe("r1", "a1", Obs { kind: Some(Kind::Tool), name: "bash".into(), attrs, ..Obs::default() })
        .unwrap();
    assert_eq!(report.secret_total(), 2);
    let trace = f.trace_jsonl("r1").unwrap();
    assert!(!trace.contains("abcdef0123456789ABCDEF"));
    assert!(!trace.contains("sk-ant-api03"));
    assert!(!trace.contains('\u{1b}'));
    assert!(trace.contains("[REDACTED:bearer_token]"));
}

#[test]
fn caller_supplied_start_end_and_reap_kinds_are_downgraded_to_marks() {
    let mut f = Fleet::new(Limits::default());
    admit(&mut f, Want::Count(1));
    let (ev, _) = f
        .observe("r1", "a1", Obs { kind: Some(Kind::End), name: "fake".into(), ..Obs::default() })
        .unwrap();
    assert_eq!(ev.kind, Kind::Mark);
    assert_eq!(f.summary("r1").unwrap()["running"], 1);
}

#[test]
fn cached_tokens_cannot_exceed_input_tokens() {
    let mut f = Fleet::new(Limits::default());
    admit(&mut f, Want::Count(1));
    let (ev, _) = f
        .observe("r1", "a1", Obs { kind: Some(Kind::Llm), name: "c".into(), tokens_in: 10, cached_in: 99, ..Obs::default() })
        .unwrap();
    assert_eq!(ev.cached_in, 10);
}

#[test]
fn reap_cancels_every_running_agent_and_frees_the_slots() {
    let mut f = Fleet::new(limits(3, 2, 64));
    admit(&mut f, Want::Max);
    f.end("r1", "a2", Status::Done, "").unwrap();
    assert_eq!(f.reap("r1").unwrap(), 2);
    let s = f.summary("r1").unwrap();
    assert_eq!((s["running"].as_u64(), s["cancelled"].as_u64(), s["done"].as_u64()), (Some(0), Some(2), Some(1)));
    assert_eq!(admit(&mut f, Want::Max).granted.len(), 3);
}

#[test]
fn the_trace_of_a_whole_run_is_a_valid_chain_with_one_start_and_one_end_per_agent() {
    let mut f = Fleet::new(Limits::default());
    admit(&mut f, Want::Count(3));
    for id in ["a1", "a2", "a3"] {
        f.observe("r1", id, Obs { kind: Some(Kind::Llm), name: "m".into(), tokens_in: 5, tokens_out: 5, ..Obs::default() })
            .unwrap();
        f.end("r1", id, Status::Done, "ok").unwrap();
    }
    let t = f.trace("r1").unwrap();
    assert_eq!(t.verify(), Ok(()));
    let starts = t.events().filter(|e| e.kind == Kind::Start).count();
    let ends = t.events().filter(|e| e.kind == Kind::End).count();
    assert_eq!((starts, ends), (3, 3));
}

#[test]
fn identical_call_sequences_produce_identical_trace_heads() {
    let run = || {
        let mut f = Fleet::new(Limits::default());
        admit(&mut f, Want::Max);
        f.observe("r1", "a1", Obs { kind: Some(Kind::Tool), name: "t".into(), t_ms: Some(7), ..Obs::default() }).unwrap();
        f.end("r1", "a1", Status::Failed, "boom").unwrap();
        f.trace("r1").unwrap().head()
    };
    assert_eq!(run(), run());
}

#[test]
fn old_finished_runs_are_evicted_but_a_run_with_live_agents_never_is() {
    let mut f = Fleet::new(Limits { max_runs: 2, ..Limits::default() });
    f.admit("r1", Want::Count(1), None, "x", "x").unwrap();
    f.admit("r2", Want::Count(1), None, "x", "x").unwrap();
    assert_eq!(
        f.admit("r3", Want::Count(1), None, "x", "x").unwrap_err(),
        Error::TooManyRuns(2)
    );
    f.end("r1", "a1", Status::Done, "").unwrap();
    f.admit("r3", Want::Count(1), None, "x", "x").unwrap();
    assert!(f.trace("r1").is_none());
    assert!(f.trace("r2").is_some());
}

#[test]
fn the_event_cap_bounds_memory_and_the_chain_still_verifies() {
    let mut f = Fleet::new(Limits { max_events: 16, ..Limits::default() });
    admit(&mut f, Want::Count(1));
    for i in 0..500 {
        f.observe("r1", "a1", Obs { name: format!("m{i}"), ..Obs::default() }).unwrap();
    }
    let t = f.trace("r1").unwrap();
    assert_eq!(t.len(), 16);
    assert!(t.dropped() > 480);
    assert_eq!(t.verify(), Ok(()));
}

#[test]
fn limits_are_clamped_so_zero_cannot_deadlock_the_fleet() {
    let f = Fleet::new(Limits { max_concurrent: 0, max_depth: 0, max_total: 0, max_tokens: 0, max_events: 0, max_runs: 0 });
    let l = f.limits();
    assert!(l.max_concurrent >= 1 && l.max_depth >= 1 && l.max_total >= 1 && l.max_tokens >= 1 && l.max_events >= 16 && l.max_runs >= 1);
}
