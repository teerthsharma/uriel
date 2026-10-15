use std::path::Path;

fn main() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let corpus = tests.join("frozen_corpus");
    let queries = tests.join("eval_queries.json");
    let generate = std::env::args().nth(1).is_some_and(|a| a == "gen");
    if generate {
        if queries.exists() {
            eprintln!("{} exists; the query file is generated once", queries.display());
            std::process::exit(1);
        }
        let set = match uriel_rag::eval::generate_queries(&corpus, 100) {
            Ok(set) => set,
            Err(e) => {
                eprintln!("generation failed: {e}");
                std::process::exit(1);
            }
        };
        if let Err(e) = std::fs::write(&queries, uriel_rag::eval::queries_json(&set)) {
            eprintln!("write failed: {e}");
            std::process::exit(1);
        }
        println!("{} queries over {} files", set.queries.len(), set.files);
        return;
    }
    match uriel_rag::eval::run_frozen(&corpus, &queries) {
        Ok(report) => print!("{report}"),
        Err(e) => {
            eprintln!("eval failed: {e}");
            std::process::exit(1);
        }
    }
}
