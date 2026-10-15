# seal: design

seal is an embedded, single-binary property-graph database in Rust. It targets the
workloads people run Neo4j for (Cypher queries, indexes, constraints, graph algorithms,
vector search, transactions, clustering) in tens of MB of RAM, with no JVM and no Docker.
It ships an MCP server that exposes the official Neo4j MCP tool names, so existing
prompts keep working.

## Crate map

| Crate | Job | Depends on |
|---|---|---|
| `uriel-core` | Value model, ids, storage on `redb`, ACID transactions, label/property indexes, uniqueness constraints, schema introspection | none |
| `uriel-cypher` | openCypher lexer, parser, AST | none |
| `uriel-cache` | Distributed cache: consistent-hash ring, gossip membership, local TinyLFU/LRU, epoch-based invalidation | none |
| `uriel-query` (wave 2) | Semantic analysis, logical plan, executor over `uriel-core`, result-cache hook into `uriel-cache` | core, cypher, cache |
| `uriel-algo` (wave 2) | GDS-style algorithms on CSR projections | none |
| `seal-vector` (wave 2) | HNSW vector index and full-text index | core |
| `uriel-mcp` (wave 2) | stdio MCP server: `get_neo4j_schema`, `read_neo4j_cypher`, `write_neo4j_cypher`, memory helpers | query |
| `seal-bolt` (wave 3) | Bolt v5 wire protocol so real Neo4j drivers connect | query |

## Neo4j capability map

| Neo4j power | seal answer | Wave |
|---|---|---|
| Property graph (labels, rel types, props) | `uriel-core` | 1 |
| ACID transactions, WAL | `redb` copy-on-write transactions, single writer, many readers | 1 |
| Label / property / composite indexes | B-tree tables in `uriel-core` | 1 |
| Uniqueness / existence constraints | enforced in write path | 1 |
| Cypher: MATCH, OPTIONAL MATCH, WHERE, WITH, RETURN, ORDER BY, SKIP, LIMIT, UNWIND, CREATE, MERGE, SET, REMOVE, DELETE, DETACH DELETE, UNION, CALL, CASE, list/pattern comprehension, var-length paths, shortestPath | `uriel-cypher` + `uriel-query` | 1 parse, 2 execute |
| Graph Data Science (PageRank, WCC, Louvain, betweenness, BFS/DFS, Dijkstra, triangle count, node similarity) | `uriel-algo` | 2 |
| Vector index, full-text index | `seal-vector` | 2 |
| Clustering, read replicas, causal consistency | `uriel-cache` gossip ring now; replication in wave 3 | 1 cache, 3 replicas |
| Bolt protocol, drivers | `seal-bolt` | 3 |
| APOC | covered by built-in functions where cheap; no plugin loader | 3 |

## Distributed caching

Each node owns a local TinyLFU cache. A consistent-hash ring (virtual nodes) maps a key
to owner nodes; gossip over UDP/TCP spreads membership. Every committed write bumps a
monotonic graph epoch; cache entries carry the epoch they were computed at and are
invalid once the epoch advances for any label or relationship type the query touched.
Query results and hot adjacency pages are the cached objects.

## Resource budget

Idle RSS under 50 MB. Storage is one file. Release builds use thin LTO and abort on panic.

## Honest scope

Neo4j is millions of lines. seal covers the capabilities above in waves; each wave is
verified by tests before the next starts. The capability map is the contract: a row is
done only when its tests pass.
