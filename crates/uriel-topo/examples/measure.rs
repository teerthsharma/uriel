use std::path::Path;
use std::time::Instant;

use uriel_topo::layout::topology_order;
use uriel_topo::measure::{
    community_graph_mixing, random_permutation, source_call_graph, two_hop_traffic, Csr, Traffic,
};
use uriel_topo::rng::SplitMix64;

const SOURCES: usize = 1000;

fn run(name: &str, n: usize, edges: &[(u32, u32)]) {
    let g = Csr::from_edges(n, edges).unwrap();
    let mut rng = SplitMix64::new(77);
    let sources: Vec<u32> = (0..SOURCES.min(n.max(1)))
        .map(|_| rng.below(n) as u32)
        .collect();
    let started = Instant::now();
    let layout = topology_order(n, edges, None).unwrap();
    let secs = started.elapsed().as_secs_f64();
    let ident: Vec<u32> = (0..n as u32).collect();
    let rows: [(&str, Traffic); 3] = [
        ("insertion", two_hop_traffic(&g, &ident, &sources).unwrap()),
        (
            "random",
            two_hop_traffic(&g, &random_permutation(n, 5), &sources).unwrap(),
        ),
        (
            "sorted",
            two_hop_traffic(&g, &layout.position, &sources).unwrap(),
        ),
    ];
    let communities = layout
        .communities
        .iter()
        .copied()
        .max()
        .map_or(0, |m| m + 1);
    println!(
        "\n{name}: {n} nodes, {} simple edges, {} louvain communities, layout {secs:.2}s, {} sources",
        g.targets.len() / 2,
        communities,
        sources.len()
    );
    println!(
        "{:<10} {:>10} {:>12} {:>12} {:>12}",
        "layout", "nodes", "node_pages", "adj_lines", "adj_pages"
    );
    for (label, t) in &rows {
        println!(
            "{label:<10} {:>10.1} {:>12.2} {:>12.2} {:>12.2}",
            t.nodes, t.node_pages, t.adj_lines, t.adj_pages
        );
    }
    let (ins, rnd, srt) = (&rows[0].1, &rows[1].1, &rows[2].1);
    println!(
        "ratio sorted/insertion: node_pages {:.3} adj_lines {:.3} adj_pages {:.3}",
        srt.node_pages / ins.node_pages,
        srt.adj_lines / ins.adj_lines,
        srt.adj_pages / ins.adj_pages
    );
    println!(
        "ratio sorted/random:    node_pages {:.3} adj_lines {:.3} adj_pages {:.3}",
        srt.node_pages / rnd.node_pages,
        srt.adj_lines / rnd.adj_lines,
        srt.adj_pages / rnd.adj_pages
    );
}

fn main() {
    println!("model: 64 node records per 4 KiB page (64 B each), adjacency 4 B entries (16 per line, 1024 per page); 2-hop BFS, distinct pages/lines per traversal, averaged over sources");
    for (label, interleaved, p_inside) in [
        (
            "community graph, interleaved ids, 92% intra edges",
            true,
            0.92,
        ),
        (
            "community graph, contiguous ids (insertion already clustered), 92% intra edges",
            false,
            0.92,
        ),
        (
            "community graph, interleaved ids, 60% intra edges",
            true,
            0.60,
        ),
        (
            "community graph, interleaved ids, 30% intra edges",
            true,
            0.30,
        ),
    ] {
        let (n, e, _) = community_graph_mixing(50_000, 8, 1, interleaved, p_inside);
        run(label, n, &e);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (names, edges) = source_call_graph(&root).unwrap();
    run(
        "rust call graph of this repo (names, insertion = first definition)",
        names.len(),
        &edges,
    );
}
