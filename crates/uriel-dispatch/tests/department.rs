mod common;

use common::*;
use serde_json::{json, Value};
use uriel_dispatch::afk::*;
use uriel_dispatch::dept::PlanSpec;
use uriel_dispatch::*;

fn open_in(h: &mut H, trig: &str, thread: &str, dept: &str) -> TicketId {
    let fx = h.ingest(person(trig, "alice", true, thread));
    let eval = started(&fx).expect("eval");
    h.go(Input::EvalDelivered { eval });
    let fx = h.go(Input::EvalResult {
        eval,
        verdicts: vec![(TriggerId::new(trig), accept_in(dept))],
    });
    opened(&fx).expect("ticket")
}

#[test]
fn a_busy_department_cannot_starve_a_quiet_one_when_slots_run_short() {
    let mut h = H::new(policy());
    let mut a = Vec::new();
    for i in 0..6 {
        a.push(open_in(&mut h, &format!("a{i}"), &format!("ta{i}"), "alpha"));
    }
    let b: Vec<TicketId> = (0..2)
        .map(|i| open_in(&mut h, &format!("b{i}"), &format!("tb{i}"), "beta"))
        .collect();
    for id in a.iter().take(3) {
        h.go(Input::PrepareDone { ticket: *id });
        h.go(Input::TurnEnded { ticket: *id, background: false });
    }
    let held = h.r.held_by_dept();
    assert_eq!(held["beta"], 1, "beta got a slot as soon as one freed: {held:?}");
    assert!(h.r.ticket(b[0]).unwrap().holds_slot());
}

#[test]
fn shares_follow_the_load_and_recompute_as_work_finishes() {
    let mut h = H::new(policy());
    for i in 0..6 {
        open_in(&mut h, &format!("a{i}"), &format!("ta{i}"), "alpha");
    }
    for i in 0..2 {
        open_in(&mut h, &format!("b{i}"), &format!("tb{i}"), "beta");
    }
    let s = h.r.shares();
    assert_eq!((s["alpha"], s["beta"]), (2, 1));
    assert_eq!(s.values().sum::<u32>(), h.r.capacity());
}

#[test]
fn no_slot_idles_while_any_department_has_queued_work() {
    let mut h = H::new(policy());
    for i in 0..5 {
        open_in(&mut h, &format!("a{i}"), &format!("ta{i}"), "alpha");
    }
    assert_eq!(h.r.slots_held(), (3, 0), "one department alone may use every slot");
}

#[test]
fn tickets_without_a_department_are_their_own_lane_and_ordering_stays_fifo() {
    let mut h = H::new(policy());
    let ids: Vec<TicketId> = (0..5).map(|i| h.open(&format!("o{i}"), &format!("i{i}"))).collect();
    assert!(ids[..3].iter().all(|i| h.r.ticket(*i).unwrap().holds_slot()));
    assert!(ids[3..].iter().all(|i| !h.r.ticket(*i).unwrap().holds_slot()));
}

const PLAN: &str = r#"{"departments":[
 {"name":"Research","goal":"Find the fix point","subgoals":[
   {"goal":"Reproduce","leaves":[{"goal":"Write the failing case","checklist":["seen failing","minimal"]},{"goal":"Pin the version","checklist":["lock"]}]}]},
 {"name":"Build","goal":"Land it","subgoals":[
   {"goal":"Implement","leaves":[{"goal":"Code the fix","checklist":["compiles","tests pass"]}]}]}]}"#;

fn log(d: &tempfile::TempDir) -> std::path::PathBuf {
    d.path().join("s.jsonl")
}

fn plan() -> PlanSpec {
    serde_json::from_str(PLAN).unwrap()
}

#[test]
fn a_session_with_a_plan_opens_leaf_tickets_in_their_department_and_ticks_checklists() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 3_600_000, 2, 1_700_000_000_000).unwrap();
    assert_eq!(a.set_plan(plan()).unwrap(), 3);
    let (t1, s1) = a.add_leaf(1_700_000_000_001, "research.1.1").unwrap();
    let (t2, _) = a.add_leaf(1_700_000_000_002, "build.1.1").unwrap();
    assert!(matches!(&s1[0], Action::Start { goal: Some(g), .. } if g == "research.1.1"));
    assert_eq!(a.router().ticket(t1).unwrap().dept.as_deref(), Some("research"));
    assert_eq!(a.router().ticket(t2).unwrap().dept.as_deref(), Some("build"));
    a.check("research.1.1", 0, true).unwrap();
    a.check("research.1.1", 1, true).unwrap();
    let st = a.status(1_700_000_000_010).unwrap();
    let deps = st["departments"].as_array().unwrap();
    assert_eq!(deps[0]["progress"], json!({"done": 2, "total": 3}));
    assert_eq!(deps[0]["subgoals"][0]["leaves"][0]["ticket"], t1.0);
    assert_eq!(deps[0]["subgoals"][0]["leaves"][0]["progress"], json!({"done": 2, "total": 2}));
    assert_eq!(deps[1]["share"], 1);
    assert_eq!(deps[1]["held"], 1);
}

#[test]
fn a_leaf_can_only_be_ticketed_once_and_only_when_it_exists() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 3_600_000, 2, 1_700_000_000_000).unwrap();
    assert!(matches!(a.add_leaf(1_700_000_000_001, "x.1.1"), Err(AfkError::NoPlan)));
    a.set_plan(plan()).unwrap();
    assert!(matches!(a.set_plan(plan()), Err(AfkError::PlanExists)));
    a.add_leaf(1_700_000_000_001, "research.1.1").unwrap();
    assert!(matches!(a.add_leaf(1_700_000_000_002, "research.1.1"), Err(AfkError::LeafTaken(_))));
    assert!(matches!(a.add_leaf(1_700_000_000_003, "nope.1.1"), Err(AfkError::Plan(_))));
    assert!(a.check("research.1.1", 9, true).is_err());
}

#[test]
fn a_bad_plan_is_refused_and_leaves_the_log_valid() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Afk::start(&log(&d), "me", 3_600_000, 2, 1_700_000_000_000).unwrap();
    let bad: PlanSpec = serde_json::from_str(r#"{"departments":[{"name":"A","goal":"g","subgoals":[{"goal":"s","leaves":[{"goal":"l","checklist":[]}]}]}]}"#).unwrap();
    assert!(matches!(a.set_plan(bad), Err(AfkError::Plan(_))));
    drop(a);
    assert!(Afk::open(&log(&d)).is_ok());
}

#[test]
fn the_plan_the_leaf_links_and_the_ticks_survive_a_reopen() {
    let d = tempfile::tempdir().unwrap();
    let t0 = 1_700_000_000_000;
    let (digest, before) = {
        let mut a = Afk::start(&log(&d), "me", 3_600_000, 2, t0).unwrap();
        a.set_plan(plan()).unwrap();
        a.add_leaf(t0 + 1, "research.1.1").unwrap();
        a.check("research.1.1", 0, true).unwrap();
        (a.router().digest(), a.departments_json())
    };
    let b = Afk::open(&log(&d)).unwrap();
    assert_eq!(b.router().digest(), digest);
    assert_eq!(b.departments_json(), before);
    assert_eq!(b.plan().unwrap().progress().done, 1);
}

#[test]
fn the_summary_carries_the_department_roll_up() {
    let d = tempfile::tempdir().unwrap();
    let t0 = 1_700_000_000_000;
    let mut a = Afk::start(&log(&d), "me", 3_600_000, 2, t0).unwrap();
    a.set_plan(plan()).unwrap();
    let (t, _) = a.add_leaf(t0 + 1, "build.1.1").unwrap();
    a.check("build.1.1", 0, true).unwrap();
    a.check("build.1.1", 1, true).unwrap();
    a.report(t0 + 2, t, Kind::Ready, Some("branch afk/b; tests ok".into())).unwrap();
    let (sum, _) = a.end(t0 + 3).unwrap();
    let build = &sum["departments"][1];
    assert_eq!(build["name"], "Build");
    assert_eq!(build["progress"], json!({"done": 2, "total": 2}));
    assert_eq!(build["subgoals"][0]["leaves"][0]["result"], Value::from("branch afk/b; tests ok"));
}

#[test]
fn a_tampered_plan_line_is_refused() {
    let d = tempfile::tempdir().unwrap();
    {
        let mut a = Afk::start(&log(&d), "me", 3_600_000, 2, 1_700_000_000_000).unwrap();
        a.set_plan(plan()).unwrap();
        a.add_leaf(1_700_000_000_001, "research.1.1").unwrap();
    }
    let text = std::fs::read_to_string(log(&d)).unwrap();
    let forged = text.replace("\"checklist\":[\"seen failing\",\"minimal\"]", "\"checklist\":[]");
    assert_ne!(text, forged);
    std::fs::write(log(&d), forged).unwrap();
    assert!(matches!(Afk::open(&log(&d)), Err(AfkError::Corrupt { why: "unreadable plan", .. })));
}

#[test]
fn when_slots_are_scarcer_than_departments_the_least_recently_served_department_goes_first() {
    let mut h = H::with(RouterConfig { max_workers: 1, ..RouterConfig::default() }, policy());
    let a0 = open_in(&mut h, "a0", "ta0", "alpha");
    for i in 1..4 {
        open_in(&mut h, &format!("a{i}"), &format!("ta{i}"), "alpha");
    }
    let c0 = open_in(&mut h, "c0", "tc0", "zulu");
    h.go(Input::PrepareDone { ticket: a0 });
    h.go(Input::TurnEnded { ticket: a0, background: false });
    assert!(
        h.r.ticket(c0).unwrap().holds_slot(),
        "zulu was served least recently even though alpha has more work: {:?}",
        h.r.held_by_dept()
    );
}
