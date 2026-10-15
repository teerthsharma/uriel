//! The events written to `house-events.jsonl` are read by the live board's `gates.py`.
//! This test replays a run through `JsonlSink` and asks the real `gates.compute_gates`
//! (skipped when Python or the skill folder is not on this machine).

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::*;
use uriel_core::Graph;
use uriel_house::*;

fn gates_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let dir = PathBuf::from(home).join(".claude/skills/dispatching-house-mode");
    dir.join("gates.py").exists().then_some(dir)
}

#[test]
fn board_gates_accept_the_emitted_events() {
    let Some(dir) = gates_dir() else {
        eprintln!("gates.py not found: skipped");
        return;
    };
    let tmp = project();
    let log = tmp.path().join("house-events.jsonl");
    let cfg = HouseConfig::at(tmp.path());
    let house = House::open(
        Graph::open_in_memory().expect("graph"),
        cfg,
        Box::new(FakeProbe::new(16_000, 16_000)),
        Box::new(JsonlSink::new(&log)),
    )
    .expect("open");
    house.need("proj", "x", &[]).expect("need");
    house
        .submit_chart(Fellow::Foreman, chart(Fellow::Foreman, base_goals()))
        .expect("foreman");
    let mut alt = leaf("A1", "G0");
    alt.parent = None;
    alt.alt_of = Some(gid("G1.1"));
    alt.est = Some(Est {
        ram_mb: 800,
        secs: 1.0,
        tokens: 100,
    });
    house
        .submit_chart(Fellow::Cameron, chart(Fellow::Cameron, vec![alt]))
        .expect("cameron");
    house.merge().expect("merge");
    loop {
        let b = house.next().expect("next");
        if b.dispatches.is_empty() {
            assert!(b.complete);
            break;
        }
        for d in b.dispatches {
            let t = d.test.name.clone();
            house
                .record_test_run(&t, TestStatus::Red, 1.0, 100, &[])
                .expect("red");
            house
                .record_test_run(&t, TestStatus::Green, 1.0, 100, &[])
                .expect("green");
            let r = house
                .report(
                    &d.goal,
                    GoalStatus::Reached,
                    evidence(&t, TestStatus::Green),
                )
                .expect("report");
            assert_eq!(r.outcome, Outcome::Accepted);
        }
    }
    let script = format!(
        "import json,sys\nsys.path.insert(0, r'{}')\nimport gates\n\
         ev=[json.loads(l) for l in open(r'{}', encoding='utf-8')]\n\
         g=gates.compute_gates(ev)\n\
         print(json.dumps({{'clear':gates.all_clear(g),'unproven':g['goal_unproven'],\
         'kill':g['kill_without_route'],'open':g['goals_open'],'charts':sorted(g['charts']),\
         'theories':[t['status'] for t in g['theories']]}}))",
        dir.display(),
        log.display()
    );
    let out = match Command::new("python").arg("-c").arg(&script).output() {
        Ok(o) => o,
        Err(_) => {
            eprintln!("python not found: skipped");
            return;
        }
    };
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["clear"], true, "{v}");
    assert_eq!(v["unproven"], serde_json::json!([]));
    assert_eq!(
        v["kill"],
        serde_json::json!([]),
        "the cost loser has a recorded route"
    );
    // the only non-reached goal is the retired ALT loser (information, not a gate)
    assert_eq!(v["open"], serde_json::json!(["Foreman:G1.1"]), "{v}");
    assert_eq!(v["charts"], serde_json::json!(["Cameron", "Foreman"]));
    assert_eq!(v["theories"], serde_json::json!(["dead"]));
}
