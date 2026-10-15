use uriel_mcp::compact::Table;
use serde_json::{json, Value};

pub fn rows(n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            json!({
                "n": i,
                "name": format!("crates/uriel-house/src/file{i}.rs"),
                "ok": i % 2 == 0,
                "score": (i as f64) * 0.25,
                "kind": if i % 3 == 0 { "node" } else { "relationship" },
            })
        })
        .collect()
}

pub fn table(n: usize) -> Table {
    Table::from_rows(&rows(n)).expect("uniform rows")
}
