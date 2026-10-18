mod common;

use std::fs;
use std::path::Path;

use common::Session;
use serde_json::{json, Value};
use uriel_mcp::compact::proxy_tokens;
use uriel_mcp::prefix_tree::PrefixLearner;
use uriel_rag::eval::{materialize, parse_queries};

const BUDGETS: [usize; 3] = [300, 800, 4000];

fn stable_bytes(text: &str) -> usize {
    let v: Value = serde_json::from_str(text).expect("json");
    usize::try_from(v["stable_prefix_bytes"].as_u64().expect("stable_prefix_bytes")).expect("fits")
}

fn view(s: &mut Session, q: &str, budget: usize) -> String {
    s.text(
        "view",
        json!({"question": q, "budget_tokens": budget, "k": 12, "format": "json"}),
    )
}

#[test]
fn the_frozen_queries_report_the_prefix_stable_share_of_the_json_view() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../uriel-rag/tests");
    let corpus = materialize(&tests.join("frozen_corpus")).expect("materialize");
    let set = parse_queries(&fs::read_to_string(tests.join("eval_queries.json")).expect("queries"))
        .expect("parse");
    assert_eq!(set.queries.len(), 100);
    let start = |flag: &str| {
        let args: Vec<String> = ["--in-memory"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut s = Session::start(&args, &[("URIEL_STABLE_PREFIX", flag)]);
        s.ok("rag_index", json!({"path": corpus.root().to_string_lossy()}));
        s
    };
    let (mut on, mut off) = (start("on"), start("off"));
    for budget in BUDGETS {
        let (mut stable_b, mut total_b, mut stable_t, mut total_t) = (0usize, 0usize, 0usize, 0usize);
        let mut on_learner = PrefixLearner::new(0.95);
        let mut off_learner = PrefixLearner::new(0.95);
        let mut min_share = 1.0f64;
        let (mut head, mut last_n, mut off_b) = (None, 0usize, 0usize);
        for q in &set.queries {
            let a = view(&mut on, &q.query, budget);
            let b = view(&mut off, &q.query, budget);
            let n = stable_bytes(&a);
            last_n = n;
            off_b += b.len();
            let first = head.get_or_insert_with(|| a.as_bytes()[..n].to_vec());
            assert_eq!(&a.as_bytes()[..n], first.as_slice());
            stable_b += n;
            total_b += a.len();
            stable_t += proxy_tokens(&a[..n]);
            total_t += proxy_tokens(&a);
            min_share = min_share.min(n as f64 / a.len() as f64);
            on_learner.observe("on", a.as_bytes());
            off_learner.observe("off", b.as_bytes());
        }
        println!(
            "PREFIXSHARE budget {budget}: stable bytes {stable_b}/{total_b} = {:.4}, stable proxy tokens {stable_t}/{total_t} = {:.4}, min per-response byte share {min_share:.4}, response bytes without the prefix {off_b}, learned prefix on {} bytes, off {} bytes, guaranteed stable bytes before: 0",
            stable_b as f64 / total_b as f64,
            stable_t as f64 / total_t as f64,
            on_learner.stable_len(),
            off_learner.stable_len(),
        );
        assert!(stable_b > 0);
        assert!(on_learner.stable_len() >= last_n);
        assert_eq!(total_b, off_b + 100 * (last_n - 1));
    }
}
