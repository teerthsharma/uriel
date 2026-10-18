mod common;

use common::{Proj, Session};
use serde_json::{json, Value};

const CLEAN: &str = "no violation found; this is not confirmation";

fn house() -> (Proj, Session) {
    let p = Proj::new();
    let s = Session::start(
        &[
            "--root".to_string(),
            p.root(),
            "--in-memory".to_string(),
            "--mode".to_string(),
            "house".to_string(),
        ],
        &[],
    );
    (p, s)
}

fn rel(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(e, a)| json!({"entity": e, "answer": a}))
            .collect(),
    )
}

fn audit(s: &mut Session, args: Value) -> Value {
    s.ok("research_orbit_audit", args)
}

fn entities(w: &Value) -> Vec<String> {
    w["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .map(|e| e.as_str().expect("str").to_string())
        .collect()
}

fn check<'a>(r: &'a Value, bound: &str) -> &'a Value {
    r["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|c| c["bound"] == bound)
        .unwrap_or_else(|| panic!("no {bound} check in {r}"))
}

#[test]
fn the_research_tools_are_listed_with_strict_schemas_in_house_mode() {
    let (_p, mut s) = house();
    let list = s.rpc("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    let find = |n: &str| {
        tools
            .iter()
            .find(|t| t["name"] == n)
            .unwrap_or_else(|| panic!("{n} missing"))
    };
    for name in ["research_orbit_audit", "research_pooling_audit", "research_record"] {
        let t = find(name);
        assert_eq!(t["inputSchema"]["additionalProperties"], false, "{name}");
        assert_eq!(t["inputSchema"]["type"], "object");
        let d = t["description"].as_str().expect("description");
        assert!(d.contains("one-sided"), "{name}: {d}");
        assert!(d.contains("never confirm"), "{name}: {d}");
    }
    assert_eq!(find("research_orbit_audit")["inputSchema"]["required"], json!(["injective"]));
    assert_eq!(find("research_pooling_audit")["inputSchema"]["required"], json!(["recoveries"]));
    assert_eq!(find("research_record")["inputSchema"]["required"], json!(["tool", "arguments", "goal"]));
    assert_eq!(find("research_orbit_audit")["annotations"]["readOnlyHint"], true);
    assert_eq!(find("research_pooling_audit")["annotations"]["readOnlyHint"], true);
    assert_eq!(find("research_record")["annotations"]["readOnlyHint"], false);
}

#[test]
fn a_collision_is_a_violation_of_the_default_all_correct_claim() {
    let (_p, mut s) = house();
    let r = audit(
        &mut s,
        json!({"injective": true, "relation": rel(&[("a", "x"), ("b", "y"), ("c", "x"), ("d", "z")])}),
    );
    assert_eq!(r["tool"], "research_orbit_audit");
    assert_eq!(r["violated"], true, "{r}");
    assert_eq!(r["bound"], "T1");
    assert_eq!(r["n"], 4);
    assert_eq!(r["orbits"], 3);
    let w = &r["witnesses"][0];
    assert_eq!(w["kind"], "collision");
    assert_eq!(w["answer"], "x");
    assert_eq!(entities(w), ["a", "c"]);
    assert_eq!(check(&r, "T1")["floor"], 1.0);
    assert_eq!(check(&r, "T1")["claimed"], 0.0);
    assert_eq!(r["claims"]["errors"], 0);
    let note = r["note"].as_str().expect("note");
    assert!(note.starts_with("violation found"), "{note}");
    assert!(note.contains("evidence"), "{note}");
    assert!(!note.contains("not confirmation"), "{note}");
}

#[test]
fn a_clean_relation_says_it_is_not_confirmation() {
    let (_p, mut s) = house();
    let r = audit(
        &mut s,
        json!({"injective": true, "relation": rel(&[("a", "x"), ("b", "y"), ("c", "z")])}),
    );
    assert_eq!(r["violated"], false, "{r}");
    assert_eq!(r["note"], CLEAN);
    assert_eq!(r["witnesses"], json!([]));
    for forbidden in ["verdict", "confirmed", "valid", "correct", "proved"] {
        assert!(r.get(forbidden).is_none(), "{forbidden} in {r}");
    }
    assert!(!r.to_string().contains("\"confirmed\""), "{r}");
}

#[test]
fn the_admissible_set_proves_what_collisions_alone_cannot() {
    let (_p, mut s) = house();
    let r = audit(
        &mut s,
        json!({
            "injective": true,
            "relation": rel(&[("a", "p"), ("b", "q"), ("c", "r"), ("d", "nobody")]),
            "admissible": ["p", "q", "r", "s"]
        }),
    );
    assert_eq!(r["violated"], true, "{r}");
    assert_eq!(r["bound"], "T1*");
    assert_eq!(check(&r, "T1")["violated"], false);
    assert_eq!(check(&r, "T1*")["floor"], 1.0);
    assert_eq!(r["admissible_orbits"], 3);
    let w = r["witnesses"]
        .as_array()
        .expect("witnesses")
        .iter()
        .find(|w| w["kind"] == "inadmissible")
        .expect("inadmissible witness");
    assert_eq!(w["answer"], "nobody");
    assert_eq!(entities(w), ["d"]);
}

#[test]
fn an_admissible_set_smaller_than_the_relation_voids_the_star_bounds() {
    let (_p, mut s) = house();
    let r = audit(
        &mut s,
        json!({
            "injective": true,
            "relation": rel(&[("a", "p"), ("b", "q"), ("c", "r")]),
            "admissible": ["p"]
        }),
    );
    assert_eq!(r["violated"], false, "{r}");
    assert_eq!(r["note"], CLEAN);
    let voided: Vec<&str> = r["inapplicable"]
        .as_array()
        .expect("inapplicable")
        .iter()
        .map(|v| v["bound"].as_str().expect("bound"))
        .collect();
    for b in ["T1*", "T6*", "T8"] {
        assert!(voided.contains(&b), "{b} in {voided:?}");
    }
    assert_eq!(r["admissible_orbits"], Value::Null);
}

#[test]
fn without_the_injectivity_premise_nothing_is_claimed() {
    let (_p, mut s) = house();
    let r = audit(
        &mut s,
        json!({"injective": false, "relation": rel(&[("a", "x"), ("b", "x")])}),
    );
    assert_eq!(r["violated"], false, "{r}");
    assert_eq!(r["note"], CLEAN);
    assert!(r["inapplicable"].to_string().contains("injective"), "{r}");
    assert_eq!(r["witnesses"], json!([]));
}

#[test]
fn precision_and_recall_claims_are_judged_against_their_floors() {
    let (_p, mut s) = house();
    let relation = rel(&[("a", "t"), ("b", "t"), ("c", "t"), ("d", "t")]);
    let low = audit(
        &mut s,
        json!({"injective": true, "relation": relation, "claimed_precision": 0.5}),
    );
    assert_eq!(low["violated"], true, "{low}");
    assert_eq!(low["bound"], "T6");
    assert_eq!(check(&low, "T6")["floor"], 0.75);
    let at = audit(
        &mut s,
        json!({"injective": true, "relation": relation, "claimed_precision": 0.75}),
    );
    assert_eq!(at["violated"], false, "{at}");
    let recall = audit(
        &mut s,
        json!({
            "injective": true,
            "relation": relation,
            "admissible": ["a", "b", "c", "d"],
            "claimed_recall": 0.5
        }),
    );
    assert_eq!(recall["violated"], true, "{recall}");
    assert_eq!(recall["bound"], "T8");
    assert_eq!(check(&recall, "T8")["floor"], 1.0);
}

#[test]
fn witnesses_are_capped_and_say_so() {
    let (_p, mut s) = house();
    let mut pairs = Vec::new();
    for i in 0..100 {
        pairs.push((format!("e{i}a"), format!("v{i}")));
        pairs.push((format!("e{i}b"), format!("v{i}")));
    }
    let relation: Vec<Value> = pairs
        .iter()
        .map(|(e, a)| json!({"entity": e, "answer": a}))
        .collect();
    let r = audit(&mut s, json!({"injective": true, "relation": relation}));
    assert_eq!(r["violated"], true);
    assert!(r["witnesses"].as_array().expect("w").len() <= 20, "{r}");
    assert_eq!(r["witnesses_truncated"], true);
    assert_eq!(check(&r, "T1")["floor"], 100.0);
}

#[test]
fn identical_calls_return_identical_bytes_across_runs_and_sessions() {
    let args = json!({
        "injective": true,
        "relation": rel(&[("a", "x"), ("b", "y"), ("c", "x"), ("d", "y"), ("e", "z")]),
        "admissible": ["x", "y", "z", "w", "v"],
        "claimed_errors": 1,
        "claimed_precision": 0.4,
        "claimed_recall": 0.1
    });
    let (_p, mut s) = house();
    let first = s.text("research_orbit_audit", args.clone());
    for _ in 0..10 {
        assert_eq!(s.text("research_orbit_audit", args.clone()), first);
    }
    let (_q, mut t) = house();
    assert_eq!(t.text("research_orbit_audit", args), first);
}

#[test]
fn adversarial_inputs_are_typed_errors_and_never_a_crash() {
    let (_p, mut s) = house();
    let ok = rel(&[("a", "x")]);
    let bad: Vec<(&str, Value)> = vec![
        ("no source", json!({"injective": true})),
        ("both sources", json!({"injective": true, "relation": ok, "edge_type": "R"})),
        ("injective missing", json!({"relation": ok})),
        ("injective not bool", json!({"injective": "yes", "relation": ok})),
        ("relation not array", json!({"injective": true, "relation": "x"})),
        ("empty relation", json!({"injective": true, "relation": []})),
        ("entry not object", json!({"injective": true, "relation": ["a"]})),
        ("entity not string", json!({"injective": true, "relation": [{"entity": 1, "answer": "x"}]})),
        ("answer missing", json!({"injective": true, "relation": [{"entity": "a"}]})),
        ("empty entity", json!({"injective": true, "relation": [{"entity": "", "answer": "x"}]})),
        ("long entity", json!({"injective": true, "relation": [{"entity": "e".repeat(513), "answer": "x"}]})),
        ("long answer", json!({"injective": true, "relation": [{"entity": "a", "answer": "x".repeat(513)}]})),
        ("nan precision", json!({"injective": true, "relation": ok, "claimed_precision": "nan"})),
        ("precision above one", json!({"injective": true, "relation": ok, "claimed_precision": 1.5})),
        ("negative recall", json!({"injective": true, "relation": ok, "claimed_recall": -0.1})),
        ("negative errors", json!({"injective": true, "relation": ok, "claimed_errors": -1})),
        ("fractional errors", json!({"injective": true, "relation": ok, "claimed_errors": 0.5})),
        ("admissible not strings", json!({"injective": true, "relation": ok, "admissible": [1]})),
        ("unknown property", json!({"injective": true, "relation": ok, "verdict": "ok"})),
        ("edge type with backtick", json!({"injective": true, "edge_type": "A`B"})),
        ("edge type empty", json!({"injective": true, "edge_type": ""})),
        ("edge type missing in graph", json!({"injective": true, "edge_type": "NOPE"})),
    ];
    for (label, args) in bad {
        let msg = s.err("research_orbit_audit", args);
        assert!(!msg.is_empty(), "{label}");
    }
    let many: Vec<Value> = (0..10_001)
        .map(|i| json!({"entity": format!("e{i}"), "answer": format!("a{i}")}))
        .collect();
    let msg = s.err("research_orbit_audit", json!({"injective": true, "relation": many}));
    assert!(msg.contains("10000"), "{msg}");
    let still = audit(&mut s, json!({"injective": true, "relation": ok}));
    assert_eq!(still["violated"], false);
}

#[test]
fn the_maximum_relation_size_is_audited() {
    let (_p, mut s) = house();
    let many: Vec<Value> = (0..10_000)
        .map(|i| json!({"entity": format!("e{i}"), "answer": "same"}))
        .collect();
    let r = audit(
        &mut s,
        json!({"injective": true, "relation": many, "claimed_errors": 9_998}),
    );
    assert_eq!(r["violated"], true);
    assert_eq!(check(&r, "T1")["floor"], 9_999.0);
}

#[test]
fn conflicting_duplicates_are_skipped_and_reported() {
    let (_p, mut s) = house();
    let r = audit(
        &mut s,
        json!({
            "injective": true,
            "relation": rel(&[("a", "x"), ("a", "y"), ("b", "z"), ("b", "z"), ("c", "z")])
        }),
    );
    assert_eq!(r["skipped"]["conflicting"], 1, "{r}");
    assert_eq!(r["n"], 2);
    assert_eq!(r["violated"], true);
    assert_eq!(entities(&r["witnesses"][0]), ["b", "c"]);
}

#[test]
fn a_relation_can_be_read_from_graph_edges() {
    let (_p, mut s) = house();
    s.ok(
        "write_neo4j_cypher",
        json!({"query": "CREATE (c1:Chunk {name: 'c1'})-[:GIST]->(g:Gist {name: 'g'}), (c2:Chunk {name: 'c2'})-[:GIST]->(g), (c3:Chunk {name: 'c3'})-[:GIST]->(:Gist {name: 'h'}), (c4:Chunk {name: 'c4'})-[:GIST]->(:Gist {name: 'i'}), (c4)-[:GIST]->(:Gist {name: 'j'})"}),
    );
    let r = audit(&mut s, json!({"injective": true, "edge_type": "GIST"}));
    assert_eq!(r["violated"], true, "{r}");
    assert_eq!(r["n"], 3);
    assert_eq!(r["skipped"]["multi_valued"], 1);
    let w = &r["witnesses"][0];
    let ents = entities(w).join(" ");
    assert!(ents.contains("c1") && ents.contains("c2") && !ents.contains("c3"), "{w}");
    assert!(w["answer"].as_str().expect("answer").contains(" g"), "{w}");
    let again = audit(&mut s, json!({"injective": true, "edge_type": "GIST"}));
    assert_eq!(again, r);
}

#[test]
fn an_edge_relation_over_the_cap_is_refused() {
    let (_p, mut s) = house();
    s.ok(
        "write_neo4j_cypher",
        json!({"query": "UNWIND range(1, 10001) AS i CREATE (:A {name: toString(i)})-[:R]->(:B)"}),
    );
    let msg = s.err("research_orbit_audit", json!({"injective": true, "edge_type": "R"}));
    assert!(msg.contains("10000"), "{msg}");
}

#[test]
fn pooling_flags_a_recovery_claim_above_one_over_the_block_size() {
    let (_p, mut s) = house();
    let r = s.ok(
        "research_pooling_audit",
        json!({
            "relation": rel(&[("a", "g"), ("b", "g"), ("c", "g"), ("d", "h")]),
            "recoveries": [
                {"entity": "a", "recovery": 0.5},
                {"entity": "d", "recovery": 1.0},
                {"entity": "b", "recovery": 0.3}
            ]
        }),
    );
    assert_eq!(r["tool"], "research_pooling_audit");
    assert_eq!(r["violated"], true, "{r}");
    assert_eq!(r["bound"], "T2");
    let w = r["witnesses"].as_array().expect("w");
    assert_eq!(w.len(), 1, "{r}");
    assert_eq!(w[0]["entity"], "a");
    assert_eq!(w[0]["block_size"], 3);
    assert_eq!(entities(&w[0]), ["a", "b", "c"]);
    let clean = s.ok(
        "research_pooling_audit",
        json!({
            "relation": rel(&[("a", "g"), ("b", "g"), ("c", "g"), ("d", "h")]),
            "recoveries": [{"entity": "d", "recovery": 1.0}, {"entity": "b", "recovery": 0.3}]
        }),
    );
    assert_eq!(clean["violated"], false);
    assert_eq!(clean["note"], CLEAN);
}

#[test]
fn pooling_rejects_unknown_entities_and_bad_numbers() {
    let (_p, mut s) = house();
    let relation = rel(&[("a", "g"), ("b", "h")]);
    for recoveries in [
        json!([{"entity": "zzz", "recovery": 0.1}]),
        json!([{"entity": "a", "recovery": 2}]),
        json!([{"entity": "a", "recovery": -1}]),
        json!([{"entity": "a"}]),
        json!([]),
        json!("a"),
    ] {
        let msg = s.err(
            "research_pooling_audit",
            json!({"relation": relation, "recoveries": recoveries}),
        );
        assert!(!msg.is_empty());
    }
}

fn audited(s: &mut Session) -> Value {
    s.ok(
        "read_neo4j_cypher",
        json!({"query": "MATCH (g:Goal {gid: 'G0'})-[r:AUDITED]->(a:Audit) RETURN r.verdict AS verdict, r.why AS why, a.key AS audit ORDER BY audit"}),
    )
}

fn open_need(s: &mut Session) {
    s.ok(
        "house_need",
        json!({"project": "p", "goal": "g", "acceptance": ["a"]}),
    );
}

#[test]
fn record_writes_an_audited_edge_with_verdict_and_why() {
    let (_p, mut s) = house();
    open_need(&mut s);
    let inner = json!({"injective": true, "relation": rel(&[("a", "x"), ("b", "x")])});
    let r = s.ok(
        "research_record",
        json!({"tool": "research_orbit_audit", "arguments": inner, "goal": "G0"}),
    );
    assert_eq!(r["violated"], true, "{r}");
    assert_eq!(r["recorded"]["goal"], "G0");
    assert_eq!(r["recorded"]["verdict"], "violation");
    let rows = audited(&mut s);
    assert_eq!(rows.as_array().expect("rows").len(), 1, "{rows}");
    assert_eq!(rows[0]["verdict"], "violation");
    let why = rows[0]["why"].as_str().expect("why");
    assert!(why.contains("research_orbit_audit"), "{why}");
    assert!(why.contains("\"entity\":\"a\""), "{why}");
    let direct = s.ok("research_orbit_audit", inner);
    let mut stripped = r.clone();
    stripped.as_object_mut().expect("object").remove("recorded");
    assert_eq!(stripped, direct);
}

#[test]
fn record_never_writes_a_confirmation_verdict_and_is_idempotent() {
    let (_p, mut s) = house();
    open_need(&mut s);
    let clean = json!({"injective": true, "relation": rel(&[("a", "x"), ("b", "y")])});
    for _ in 0..3 {
        let r = s.ok(
            "research_record",
            json!({"tool": "research_orbit_audit", "arguments": clean, "goal": "G0"}),
        );
        assert_eq!(r["recorded"]["verdict"], "no_violation_found", "{r}");
    }
    let rows = audited(&mut s);
    assert_eq!(rows.as_array().expect("rows").len(), 1, "{rows}");
    assert_eq!(rows[0]["verdict"], "no_violation_found");
    let pooled = json!({"relation": rel(&[("a", "x"), ("b", "x")]), "recoveries": [{"entity": "a", "recovery": 0.9}]});
    s.ok(
        "research_record",
        json!({"tool": "research_pooling_audit", "arguments": pooled, "goal": "G0"}),
    );
    assert_eq!(audited(&mut s).as_array().expect("rows").len(), 2);
}

#[test]
fn record_on_an_unknown_goal_or_tool_writes_nothing() {
    let (_p, mut s) = house();
    open_need(&mut s);
    let before = s.ok("read_neo4j_cypher", json!({"query": "MATCH (n) RETURN count(n) AS c"}));
    let inner = json!({"injective": true, "relation": rel(&[("a", "x")])});
    s.err(
        "research_record",
        json!({"tool": "research_orbit_audit", "arguments": inner, "goal": "G404"}),
    );
    s.err(
        "research_record",
        json!({"tool": "house_status", "arguments": {}, "goal": "G0"}),
    );
    s.err(
        "research_record",
        json!({"tool": "research_record", "arguments": {}, "goal": "G0"}),
    );
    s.err(
        "research_record",
        json!({"tool": "research_orbit_audit", "arguments": {"injective": true}, "goal": "G0"}),
    );
    let after = s.ok("read_neo4j_cypher", json!({"query": "MATCH (n) RETURN count(n) AS c"}));
    assert_eq!(before, after);
}

#[test]
fn the_audits_themselves_never_write() {
    let (_p, mut s) = house();
    open_need(&mut s);
    let count = |s: &mut Session| {
        s.ok(
            "read_neo4j_cypher",
            json!({"query": "MATCH (n) RETURN count(n) AS n"}),
        )
    };
    let rels = |s: &mut Session| {
        s.ok(
            "read_neo4j_cypher",
            json!({"query": "MATCH ()-[r]->() RETURN count(r) AS r"}),
        )
    };
    let (n0, r0) = (count(&mut s), rels(&mut s));
    audit(&mut s, json!({"injective": true, "relation": rel(&[("a", "x"), ("b", "x")])}));
    s.ok(
        "research_pooling_audit",
        json!({"relation": rel(&[("a", "x")]), "recoveries": [{"entity": "a", "recovery": 1}]}),
    );
    assert_eq!((count(&mut s), rels(&mut s)), (n0, r0));
}

#[test]
fn read_only_servers_can_audit_but_the_audit_is_the_whole_of_it() {
    let p = Proj::new();
    let mut s = Session::start(
        &[
            "--root".to_string(),
            p.root(),
            "--in-memory".to_string(),
            "--mode".to_string(),
            "house".to_string(),
            "--read-only".to_string(),
        ],
        &[],
    );
    let r = audit(&mut s, json!({"injective": true, "relation": rel(&[("a", "x"), ("b", "x")])}));
    assert_eq!(r["violated"], true);
}
