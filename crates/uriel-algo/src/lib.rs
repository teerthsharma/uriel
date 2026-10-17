//! uriel-algo: graph-data-science style algorithms on CSR projections.

mod centrality;
mod community;
mod components;
mod error;
mod flow;
mod islands;
mod paths;
mod projection;
mod rng;
mod similarity;
mod spanning;
mod traversal;
mod triangles;
mod walk;

pub use centrality::{
    betweenness_centrality, closeness_centrality, degree_centrality, pagerank,
    personalized_pagerank, BetweennessConfig, Direction, PageRankConfig, PageRankResult,
};
pub use community::{
    label_propagation, louvain, modularity, LabelPropagationConfig, LabelPropagationResult,
    LouvainConfig, LouvainResult,
};
pub use components::{
    component_count, component_sizes, strongly_connected_components, topological_sort,
    weakly_connected_components,
};
pub use error::{Error, Result};
pub use flow::{max_flow, MaxFlow};
pub use islands::{bucket_islands, island_labels};
pub use paths::{
    all_pairs_shortest_paths, astar, bellman_ford, dijkstra, dijkstra_path, yen_k_shortest_paths,
    Path, ShortestPaths, MAX_ALL_PAIRS_NODES,
};
pub use projection::{Aggregation, Orientation, Projection, ProjectionBuilder};
pub use rng::XorShift64;
pub use similarity::{
    cosine_similarity, jaccard_similarity, node_similarity, SimilarityConfig, SimilarityMetric,
    SimilarityPair,
};
pub use spanning::{kruskal_mst, prim_mst, SpanningForest};
pub use traversal::{bfs, dfs, Traversal};
pub use triangles::{
    k_core_decomposition, local_clustering_coefficient, triangle_count, Clustering, TriangleCount,
};
pub use walk::{random_walk, random_walks, RandomWalkConfig};
