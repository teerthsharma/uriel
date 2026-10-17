use std::fs;
use std::path::Path;

use uriel_core::Graph;
use uriel_rag::{IndexOptions, Rag, RagConfig, SearchOptions};
use tempfile::TempDir;

pub const QUERY: &str = "running checksum of the input buffer for stream";

pub struct Corpus {
    pub rag: Rag,
    _dir: TempDir,
}

fn write_corpus(dir: &Path, files: usize) {
    for i in 0..files {
        let mut body = vec![
            format!(
                "{} Compute the running checksum of the input buffer for stream {i}. It resets per block.",
                "/".repeat(3)
            ),
            format!("pub fn checksum_stream_{i}(buffer: &[u8], block: usize) -> u32 {{"),
            "    let mut acc: u32 = 1;".to_string(),
        ];
        for j in 0..24 {
            body.push(format!(
                "    acc = acc.wrapping_mul(31).wrapping_add(buffer[{j} % buffer.len()] as u32 + block as u32 + {i});"
            ));
        }
        body.push("    acc".to_string());
        body.push("}".to_string());
        fs::write(dir.join(format!("s{i}.rs")), body.join("\n")).expect("write corpus file");
    }
}

pub fn corpus(files: usize) -> Corpus {
    let dir = tempfile::tempdir().expect("tempdir");
    write_corpus(dir.path(), files);
    let graph = Graph::open_in_memory(uriel_core::MemoryBudget::mib(80)).expect("graph");
    let rag = Rag::open(graph, RagConfig::default()).expect("rag");
    rag.index_dir(dir.path(), &IndexOptions::default())
        .expect("index");
    Corpus { rag, _dir: dir }
}

pub fn search_options(k: usize, budget_tokens: usize) -> SearchOptions {
    SearchOptions {
        k,
        budget_tokens,
        ..SearchOptions::default()
    }
}
