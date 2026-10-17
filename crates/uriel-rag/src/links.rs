use std::collections::{BTreeSet, HashSet};

use uriel_core::NodeId;

use crate::chunk::Lang;
use crate::index::{rust_defs, rust_mentions};
use crate::{prop_str, Rag, RagError};

fn ordered(a: u64, b: u64) -> (u64, u64) {
    (a.min(b), a.max(b))
}

impl Rag {
    pub fn edges_among(&self, ids: &[u64]) -> Result<Vec<(u64, u64)>, RagError> {
        let set: BTreeSet<u64> = ids.iter().copied().collect();
        let read = self.graph.begin_read()?;
        let mut edges: BTreeSet<(u64, u64)> = BTreeSet::new();
        let mut rust: Vec<(u64, Vec<String>, HashSet<String>)> = Vec::new();
        for &id in &set {
            let Some(node) = read
                .get_node(NodeId(id))?
                .filter(|n| n.labels.contains("Chunk"))
            else {
                continue;
            };
            for edge in read.out_edges(NodeId(id), Some("NEXT"))? {
                let other = edge?.other.0;
                if other != id && set.contains(&other) {
                    edges.insert(ordered(id, other));
                }
            }
            if Lang::from_path(prop_str(&node, "file")?) == Lang::Rust {
                let text = prop_str(&node, "text")?;
                rust.push((id, rust_defs(text), rust_mentions(text).into_iter().collect()));
            }
        }
        for (definer, defs, _) in &rust {
            for (mentioner, _, mentions) in &rust {
                if definer != mentioner && defs.iter().any(|d| mentions.contains(d)) {
                    edges.insert(ordered(*definer, *mentioner));
                }
            }
        }
        Ok(edges.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use uriel_core::{Graph, MemoryBudget};

    use crate::chunk::ChunkOptions;
    use crate::{Hit, Rag, RagConfig, SearchOptions};

    const DEFINER: &str = "pub fn compute_checksum(data: &[u8]) -> u32 {\n    let mut a: u32 = 1;\n    for byte in data {\n        a = a.wrapping_add(u32::from(*byte));\n    }\n    a\n}\n";
    const CALLER: &str = "pub fn verify_payload(data: &[u8]) -> bool {\n    let sum = compute_checksum(data);\n    sum != 0\n}\n";
    const UNRELATED: &str = "pub fn render_table(rows: &[String]) -> String {\n    rows.join(\"\n\")\n}\n";
    const PROSE: &str = "alpha one\nalpha two\nalpha three\nalpha four\nbeta one\nbeta two\nbeta three\nbeta four\ngamma one\ngamma two\ngamma three\ngamma four\n";

    fn config() -> RagConfig {
        RagConfig {
            chunking: ChunkOptions {
                window_lines: 4,
                overlap_lines: 0,
                min_lines: 1,
                max_lines: 4,
            },
            ..RagConfig::default()
        }
    }

    fn corpus() -> (Graph, Vec<Hit>) {
        let graph = Graph::open_in_memory(MemoryBudget::mib(64)).expect("graph");
        let rag = Rag::open(graph.clone(), config()).expect("rag");
        for (name, text) in [
            ("a.rs", DEFINER),
            ("b.rs", CALLER),
            ("c.rs", UNRELATED),
            ("notes.txt", PROSE),
        ] {
            rag.index_text(name, text).expect("index");
        }
        let mut hits = Vec::new();
        for q in ["compute_checksum", "verify_payload", "render_table", "alpha", "beta", "gamma"] {
            for h in rag
                .search(q, &SearchOptions { k: 64, ..SearchOptions::default() })
                .expect("search")
            {
                if !hits.iter().any(|x: &Hit| x.chunk_id == h.chunk_id) {
                    hits.push(h);
                }
            }
        }
        hits.sort_by_key(|h| (h.file.clone(), h.start_line));
        (graph, hits)
    }

    fn ids(hits: &[Hit], file: &str) -> Vec<u64> {
        hits.iter().filter(|h| h.file == file).map(|h| h.chunk_id).collect()
    }

    fn every(hits: &[Hit]) -> Vec<u64> {
        hits.iter().map(|h| h.chunk_id).collect()
    }

    fn spilled(graph: &Graph) -> Rag {
        let rag = Rag::open_capped(graph.clone(), config(), 1).expect("rag");
        assert!(rag.stats().expect("stats").spilled);
        rag
    }

    #[test]
    fn consecutive_chunks_of_a_file_are_linked_and_nothing_else_in_prose() {
        let (graph, hits) = corpus();
        let rag = Rag::open(graph, config()).expect("rag");
        let prose = ids(&hits, "notes.txt");
        assert_eq!(prose.len(), 3, "{prose:?}");
        let edges = rag.edges_among(&prose).expect("edges");
        assert_eq!(edges, vec![(prose[0], prose[1]), (prose[1], prose[2])]);
    }

    #[test]
    fn a_chunk_that_mentions_a_definition_is_linked_to_its_definer() {
        let (graph, hits) = corpus();
        let rag = Rag::open(graph, config()).expect("rag");
        let (definer, caller) = (ids(&hits, "a.rs")[0], ids(&hits, "b.rs")[0]);
        let unrelated = ids(&hits, "c.rs")[0];
        let edges = rag.edges_among(&every(&hits)).expect("edges");
        assert!(edges.contains(&(definer.min(caller), definer.max(caller))), "{edges:?}");
        assert!(edges.iter().all(|(a, b)| *a != unrelated && *b != unrelated), "{edges:?}");
    }

    #[test]
    fn edges_never_leave_the_requested_set() {
        let (graph, hits) = corpus();
        let rag = Rag::open(graph, config()).expect("rag");
        let prose = ids(&hits, "notes.txt");
        let edges = rag.edges_among(&prose[..2]).expect("edges");
        assert_eq!(edges, vec![(prose[0], prose[1])]);
        assert!(rag.edges_among(&[]).expect("empty").is_empty());
        assert!(rag.edges_among(&[u64::MAX]).expect("absent").is_empty());
    }

    #[test]
    fn a_spilled_index_returns_exactly_the_edges_of_a_resident_one() {
        let (graph, hits) = corpus();
        let all = every(&hits);
        let resident = Rag::open(graph.clone(), config()).expect("rag");
        assert!(!resident.stats().expect("stats").spilled);
        let want = resident.edges_among(&all).expect("resident");
        assert_eq!(want.len(), 3, "{want:?}");
        let got = spilled(&graph).edges_among(&all).expect("spilled");
        assert_eq!(got, want);
    }

    #[test]
    fn the_edge_list_is_sorted_deduplicated_and_independent_of_input_order() {
        let (graph, hits) = corpus();
        let mut all = every(&hits);
        let rag = Rag::open(graph, config()).expect("rag");
        let want = rag.edges_among(&all).expect("edges");
        let mut sorted = want.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(want, sorted);
        all.reverse();
        all.extend(all.clone());
        assert_eq!(rag.edges_among(&all).expect("edges"), want);
    }
}
