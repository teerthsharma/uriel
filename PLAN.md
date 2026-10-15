# PLAN.md: Master Task Registry

**Code-verified pass (2026-10-15, workspace 1,441 passed / 0 failed / 6 ignored across 83 binaries, clippy clean, cargo-deny licenses/bans/sources ok; boxes for E6-E8, F3, G1, H1, H2, D5, I4, I5 built and green but not yet re-ticked):** 38/64 boxes ticked. Every `[x]` carries an inline `path:line` citation to the implementing code and the gate that passed. Every unticked item is unbuilt, or partial with its gap stated. This file is the single source of truth for remaining work; agents tick `[ ] -> [x]` only when the gate passes, with the passing run cited.

**Method (how this project is built):**

1. **Phase 1 is building. Always.** Publishing, naming and polish are Phase 2 and start only when every Phase 1 box is ticked or explicitly deferred by the owner.
2. **Gate first, seen failing.** A gate (test, simulation, benchmark with a bar) is written and watched failing before the work lands. A box without a gate is not a box.
3. **Evidence over assertion.** A number states what it was compared against. Baselines are included. Losses are recorded here, not dropped.
4. **No comments, no docs in code.** Zero `//`, `///` or `//!` in new code; test names carry the meaning. No new markdown during Phase 1. Scope, results and misses go to the blog (teerth.blog) and the paper (blog.teerth.dev) in Phase 2, written from this registry.
5. **No panics on adversarial bytes.** Every decode path is bounds-checked and returns an error. `#![deny(unsafe_code)]` everywhere except `uriel-sys`.
6. **Determinism is a feature with a test.** Same input log, same output, independent of thread count and wall clock.
7. **Hardware is reached only through capabilities.** No raw addresses or sizes from callers; every unsafe operation takes a validated range object (see `docs/design/CAPABILITIES.md`, to be written with F1).

Specs: [`docs/DESIGN.md`](docs/DESIGN.md) (crate map), [`docs/HOUSE-SEAL.md`](docs/HOUSE-SEAL.md) (modes, charter harness), [`docs/SEAL-DB-V2.md`](docs/SEAL-DB-V2.md) (tiers, tokens, resilience).

---

## Phase 0: Foundations (built)

- [x] Property graph store, ACID transactions on a single writer, many readers: `crates/uriel-core/src/graph.rs:179` (`open`), `:291` (`begin_write`). Gate: `cargo test -p uriel-core`, 89 passed (workspace run 2026-10-15)
- [x] Unique and existence constraints enforced on the write path: `crates/uriel-core/src/graph.rs:364`. Gate: rollback-on-violation test in `uriel-core`
- [x] Commit callbacks with epoch and touched label/type sets: `crates/uriel-core/src/graph.rs:310`. Gate: epoch/callback tests in `uriel-core`
- [x] openCypher parser with Display round trip: `crates/uriel-cypher/src/lib.rs:26`. Gate: 303 passed incl. 4 generators x 5,000 fuzz cases
- [x] Cypher executor, aggregation, MERGE, var-length paths, EXPLAIN, procedures: `crates/uriel-query/src/lib.rs:256`. Gate: 38 passed incl. a 105-row TCK-style table
- [x] Result cache with tag invalidation by commit: `crates/uriel-query/src/lib.rs:209` (`with_dist_cache`). Gate: 9 cache tests, 6 fail with invalidation disabled
- [x] Graph algorithms: PageRank `crates/uriel-algo/src/centrality.rs:63`, Louvain `community.rs:252`, Kruskal MST `spanning.rs:31`. Gate: 138 passed, karate-club values match networkx
- [x] Distributed cache, gossip membership, epoch invalidation: `crates/uriel-cache/src/dist.rs:289`. Gate: 67 passed, 5 stability runs
- [x] Trainless compress-then-retrieve RAG: `crates/uriel-rag/src/lib.rs:924` (`pack`), `:955` (`expand`), `:1136` (`feedback`). Gate: 42 passed. **Measured loss:** recall@5 0.880 vs 1.000 for full-text BM25 on 100 doc-comment queries over this repo, compression 12.29x against a 16x target, index not meaningfully smaller (3.21 MB vs 3.31 MB)
- [x] Goal-tree harness: merge `crates/uriel-house/src/house.rs:350`, next `:388`, report `:478`, leap slot `:865`, governor `crates/uriel-house/src/governor.rs:41`. Gate: 103 passed incl. a 40-leaf simulation under RAM 6000, 2000, 5000 MB
- [x] MCP server, 29 tools (Cypher, memory, rag, house): `crates/uriel-mcp/src`. Gate: 120 passed; real-binary run indexed 133 files into 2,538 chunks in 0.2 s; release binary 4.72 MiB, idle RSS 10.3 MiB
- [x] Live board with whiteboard gates: `~/.claude/skills/dispatching-house-mode/gates.py`. Gate: 31 passed, 5 mutation checks caught

## A. Storage tiers (bytes). Design: `docs/SEAL-DB-V2.md` section 1

- [x] A1 Sketch, TinyLFU gate, hot cache, tagged codecs, tier controller: `crates/uriel-tier/src/codec.rs:133` (`encode_value`), scan pair `crates/uriel-tier/src/cache.rs:426` and `:437` (gate on keeps a 100-key hot set through a 100k-key scan, gate off keeps 0 of 100 in the long scan). Gate: 31 passed; admit-always mutation failed 5 tests
- [x] A2 Record compression at rest, Cold codec = zstd level 19: `crates/uriel-tier/src/codec.rs:133`; legacy format-1 database reads and writes: `crates/uriel-core/src/tests/tier.rs:442`; decoder fuzz 20,000 inputs `crates/uriel-tier/src/codec.rs:338`. **Irreversible:** opening bumps the database format 1 to 2, the previous release cannot read an upgraded file
- [x] A3 T0 gated read-through cache with exact invalidation: `crates/uriel-core/src/tier.rs:342` (`begin_commit`), `crates/uriel-core/src/tests/tier.rs:105` (touched ids only), `:160` (rollback invalidates nothing). RED seen: a stub `remove_key` failed 6 tests, fix at `tier.rs:232`
- [x] A4 T1 warm lz4 tier under a byte budget: re-promotion `crates/uriel-core/src/tests/tier.rs:136`, budget never exceeded over 100k random ops `:356`. Byte counts are estimates, not allocator RSS
- [x] A5 Differential test against a HashMap model (5 seeds x 2,000 ops, T0 = 3 entries, T1 = 1 KiB): `crates/uriel-core/src/tests/tier.rs:484`; reads equal writes through every tier `:49`
- [x] A6 `Graph::tier_stats()`: `crates/uriel-core/src/graph.rs:252` (O(records)). Measured 2026-10-15 on this machine, release: 20k nodes of this repo's text 22,260,000 logical bytes, lz4 13,978,613 (1.59x), zstd-19 10,532,683 (2.11x); redb file 33,689,600 vs 16,846,848 (2.00x); Zipf(1.0) hit rate gate on / off 65.8 / 61.2 % plain, 60.2 / 51.6 % with a scan read per read, 63.1 / 60.4 % with a 20k scan burst (oracle ceiling 71.4 %); read ns median of 1M: T0 242, T1 1,018, T2 3,882 (page-cached)

## B. Token tiers (tokens). Design: `docs/SEAL-DB-V2.md` section 2

- [x] B1 Resolution ladder: `crates/uriel-rag/src/lib.rs:867` (`pack_ladder`), `Level` and `gist` in `crates/uriel-rag/src/ladder.rs`; gate `crates/uriel-rag/tests/ladder.rs:222` (200 random budgets, never over budget, no hit dropped before every hit is at L3); old chunks upgrade lazily
- [x] B2 Compact encoding with decoder: `crates/uriel-mcp/src/compact.rs:312` (`encode_table`), round trip over 2,000 tables `:763`, 20,000 adversarial inputs per decoder `:954`; `format` argument and `--format` flag
- [x] B3 Session dedup: `crates/uriel-mcp/src/session.rs:18`, `session_reset` `crates/uriel-mcp/src/rag_tools.rs:156`, repeat gate `crates/uriel-mcp/tests/compact.rs:138`. Identical repeat search is 7.4x smaller in bytes and passes the 5x bar by chars/4, but only 4.62x by cl100k proxy (905 to 196 tokens)
- [ ] B4 Token evaluation (`crates/uriel-mcp/tests/token_eval/`): **bar missed, 2.55x vs 3x** after B6. cl100k proxy, 20 fixed queries, same run, v1 vs v2: RAG 23,959 to 9,255 (2.59x), Cypher 17,655 to 7,038 (2.51x), total 41,614 to 16,293. Citations preserved 305 of 305. Gap: 2,422 tokens (14.9 %) to reach 13,871. Largest contributor RAG at 57 % of v2, 249 of 294 hits are L3 gists plus a citation line; unique identifiers in Cypher cannot be shrunk by this scheme. Next lever: drop the per-hit citation line to a compact id when the gist already names the symbol (B8)
- [x] B5 Recall with the ladder, frozen corpus (207 files, 3,534 chunks, 100 queries, identical on two runs): hybrid 0.880 / 0.890, hybrid through `pack_ladder` 0.880 / 0.890 (ladder changes nothing), BM25 over full chunks, same run, 1.000 / 1.000. Hybrid trails BM25 by 12 points at @5. Gate: `crates/uriel-rag/src/eval.rs:194`

## C. Topology (context and layout). Design: `docs/SEAL-DB-V2.md`, `docs/design/TOPOLOGY.md` (to write)

- [x] C1 `uriel-topo` 0-D persistence: `crates/uriel-topo/src/persistence.rs:54`, elder rule `:335`. Gate: hand-computed diagrams, stability within 2 epsilon over 200 random sets, isometry, scale, permutation, duplicates die at 0, n-1 finite pairs; flipped elder rule failed 5 of 15 tests
- [x] C2 Context schedule: `crates/uriel-topo/src/schedule.rs:298`, `select_row` `:274`, `dense_causal` `:136`. Gate: dense parity at salient_k = all, causality, lowest priority dropped first, CSR output
- [x] C3 Needle-recall evaluation built: `crates/uriel-topo/src/eval.rs:123`, `cargo run --release -p uriel-topo --example eval`. **Verdict: the topological arm loses to top-k.** Synthetic (20 contexts, alpha 0.25, recall at 5 / 10 / 25 % budget): recency .025 / .085 / .250, top-k .865 / .957 / 1.000, random .047 / .092 / .250, topological .790 / .947 / 1.000. Real data (1,647 blocks, 759 queries): top-k .520 / .626 / .798, topological .439 / .542 / .721; SALIENT off equals top-k there. Wins only at alpha 0.10 (+0.335 / +0.620 / +0.365). **Decision:** do not wire the salient term into retrieval; the shipped schedule is SINK + LOCAL + RELEVANT
- [x] C4 Topology-sorted layout: `crates/uriel-topo/src/layout.rs:171`. Gate: identical over 8 runs and across rayon and std thread counts, empty, single node, disconnected, self loops, parallel edges; reversed-degree RCM mutation caught by a hand-computed test
- [x] C5 Layout benefit: `crates/uriel-topo/src/measure.rs:89`, `cargo run --release -p uriel-topo --example measure`. Node pages touched, sorted vs insertion order: 0.379 (interleaved ids, 92 % intra edges), 0.728 (contiguous ids), 0.668 (60 % intra), 0.816 (30 % intra), 0.966 on this repo's call graph (no gain on adjacency lines, 1.007). Louvain found 18 communities vs 8 planted

## D. Scheduler (determinism). Design: `docs/design/PLACEMENT.md` (to write)

- [x] D1 Deterministic placement: `crates/uriel-place/src/place.rs:157` (`apply`), `:303` (`plan`), checkpoint `:507` and `:533`. Replay gate `crates/uriel-place/tests/replay_gate.rs:66`: 200 logs x 8 thread counts with shuffled arrival = 1,600 runs, all equal to the single-thread SHA-256; 200 distinct digests; checkpoint replay equals scratch on 200 of 200. Only 156 of 200 single-event changes altered the digest (the rest changed no decision). Scan pair `gate_scan.rs:52`: hot set keeps 4/4 gate on, 0/4 gate off. The hot gate is a serialisable copy of the uriel-tier sketch (`src/gate.rs`) checked against it on 480 keys
- [x] D2 One test per OS role in `crates/uriel-place/tests/roles.rs`: locality `:63`, prefetch `:95`, fragmentation entropy `:121`, pressure governor `:158`, lifetime `:205` and `:268`
- [x] D3 `calibrate`: `crates/uriel-place/src/calibrate.rs:25` (`find_knees`), `:217` (`measure`), knee tests `crates/uriel-place/tests/calibrate_knees.rs:45`. Measured on this machine 2026-10-15 (12.26 s run): 4-32 KiB 1.18 ns, 64-256 KiB 3.2, 1 MiB 4.1, 2 MiB 4.4, 4 MiB 13.7, 8 MiB 15.0, 16 MiB 72, 32 MiB 92, 64-256 MiB 96-100 ns; knees 32 to 64 KiB, 2 to 4 MiB, 8 to 16 MiB; suggested hot size 4 MiB. **Does not match the spec sheet** (28 MB L2, 33 MB L3): single core, 4 KiB pages, TLB not isolated
- [x] D4 Pressure ramp: `crates/uriel-place/tests/pressure_ramp.rs:74` (hot bytes within budget at all 7 steps, pins kept below Critical), `:102` (recovery restores the hot set), `:123` (identical across runs)
- [x] D6 Planner drives tiering behind `Options::placement: Local | Planned`: `crates/uriel-core/src/placement.rs:198` (`PlanCore::advance`), `crates/uriel-core/src/tier.rs:245`, `:451`, `crates/uriel-core/src/read.rs:327`. Gates: all 18 tier tests under both modes (36 runs), differential vs HashMap under Planned, replay of a 5,000-op log with 1 and 8 reader threads gives identical tier_stats, T0/T1 keys, hot regions and event log (`crates/uriel-core/src/tests/placed.rs:402`), 100k-cold scan vs 100-node hot set: gate on 99.7 % hot-read hit rate vs always-admit 49.9 %. **Measured verdict: Planned does not beat Local.** Zipf(1.0), 20k nodes, 1,000 hot, 200k reads: Local gate on 65.8 / 60.2 / 63.1 % (plain / +scan per read / +20k burst); Planned region size 1: 66.3 / 63.8 / 64.8 %; Planned region size 16: 50.5 / 42.7 / 45.4 %. Per-read cost: Local 216 ns, Planned about 5,200 ns (N=128) or 983 ns (N=1024). **Decision:** default stays `Local`; `Planned` is for replay and analysis. Also: `Placer::plan` was quadratic (82 ms at 1,250 regions) and is now 0.63 ms, output identical on 3,000 replayed streams (SHA-256 `83ec0d57…2339`)
- [ ] D5 Charter-harness governor and smart test selection (already in `uriel-house`): add replay determinism to the same hash test. Gate: not yet written

## E. Resilience (survival). Design: `docs/SEAL-DB-V2.md` section 3

- [ ] E1 HLC and MVCC: partial. `crates/uriel-kv/src/{hlc,mvcc}.rs` (+ `rng.rs`) exist; the lib-test target currently fails clippy (6 errors in `mvcc.rs` tests: `is_multiple_of`, 3x type complexity, `map_or`, map keys iteration). Gap: gc-horizon and snapshot-repeatability property tests
- [ ] E2 Sans-IO Raft: election, replication, current-term commit rule, PreVote, snapshots, single-step membership. `crates/uriel-kv/src/raft.rs` is empty (0 bytes; the build agent returned nothing). Gate: invariant checker after every simulated step; not yet written
- [ ] E3 Ranges: descriptors, split by size, merge, lease exclusivity. `crates/uriel-kv/src/range.rs` is empty. Gate: not yet written
- [ ] E4 Cluster simulator with seeded delay, drop, duplication, reorder, partition, kill, restart. `crates/uriel-kv/src/sim.rs` is empty. Gate: 50 seeded random fault schedules x 3,000 steps, all replicas converge, no acknowledged write lost
- [ ] E5 Mutation gate for E2: drop the current-term rule, drop log truncation, remove PreVote, allow two lease holders; each must fail the suite. Gate: not yet written
- [ ] E6 WAL and segment checksums, scrub, self-heal from a replica. Gate: flip random bits in a database file, scrub reports and repairs; not yet written
- [ ] E7 Crash fault-injection file layer (power cut at every write boundary). Gate: reopen is consistent at every cut point; not yet written
- [ ] E8 Degraded modes Full, ReadOnly, CacheOnly with the transitions tested. Gate: not yet written
- [ ] E9 The property graph carried on ranges (`uriel-core` over `uriel-kv`). Gate: the whole `uriel-core` suite passes on a 3-replica cluster with one replica killed mid-run; not yet written

## F. Embedding and hardware. Design: `docs/design/CAPABILITIES.md` (to write)

- [ ] F1 `uriel-sys` capabilities and backends: partial. `crates/uriel-sys/src/region.rs:61` (`allocate`), `win.rs:90` (large pages), `winio.rs:207` (IOCP reads), `reader.rs:137` (`read_batch`); 36 tests pass: raw address or size, out-of-range, misaligned and wrong-width access rejected, 20k random capability accesses match a bounds model, unbuffered IOCP reader and forced-buffered reader return bytes equal to the file. **Gaps:** the large-page SUCCESS path never ran (this account lacks SeLockMemoryPrivilege per `whoami /priv`, fallback `NormalPages` / `PrivilegeMissing` verified); the buffered fallback on a failed unbuffered open (`reader.rs:82`) never ran; no non-Windows run. One mutant survived: the bounds check inside `raw` is unreachable through the public API. `unsafe` exists only under `crates/uriel-sys/src/raw/` (28 lines)
- [x] F2 Hot arena and CSR: `crates/uriel-sys/src/arena.rs:51`, `csr.rs:46` (`HotCsr::build`), `csr.rs:152`. Gate: 100 random graphs equal a plain adjacency model (empty, self loops, parallel edges, isolated), 100k random allocations never pass the cap, 64-byte alignment on every slab, two arenas cannot alias; 4 mutants caught
- [ ] F3 Host embedding API for an IDE process (library, plus a named-pipe or shared-memory channel with no copies on the hot path). Gate: round trip with a stub host; not yet written
- [x] F4 Hard cap is a required constructor argument (`Arena::with_cap`, no unlimited default). Measured 256 MiB cap, 1 MiB slabs touched: RSS +268,460,032 B at cap (24,576 B over), reserve alone +12,288 B (control), +0 B after 10,000 refused allocations; private bytes +528,384 B over cap. Gate: `crates/uriel-sys/tests/rss.rs`

## G. MCP surface

- [x] B6 Repeated-value shortcut: `crates/uriel-mcp/src/compact.rs:484` (`encode_features`: `^` run marker, `$a=` value dictionary, ` +` header flag), decoder `:608`. Gate: 2,000 random tables round trip, 2,000 old-format tables still decode, 20,000 adversarial inputs per decoder, expansion-bomb guard. Cypher 2.10x to 2.51x (cl100k proxy)
- [x] B7 Frozen RAG eval corpus: 207 files in `crates/uriel-rag/tests/frozen_corpus/`, queries `tests/eval_queries.json` carry the corpus SHA-256, `run_frozen` `crates/uriel-rag/src/eval.rs:194` refuses drift; `a_drifted_corpus_is_refused` `tests/frozen.rs:106` fails with the check disabled
- [ ] B8 Compact per-hit citation when the gist names the symbol, to close the B4 gap. Gate: citations still resolve (every id expands to the exact lines); B4 re-measured
- [ ] G1 Tools for placement and views: `why_hot(node)`, `pin(node)`, `view(question, budget, focus)`, `replay(log)`, `calibrate()`. Gate: stdio tests like the existing 120; not yet written
- [x] G2 Existing tools adopt `format=compact` and the ladder: `crates/uriel-mcp/src/rag_tools.rs:47` (`format` on rag_search, default compact), `:57` (ladder default on with compact, off with json), `:62` (`fresh`), `:123`. Gate: `crates/uriel-mcp/tests/compact.rs` (12) and `tests/stdio.rs` (51) pass

## H. Verification and hygiene

- [ ] H1 `clippy.toml` (msrv) and `deny.toml` (licences, advisories offline) at the workspace root. Gate: `cargo deny check` and workspace clippy clean; not yet written
- [ ] H2 Criterion benches (code only, no markdown) per tier operation with a regression ceiling asserted in a test. Gate: not yet written
- [ ] H3 Deferred to Phase 2: spec-vs-reality table, written into the blog and paper from this registry (no docs in Phase 1)
- [x] H4 Workspace gate: `cargo test --workspace` 1,193 passed, 0 failed, 6 ignored across 73 binaries; `cargo clippy --workspace --all-targets --no-deps -- -D warnings` clean (2026-10-15, after A, B6, B7, C, D, D6, F landed; uriel-kv has no Raft yet)

## I. Deferred to Phase 2 (do not start)

- [x] I1a Name decided: **Uriel** (owner decision 2026-10-15), mark and lockups in `assets/logo-mark.svg`, `assets/logo-lockup-light.svg`, `assets/logo-lockup-dark.svg`, rationale and limits in `docs/BRAND.md`. Gate: rendered in a browser at 512/300/64/32/16 px and read
- [ ] I1b Rename crates `seal-*` to `uriel-*`, the binary to `uriel-mcp`, update the registered MCP and all docs. Deferred so Phase 1 builds are not editing renamed crates; gate: `cargo test --workspace` count unchanged after the rename
- [ ] I2 Publishing to GitHub (the empty private repo `teerthsharma/seal` exists), README, Pages page
- [ ] I4 CLI crate with binary `uriel`: prints the logo banner (truecolor ANSI flame with tier colours, plain fallback when `NO_COLOR` is set or stdout is not a TTY), `uriel mcp` runs the server, plus `index`, `calibrate`, `doctor`, `--version`. Gate: banner snapshot tests for colour, NO_COLOR and piped output; subcommand smoke tests
- [ ] I5 Release package with a one-line install command that prints the banner (`cargo install` from crates.io, Windows installer script). Gate: clean-machine install in a fresh `CARGO_HOME`
- [ ] I6 Blog post on teerth.blog from this registry (what was built, measured numbers, the misses)
- [ ] I7 Paper on blog.teerth.dev: topological context scheduling, deterministic placement, tiered gated storage; every number with its baseline and the negatives
- [ ] I3 Persona-free public pack: persona text for every role stays an empty string in the public tree; the private pack is kept out of the repo
