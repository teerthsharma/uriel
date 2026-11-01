mod common;

use common::{Proj, Session};
use serde_json::{json, Value};

const VERTICES: [&str; 6] = ["abduce", "analogize", "question", "refute", "eliminate", "monitor"];
const NEW_TOOLS: [&str; 6] = ["genius_plan", "genius_brief", "genius_submit", "fleet_admit", "fleet_event", "fleet_trace"];

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
fn initialize_advertises_prompts_and_the_genius_prompt_is_listed() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[]);
    let init = s.rpc("initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["capabilities"]["prompts"]["listChanged"], false);
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], false);
    let list = s.rpc("prompts/list", json!({}));
    let prompts = list["result"]["prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0]["name"], "genius");
    assert_eq!(prompts[0]["arguments"][0]["required"], true);
}

#[test]
fn the_genius_prompt_is_byte_identical_for_the_same_idea_and_names_the_tools() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[]);
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
    let mut s = start(&p, &[], &[]);
    let idea = "bug \u{1b}[31m sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 \u{202E}x";
    let r = s.rpc("prompts/get", json!({"name": "genius", "arguments": {"idea": idea}}));
    let text = r["result"]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(!text.contains("sk-ant-api03") && !text.contains('\u{1b}') && !text.contains('\u{202E}'));
}

#[test]
fn prompt_errors_are_typed_invalid_params() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[]);
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
fn the_six_new_tools_are_listed_in_both_modes_and_survive_read_only() {
    for (extra, base) in [(vec![], 27usize), (vec!["--mode", "house"], 41), (vec!["--read-only"], 15), (vec!["--mode", "house", "--read-only"], 20)] {
        let p = Proj::new();
        let mut s = start(&p, &extra, &[]);
        let names = s.names();
        for t in NEW_TOOLS {
            assert!(names.iter().any(|n| n == t), "{t} missing with {extra:?}");
        }
        assert_eq!(names.len(), base + 6, "{extra:?}");
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
    let mut s = start(&p, &[], &[("URIEL_FLEET_MAX", "3")]);
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
    let mut s = start(&p, &[], &[]);
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
    let mut s = start(&p, &[], &[]);
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
    let mut s = start(&p, &[], &[]);
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
        let mut s = start(&p, &[], &[]);
        s.text("genius_plan", json!({"idea": idea}))
    };
    assert_eq!(plan("same idea"), plan("same idea"));
    assert_ne!(plan("same idea"), plan("other idea"));
}

#[test]
fn plan_arguments_are_validated() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[]);
    assert!(s.err("genius_plan", json!({"idea": "x", "mode": "hex"})).contains("'mode'"));
    assert!(s.err("genius_plan", json!({"idea": "  "})).contains("'idea'"));
    let fork = s.ok("genius_plan", json!({"idea": "x", "mode": "fork", "warm": true}));
    assert_eq!(fork["waves"][0], json!(["abduce"]));
    assert_eq!(fork["mode"], "fork");
}

#[test]
fn every_new_tool_has_an_object_schema_with_the_right_required_fields_and_no_extra_properties() {
    let p = Proj::new();
    let mut s = start(&p, &[], &[]);
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
