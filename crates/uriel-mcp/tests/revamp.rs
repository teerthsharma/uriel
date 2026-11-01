mod common;

use common::{Proj, Session};
use serde_json::{json, Value};

fn adaptive(extra: &[&str]) -> (Proj, Session) {
    let p = Proj::new();
    let mut args = vec!["--root".to_string(), p.root(), "--in-memory".to_string()];
    args.extend(extra.iter().map(|s| s.to_string()));
    let s = Session::start(&args, &[("URIEL_TOOLS", "adaptive")]);
    (p, s)
}

fn listed(s: &mut Session) -> Vec<String> {
    let r = s.rpc("tools/list", json!({}));
    r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn list_bytes(s: &mut Session) -> usize {
    s.rpc("tools/list", json!({}))["result"]["tools"]
        .to_string()
        .len()
}

#[test]
fn a_tools_call_sent_as_a_notification_is_never_executed() {
    let (_p, mut s) = adaptive(&[]);
    s.raw(
        format!(
            "{}\n",
            json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"create_entities","arguments":{"entities":[{"name":"ghost","type":"T"}]}}})
        )
        .as_bytes(),
    );
    let (g, _) = s.tool("read_graph", json!({}));
    assert_eq!(
        serde_json::from_str::<Value>(&g).unwrap(),
        json!({"entities": [], "relations": []})
    );
}

#[test]
fn an_oversized_line_is_dropped_with_an_error_and_the_server_keeps_serving() {
    let (_p, mut s) = adaptive(&[]);
    let big = vec![b'x'; 9 * 1024 * 1024];
    s.raw(&big);
    s.raw(b"\n");
    let v: Value = serde_json::from_str(&s.recv_line()).unwrap();
    assert_eq!(v["error"]["code"], -32600, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("exceeds"));
    let ping = s.rpc("ping", json!({}));
    assert_eq!(ping["result"], json!({}));
}

#[test]
fn the_default_list_is_the_core_set_and_costs_under_half_of_the_full_list() {
    let (_p, mut a) = adaptive(&[]);
    let names = listed(&mut a);
    for core in [
        "rag_search",
        "rag_expand",
        "seal_recall",
        "read_neo4j_cypher",
        "view",
        "more",
        "usage",
        "tools_enable",
    ] {
        assert!(
            names.iter().any(|n| n == core),
            "{core} missing from {names:?}"
        );
    }
    for later in [
        "create_entities",
        "write_neo4j_cypher",
        "pin",
        "house_next",
        "dispatch_add",
    ] {
        assert!(
            !names.iter().any(|n| n == later),
            "{later} should not be listed yet"
        );
    }
    let adaptive_bytes = list_bytes(&mut a);
    let p = Proj::new();
    let mut f = Session::start(
        &["--root".to_string(), p.root(), "--in-memory".to_string()],
        &[("URIEL_TOOLS", "full")],
    );
    assert!(adaptive_bytes * 2 < list_bytes(&mut f), "{adaptive_bytes}");
}

#[test]
fn every_unlisted_tool_still_works_and_listing_it_is_announced_once() {
    let (_p, mut s) = adaptive(&[]);
    let (_, err) = s.tool(
        "create_entities",
        json!({"entities": [{"name": "a", "type": "T"}]}),
    );
    assert!(!err);
    assert!(listed(&mut s).iter().any(|x| x == "read_graph"));
    assert_eq!(
        s.notes
            .iter()
            .filter(|n| n["method"] == "notifications/tools/list_changed")
            .count(),
        1
    );
    s.tool(
        "create_entities",
        json!({"entities": [{"name": "b", "type": "T"}]}),
    );
    listed(&mut s);
    assert_eq!(
        s.notes.len(),
        1,
        "a second use of a listed group announces nothing"
    );
}

#[test]
fn tools_enable_lists_a_group_and_rejects_nonsense() {
    let (_p, mut s) = adaptive(&[]);
    let (t, err) = s.tool("tools_enable", json!({"group": "place"}));
    assert!(!err, "{t}");
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(v["changed"], true);
    assert!(listed(&mut s).iter().any(|x| x == "why_hot"));
    let (_, err) = s.tool("tools_enable", json!({"group": "place", "off": true}));
    assert!(!err);
    assert!(!listed(&mut s).iter().any(|x| x == "why_hot"));
    let (t, err) = s.tool("tools_enable", json!({"group": "warp"}));
    assert!(err && t.contains("unknown group"), "{t}");
    let (t, err) = s.tool("tools_enable", json!({"group": "house"}));
    assert!(
        err && t.contains("no tools"),
        "normal mode has no house tools: {t}"
    );
}

#[test]
fn a_cut_result_continues_through_more_and_the_pages_rebuild_the_full_text() {
    let q = "UNWIND range(1, 180) AS i RETURN i AS id, 'row-' + toString(i) AS label ORDER BY id";
    let (_p1, mut whole) = adaptive(&["--token-limit", "100000"]);
    let (full, _) = whole.tool("read_neo4j_cypher", json!({"query": q, "format": "json"}));
    let (_p2, mut s) = adaptive(&["--token-limit", "600"]);
    let (first, err) = s.tool("read_neo4j_cypher", json!({"query": q, "format": "json"}));
    assert!(!err);
    let marker = first.find("\n... [truncated").expect("cut notice");
    assert!(first.contains("more(h=1)"), "{first}");
    let mut rebuilt = first[..marker].to_string();
    loop {
        let (page, err) = s.tool("more", json!({"h": 1}));
        assert!(!err, "{page}");
        match page.find("\n... [more(h=1) continues]") {
            Some(i) => rebuilt.push_str(&page[..i]),
            None => {
                rebuilt.push_str(&page);
                break;
            }
        }
    }
    assert_eq!(rebuilt, full);
    let (gone, err) = s.tool("more", json!({"h": 1}));
    assert!(err && gone.contains("no stored remainder"), "{gone}");
}

#[test]
fn a_repeated_large_read_is_a_stub_until_it_changes_or_fresh_is_asked() {
    let (_p, mut s) = adaptive(&[]);
    s.tool("write_neo4j_cypher", json!({"query": "UNWIND range(1, 120) AS i CREATE (:Bulk {id: i, pad: 'row-' + toString(i * 7919) + '-padpadpad'})"}));
    let q = json!({"query": "MATCH (b:Bulk) RETURN b.id AS id, b.pad AS pad ORDER BY id"});
    let (first, _) = s.tool("read_neo4j_cypher", q.clone());
    assert!(first.len() > 800);
    let (second, _) = s.tool("read_neo4j_cypher", q.clone());
    assert!(second.starts_with("=same"), "{second}");
    assert!(second.len() < 100);
    let mut fresh = q.clone();
    fresh["fresh"] = json!(true);
    let (again, _) = s.tool("read_neo4j_cypher", fresh);
    assert_eq!(again, first);
    s.tool(
        "write_neo4j_cypher",
        json!({"query": "MATCH (b:Bulk {id: 1}) SET b.pad = 'changed'"}),
    );
    let (changed, _) = s.tool("read_neo4j_cypher", q);
    assert!(!changed.starts_with("=same") && changed.contains("changed"));
}

#[test]
fn small_repeated_reads_are_never_stubbed_and_errors_never_are() {
    let (_p, mut s) = adaptive(&[]);
    for _ in 0..3 {
        let (t, _) = s.tool("read_neo4j_cypher", json!({"query": "RETURN 1 AS one"}));
        assert!(!t.starts_with("=same"));
    }
    for _ in 0..2 {
        let (t, err) = s.tool("read_neo4j_cypher", json!({"query": "RETURN"}));
        assert!(err && !t.starts_with("=same"));
    }
}

#[test]
fn session_reset_makes_a_repeated_read_whole_again() {
    let (_p, mut s) = adaptive(&[]);
    s.tool("write_neo4j_cypher", json!({"query": "UNWIND range(1, 120) AS i CREATE (:Bulk {id: i, pad: 'row-' + toString(i * 7919) + '-padpadpad'})"}));
    let q = json!({"query": "MATCH (b:Bulk) RETURN b.id AS id, b.pad AS pad ORDER BY id"});
    let (first, _) = s.tool("read_neo4j_cypher", q.clone());
    s.tool("session_reset", json!({}));
    let (after, _) = s.tool("read_neo4j_cypher", q);
    assert_eq!(after, first);
}

#[test]
fn rows_past_the_cap_are_counted_not_sent_in_both_formats() {
    let (_p, mut s) = adaptive(&[]);
    let q = "UNWIND range(1, 260) AS i RETURN i AS id";
    let (c, _) = s.tool("read_neo4j_cypher", json!({"query": q}));
    assert!(c.starts_with("#200"), "{}", &c[..40]);
    assert!(
        c.ends_with("+60 rows not shown (cap 200); add LIMIT or SKIP"),
        "{c}"
    );
    let (j, _) = s.tool("read_neo4j_cypher", json!({"query": q, "format": "json"}));
    let v: Value = serde_json::from_str(&j).unwrap();
    assert_eq!(v["rows"].as_array().unwrap().len(), 200);
    assert_eq!(v["omitted"], 60);
    let (exact, _) = s.tool(
        "read_neo4j_cypher",
        json!({"query": "UNWIND range(1, 200) AS i RETURN i AS id", "format": "json"}),
    );
    assert!(
        serde_json::from_str::<Value>(&exact).unwrap().is_array(),
        "exactly the cap is not cut"
    );
}

#[test]
fn the_memory_graph_is_capped_and_says_how_to_reach_the_rest() {
    let (_p, mut s) = adaptive(&["--reduce", "off"]);
    let ents: Vec<Value> = (0..230)
        .map(|i| json!({"name": format!("e{i:03}"), "type": "T"}))
        .collect();
    s.tool("create_entities", json!({"entities": ents}));
    let (g, _) = s.tool("read_graph", json!({"fresh": true}));
    let v: Value = serde_json::from_str(&g)
        .unwrap_or_else(|e| panic!("{e}: {}", &g[g.len().saturating_sub(200)..]));
    assert_eq!(v["entities"].as_array().unwrap().len(), 200);
    assert_eq!(v["omitted"], 30);
}

#[test]
fn pressure_thresholds_tighten_the_cap_and_say_so_once() {
    let (_p, mut s) = adaptive(&[
        "--soft-tokens",
        "300",
        "--hard-tokens",
        "100000",
        "--token-limit",
        "6000",
    ]);
    let q = "UNWIND range(1, 150) AS i RETURN i AS id, 'abcdefghij' AS pad";
    let (a, _) = s.tool("read_neo4j_cypher", json!({"query": q, "format": "json"}));
    assert!(
        !a.contains("truncated"),
        "under the threshold nothing is cut"
    );
    let (u, _) = s.tool("usage", json!({}));
    assert_eq!(
        serde_json::from_str::<Value>(&u).unwrap()["pressure"],
        "tight"
    );
    let q2 =
        "UNWIND range(1, 200) AS i RETURN i AS id, 'item-' + toString(i * 104729) + '-zz' AS pad";
    let (b, _) = s.tool(
        "read_neo4j_cypher",
        json!({"query": q2, "format": "compact"}),
    );
    assert!(
        b.contains("truncated"),
        "tight pressure caps at half the limit: {} chars",
        b.len()
    );
    assert!(
        b.contains("[budget:"),
        "the first compact result after the crossing says why"
    );
    let (c, _) = s.tool(
        "read_neo4j_cypher",
        json!({"query": q2, "format": "compact", "fresh": true}),
    );
    assert!(!c.contains("[budget:"), "said once");
}

#[test]
fn usage_counts_tokens_calls_and_what_it_saved() {
    let (_p, mut s) = adaptive(&[]);
    s.tool("read_neo4j_cypher", json!({"query": "RETURN 1 AS one"}));
    let (u, _) = s.tool("usage", json!({}));
    let v: Value = serde_json::from_str(&u).unwrap();
    assert!(v["tokens_sent"].as_u64().unwrap() >= 1);
    assert_eq!(v["calls"], 1);
    assert_eq!(v["pressure"], "normal");
}

#[test]
fn full_mode_keeps_the_legacy_surface_and_never_announces_changes() {
    let p = Proj::new();
    let mut s = Session::start(
        &["--root".to_string(), p.root(), "--in-memory".to_string()],
        &[("URIEL_TOOLS", "full")],
    );
    let init = s.rpc("initialize", json!({}));
    assert_eq!(
        init["result"]["capabilities"]["tools"]["listChanged"],
        false
    );
    s.tool(
        "create_entities",
        json!({"entities": [{"name": "a", "type": "T"}]}),
    );
    assert!(s.notes.is_empty());
    assert!(listed(&mut s).iter().any(|n| n == "write_neo4j_cypher"));
}

#[test]
fn adaptive_mode_declares_list_changed_and_its_instructions_name_every_hidden_group() {
    let (_p, mut s) = adaptive(&["--mode", "house"]);
    let init = s.rpc("initialize", json!({}));
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);
    let ins = init["result"]["instructions"].as_str().unwrap();
    for g in [
        "memory:",
        "write:",
        "place:",
        "house:",
        "research:",
        "dispatch:",
        "tools_enable",
        "more(h=N)",
        "fresh=true",
    ] {
        assert!(ins.contains(g), "{g} missing from the instructions");
    }
}

#[test]
fn house_tools_appear_once_house_need_succeeds_and_dispatch_follows_open_and_close() {
    let (_p, mut s) = adaptive(&["--mode", "house"]);
    let before = listed(&mut s);
    assert!(
        before.iter().any(|n| n == "house_need") && before.iter().any(|n| n == "dispatch_open")
    );
    assert!(!before
        .iter()
        .any(|n| n == "house_next" || n == "dispatch_add"));
    s.tool("dispatch_open", json!({"duration": "2h", "workers": 1}));
    let mid = listed(&mut s);
    assert!(
        mid.iter().any(|n| n == "dispatch_add" && true)
            && mid.iter().any(|n| n == "dispatch_close")
    );
    s.tool("dispatch_close", json!({}));
    assert!(!listed(&mut s).iter().any(|n| n == "dispatch_add"));
}

#[test]
fn dispatch_tools_run_a_window_from_open_to_close_through_the_gate_and_the_slot_limit() {
    let (p, mut s) = adaptive(&["--mode", "house"]);
    let (t, err) = s.tool("dispatch_open", json!({"duration": "90m", "workers": 1}));
    assert!(!err, "{t}");
    assert_eq!(serde_json::from_str::<Value>(&t).unwrap()["minutes"], 90);
    let (t, _) = s.tool(
        "dispatch_add",
        json!({"scope": "write parser", "goal_id": "G1"}),
    );
    let one: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(one["actions"][0]["do"], "start");
    let (t, _) = s.tool(
        "dispatch_add",
        json!({"scope": "write printer", "goal_id": "G2"}),
    );
    let two: Value = serde_json::from_str(&t).unwrap();
    assert!(
        two["actions"].as_array().unwrap().is_empty(),
        "one worker: the second ticket queues"
    );
    let (t, _) = s.tool(
        "dispatch_report",
        json!({"ticket": 1, "kind": "ready", "note": "branch afk/g1; tests ok"}),
    );
    let next: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(next["actions"][0]["ticket"], 2);
    let (t, _) = s.tool("dispatch_status", json!({}));
    let st: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(st["window"]["open"], true);
    assert_eq!(st["tickets"].as_array().unwrap().len(), 2);
    let (t, err) = s.tool("dispatch_close", json!({}));
    assert!(!err, "{t}");
    let sum: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(sum["ready"][0]["result"], "branch afk/g1; tests ok");
    assert_eq!(sum["unfinished"][0]["goal"], "G2");
    let dir = std::path::PathBuf::from(p.root())
        .join(".uriel")
        .join("afk");
    assert!(!dir.join("session.jsonl").exists());
    assert_eq!(
        std::fs::read_dir(dir).unwrap().count(),
        1,
        "the log is archived, not deleted"
    );
}

#[test]
fn dispatch_refuses_bad_input_and_a_second_window_without_touching_the_first() {
    let (_p, mut s) = adaptive(&["--mode", "house"]);
    let (t, err) = s.tool("dispatch_open", json!({"duration": "soon"}));
    assert!(err && t.contains("bad duration"), "{t}");
    let (t, err) = s.tool("dispatch_add", json!({"scope": "x"}));
    assert!(err && t.contains("no afk session"), "{t}");
    s.tool("dispatch_open", json!({"duration": "1h"}));
    let (t, err) = s.tool("dispatch_open", json!({"duration": "1h"}));
    assert!(err && t.contains("already open"), "{t}");
    let (t, err) = s.tool("dispatch_report", json!({"ticket": 9, "kind": "ready"}));
    assert!(err && t.contains("no ticket"), "{t}");
    let (t, err) = s.tool("dispatch_report", json!({"ticket": 1, "kind": "great"}));
    assert!(err && t.contains("not one of"), "{t}");
}

#[test]
fn dispatch_belongs_to_house_mode_and_to_writable_servers_only() {
    let (_p, mut normal) = adaptive(&[]);
    let (t, err) = normal.tool("dispatch_open", json!({"duration": "1h"}));
    assert!(err && t.contains("wrong_mode"), "{t}");
    let (_p2, mut ro) = adaptive(&["--mode", "house", "--read-only"]);
    let (t, err) = ro.tool("dispatch_open", json!({"duration": "1h"}));
    assert!(err && t.contains("read-only"), "{t}");
    assert!(!listed(&mut ro).iter().any(|n| n.starts_with("dispatch_")));
}

#[test]
fn a_cli_session_and_the_mcp_tools_share_one_log() {
    let (p, mut s) = adaptive(&["--mode", "house"]);
    s.tool("dispatch_open", json!({"duration": "1h"}));
    let log = std::path::PathBuf::from(p.root())
        .join(".uriel")
        .join("afk")
        .join("session.jsonl");
    let afk = uriel_dispatch::afk::Afk::open(&log);
    assert!(
        afk.is_ok(),
        "the CLI's reader accepts what the MCP tool wrote"
    );
}

#[test]
fn a_department_plan_runs_through_the_mcp_tools_with_checklists_and_shares() {
    let (_p, mut s) = adaptive(&["--mode", "house"]);
    s.tool("dispatch_open", json!({"duration": "2h", "workers": 2}));
    let plan = json!({"departments": [
        {"name": "Research", "goal": "Find the fix", "subgoals": [{"goal": "Reproduce", "leaves": [{"goal": "Failing case", "checklist": ["seen failing", "minimal"]}]}]},
        {"name": "Build", "goal": "Land it", "subgoals": [{"goal": "Implement", "leaves": [{"goal": "Code", "checklist": ["compiles"]}]}]}
    ]});
    let (t, err) = s.tool("dispatch_plan", json!({"plan": plan}));
    assert!(!err, "{t}");
    assert_eq!(serde_json::from_str::<Value>(&t).unwrap()["leaves"], 2);
    let (t, err) = s.tool("dispatch_plan", json!({"plan": {"departments": []}}));
    assert!(err && t.contains("already has a plan"), "{t}");
    let (t, _) = s.tool("dispatch_add", json!({"leaf": "research.1.1"}));
    assert_eq!(serde_json::from_str::<Value>(&t).unwrap()["actions"][0]["goal"], "research.1.1");
    let (t, err) = s.tool("dispatch_add", json!({"leaf": "research.1.1"}));
    assert!(err && t.contains("already has a ticket"), "{t}");
    let (t, _) = s.tool("dispatch_check", json!({"leaf": "research.1.1", "index": 0}));
    assert_eq!(serde_json::from_str::<Value>(&t).unwrap()["plan"], json!({"done": 1, "total": 3}));
    let (t, err) = s.tool("dispatch_check", json!({"leaf": "research.1.1", "index": 7}));
    assert!(err && t.contains("no checklist item"), "{t}");
    let (t, err) = s.tool("dispatch_add", json!({}));
    assert!(err && t.contains("leaf"), "{t}");
    let (t, _) = s.tool("dispatch_status", json!({}));
    let st: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(st["departments"][0]["progress"], json!({"done": 1, "total": 2}));
}

#[test]
fn the_reducer_shrinks_big_json_results_losslessly_and_reports_the_saving() {
    let ents: Vec<Value> = (0..120).map(|i| json!({"name": format!("e{i:03}"), "type": "Thing"})).collect();
    let (_p1, mut off) = adaptive(&["--reduce", "off"]);
    off.tool("create_entities", json!({"entities": ents.clone()}));
    let (plain, _) = off.tool("read_graph", json!({}));
    let (_p2, mut on) = adaptive(&["--reduce", "on"]);
    on.tool("create_entities", json!({"entities": ents}));
    let (reduced, _) = on.tool("read_graph", json!({}));
    assert!(reduced.len() * 100 < plain.len() * 85, "{} vs {}", reduced.len(), plain.len());
    let back = uriel_terse::data::decode(&reduced).expect("terse text decodes");
    assert_eq!(back, uriel_terse::Value::parse(&plain).unwrap());
    let (u, _) = on.tool("usage", json!({}));
    let v: Value = serde_json::from_str(&u).unwrap();
    assert_eq!(v["reducer"]["on"], true);
    assert!(v["reducer"]["by_stage"]["data"].as_u64().unwrap() > 0, "{u}");
}

#[test]
fn explicit_json_and_exact_tools_bypass_the_reducer_and_off_means_off() {
    let ents: Vec<Value> = (0..120).map(|i| json!({"name": format!("e{i:03}"), "type": "Thing"})).collect();
    let (_p, mut on) = adaptive(&["--reduce", "on"]);
    on.tool("create_entities", json!({"entities": ents}));
    let (j, _) = on.tool("search_memories", json!({"query": "e0", "format": "json"}));
    assert!(serde_json::from_str::<Value>(&j).is_ok(), "an explicit format=json stays JSON");
    let (_p2, mut off) = adaptive(&["--reduce", "off"]);
    let (u, _) = off.tool("usage", json!({}));
    assert_eq!(serde_json::from_str::<Value>(&u).unwrap()["reducer"]["on"], false);
}

#[test]
fn the_reducer_defaults_to_on_with_adaptive_tools_and_off_with_full() {
    let (_p, mut a) = adaptive(&[]);
    let (u, _) = a.tool("usage", json!({}));
    assert_eq!(serde_json::from_str::<Value>(&u).unwrap()["reducer"]["on"], true);
    let p = Proj::new();
    let mut f = Session::start(
        &["--root".to_string(), p.root(), "--in-memory".to_string()],
        &[("URIEL_TOOLS", "full")],
    );
    let (u, _) = f.tool("usage", json!({}));
    assert_eq!(serde_json::from_str::<Value>(&u).unwrap()["reducer"]["on"], false);
}
