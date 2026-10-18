mod common;

use std::fs;
use std::path::Path;

use common::Session;
use serde_json::{json, Value};
use uriel_mcp::compact::proxy_tokens;
use uriel_rag::eval::{materialize, parse_queries, EvalQuery};

const BUDGETS: [usize; 3] = [300, 800, 4000];

fn args(question: &str, budget: usize, islands: Option<bool>) -> Value {
    let mut a = json!({"question": question, "budget_tokens": budget, "k": 12});
    if let Some(i) = islands {
        a["islands"] = json!(i);
    }
    a
}

struct Measured {
    line: String,
    outputs: Vec<(String, String)>,
}

fn measure(s: &mut Session, queries: &[EvalQuery], budget: usize, islands: Option<bool>) -> Measured {
    let (mut r5, mut r10, mut tokens, mut multi) = (0usize, 0usize, 0usize, 0usize);
    let mut outputs = Vec::new();
    for q in queries {
        let mut compact_args = args(&q.query, budget, islands);
        compact_args["format"] = json!("compact");
        let compact = s.text("view", compact_args);
        let mut json_args = args(&q.query, budget, islands);
        json_args["format"] = json!("json");
        let json_text = s.text("view", json_args);
        tokens += proxy_tokens(&compact);
        let v: Value = serde_json::from_str(&json_text).expect("json");
        multi += usize::from(v.get("islands").is_some());
        let pos = v["hits"].as_array().expect("hits").iter().position(|h| {
            h["file"] == q.file
                && h["start_line"].as_u64() <= Some(u64::from(q.signature_line))
                && Some(u64::from(q.signature_line)) <= h["end_line"].as_u64()
        });
        r5 += usize::from(pos.is_some_and(|p| p < 5));
        r10 += usize::from(pos.is_some_and(|p| p < 10));
        outputs.push((compact, json_text));
    }
    Measured {
        line: format!(
            "TOOLVIEW islands={islands:?} budget {budget}: recall@5 {r5}/100, recall@10 {r10}/100, multi-island queries {multi}, mean proxy tokens {:.1}",
            tokens as f64 / 100.0
        ),
        outputs,
    }
}

#[test]
fn the_view_tool_on_the_frozen_queries_reports_recall_and_proxy_tokens() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../uriel-rag/tests");
    let corpus = materialize(&tests.join("frozen_corpus")).expect("materialize");
    let set = parse_queries(&fs::read_to_string(tests.join("eval_queries.json")).expect("queries"))
        .expect("parse");
    assert_eq!(set.queries.len(), 100);
    let mut s = Session::start(&["--in-memory".to_string()], &[]);
    s.ok("rag_index", json!({"path": corpus.root().to_string_lossy()}));
    for budget in BUDGETS {
        let default = measure(&mut s, &set.queries, budget, None);
        let off = measure(&mut s, &set.queries, budget, Some(false));
        let on = measure(&mut s, &set.queries, budget, Some(true));
        println!("{}
{}
{}", default.line, off.line, on.line);
        assert_eq!(default.outputs, off.outputs, "the default differs from islands=false at budget {budget}");
        for (i, (want, got)) in off.outputs.iter().zip(&on.outputs).enumerate() {
            let single = !got.1.contains("\"islands\":");
            assert!(!single || want == got, "query {i} has one island but differs at budget {budget}");
        }
    }
}
