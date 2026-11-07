# Retrieval (RAG)

The retrieval engine indexes text files into the graph as chunks. Each chunk keeps four resolutions: the full text, a memory at about a quarter of its size, a memory at about 1/16 of its size and a one-line gist. A query is answered by a hybrid of BM25 and vector search, and the results are packed under a token budget so the caller can zoom into any hit. The MCP tools `rag_index`, `rag_search`, `rag_expand`, `rag_feedback`, `seal_remember`, `seal_recall`, `view`, `soul` and `session_reset` all run on this engine.

Engine: `crates/uriel-rag/src/`. Tools: `crates/uriel-mcp/src/rag_tools.rs`, `view.rs`, `soul.rs`.

## Contents

1. [What is indexed](#1-what-is-indexed)
2. [Chunking](#2-chunking)
3. [What is stored](#3-what-is-stored)
4. [Incremental indexing](#4-incremental-indexing)
5. [Query: hybrid first stage](#5-query-hybrid-first-stage)
6. [Query: BM25 rerank](#6-query-bm25-rerank)
7. [Query: graph expansion](#7-query-graph-expansion)
8. [Resolution ladder and budgets](#8-resolution-ladder-and-budgets)
9. [Memory budget and spill](#9-memory-budget-and-spill)
10. [Tools](#10-tools)
11. [Evaluation](#11-evaluation)
12. [See also](#see-also)

## 1. What is indexed

`index_dir` walks a directory in name order and reads every regular file (`walk` in `crates/uriel-rag/src/lib.rs`). It skips:

| Case | Rule | Source |
|---|---|---|
| Symbolic links | not followed | `walk` |
| Directories named `.git`, `target`, `node_modules` | skipped at any depth; extra names come from `IndexOptions::skip_dirs` | `SKIP_DIRS`, `IndexOptions` |
| Files larger than `max_file_bytes` | skipped; default 1 MiB (`1 << 20`) | `RagConfig::default` |
| A NUL byte in the first 8000 bytes | skipped as binary | `read_text` |
| Bytes that are not valid UTF-8 | skipped | `read_text` |

Everything else is indexed, including Markdown, JSON, TOML and plain text. The extension picks the chunking rules (section 2). The MCP server and the CLI use `IndexOptions::default()`, so `skip_dirs` is available only to library callers.

Paths in the index are relative to the directory that was indexed, with `/` as the separator on every platform.

## 2. Chunking

`chunk_text` in `crates/uriel-rag/src/chunk.rs` cuts a file at structural lines. The language comes from the extension (`Lang::from_path`):

| Extension | Language | Cut points |
|---|---|---|
| `.rs` | Rust | `fn`, `struct`, `enum`, `trait`, `impl`, `mod`, `union`, `macro_rules!`, `type`, `static`, and `const` items |
| `.py` | Python | `def`, `async def`, `class` |
| `.ts .tsx .js .jsx .mjs .cjs` | Script | top-level `function`, `class`, `export`, `const`, `let`, `var`, `type`, `interface` and similar; methods at indent 2 to 4 that end in `{` |
| `.md .markdown` | Markdown | headings `#` to `######`, ignoring lines inside fenced code |
| `.toml` | TOML | `[table]` headers |
| `.json` | JSON | keys at indent 2 or one tab |
| anything else | Text | no cut points |

Rules:

- A chunk starts at a cut point. Comment, attribute and decorator lines directly above it (`///`, `#[`, `@`, `#`) stay with it.
- A segment with fewer than 4 non-blank lines (`min_lines`) is merged into its neighbour.
- Code segments of up to 80 lines (`max_lines`) stay whole. A longer code segment, and any plain-text file longer than 40 lines, is cut into windows of 40 lines (`window_lines`) that overlap by 5 lines (`overlap_lines`).
- Leading and trailing blank lines are trimmed.

Line numbers are 1-based and inclusive. The chunk stores them as `start` and `end`, and citations print them as `file:start-end`.

The MCP server and the CLI use `ChunkOptions::default()`, which has the values above.

## 3. What is stored

Everything lives in the property graph (`uriel-core`). `apply` in `lib.rs` writes these nodes and edges.

| Label | Properties | Notes |
|---|---|---|
| `File` | `path`, `hash`, `kind` | `hash` is the SHA-256 of the file text. `kind` is `dir` for files from `index_dir` and `text` for sources from `index_text`, such as remembered findings. The `path` index is created when the graph is writable. |
| `Chunk` | `file`, `start`, `end`, `text`, `mem`, `l1`, `g3`, `hash`, `vec`, `tc`, `utility` | See the table below. |
| `Decision` | `key`, `kind`, `text`, `source_id`, `tags` | Written by `seal_remember`. `key` is unique (`finding:N`). |
| `Soul`, `SoulSection`, `SoulFeat` | guide and feature cache | Written by `soul`, capped (section 9). |

Chunk properties:

| Property | Content | Used for |
|---|---|---|
| `text` | the original chunk text | L0 and `rag_expand` level 0; the gist and the embedding are derived from it |
| `mem` | compressed memory at `compression_ratio` (default 16) | L2; the text that BM25 searches in the first stage |
| `l1` | compressed memory at `compression_ratio / 4`, clamped to 1 to 4 (4 at the default) | L1 |
| `g3` | one-line gist | L3 |
| `vec` | 256-dimension embedding of `text` | vector leg of the first stage |
| `tc` | term counts of `text` as a binary blob: a `u32` token count, then records of a `u64` term hash and a `u32` count, sorted by hash | BM25 rerank |
| `hash` | SHA-256 of `text` | fingerprint for session dedup and the soul cache |
| `utility` | float in [-1, 1], starts at 0 | feedback prior (`rag_feedback`) |

Remembered findings are the exception to compression: `prepare` in `lib.rs` uses ratio 1 for paths under `memory/`, so their `mem` and `l1` keep the text except for whitespace runs and lines over 160 characters.

Edges: `IN_FILE` from each Chunk to its File, `NEXT` from each Chunk to the next Chunk of the same file, `CITES` from a Decision to its source Chunks, and `HAS_SECTION` from a Soul to its sections.

**Chunk ids.** `chunk_id` is the database node id of the Chunk. Every tool that takes or returns a chunk uses this id. An id stays valid while its file is unchanged. Once the file is re-indexed, its chunks are deleted and recreated with new ids, so an old id makes `rag_expand` return an error.

## 4. Incremental indexing

`index_dir` does this, in order:

1. Walk the tree and read every file. Compute the SHA-256 of its text.
2. A file whose hash equals the stored `File.hash` is counted as unchanged and not touched.
3. A new or changed file is chunked again. The file's old Chunk nodes are deleted and new ones are created, with new ids.
4. Writes are batched: a transaction is committed after about 512 chunks.
5. With `prune` on (the default), `dir` entries that were not seen in this walk are deleted together with their chunks.

`index_text` does the same for one in-memory source. It is how `seal_remember` stores a finding under `memory/finding-N.md`.

The IDF weights used for compression come from the chunks of the current walk (`index_dir`) or from the one text (`index_text`). Chunks of unchanged files keep the memory they were first stored with.

**Caution on prune.** Paths are relative to the directory passed in, so pruning compares relative paths. Indexing a subdirectory with the default prune on deletes the entries of an earlier index of its parent, because their relative paths are not seen in the new walk. For example, after indexing the project root, indexing `src` as a root removes `src/lib.rs` (it is not seen as `src/lib.rs`) and adds `lib.rs`. Files that were skipped in this walk (binary, too large, not UTF-8) also count as unseen. The MCP `rag_index` tool and `uriel index` both use the default. Keep one indexed root per database, or set `IndexOptions { prune: false, .. }` in library code. This page cites no test for the behaviour; it follows from `index_dir` in `lib.rs`.

Indexing commands:

```bash
uriel index ~/src/my-project --db ~/.uriel/graph.uriel
```

```powershell
uriel index "$HOME\src\my-project" --db "$HOME\.uriel\graph.uriel"
```

The command prints the counts from `IndexReport`: files scanned, indexed, unchanged, skipped and removed, and chunks added and removed (`crates/uriel-cli/src/commands.rs`).

## 5. Query: hybrid first stage

`search` in `lib.rs` embeds the query and computes two candidate lists.

**Terms** (`crates/uriel-rag/src/tokenize.rs`). Words are runs of letters, digits and `_`. An identifier is split at `_` and at case changes (`parse_duration` gives `parse` and `duration`; `HttpServer` gives `http` and `server`), and the whole identifier is kept too. Parts shorter than 2 characters and a short list of English stop words are dropped. Longer ASCII words lose a final `ing`, `ed`, `es` or `s`.

**Embedding** (`crates/uriel-rag/src/embed.rs`). The default `HashEmbedder` is feature hashing, not a learned model. Each term adds 1.0 to one hashed bucket. Each 3-character and 4-character substring of `<term>` adds 0.5 to its bucket. A hash bit gives the sign, and the vector is L2-normalised. The dimension is 256 by default (`RagConfig::embed_dim`, minimum 8). Library callers can pass their own `Embedder` in `RagConfig`; the MCP server and the CLI use the default.

**Vectors** are stored as int8 with one `f32` scale per vector (`QVec`). The vector leg scores with the int8 dot product.

**The first stage** combines two ranked lists:

The first stage asks for `n = max(k, 100)` results (`RERANK_DEPTH` in `rerank.rs`). Each list is cut to `max(4n, 50)` candidates, which is 400 at the defaults.

- BM25 over `mem` (the compressed memory). The constants are k1 = 1.2 and b = 0.75.
- Dot product over `vec` (the full text).
- Reciprocal rank fusion: each list adds `1 / (60 + rank + 1)` to a chunk's score.
- With `use_utility` on, the fused score is multiplied by `1 + 0.25 × utility`.

The fused list is cut to `n` results.

## 6. Query: BM25 rerank

`rerank` in `crates/uriel-rag/src/rerank.rs` scores the first-stage pool again, this time with BM25 over the full text (the `tc` counts). IDF is computed inside the pool, not over the whole index.

For each candidate:

- The lexical score is its BM25 score divided by the best score in the pool, so it lies in [0, 1].
- A prior, `0.5 × 61 / (61 + first_rank + 1)`, keeps the first-stage order as a tie-breaker.
- With `use_utility` on, the total is multiplied by `1 + 0.25 × utility`.

The top `k` are returned. The rerank is skipped when the query has no terms or the pool has fewer than two candidates.

PLAN.md B9 describes the first-stage pool as 200. The constant in code is 100 (`RERANK_DEPTH`), and this page follows the code.

## 7. Query: graph expansion

With `expand_hops` of 1 or 2, `expand_ids` adds neighbours of each hit, one hop at a time:

- the adjacent chunks in the same file (`NEXT`, both directions);
- for a Rust hit in a resident index, up to 3 chunks that mention a name the hit defines (callers).

Each hop multiplies the score by 0.5 (`HOP_DECAY`). Neighbours are returned after the direct hits with `via` set to `graph`. `expand_hops` above 2 is an error.

The library default is 0. The MCP `rag_search` tool defaults to 1 (`rag_tools.rs`). `seal_recall` and `view` search with 0.

## 8. Resolution ladder and budgets

| Level | Content | Size | Reached by |
|---|---|---|---|
| L0 | `text` | the original | `rag_expand` with `level` 0 |
| L1 | `l1` | about a quarter (ratio 4 at the default) | `level` 1; the top hit in a packed result |
| L2 | `mem` | about 1/16 (ratio 16 at the default); 16.37x smaller on the frozen corpus (PLAN.md B5) | `level` 2; hits 2 to 4 in a packed result |
| L3 | `g3` | one line | `level` 3; all remaining hits |

The gist (`crates/uriel-rag/src/ladder.rs`) is the first signature line with trailing `{`, `;` and `,` removed, cut at 64 characters. If the chunk has a doc comment (`///`, `//!`, `/** */`, a Python docstring, or the first sentence under a Markdown heading), its first sentence is appended after ` -- ` when it fits in 96 characters. A gist is therefore at most 96 characters when a doc sentence is appended, and at most 64 when it is not.

`pack_ladder` in `lib.rs` packs a result list under `budget_tokens`:

1. Rank 0 goes to L1, ranks 1 to 3 to L2, the rest to L3. A hit whose L1 text cannot be produced stays at L2.
2. While the total cost is over budget, the lowest-ranked hit that can move to a strictly cheaper rung does so. When no hit can move, the lowest-ranked hit is dropped.
3. Cost is `ceil(chars / 4)` of each rendered piece. Each piece starts with the head `// file:start-end #chunk_id`.

The MCP tools render the packed hits in compact form and check the encoded size against the budget again. If the encoding is over, the inner budget is reduced by the overflow and the pack is rendered again. If nothing fits, the result is `#0`. The compact encoding is specified in the `rag_search` tool description (`rag_tools.rs`) and implemented in `crates/uriel-mcp/src/compact.rs`.

Tokens are a proxy, `ceil(chars / 4)`, not a count from a model tokenizer. Every budget in this page is in that unit.

## 9. Memory budget and spill

- **Index cap.** `RAG_INDEX` is 240 per mille of the total memory budget (`crates/uriel-core/src/budget.rs`). The cap is therefore 24 % of `--memory-mib`. PLAN.md A7 gives the server default as 50 MiB and the minimum as 16 MiB.
- **Estimate.** `resident_bytes` adds the slots, vectors, id map, BM25 postings and mention map, then multiplies by 5/4 for slack.
- **Spill.** Load checks the estimate every 256 chunks, and `apply` checks it after each write. Over the cap, the in-memory index is dropped, `stats().spilled` becomes true, and each query scans every chunk in the graph (`crates/uriel-rag/src/scan.rs`). Spilled queries follow `NEXT` links but not Rust symbol links. A spilled index returns to memory only after the database is reopened.
- **Soul cache.** Capped at 1/8 of the index cap (`CACHE_DIV`), with at most 8 stored guides.
- **Read-only servers.** `--read-only` computes gists and derived levels but does not store them, and does not store soul guides.

PLAN.md A7 records the whole-server peak working set of 52.7 MiB at commit `7d74e6c`, with a gate of at most 60 MiB. The phases in that record are idle 9.4, index 36.7, 100 searches 38.0, 20 views 38.1 and 20 souls 52.7 (MiB). These figures cover the whole server, not the RAG index alone.

## 10. Tools

Shared behaviour:

- **Output.** Compact by default. `format=json` returns the older JSON shape. A hit is cited as `file:start-end#chunk_id`. The JSON shape carries the same fields.
- **Flags** on compact hits: `g` graph neighbour, `m` remembered finding, `L1` or `L2` for the level (none means L3), `r` already delivered this session, `~` delivered but changed since.
- **Session dedup.** A chunk already delivered in this session comes back as a bare `#chunk_id` with flag `r`. `fresh=true` returns the text again. `session_reset` clears the record. Gate: `crates/uriel-mcp/tests/compact.rs` (PLAN.md B3).
- **Budgets** are in proxy tokens (section 8).

### rag_index

Indexes a directory. Writes to the graph. Unchanged files are skipped and deleted files are pruned (section 4).

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `path` | string | no | project root | Directory to index. A relative path is joined to the project root. |

Returns JSON with `path`, `files_scanned`, `files_indexed`, `files_unchanged`, `files_skipped`, `files_removed`, `chunks_added` and `chunks_removed`.

### rag_search

Hybrid search with the ladder. Does not write, except that the ladder may store a derived level on first use.

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `query` | string | yes | | What to look for. |
| `k` | integer, at least 1 | no | 8 | Direct hits to return. |
| `budget_tokens` | integer, at least 1 | no | 2000 | Token budget for the returned hits. |
| `expand_hops` | integer, 0 to 2 | no | 1 | Graph expansion hops (section 7). |
| `format` | `compact` or `json` | no | `compact` | Output shape. The server's `--format` setting changes the default. |
| `ladder` | boolean | no | `true` with compact, `false` with json | Use the resolution ladder (section 8). With `false`, every hit is shown at L2. |
| `fresh` | boolean | no | `false` | Return text for hits already delivered this session. |

### rag_expand

Returns one chunk at a chosen resolution and records it as delivered in the session. Level 1 and level 3 are computed on first use and stored on the chunk, unless the server is read-only.

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `chunk_id` | integer, at least 0 | yes | | From `rag_search` or `seal_recall`. |
| `level` | integer, 0 to 3 | no | 0 | 0 original text; 1 about a quarter; 2 compressed memory; 3 one-line gist. |

### rag_feedback

Moves the utility prior of each listed chunk. Writes to the graph. At least one id is needed. Ids that are not chunks are ignored.

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `used` | array of integers | no | `[]` | Chunks that helped. Utility moves toward +1. |
| `unused` | array of integers | no | `[]` | Chunks that were retrieved but not used. Utility moves toward -1. |

Each update is `u' = clamp(0.7 × u + 0.3 × target, -1, 1)` (`UTILITY_ALPHA` 0.3, target +1 or -1). Returns `used` and `unused` counts. The utility then scales scores by `1 + 0.25 × u` (sections 5 and 6).

### seal_remember

Stores a finding as a `Decision` node with key `finding:N`, links it to its source chunks with `CITES` edges, and indexes its text as `memory/finding-N.md`. Writes to the graph. The `seal_` prefix is kept for compatibility (PLAN.md I1b).

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `text` | string | yes | | The finding or decision. |
| `tags` | array of strings | no | `[]` | Indexed with the text as a `tags:` line. |
| `sources` | array of integers | no | `[]` | Chunk ids the finding rests on. Each must be a chunk id, or the call fails. |

Returns `id` (`finding:N`), `source_id` (`memory/finding-N.md`) and the number of `sources`. If the node is stored but indexing fails, the error says so.

### seal_recall

Searches the general index with a wider candidate list (`max(4k, 32)`), puts remembered findings first, and takes `k`. Same output as `rag_search`.

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `query` | string | yes | | What to recall. |
| `k` | integer, at least 1 | no | 8 | Hits to return. |
| `budget_tokens` | integer, at least 1 | no | 2000 | Token budget. |
| `format` | `compact` or `json` | no | `compact` | Output shape. |
| `ladder` | boolean | no | as `rag_search` | Resolution ladder. |
| `fresh` | boolean | no | `false` | Return text for hits already delivered. |

### view

Builds a context view for a question under a hard budget. It searches with direct hits only (`expand_hops` 0), orders the candidates by file and start line, and embeds each with the 256-dimension hash embedder. The `uriel-topo` schedule then keeps the first block (sink), the last two blocks (local window), the `max(2, k/4)` blocks nearest the focus (relevant), and `salient_k` extra blocks. The result is rendered through the ladder under the budget. It is deterministic and does not depend on the session. Code: `crates/uriel-mcp/src/view.rs`.

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `question` | string | yes | | What the context is for. |
| `budget_tokens` | integer, at least 1 | yes | | Hard budget for the whole view. |
| `focus` | string | no | the question | Text that the relevant blocks are matched against. |
| `k` | integer, 1 to 256 | no | 12 | Candidate chunks. |
| `salient_k` | integer, 0 to 256 | no | 0 | Extra topologically salient blocks. The default is 0 because of the C3 verdict (section 11). |
| `islands` | boolean | no | `false` | Group candidates into connected islands and render one budgeted section each. Off by default because of the B11 verdict. |
| `format` | `compact` or `json` | no | `compact` | Output shape. JSON adds `candidates` and `selected`. |

### soul

Returns a small reference guide to the indexed code. Sections are ranked by query-free salience: documentation density, definitions and rare vocabulary. A seeded score diffusion along `NEXT` and definition-to-mention links refines the ranking. The guide is packed under the budget through the ladder. Each section cites `file:start-end#chunk_id`, and `rag_expand` with that id returns the exact original. No model is used, and the same index and arguments give the same bytes. The computed guide is cached in the graph unless the server is read-only. Code: `crates/uriel-rag/src/soul.rs`, `crates/uriel-mcp/src/soul.rs`.

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `budget_tokens` | integer, 1 to 100000 | no | 1200 | Hard budget for the guide. |
| `focus` | string | no | none | Path prefix. Only files under it enter the guide. |
| `sections` | array of integers | no | all | Chunk ids of guide sections to return alone. Each must be a section of the guide for the same `budget_tokens` and `focus`. |
| `format` | `compact` or `json` | no | `compact` | Output shape. JSON adds `rounds`, `seed`, `digest` and `cached`. |

Diffusion stops after two rounds with the same packed result, or after 8 rounds. The pool is the top 2048 chunks by salience.

### session_reset

Clears the record of delivered chunks and the repeat guard, so the next search returns text again. Takes no arguments. Returns a `cleared` count.

## 11. Evaluation

### Frozen corpus and queries

- **Corpus.** 207 source files under `crates/uriel-rag/tests/frozen_corpus/`, each stored with a `.frozen` suffix that is stripped when the corpus is materialised into a temporary directory (`eval.rs`). PLAN.md B7 and B5 record the count, and the test `frozen_corpus_holds_sources_only` checks it.
- **Digest.** `corpus_digest` hashes the relative path and bytes of every file. The SHA-256 is recorded in `tests/eval_queries.json`, and `run_frozen` refuses to run on a corpus whose digest differs.
- **Queries.** `generate_queries` takes Rust functions with doc comments. It removes identifiers from the doc text (backticked words, words with `_`, `::`, `/`, `<`, `>`, `#`, `(` or `[`, CamelCase tails, and the function name's own words) and keeps the candidates with at least 6 words left. It samples 100 evenly. The gold chunk for a query is the chunk containing the function's signature line.
- **Metric.** Recall at k is the fraction of queries whose gold chunk is among the top k. Each query is checked against three systems: BM25 over full chunk text (the baseline), `rag.search` (the hybrid), and `pack_ladder` (the same hits, presented through the ladder).

### Recorded results

| Measure | Result | Date and commit | Source |
|---|---|---|---|
| Recall at 5 and 10, hybrid over compressed memories | 1.000 and 1.000 | 2026-10-18 | PLAN.md B5 |
| Recall at 5 and 10, BM25 over full chunk text | 1.000 and 1.000 | 2026-10-18 | PLAN.md B5 |
| Recall at 5 and 10, through `pack_ladder` | 1.000 and 1.000 | 2026-10-18 | PLAN.md B5 |
| Memory size against full text | 16.37x smaller | 2026-10-18 | PLAN.md B5 |
| Hybrid recall at 5, before the int8 and BM25-rerank change | 0.880 | 2026-10-15 | PLAN.md B9 |
| Hybrid recall at 5 after the change | 1.000 | `8753a96` | PLAN.md B9 |
| Proxy tokens, `rag_search` and Cypher, v2 against v1 | 2.74x overall (`rag_search` 3.05x, Cypher 2.47x); bar was at least 3x, **missed** | 2026-10-18 | PLAN.md B4 |
| `rag_search` repeat with session dedup | 3.59x fewer proxy tokens | 2026-10-18 | PLAN.md B4 |
| Identical repeat search, bytes | 7.4x smaller; 4.62x by cl100k proxy (905 to 196 tokens) | 2026-10-15 | PLAN.md B3 |

The int8 vector leg is tested against the f32 vector leg on the same queries. The test asserts top-1 agreement of at least 99 of 100 and top-10 overlap of at least 940 of 1000 (`int8_vector_leg_keeps_the_f32_top_10_on_the_frozen_queries` in `lib.rs`). These are the test bounds, not measured values.

**Limits.** The frozen set is saturated: recall is 1.000 for both the hybrid and BM25, so it cannot show a gain (PLAN.md K0). The 100 queries are generated from doc comments, not written by hand. Compression ratios depend on the corpus, and code compresses differently from prose.

### Negative results

These are recorded in `PLAN.md` and stand as written.

- **Islands in `view` (B11): off by default.** On the 100 frozen queries, recall at 5 was 84/100 with islands off and 92, 94 and 94 with them on, at budgets 300, 800 and 4000. Mean proxy tokens rose from 148.6 to 196.4, 257.6 and 274.6 at the same budgets. Islands raise recall and raise the token cost, so the default stays off. An earlier build (`f7e7a26`) recorded a 40 to 64 % range; that record is superseded.
- **Soul guide (B12): does not beat the plain salience baseline.** The gold chunk was inside the guide for 0, 3 and 7 of 100 queries at budgets 300, 1200 and 4000. A plain top-by-salience selection of the same size held 0, 2 and 7. Chance was 0.34, 1.27 and 3.99.
- **Topological salience term (C3): loses to top-k, not wired into retrieval.** Synthetic contexts, recall at 5, 10 and 25 % budget: top-k .865, .957 and 1.000; topological .790, .947 and 1.000. Real data, 1,647 blocks and 759 queries: top-k .520, .626 and .798; topological .439, .542 and .721. The term wins only at alpha 0.10. This is why `salient_k` defaults to 0.

### Running the evals

Run from the repository root. The evals that are marked `--ignored` need the flag.

```bash
cargo test --release -p uriel-rag --test eval recall_on_the_frozen_corpus -- --ignored --nocapture
cargo test --release -p uriel-rag --test eval soul_guide_coverage_on_the_frozen_corpus -- --ignored --nocapture
cargo test --release -p uriel-rag --test quantised -- --nocapture
cargo test --release -p uriel-rag int8_vector_leg -- --nocapture
cargo test --release -p uriel-mcp --test view_frozen_tool -- --nocapture
python crates/uriel-mcp/tests/token_eval/eval.py --bin target/release/uriel-mcp.exe --out <scratch>
```

On Windows the binary is `target\release\uriel-mcp.exe`. On Linux and macOS cargo builds it without the `.exe`.

**Digest check.** `corpus_digest_matches_the_recorded_one` in `crates/uriel-rag/tests/frozen.rs` compares the corpus digest with the one in `tests/eval_queries.json`. The digest covers the raw bytes, so line endings matter. The header of `PLAN.md` records that on the 2026-10-18 checkout the three digest checks failed: the recorded digest matched neither the CRLF checkout nor the LF checkout. Until the digest is refreshed, `run_frozen` refuses with a message about drift, and the recall eval does not run. Refreshing the digest is a deliberate step, not a fix to make silently.

**Recorded measurement.** Each number in the table above was measured on the commit and date given. Re-run the command to compare on your own checkout, and report the result with its commit.

## 12. Not built

`docs/PHYSICS-RAG.md` lists candidate retrieval changes (anti-kt clustering, unfolding, property tests and synthetic queries). None is built. Each has a baseline and a deletion condition, and the first step is a harder benchmark, because the current frozen set is saturated.

## See also

- [`SEAL-DB-V2.md`](SEAL-DB-V2.md): the design behind the ladder, compact encoding and session dedup.
- [`PHYSICS-RAG.md`](PHYSICS-RAG.md): unbuilt retrieval ideas with their gates.
- [`claude-code-hooks.md`](claude-code-hooks.md): compact capture and recall for Claude Code compaction, which uses this engine.
- [`../PLAN.md`](../PLAN.md): the registry with every measured number and its date.
