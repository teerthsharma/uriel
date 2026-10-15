#[allow(dead_code, unused_imports)]
#[path = "simulation.rs"]
mod simulation;

use std::collections::BTreeMap;
use std::path::Path;

use simulation::common::*;
use uriel_house::*;

struct Recorded {
    calls: Vec<HouseCall>,
    script: Vec<SystemSnapshot>,
    log: Vec<DecisionRecord>,
    digest: [u8; 32],
}

fn capture(e: &Env) -> Recorded {
    let log = e.house.decision_log();
    Recorded {
        calls: e.house.history(),
        script: log.iter().map(|r| r.probe).collect(),
        digest: e.house.decision_digest(),
        log,
    }
}

fn drive_forty(e: &Env) {
    merged(e, simulation::forty());
    let mut in_flight: BTreeMap<String, usize> = BTreeMap::new();
    for step in 0..400 {
        e.probe.set_free(simulation::free_at(step));
        let due: Vec<String> = in_flight
            .iter()
            .filter(|(_, finish)| **finish <= step)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let t = format!("t_{id}");
            e.house
                .record_test_run(&t, TestStatus::Green, 1.0, 100, &[])
                .expect("green");
            e.house
                .report(
                    &gid(&id),
                    GoalStatus::Reached,
                    evidence(&t, TestStatus::Green),
                )
                .expect("report");
            in_flight.remove(&id);
        }
        let batch = e.house.next().expect("next");
        for d in &batch.dispatches {
            e.house
                .record_test_run(&d.test.name, TestStatus::Red, 1.0, d.cost_mb.max(1), &[])
                .expect("red");
            let duration = if d.class.is_heavy() { 3 } else { 1 };
            in_flight.insert(d.goal.to_string(), step + duration);
        }
        if batch.complete && in_flight.is_empty() {
            return;
        }
    }
    panic!("the 40 leaves did not finish");
}

fn record_forty() -> (Env, Recorded) {
    let e = env_with(6_000, simulation::TOTAL_MB, |_| {});
    drive_forty(&e);
    assert_eq!(status_of(&e, "G0"), "reached");
    let rec = capture(&e);
    (e, rec)
}

fn replay_once(root: &Path, rec: &Recorded) -> Vec<DecisionRecord> {
    let graph = uriel_core::Graph::open_in_memory().expect("graph");
    House::replay(
        HouseConfig::at(root),
        graph,
        rec.script.clone(),
        rec.calls.clone(),
    )
    .expect("replay")
}

fn report_order(calls: &[HouseCall]) -> Vec<String> {
    calls
        .iter()
        .filter_map(|c| match c {
            HouseCall::Report { goal, .. } => Some(goal.to_string()),
            _ => None,
        })
        .collect()
}

fn two_independent_reports(order: [&str; 2], interleave_next: bool) -> (Env, Recorded) {
    let e = env();
    merged(&e, base_goals());
    e.house.next().expect("next");
    for id in ["G1.1", "G2.1"] {
        let t = format!("t_{id}");
        e.house
            .record_test_run(&t, TestStatus::Red, 1.0, 100, &[])
            .expect("red");
        e.house
            .record_test_run(&t, TestStatus::Green, 1.0, 100, &[])
            .expect("green");
    }
    for (n, id) in order.iter().enumerate() {
        let t = format!("t_{id}");
        let r = e
            .house
            .report(&gid(id), GoalStatus::Reached, evidence(&t, TestStatus::Green))
            .expect("report");
        assert_eq!(r.outcome, Outcome::Accepted, "{id}: {}", r.reason);
        if interleave_next && n == 0 {
            e.house.next().expect("next");
        }
    }
    e.house.next().expect("next");
    let rec = capture(&e);
    (e, rec)
}

#[test]
fn forty_leaf_run_replays_to_one_digest_twenty_times() {
    let (e, rec) = record_forty();
    assert!(!rec.log.is_empty());
    let dispatched: usize = rec.log.iter().map(|r| r.dispatched.len()).sum();
    assert!(dispatched >= 40, "dispatched {dispatched}");
    for (i, r) in rec.log.iter().enumerate() {
        assert_eq!(r.call_index, i as u64);
    }
    assert_eq!(rec.digest, decision_digest(&rec.log));
    for run in 0..20 {
        let log = replay_once(e.dir.path(), &rec);
        assert_eq!(log, rec.log, "replay {run}");
        assert_eq!(decision_digest(&log), rec.digest, "replay {run}");
    }
}

#[test]
fn replay_on_a_graph_in_another_temp_path_gives_the_same_digest() {
    let (_e, rec) = record_forty();
    let other_root = project();
    let graph_dir = tempfile::tempdir().expect("tempdir");
    let graph = uriel_core::Graph::open(graph_dir.path().join("replay.seal")).expect("graph");
    let log = House::replay(
        HouseConfig::at(other_root.path()),
        graph,
        rec.script.clone(),
        rec.calls.clone(),
    )
    .expect("replay");
    assert_eq!(decision_digest(&log), rec.digest);
    assert_eq!(log, rec.log);
}

#[test]
fn changing_one_probe_value_changes_the_digest() {
    let (e, rec) = record_forty();
    let k = rec.script.len() / 2;
    let mut script = rec.script.clone();
    script[k].free_ram_mb += 1;
    let graph = uriel_core::Graph::open_in_memory().expect("graph");
    let log = House::replay(
        HouseConfig::at(e.dir.path()),
        graph,
        script,
        rec.calls.clone(),
    )
    .expect("replay");
    assert_eq!(log.len(), rec.log.len());
    assert_eq!(log[k].probe.free_ram_mb, rec.log[k].probe.free_ram_mb + 1);
    assert_ne!(decision_digest(&log), rec.digest);
}

#[test]
fn independent_reports_are_deterministic_per_call_order() {
    let (e_ab, ab) = two_independent_reports(["G1.1", "G2.1"], false);
    let (e_ba, ba) = two_independent_reports(["G2.1", "G1.1"], false);
    let (e_mid, mid) = two_independent_reports(["G1.1", "G2.1"], true);
    assert_eq!(report_order(&ab.calls), vec!["G1.1", "G2.1"]);
    assert_eq!(report_order(&ba.calls), vec!["G2.1", "G1.1"]);
    assert_ne!(ab.calls, ba.calls);
    for (e, rec) in [(&e_ab, &ab), (&e_ba, &ba), (&e_mid, &mid)] {
        for run in 0..5 {
            let log = replay_once(e.dir.path(), rec);
            assert_eq!(log, rec.log, "replay {run}");
            assert_eq!(decision_digest(&log), rec.digest, "replay {run}");
        }
    }
    assert_ne!(ab.digest, mid.digest);
    assert_ne!(ba.digest, mid.digest);
    assert_eq!(ab.log.len() + 1, mid.log.len());
}

#[test]
fn a_probe_below_the_reserve_defers_every_heavy_leaf_identically() {
    let e = env_with(2_000, simulation::TOTAL_MB, |_| {});
    let goals = simulation::forty();
    let heavy: Vec<String> = goals
        .iter()
        .filter(|g| {
            g.leaf
                && g.test
                    .as_ref()
                    .is_some_and(|t| t.class == "build" || t.class == "test_unit")
        })
        .map(|g| g.id.to_string())
        .collect();
    assert_eq!(heavy.len(), 6);
    let ready_heavy = ["G1.1", "G1.2", "G1.3"];
    merged(&e, goals);
    for _ in 0..3 {
        e.house.next().expect("next");
    }
    let calls = e.house.history();
    let low = SystemSnapshot {
        free_ram_mb: 2_000,
        total_ram_mb: simulation::TOTAL_MB,
        logical_cores: 8,
        load_pct: 0.0,
    };
    let rec = Recorded {
        calls,
        script: vec![low; 3],
        log: e.house.decision_log(),
        digest: e.house.decision_digest(),
    };
    assert_eq!(rec.log.len(), 3);
    let first = &rec.log[0];
    for id in ready_heavy {
        let d = first
            .deferrals
            .iter()
            .find(|d| d.goal.as_str() == id)
            .unwrap_or_else(|| panic!("{id} not deferred"));
        assert_eq!(d.reason, DeferReason::Ram, "{id}");
    }
    assert!(
        !first.dispatched.is_empty(),
        "light work continues below the reserve"
    );
    for r in &rec.log {
        assert_eq!(r.probe, low);
        for d in &r.dispatched {
            assert!(
                !heavy.contains(&d.goal.to_string()),
                "heavy {} dispatched below the reserve",
                d.goal
            );
        }
    }
    for run in 0..10 {
        let log = replay_once(e.dir.path(), &rec);
        assert_eq!(log, rec.log, "replay {run}");
        assert_eq!(decision_digest(&log), rec.digest, "replay {run}");
    }
}
