use std::collections::BTreeMap;

use uriel_mcp::reduce;
use uriel_terse::count_tokens;

#[test]
#[ignore]
fn measure_the_reducer_over_the_recorded_frozen_outputs() {
    let dir = std::env::var("REDUCE_MEASURE_DIR").expect("REDUCE_MEASURE_DIR");
    let mut rows: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).expect("dir") {
        let path = entry.expect("entry").path();
        let name = path.file_name().expect("name").to_string_lossy().to_string();
        let ext = name.rsplit('.').next().unwrap_or("").to_string();
        let tool = name.rsplit_once('-').map_or(name.clone(), |(a, _)| a.to_string());
        let key = format!("{} .{ext}", tool.rsplit_once('-').map_or(tool.clone(), |(a, _)| a.to_string()));
        let text = std::fs::read_to_string(&path).expect("read");
        let before = count_tokens(&text);
        let (out, _) = reduce::apply("rag_search", &serde_json::json!({}), text, true);
        let after = count_tokens(&out);
        let e = rows.entry(key).or_default();
        e.0 += 1;
        e.1 += before;
        e.2 += after;
    }
    let mut total = (0usize, 0usize);
    for (k, (n, b, a)) in &rows {
        println!("{k:<18} files {n:>4}  before {b:>8}  after {a:>8}  saved {:>5.1}%", 100.0 * (*b as f64 - *a as f64) / *b as f64);
        total.0 += b;
        total.1 += a;
    }
    println!("{:<18} before {:>8}  after {:>8}  saved {:>5.1}%", "ALL", total.0, total.1, 100.0 * (total.0 as f64 - total.1 as f64) / total.0 as f64);
}
