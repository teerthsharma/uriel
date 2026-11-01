use std::fs;
use std::path::PathBuf;
use uriel_page::{demo, render};

fn committed(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/pages")
        .join(name);
    fs::read_to_string(&p).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; run `cargo run -p uriel-page --example demo`",
            p.display()
        )
    })
}

#[test]
fn committed_index_page_equals_a_fresh_render() {
    assert_eq!(committed("index.html"), render(&demo::index()));
}

#[test]
fn committed_research_page_equals_a_fresh_render() {
    assert_eq!(
        committed("department-research.html"),
        render(&demo::department_research())
    );
}
