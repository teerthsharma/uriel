use peak_alloc::PeakAlloc;
use uriel_algo::{
    articulation_points, betweenness_centrality, bridges, condensation_layers, feedback_arc_set,
    louvain, pagerank, BetweennessConfig, LouvainConfig, Orientation, PageRankConfig,
    ProjectionBuilder, XorShift64,
};

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

const NODES: usize = 50_000;
const EDGES: usize = 200_000;
const LIMIT_BYTES: usize = 40 << 20;

#[test]
fn the_default_node_cap_pipeline_peaks_under_forty_mib_above_its_input() {
    let mut rng = XorShift64::new(5);
    let edges: Vec<(u32, u32, f64)> = (0..EDGES)
        .map(|_| (rng.gen_range(NODES as u64) as u32, rng.gen_range(NODES as u64) as u32, 1.0))
        .collect();
    let base = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let build = |o: Orientation| {
        let mut b = ProjectionBuilder::new(NODES).orientation(o);
        for &(u, v, _) in &edges {
            b = b.add_edge(u, v);
        }
        b.build().unwrap()
    };
    let directed = build(Orientation::Natural);
    assert!(pagerank(&directed, &PageRankConfig::default()).unwrap().converged);
    let cfg = BetweennessConfig {
        normalize: false,
        sample_size: Some(1024),
        seed: 0,
    };
    assert_eq!(betweenness_centrality(&directed, &cfg).unwrap().len(), NODES);
    assert!(!feedback_arc_set(&directed).unwrap().is_empty());
    assert_eq!(condensation_layers(&directed).unwrap().len(), NODES);
    drop(directed);
    let undirected = build(Orientation::Undirected);
    assert!(louvain(&undirected, &LouvainConfig::default()).unwrap().modularity > 0.0);
    assert!(!articulation_points(&undirected).is_empty() || !bridges(&undirected).is_empty());
    let peak = ALLOC.peak_usage();
    let used = peak.saturating_sub(base);
    assert!(used < LIMIT_BYTES, "peak {:.1} MiB above the input", used as f64 / (1 << 20) as f64);
}
