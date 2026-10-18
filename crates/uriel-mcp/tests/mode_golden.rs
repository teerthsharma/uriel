mod common;

use common::{Proj, Session};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const TOOLS_WITHOUT_HOUSE: &str = "d99ad326e8962a3b55df7ed0fdeae51b1973f42668024e0015d6e325be64aa16";
const RAG_RESULTS: &str = "39f85b0277d0161aef02703c1f4c7891611326bc5f9893c728f0fb4322694ba9";

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn default_session(p: &Proj) -> Session {
    Session::start(
        &["--root".to_string(), p.root(), "--in-memory".to_string()],
        &[],
    )
}

fn tools_without_house(s: &mut Session) -> String {
    let list = s.rpc("tools/list", json!({}));
    let kept: Vec<&Value> = list["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter(|t| !t["name"].as_str().expect("name").starts_with("house_"))
        .collect();
    serde_json::to_string(&kept).expect("json")
}

fn rag_transcript(s: &mut Session) -> String {
    let mut out = String::new();
    s.text("rag_index", json!({}));
    for q in ["backoff delay", "invoice customer billing", "scheduler priority", "parse settings cache"] {
        out.push_str(&s.text("rag_search", json!({"query": q, "format": "compact"})));
        out.push('\n');
        out.push_str(&s.text("rag_search", json!({"query": q, "format": "json"})));
        out.push('\n');
    }
    out.push_str(&s.text("seal_recall", json!({"query": "scheduler", "format": "compact"})));
    out
}

#[test]
fn the_normal_mode_tool_list_is_the_pre_modes_list_minus_house() {
    let p = Proj::new();
    let mut s = default_session(&p);
    let text = tools_without_house(&mut s);
    assert_eq!(digest(&text), TOOLS_WITHOUT_HOUSE);
}

#[test]
fn default_rag_results_are_byte_identical_to_the_pre_modes_server() {
    let first = {
        let p = Proj::new();
        let mut s = default_session(&p);
        rag_transcript(&mut s)
    };
    let second = {
        let p = Proj::new();
        let mut s = default_session(&p);
        rag_transcript(&mut s)
    };
    assert_eq!(first, second, "transcript must not depend on the temp path or the run");
    assert!(first.len() > 1500 && first.contains("src/backoff.rs") && first.contains("src/invoice.rs"), "{first}");
    assert_eq!(digest(&first), RAG_RESULTS);
}
