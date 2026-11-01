mod common;

use common::{Proj, Session};
use serde_json::{json, Value};

const VERTICES: [&str; 6] = ["abduce", "analogize", "question", "refute", "eliminate", "monitor"];
const NEW_TOOLS: [&str; 6] = ["genius_plan", "genius_brief", "genius_submit", "fleet_admit", "fleet_event", "fleet_trace"];
const LEDGER_TOOLS: [&str; 3] = ["ledger_add", "ledger_next", "ledger_done"];

fn start(p: &Proj, extra: &[&str], envs: &[(&str, &str)]) -> Session {
    let mut args = vec!["--root".to_string(), p.root(), "--in-memory".to_string()];
    args.extend(extra.iter().map(|s| s.to_string()));
    Session::start(&args, envs)
}

fn r1(plan: &str, v: &str) -> Value {
    json!({"plan": plan, "vertex": v, "claims": [
        {"id": format!("{v}.c1"), "kind": "conjecture", "text": format!("{v} explains it"), "falsifier": "f", "test": "t", "cost": "S"},
        {"id": format!("{v}.c2"), "kind": "wild", "text": format!("{v} strange"), "falsifier": "f", "test": "t", "cost": "M", "confidence": 0.3}
    ]})
}

fn r2(plan: &str, i: usize) -> Value {
    let v = VERTICES[i];
    let left = VERTICES[(i + 5) % 6];
    json!({"plan": plan, "vertex": v, "claims": [
        {"id": format!("{v}.d1"), "kind": "question", "text": "reply", "targets": [format!("{left}.c1")], "stance": "support"}]})
}

#[test]
fn house_mode_advertises_prompts_and_lists_the_genius_prompt() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[]);
    let init = s.rpc("initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["capabilities"]["prompts"]["listChanged"], false);
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], false);
    let list = s.rpc("prompts/list", json!({}));
    let prompts = list["result"]["prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0]["name"], "genius");
    assert_eq!(prompts[0]["arguments"][0]["required"], true);
    assert!(prompts[0]["description"].as_str().unwrap().contains("unproven"));
}

#[test]
fn normal_mode_is_unchanged_it_has_no_prompts_and_refuses_genius_with_a_typed_wrong_mode() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[]);
    let init = s.rpc("initialize", json!({"protocolVersion": "2025-06-18"}));
    assert!(init["result"]["capabilities"].get("prompts").is_none());
    assert_eq!(s.rpc("prompts/list", json!({}))["result"]["prompts"], json!([]));
    let r = s.rpc("prompts/get", json!({"name": "genius", "arguments": {"idea": "x"}}));
    assert_eq!(r["error"]["code"], -32602);
    assert!(r["error"]["message"].as_str().unwrap().contains("house mode"));
    for tool in ["genius_plan", "genius_brief", "genius_submit"] {
        let e = s.err(tool, json!({"idea": "x", "plan": "p"}));
        let v: Value = serde_json::from_str(&e).unwrap();
        assert_eq!((v["error"].as_str(), v["requires"].as_str()), (Some("wrong_mode"), Some("house")), "{tool}");
    }
}

#[test]
fn the_genius_prompt_is_byte_identical_for_the_same_idea_and_names_the_tools() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[]);
    let get = |s: &mut Session| s.rpc("prompts/get", json!({"name": "genius", "arguments": {"idea": "why is it slow?"}}));
    let a = get(&mut s);
    let b = get(&mut s);
    assert_eq!(a["result"], b["result"]);
    let text = a["result"]["messages"][0]["text"].as_str().or_else(|| a["result"]["messages"][0]["content"]["text"].as_str()).unwrap();
    assert!(text.ends_with("Idea: why is it slow?"));
    for tool in NEW_TOOLS {
        assert!(text.contains(tool), "{tool}");
    }
    assert_eq!(a["result"]["messages"][0]["role"], "user");
}

#[test]
fn the_genius_prompt_strips_control_bytes_and_secrets_from_the_idea() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[]);
    let idea = "bug \u{1b}[31m sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 \u{202E}x";
    let r = s.rpc("prompts/get", json!({"name": "genius", "arguments": {"idea": idea}}));
    let text = r["result"]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(!text.contains("sk-ant-api03") && !text.contains('\u{1b}') && !text.contains('\u{202E}'));
}

#[test]
fn prompt_errors_are_typed_invalid_params() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[]);
    for params in [
        json!({"name": "nope", "arguments": {"idea": "x"}}),
        json!({"name": "genius"}),
        json!({"name": "genius", "arguments": {"idea": "   "}}),
        json!({"arguments": {"idea": "x"}}),
    ] {
        let r = s.rpc("prompts/get", params.clone());
        assert_eq!(r["error"]["code"], -32602, "{params}");
    }
}

#[test]
fn fleet_and_ledger_tools_are_in_both_modes_genius_only_in_house_and_ledger_not_in_read_only() {
    for (extra, base, fleet, ledger, genius) in [
        (vec![], 27usize, 3usize, 3usize, 0usize),
        (vec!["--mode", "house"], 41, 3, 3, 3),
        (vec!["--read-only"], 15, 3, 0, 0),
        (vec!["--mode", "house", "--read-only"], 20, 3, 0, 3),
    ] {
        let p = Proj::new();
        let mut s = start(&p, &extra, &[]);
        let names = s.names();
        let has = |group: &[&str]| group.iter().filter(|t| names.iter().any(|n| n == *t)).count();
        assert_eq!(has(&["fleet_admit", "fleet_event", "fleet_trace"]), fleet, "{extra:?}");
        assert_eq!(has(&LEDGER_TOOLS), ledger, "{extra:?}");
        assert_eq!(has(&["genius_plan", "genius_brief", "genius_submit"]), genius, "{extra:?}");
        assert_eq!(names.len(), base + fleet + ledger + genius, "{extra:?}");
    }
}

#[test]
fn fleet_admit_max_grants_exactly_the_configured_cap_and_defers_the_rest() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[("URIEL_FLEET_MAX", "4")]);
    let a = s.ok("fleet_admit", json!({"run": "r1", "max": true}));
    assert_eq!(a["granted"], json!(["a1", "a2", "a3", "a4"]));
    assert_eq!(a["free_after"], 0);
    let b = s.ok("fleet_admit", json!({"run": "r1", "count": 3}));
    assert_eq!(b["granted"], json!([]));
    assert_eq!(b["deferred"], 3);
    assert_eq!(b["denied_by"], "concurrency");
    s.ok("fleet_event", json!({"run": "r1", "agent": "a1", "kind": "end"}));
    let c = s.ok("fleet_admit", json!({"run": "r1", "count": 3}));
    assert_eq!(c["granted"], json!(["a5"]));
}

#[test]
fn a_bad_cap_in_the_environment_falls_back_to_the_default_and_never_to_zero() {
    for bad in ["0", "abc", "-3", ""] {
        let p = Proj::new();
        let mut s = start(&p, &[], &[("URIEL_FLEET_MAX", bad)]);
        let a = s.ok("fleet_admit", json!({"run": "r1", "max": true}));
        let n = a["granted"].as_array().unwrap().len();
        assert!((1..=8).contains(&n), "{bad:?} gave {n}");
    }
}

#[test]
fn fleet_events_are_sanitised_chained_and_exported() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[]);
    s.ok("fleet_admit", json!({"run": "r1", "count": 1, "role": "lens", "tier": "small"}));
    let ev = s.ok("fleet_event", json!({
        "run": "r1", "agent": "a1", "kind": "tool", "name": "bash",
        "attrs": {"cmd": "curl -H 'Authorization: Bearer abcdef0123456789ABCDEF' \u{1b}[31m"},
        "tokens_in": 100, "tokens_out": 20, "cached_in": 80
    }));
    assert_eq!(ev["sanitised"]["secrets"]["bearer_token"], 1);
    assert_eq!(ev["sanitised"]["ansi"], 1);
    s.ok("fleet_event", json!({"run": "r1", "agent": "a1", "kind": "end", "status": "failed", "note": "boom"}));
    let sum = s.ok("fleet_trace", json!({"run": "r1"}));
    assert_eq!(sum["chain_ok"], true);
    assert_eq!(sum["failed"], 1);
    assert_eq!(sum["tokens"], 120);
    let jsonl = s.text("fleet_trace", json!({"run": "r1", "format": "jsonl"}));
    assert!(!jsonl.contains("abcdef0123456789ABCDEF") && !jsonl.contains('\u{1b}'));
    let lines: Vec<&str> = jsonl.lines().collect();
    assert_eq!(lines.len(), 1 + 3);
    for l in lines {
        serde_json::from_str::<Value>(l).unwrap();
    }
}

#[test]
fn fleet_errors_are_typed_and_late_events_are_refused() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[]);
    assert!(s.err("fleet_event", json!({"run": "nope", "agent": "a1", "kind": "mark"})).contains("unknown run"));
    assert!(s.err("fleet_admit", json!({"run": "bad id"})).contains("bad id"));
    s.ok("fleet_admit", json!({"run": "r1", "count": 1}));
    s.ok("fleet_event", json!({"run": "r1", "agent": "a1", "kind": "end"}));
    assert!(s.err("fleet_event", json!({"run": "r1", "agent": "a1", "kind": "mark"})).contains("not running"));
    assert!(s.err("fleet_event", json!({"run": "r1", "kind": "mark"})).contains("'agent' is required"));
    assert!(s.err("fleet_event", json!({"run": "r1", "agent": "a1", "kind": "scope_start"})).contains("'kind' must be"));
}

#[test]
fn reap_frees_agents_that_never_reported() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[("URIEL_FLEET_MAX", "2")]);
    s.ok("fleet_admit", json!({"run": "r1", "max": true}));
    assert_eq!(s.ok("fleet_event", json!({"run": "r1", "kind": "reap"}))["reaped"], 2);
    assert_eq!(s.ok("fleet_admit", json!({"run": "r1", "max": true}))["granted"].as_array().unwrap().len(), 2);
}

#[test]
fn waves_under_a_cap_of_three_admit_release_and_trace_every_lens() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[("URIEL_FLEET_MAX", "3")]);
    let plan = s.ok("genius_plan", json!({"idea": "why do short tables cost more tokens?"}));
    let id = plan["plan"].as_str().unwrap().to_string();
    let waves = plan["waves"].as_array().unwrap();
    assert_eq!(waves.iter().map(|w| w.as_array().unwrap().len()).collect::<Vec<_>>(), vec![3, 3]);
    assert!(plan.get("briefs").is_none());
    let mut ready = false;
    for wave in waves {
        let names = wave.as_array().unwrap();
        let g = s.ok("fleet_admit", json!({"run": id, "count": names.len(), "role": "lens", "tier": "small"}));
        assert_eq!(g["deferred"], 0);
        let granted = g["granted"].as_array().unwrap();
        assert_eq!(s.ok("fleet_admit", json!({"run": id, "count": 1}))["denied_by"], "concurrency");
        for (v, a) in names.iter().zip(granted) {
            let brief = s.text("genius_brief", json!({"plan": id, "vertex": v, "round": 1}));
            assert!(brief.starts_with("URIEL GENIUS 1 plan ") && brief.contains("LENS"), "{brief}");
            let mut sub = r1(&id, v.as_str().unwrap());
            sub["agent"] = a.clone();
            let out = s.ok("genius_submit", sub);
            assert_eq!(out["fleet"]["released"], true);
            ready |= out["round2_ready"] == true;
        }
    }
    assert!(ready);
    let sum = s.ok("fleet_trace", json!({"run": id}));
    assert_eq!(sum["running"], 0);
    assert_eq!(sum["done"], 6);
    assert_eq!(sum["chain_ok"], true);
}

#[test]
fn rejected_submissions_come_back_typed_and_are_traced_without_releasing_the_slot() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[]);
    let plan = s.ok("genius_plan", json!({"idea": "x marks the spot"}));
    let id = plan["plan"].as_str().unwrap().to_string();
    let g = s.ok("fleet_admit", json!({"run": id, "count": 1}));
    let agent = g["granted"][0].as_str().unwrap().to_string();
    let bad = json!({"plan": id, "vertex": "abduce", "agent": agent, "claims": [
        {"id": "abduce.c1", "kind": "wild", "text": "crazy", "falsifier": "f", "test": "t", "cost": "L"}]});
    let e = s.err("genius_submit", bad);
    let v: Value = serde_json::from_str(&e).unwrap();
    assert_eq!(v["error"], "wild_not_cheap");
    let sum = s.ok("fleet_trace", json!({"run": id}));
    assert_eq!(sum["running"], 1);
    let jsonl = s.text("fleet_trace", json!({"run": id, "format": "jsonl"}));
    assert!(jsonl.contains("submit_rejected") && jsonl.contains("wild_not_cheap"));
}

#[test]
fn the_full_twelve_submission_run_over_stdio_ends_in_a_verdict_with_a_test_queue() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[]);
    let id = s.ok("genius_plan", json!({"idea": "the cache is cold after compaction"}))["plan"].as_str().unwrap().to_string();
    assert!(s.err("genius_brief", json!({"plan": id, "vertex": "abduce", "round": 2})).contains("wrong_round"));
    let mut ready = false;
    for v in VERTICES {
        ready = s.ok("genius_submit", r1(&id, v))["round2_ready"] == true;
    }
    assert!(ready);
    for v in VERTICES {
        let brief = s.text("genius_brief", json!({"plan": id, "vertex": v, "round": 2}));
        assert!(brief.contains("ROUND 2") && brief.len() < 20_000);
    }
    let mut verdict = Value::Null;
    for i in 0..6 {
        verdict = s.ok("genius_submit", r2(&id, i))["verdict"].clone();
    }
    assert_eq!(verdict["test_queue_total"], 12);
    assert_eq!(verdict["test_queue"][0]["kind"], "wild");
    let first = verdict["test_queue"][0]["id"].as_str().unwrap();
    let done = s.ok("genius_submit", json!({"plan": id, "experiment": [{"claim": first, "outcome": "killed", "evidence": ["ran it"]}]}));
    assert_eq!(done["verdict"]["refuted"][0]["id"], first);
    assert!(s.err("genius_submit", json!({"plan": id, "experiment": [{"claim": first, "outcome": "killed", "evidence": ["x"]}]})).contains("already_tested"));
}

#[test]
fn fork_mode_serves_the_shared_text_once_and_the_lens_blocks_after_it() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[]);
    let id = s.ok("genius_plan", json!({"idea": "x", "mode": "fork", "warm": true}))["plan"].as_str().unwrap().to_string();
    let shared = s.text("genius_brief", json!({"plan": id}));
    assert!(shared.contains("IDEA:") && !shared.contains("LENS 1/6"));
    let lens = s.text("genius_brief", json!({"plan": id, "vertex": "refute"}));
    assert!(lens.starts_with("\n\nLENS 4/6: refute"));
    assert!(s.err("genius_brief", json!({"plan": id, "vertex": "hexagon"})).contains("bad_vertex"));
}

#[test]
fn the_same_idea_over_two_servers_gives_the_same_plan_bytes() {
    let plan = |idea: &str| {
        let p = Proj::new();
        let mut s = start(&p, &["--mode", "house"], &[]);
        s.text("genius_plan", json!({"idea": idea}))
    };
    assert_eq!(plan("same idea"), plan("same idea"));
    assert_ne!(plan("same idea"), plan("other idea"));
}

#[test]
fn plan_arguments_are_validated() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[]);
    assert!(s.err("genius_plan", json!({"idea": "x", "mode": "hex"})).contains("'mode'"));
    assert!(s.err("genius_plan", json!({"idea": "  "})).contains("'idea'"));
    let fork = s.ok("genius_plan", json!({"idea": "x", "mode": "fork", "warm": true}));
    assert_eq!(fork["waves"][0], json!(["abduce"]));
    assert_eq!(fork["mode"], "fork");
}

#[test]
fn every_new_tool_has_an_object_schema_with_the_right_required_fields_and_no_extra_properties() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[]);
    let list = s.rpc("tools/list", json!({}));
    let want = [
        ("genius_plan", vec!["idea"]),
        ("genius_brief", vec!["plan"]),
        ("genius_submit", vec!["plan"]),
        ("fleet_admit", vec!["run"]),
        ("fleet_event", vec!["run", "kind"]),
        ("fleet_trace", vec!["run"]),
    ];
    for (name, required) in want {
        let t = list["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap();
        let schema = &t["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        let got: Vec<&str> = schema["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(got, required, "{name}");
        for r in required {
            assert!(schema["properties"][r].is_object(), "{name}.{r}");
        }
        assert_eq!(t["annotations"]["readOnlyHint"], true, "{name}");
    }
}

fn persistent(dir: &std::path::Path, envs: &[(&str, &str)]) -> Session {
    let db = dir.join("g.uriel");
    Session::start(&["--db".to_string(), db.display().to_string()], envs)
}

fn goals(spec: &[(&str, &[&str])]) -> Value {
    json!(spec.iter().map(|(id, needs)| json!({"id": id, "title": format!("do {id}"), "needs": needs})).collect::<Vec<_>>())
}

#[test]
fn the_ledger_frontier_feeds_the_deck_which_asks_for_exactly_the_free_slots_and_refills_as_agents_finish() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = persistent(dir.path(), &[("URIEL_FLEET_MAX", "4")]);
    let all: Vec<String> = (0..10).map(|i| format!("g{i}")).collect();
    let spec: Vec<(&str, &[&str])> = all.iter().map(|g| (g.as_str(), &[][..])).collect();
    s.ok("ledger_add", json!({"ledger": "w", "goals": goals(&spec)}));
    let first = s.ok("ledger_next", json!({"ledger": "w"}));
    assert_eq!(first["issued"].as_array().unwrap().len(), 4);
    assert!(first["deck"].as_str().unwrap().contains("launch 4 now"), "{first}");
    let admitted = s.ok("fleet_admit", json!({"run": "w", "count": 4}));
    assert_eq!(admitted["granted"].as_array().unwrap().len(), 4);
    assert!(admitted["deck"].as_str().unwrap().starts_with("deck saturated: launch none"), "{admitted}");
    s.ok("fleet_event", json!({"run": "w", "agent": "a1", "kind": "end"}));
    let id = first["issued"][0]["id"].as_str().unwrap();
    s.ok("ledger_done", json!({"ledger": "w", "id": id, "outcome": "done", "result": "ok"}));
    let second = s.ok("ledger_next", json!({"ledger": "w"}));
    assert_eq!(second["issued"].as_array().unwrap().len(), 1, "one slot was freed");
    assert!(second["deck"].as_str().unwrap().contains("launch 1 now"), "{second}");
}

#[test]
fn dependent_goals_open_layer_by_layer_and_a_failed_goal_blocks_its_subtree() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = persistent(dir.path(), &[]);
    s.ok("ledger_add", json!({"ledger": "d", "goals": goals(&[("a", &[]), ("b", &["a"]), ("c", &["a"]), ("e", &["b"])])}));
    let one = s.ok("ledger_next", json!({"ledger": "d"}));
    assert_eq!(one["issued"], json!([{"id": "a"}]));
    s.ok("ledger_done", json!({"ledger": "d", "id": "a", "outcome": "failed", "result": "boom"}));
    let sum = s.ok("ledger_next", json!({"ledger": "d"}));
    assert_eq!(sum["issued"], json!([]));
    assert_eq!(sum["summary"]["blocked"], 3);
}

#[test]
fn the_python_script_renders_the_prompts_for_the_goals_the_server_issued_and_caches_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = persistent(dir.path(), &[]);
    s.ok("ledger_add", json!({"ledger": "p", "goals": goals(&[("a", &[]), ("b", &["a"])])}));
    s.ok("ledger_next", json!({"ledger": "p"}));
    s.ok("ledger_done", json!({"ledger": "p", "id": "a", "outcome": "done", "result": "found it in src/x.rs:9"}));
    let n = s.ok("ledger_next", json!({"ledger": "p"}));
    let cmd = n["prompts"].as_str().unwrap();
    assert!(cmd.starts_with("python3 scripts/ledger_next.py "));
    let file = n["file"].as_str().unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ledger_next.py");
    let out = std::process::Command::new("python3").arg("-I").arg(&script).arg(file).arg("--issued").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ready"][0]["id"], "b");
    let text = std::fs::read_to_string(v["ready"][0]["path"].as_str().unwrap()).unwrap();
    assert!(text.contains("GOAL b: do b") && text.contains("found it in src/x.rs:9") && text.contains("<<uriel-data "));
}

#[test]
fn a_ledger_survives_a_server_restart_and_its_file_is_private() {
    let dir = tempfile::tempdir().unwrap();
    let file;
    {
        let mut s = persistent(dir.path(), &[]);
        let added = s.ok("ledger_add", json!({"ledger": "r", "goals": goals(&[("a", &[]), ("b", &["a"])])}));
        file = added["file"].as_str().unwrap().to_string();
        s.ok("ledger_next", json!({"ledger": "r"}));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let mut s = persistent(dir.path(), &[]);
    let done = s.ok("ledger_done", json!({"ledger": "r", "id": "a", "outcome": "done", "result": "x"}));
    assert_eq!(done["summary"]["done"], 1);
    assert_eq!(s.ok("ledger_next", json!({"ledger": "r"}))["issued"], json!([{"id": "b"}]));
    assert!(std::fs::read_dir(std::path::Path::new(&file).parent().unwrap()).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains(".tmp")));
}

#[test]
fn a_tampered_ledger_file_is_refused_instead_of_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let file;
    {
        let mut s = persistent(dir.path(), &[]);
        file = s.ok("ledger_add", json!({"ledger": "t", "goals": goals(&[("a", &[]), ("b", &["a"])])}))["file"].as_str().unwrap().to_string();
    }
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    v["goals"][0]["needs"] = json!(["b"]);
    std::fs::write(&file, v.to_string()).unwrap();
    let mut s = persistent(dir.path(), &[]);
    assert!(s.err("ledger_next", json!({"ledger": "t"})).contains("cycle"));
    std::fs::write(&file, "not json").unwrap();
    let mut s = persistent(dir.path(), &[]);
    assert!(s.err("ledger_next", json!({"ledger": "t"})).contains("malformed"));
}

#[test]
fn ledger_errors_are_typed_and_names_cannot_escape_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = persistent(dir.path(), &[]);
    for bad in ["../x", "a/b", "", "a b", "..\\x"] {
        let e = s.err("ledger_add", json!({"ledger": bad, "goals": goals(&[("a", &[])])}));
        assert!(e.contains("bad_id") || e.contains("'ledger'"), "{bad:?}: {e}");
    }
    assert!(s.err("ledger_next", json!({"ledger": "nope"})).contains("unknown_ledger"));
    s.ok("ledger_add", json!({"ledger": "e", "goals": goals(&[("a", &[])])}));
    assert!(s.err("ledger_done", json!({"ledger": "e", "id": "a", "outcome": "done"})).contains("not_issued"));
    assert!(s.err("ledger_add", json!({"ledger": "e", "goals": goals(&[("x", &["y"]), ("y", &["x"])])})).contains("cycle"));
    assert!(s.err("ledger_add", json!({"ledger": "e", "goals": goals(&[("q", &["ghost"])])})).contains("unknown_need"));
    assert!(s.err("ledger_done", json!({"ledger": "e", "id": "a", "outcome": "maybe"})).contains("'outcome'"));
    let files: Vec<_> = std::fs::read_dir(dir.path().join("uriel-ledger")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(files, vec!["e.json"]);
}

#[test]
fn at_most_sixteen_ledgers_are_open_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = persistent(dir.path(), &[]);
    for i in 0..16 {
        s.ok("ledger_add", json!({"ledger": format!("l{i}"), "goals": goals(&[("a", &[])])}));
    }
    assert!(s.err("ledger_add", json!({"ledger": "l16", "goals": goals(&[("a", &[])])})).contains("too_many_ledgers"));
}

#[test]
fn the_ledger_works_in_memory_without_a_file_and_says_so() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[]);
    let added = s.ok("ledger_add", json!({"ledger": "m", "goals": goals(&[("a", &[])])}));
    assert!(added["file"].is_null());
    let n = s.ok("ledger_next", json!({"ledger": "m"}));
    assert_eq!(n["issued"], json!([{"id": "a"}]));
    assert!(n["prompts"].as_str().unwrap().starts_with("no file"));
}

#[test]
fn ledger_text_is_sanitised_before_it_is_stored_or_returned() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = persistent(dir.path(), &[]);
    let g = json!([{"id": "a", "title": "go \u{1b}[31m sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"}]);
    let added = s.ok("ledger_add", json!({"ledger": "s", "goals": g}));
    assert_eq!(added["sanitised"]["secrets"]["anthropic_key"], 1);
    let disk = std::fs::read_to_string(added["file"].as_str().unwrap()).unwrap();
    assert!(!disk.contains("sk-ant-api03") && !disk.contains('\u{1b}'));
}

#[test]
fn genius_seeds_the_deck_so_the_first_call_already_says_how_many_lenses_to_launch() {
    let p = Proj::new();
    let mut s = start(&p, &["--mode", "house"], &[("URIEL_FLEET_MAX", "3")]);
    let plan = s.ok("genius_plan", json!({"idea": "why is the deck empty"}));
    let id = plan["plan"].as_str().unwrap().to_string();
    assert!(plan["deck"].as_str().unwrap().contains("launch 3 now"), "{plan}");
    let again = s.ok("genius_plan", json!({"idea": "why is the deck empty"}));
    assert_eq!(again["deck"], plan["deck"], "re-planning must not double the backlog");
    let g = s.ok("fleet_admit", json!({"run": id, "count": 3}));
    assert!(g["deck"].as_str().unwrap().contains("launch none"), "{g}");
    s.ok("fleet_event", json!({"run": id, "agent": "a1", "kind": "end"}));
    let sum = s.ok("fleet_trace", json!({"run": id, "format": "deck"}));
    assert_eq!((sum["backlog"].as_u64(), sum["launch_now"].as_u64()), (Some(3), Some(1)));
    assert_eq!(s.ok("fleet_trace", json!({"run": id}))["deck"]["launch_now"], 1);
}

#[test]
fn max_policy_over_stdio_reports_the_split_the_model_must_make() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[("URIEL_FLEET_MAX", "6")]);
    let a = s.ok("fleet_admit", json!({"run": "m", "max": true}));
    assert_eq!(a["granted"].as_array().unwrap().len(), 6);
    for id in ["a1", "a2", "a3", "a4"] {
        s.ok("fleet_event", json!({"run": "m", "agent": id, "kind": "end"}));
    }
    let d = s.ok("fleet_trace", json!({"run": "m", "format": "deck"}));
    assert_eq!((d["policy"].as_str(), d["split_needed"].as_u64()), (Some("max"), Some(4)));
    assert!(d["line"].as_str().unwrap().contains("split work into 4 more independent units"));
}

fn goal_claim(owner: &str, goal: &str) -> Value {
    json!({"id": format!("{owner}.c1"), "kind": "conjecture", "text": "G1 is the real cause", "falsifier": "f", "test": "t", "cost": "S", "goal": goal})
}

fn house_with_goals(p: &Proj) -> Session {
    let mut s = start(p, &["--mode", "house"], &[("SEAL_MCP_FAKE_PROBE", "16000,16000")]);
    common::need(&mut s);
    common::chart_and_merge(&mut s, common::tiny());
    s
}

fn finish_to_review(s: &mut Session, id: &str, bound: &str) {
    for v in VERTICES {
        let mut sub = r1(id, v);
        if v == "abduce" {
            sub["claims"][0] = goal_claim("abduce", bound);
        }
        s.ok("genius_submit", sub);
    }
    for i in 0..6 {
        s.ok("genius_submit", r2(id, i));
    }
}

#[test]
fn a_killed_claim_bound_to_a_house_goal_records_a_fact_that_refutes_the_goal() {
    let p = Proj::new();
    let mut s = house_with_goals(&p);
    let id = s.ok("genius_plan", json!({"idea": "why is G1 failing"}))["plan"].as_str().unwrap().to_string();
    finish_to_review(&mut s, &id, "G1");
    let done = s.ok("genius_submit", json!({"plan": id, "experiment": [{"claim": "abduce.c1", "outcome": "killed", "evidence": ["ran G1 test, passes without the suspect"]}]}));
    assert_eq!(done["house"][0]["goal"], "G1");
    assert_eq!(done["house"][0]["refuted"], true);
    let st = s.ok("house_status", json!({}));
    let fact = st["facts"].as_array().unwrap().iter().find(|f| f["id"] == "GX.abduce.c1").unwrap();
    assert_eq!((fact["refutes"].as_str(), fact["by"].as_str()), (Some("G1"), Some("genius")));
    assert!(fact["source"].as_str().unwrap().contains("ran G1 test"));
    let g1 = st["goal_list"].as_array().unwrap().iter().find(|g| g["id"] == "G1").unwrap();
    assert_eq!((g1["status"].as_str(), g1["blocked_reason"].as_str()), (Some("blocked"), Some("refuted")), "{g1}");
}

#[test]
fn a_survived_claim_records_a_fact_that_does_not_refute_anything() {
    let p = Proj::new();
    let mut s = house_with_goals(&p);
    let id = s.ok("genius_plan", json!({"idea": "why is G1 failing"}))["plan"].as_str().unwrap().to_string();
    finish_to_review(&mut s, &id, "G2");
    let done = s.ok("genius_submit", json!({"plan": id, "experiment": [{"claim": "abduce.c1", "outcome": "survived", "evidence": ["bench.txt:3"]}]}));
    assert_eq!(done["house"][0]["refuted"], false);
    let st = s.ok("house_status", json!({}));
    let fact = st["facts"].as_array().unwrap().iter().find(|f| f["id"] == "GX.abduce.c1").unwrap();
    assert!(fact["refutes"].is_null());
    let g2 = st["goal_list"].as_array().unwrap().iter().find(|g| g["id"] == "G2").unwrap();
    assert_ne!(g2["status"], "retired");
}

#[test]
fn a_claim_bound_to_a_goal_that_does_not_exist_is_refused_before_anything_is_stored() {
    let p = Proj::new();
    let mut s = house_with_goals(&p);
    let id = s.ok("genius_plan", json!({"idea": "x"}))["plan"].as_str().unwrap().to_string();
    let mut sub = r1(&id, "abduce");
    sub["claims"][0] = goal_claim("abduce", "G99");
    assert!(s.err("genius_submit", sub.clone()).contains("unknown_goal"));
    sub["claims"][0] = goal_claim("abduce", "G1");
    assert_eq!(s.ok("genius_submit", sub)["accepted"], 2, "the refused attempt stored nothing");
}

#[test]
fn a_goal_may_only_ride_on_a_conjecture_or_wild_claim_and_must_look_like_a_goal_id() {
    let p = Proj::new();
    let mut s = house_with_goals(&p);
    let id = s.ok("genius_plan", json!({"idea": "x"}))["plan"].as_str().unwrap().to_string();
    let mut obs = r1(&id, "abduce");
    obs["claims"] = json!([{"id": "abduce.c1", "kind": "observation", "text": "saw", "evidence": ["a:1"], "goal": "G1"}]);
    assert!(s.err("genius_submit", obs).contains("bad_field"));
    for bad in ["../x", "G1 G2", ""] {
        let mut sub = r1(&id, "abduce");
        sub["claims"][0] = goal_claim("abduce", bad);
        assert!(s.err("genius_submit", sub).contains("bad_field"), "{bad:?}");
    }
}

#[test]
fn unbound_experiments_touch_nothing_in_the_house() {
    let p = Proj::new();
    let mut s = house_with_goals(&p);
    let id = s.ok("genius_plan", json!({"idea": "x"}))["plan"].as_str().unwrap().to_string();
    for v in VERTICES {
        s.ok("genius_submit", r1(&id, v));
    }
    for i in 0..6 {
        s.ok("genius_submit", r2(&id, i));
    }
    let done = s.ok("genius_submit", json!({"plan": id, "experiment": [{"claim": "abduce.c1", "outcome": "killed", "evidence": ["x:1"]}]}));
    assert!(done.get("house").is_none());
    assert!(s.ok("house_status", json!({}))["facts"].as_array().unwrap().is_empty());
}

#[test]
fn the_verdict_shows_which_house_goal_each_queued_test_is_about() {
    let p = Proj::new();
    let mut s = house_with_goals(&p);
    let id = s.ok("genius_plan", json!({"idea": "x"}))["plan"].as_str().unwrap().to_string();
    let mut verdict = Value::Null;
    for v in VERTICES {
        let mut sub = r1(&id, v);
        if v == "abduce" {
            sub["claims"][0] = goal_claim("abduce", "G1");
        }
        s.ok("genius_submit", sub);
    }
    for i in 0..6 {
        verdict = s.ok("genius_submit", r2(&id, i))["verdict"].clone();
    }
    let q = verdict["test_queue"].as_array().unwrap();
    let bound = q.iter().find(|e| e["id"] == "abduce.c1").unwrap();
    assert_eq!(bound["goal"], "G1");
}
