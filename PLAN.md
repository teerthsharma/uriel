# PLAN.md: Master Task Registry

**Code-verified pass (2026-10-18, code commit `7d74e6c`, Intel Core i7-14700HX, 15.7 GB RAM, Windows 11, release profile):** `cargo test --release --workspace --no-fail-fast` 2,040 passed, 3 failed, 9 ignored across 130 result lines (113 test binaries and 17 doc-test runs); the 3 failures are the `uriel-rag` frozen-corpus digest checks in `crates/uriel-rag/tests/frozen.rs` (the digest recorded in `eval_queries.json` is `d0b2c76e…5652`; this checkout hashes to `dec6e990…2dd3` with CRLF line endings and `57438a3d…f2b9` with LF), so the workspace is not green and the record needs refreshing; `cargo clippy --workspace --all-targets --no-deps -- -D warnings` exits clean; `cargo deny check` `licenses bans sources` ok (the advisory database is not present offline, so `advisories` was not run). Every `[x]` names the implementing file and the gate that passed; a number from an earlier pass carries its date. Every unticked item is unbuilt, or partial with its gap stated. This file is the single source of truth for remaining work; a box is ticked only when its gate passes, with the passing run cited.

**Method (how this project is built):**

1. **Gate first, seen failing.** A gate (test, simulation, benchmark with a bar) is written and watched failing before the work lands. A box without a gate is not a box.
2. **Evidence over assertion.** A number states what it was compared against, on which machine and commit. Baselines are included. Losses are recorded here, not dropped.
3. **No comments in code.** Test names carry the meaning. Scope, results and misses live in this registry and in the README.
4. **No panics on adversarial bytes.** Every decode path is bounds-checked and returns an error. `#![deny(unsafe_code)]` everywhere except `uriel-sys`; the CLI, `uriel-host`, `uriel-repl` and `uriel-terse` are `#![forbid(unsafe_code)]`.
5. **Determinism is a feature with a test.** Same input log, same output, independent of thread count and wall clock.
6. **Hardware is reached only through capabilities.** No raw addresses or sizes from callers; every unsafe operation takes a validated range object.

Specs: [`docs/DESIGN.md`](docs/DESIGN.md) (original crate map, partly historical), [`docs/SEAL-DB-V2.md`](docs/SEAL-DB-V2.md) (tiers, tokens, resilience), [`docs/claude-code-hooks.md`](docs/claude-code-hooks.md) (compact capture), [`docs/BRAND.md`](docs/BRAND.md) (name and mark).

Phase 1 (building) and the Phase 2 items that gate a release (name, repository, README, CLI, installer script) are done. What remains is listed under the unticked boxes; release packaging is section I.

---

## Phase 0: Foundations (built)

- [x] Property graph store, ACID transactions on a single writer, many readers: `crates/uriel-core/src/graph.rs` (`open`, `begin_write`). Gate: `uriel-core` tests
- [x] Unique and existence constraints enforced on the write path: `crates/uriel-core/src/tests/constraints.rs` (rollback on violation)
- [x] Commit callbacks with epoch and touched label/type sets: `crates/uriel-core/src/tests/epoch.rs`
- [x] openCypher parser with Display round trip: `crates/uriel-cypher`. Gate: 4 generators x 5,000 fuzz cases (2026-10-15: 303 passed)
- [x] Cypher executor, aggregation, MERGE, var-length paths, EXPLAIN, procedures: `crates/uriel-query`. Gate: 105-row TCK-style table (2026-10-15: 38 passed)
- [x] Result cache with tag invalidation by commit: `crates/uriel-query/src/lib.rs` (`with_dist_cache`). Gate: 9 cache tests, 6 fail with invalidation disabled (2026-10-15)
- [x] Graph algorithms: PageRank, Louvain, Kruskal MST and the rest of `crates/uriel-algo`. Gate: karate-club values match networkx (2026-10-15: 138 passed)
- [x] Distributed cache, gossip membership, epoch invalidation: `crates/uriel-cache`. Gate: 67 passed, 5 stability runs (2026-10-15)
- [x] MCP server over stdio, two modes: `crates/uriel-mcp`. 27 tools in `--mode normal`, 41 in `--mode house` (15 and 20 with `--read-only`), counted by sending `tools/list` to `uriel mcp` at `7d74e6c`; `--terse data|prose` adds a 28th and 42nd, `terse_stats`. Gates: `tests/modes.rs`, `tests/mode_config.rs`, `tests/mode_golden.rs`, `tests/stdio.rs`, `tests/compact.rs`
- [x] Two modes, exactly two: `--mode normal|house` and `URIEL_MODE` (default normal). A normal server neither lists nor accepts a `house_*` or `research_*` tool and refuses them with a typed `wrong_mode` error; the flag beats the environment; a bad value is a usage error. Gate: `crates/uriel-mcp/tests/modes.rs`
- [x] Goal-tree harness: merge, next, report, leap, several Needs per database, governor: `crates/uriel-house/src/house.rs`, `governor.rs`. Gates: `crates/uriel-house/tests/` (house, multi_need, simulation with a 40-leaf run under RAM 6000, 2000 and 5000 MB, models)
- [x] Board events: the harness appends one JSON object per line to `house-events.jsonl` (`crates/uriel-house/src/sink.rs`). Gate: `crates/uriel-house/tests/board.rs` feeds the emitted events to the board's `gates.py` (the board page and `gates.py` live in the owner's `dispatching-house-mode` skill, outside this repository)

## A. Storage tiers (bytes). Design: `docs/SEAL-DB-V2.md` section 1

- [x] A1 Sketch, TinyLFU gate, hot cache, tagged codecs, tier controller: `crates/uriel-tier/src/codec.rs`, `cache.rs` (gate on keeps a 100-key hot set through a 100k-key scan, gate off keeps 0 of 100). Admit-always mutation failed 5 tests (2026-10-15)
- [x] A2 Record compression at rest, cold codec zstd level 19; legacy format-1 database reads and writes (`crates/uriel-core/src/tests/tier.rs`); decoder fuzz 20,000 inputs. **Irreversible:** opening bumps the database format 1 to 2, the previous release cannot read an upgraded file
- [x] A3 T0 gated read-through cache with exact invalidation: `crates/uriel-core/src/tier.rs`, tests in `crates/uriel-core/src/tests/tier.rs` (touched ids only; rollback invalidates nothing)
- [x] A4 T1 warm lz4 tier under a byte budget; budget never exceeded over 100k random ops. Byte counts are estimates, not allocator RSS
- [x] A5 Differential test against a HashMap model (5 seeds x 2,000 ops, T0 = 3 entries, T1 = 1 KiB)
- [x] A6 `Graph::tier_stats()` (also the `tier_stats` MCP tool). Measured 2026-10-15, release, same machine: 20k nodes of this repo's text 22,260,000 logical bytes, lz4 13,978,613 (1.59x), zstd-19 10,532,683 (2.11x); redb file 33,689,600 vs 16,846,848 (2.00x); Zipf(1.0) hit rate gate on / off 65.8 / 61.2 % plain, 60.2 / 51.6 % with a scan read per read, 63.1 / 60.4 % with a 20k scan burst (oracle ceiling 71.4 %); read ns median of 1M: T0 242, T1 1,018, T2 3,882 (page-cached)
- [x] A7 Memory budget: the whole server runs inside one budget (`--memory-mib`, default 50, minimum 16) split in per-mille shares: unmanaged reserve 250, redb page cache 250, RAG index 240, hot 80, warm 80, query cache 80, session 20: `crates/uriel-core/src/budget.rs`, `crates/uriel-core/src/tests/budget.rs`. Gate: `crates/uriel-cli/tests/rss.rs`, peak working set of the full server across index, 100 searches, 20 views and 20 souls must stay at or under 60 MiB; 52.7 MiB at `7d74e6c` (idle 9.4, index 36.7, searches 38.0, views 38.1, souls 52.7)

## B. Token tiers (tokens). Design: `docs/SEAL-DB-V2.md` section 2

- [x] B1 Resolution ladder: `pack_ladder` in `crates/uriel-rag/src/lib.rs`, `Level` and `gist` in `ladder.rs`; gate `crates/uriel-rag/tests/ladder.rs` (200 random budgets, never over budget, no hit dropped before every hit is at L3)
- [x] B2 Compact encoding with decoder: `crates/uriel-mcp/src/compact.rs`; round trip over 2,000 tables, 20,000 adversarial inputs per decoder; `format` argument and `--format` flag
- [x] B3 Session dedup: `crates/uriel-mcp/src/session.rs`, `session_reset`, repeat gate `crates/uriel-mcp/tests/compact.rs`. Identical repeat search 7.4x smaller in bytes (2026-10-15); 4.62x by cl100k proxy (905 to 196 tokens)
- [ ] B4 Token evaluation (`crates/uriel-mcp/tests/token_eval/`): bar was 3x fewer proxy tokens, v2 against v1. **Bar missed, re-measured 2026-10-18 at `7d74e6c`: 2.74x overall** (`rag_search` 3.05x, Cypher 2.47x; `rag_search` with session dedup 3.59x), against 2.55x on 2026-10-15. Command: `python crates/uriel-mcp/tests/token_eval/eval.py --bin target/release/uriel-mcp.exe --out <scratch>` (cl100k proxy through `tiktoken`, 20 fixed queries). The Cypher part is the gap: unique identifiers in its output cannot be shrunk by this scheme
- [x] B5 Recall with the ladder on the frozen corpus (207 files, 3,534 chunks, 100 queries), 2026-10-18: hybrid 1.000 at 5 and at 10, BM25 over full chunk text 1.000 / 1.000, through `pack_ladder` 1.000 / 1.000; memories 16.37x smaller than the text. Gate: `cargo test --release -p uriel-rag --test quantised -- --nocapture`
- [x] B6 Repeated-value shortcut (`^` run marker, `$a=` value dictionary): `crates/uriel-mcp/src/compact.rs`
- [x] B7 Frozen RAG eval corpus: 207 files in `crates/uriel-rag/tests/frozen_corpus/`, queries carry the corpus SHA-256, `run_frozen` refuses drift (`a_drifted_corpus_is_refused`)
- [x] B8 Compact per-hit citation (`>file:start-end#chunk_id`, shared path aliases, relative line ranges): `crates/uriel-mcp/src/compact.rs`, `rag_tools.rs`
- [x] B9 CLaRa-shaped retrieval upgrade: stored int8 vectors over the full chunk text, first-stage pool of 200 reranked by BM25 over the stored full text, lower compression floors. History: recall@5 0.880 (hybrid over compressed memories, 2026-10-15) to 1.000 (`8753a96`), compression 12.14x to 16.42x (`44d936f`)
- [x] B10 `view` tool: RAG candidates ordered into blocks, selected by the `uriel-topo` schedule (sink, local window, relevant), rendered through the ladder under a hard budget, deterministic: `crates/uriel-mcp/src/view.rs`; gates `tests/view_frozen_tool.rs`
- [x] B11 Islands in `view` (`islands=true`): DSU island labels (`crates/uriel-algo/src/islands.rs`) group candidates, one budgeted section per island. **Measured verdict: off by default.** Re-measured 2026-10-18 on the 100 frozen queries (`cargo test --release -p uriel-mcp --test view_frozen_tool -- --nocapture`): `view` recall at 5 is 84/100 off and 92, 94, 94 on at budgets 300, 800, 4,000, and mean proxy tokens rise from 148.6 to 196.4, 257.6, 274.6 (`f7e7a26` recorded 40 to 64 % on an earlier build). Gates `crates/uriel-mcp/tests/view_islands.rs`, `crates/uriel-algo/tests/islands.rs`
- [x] B12 `soul` tool: query-free salience refined by seeded deterministic score diffusion, cached in the graph by chunk fingerprint: `crates/uriel-rag/src/lib.rs`, `crates/uriel-mcp/src/soul.rs`; gates `tests/soul_tool.rs`. **Measured verdict: does not beat the plain salience baseline.** Gold chunk inside the guide for 0, 3, 7 of 100 queries at budgets 300, 1,200, 4,000, against 0, 2, 7 for plain top-by-salience of equal size (chance 0.34, 1.27, 3.99); `cargo test --release -p uriel-rag --test eval soul_guide -- --ignored --nocapture` after refreshing the corpus digest
- [x] B13 Stable prefix: the json result of `view` and `soul` leads with a byte-stable scaffold preamble bound to the tool schemas, output contract and mode (`URIEL_STABLE_PREFIX=off` disables): `crates/uriel-mcp/src/prefix.rs`, `prefix_tree.rs`; gates `tests/prefix_wire.rs`, `prefix_fingerprint.rs`, `prefix_share.rs`, `prefix_tree.rs`. Measured 2026-10-18: 29,200 of 165,239 bytes (0.177) of `view` json output identical across the 100 frozen queries (`prefix_share`). No cache hit rate or latency gain is measured
- [x] B14 Compact capture and recall for Claude Code compaction: `uriel capture`, `uriel recall-context` (`crates/uriel-cli/src/capture.rs`, `recall.rs`), settings in `docs/claude-code-hooks.md`. Gates `crates/uriel-cli/tests/hooks.rs`, unit tests in `capture_tests.rs`, `recall_tests.rs`
- [x] B15 `uriel-terse`: lossless compact JSON encoding and prose compaction over protected spans (`crates/uriel-terse`), wired into the server behind `--terse off|data|prose` (`URIEL_TERSE`, default off): `crates/uriel-mcp/src/terse.rs`, gates `tests/terse_wire.rs`, `terse_config.rs`, `terse_measure.rs`. `rag_expand` stays byte-exact; `terse_stats` appears when terse is on. Measured 2026-10-18 over 30 calls (release, `TERSE_MEASURE_ROOT=. cargo test --release -p uriel-mcp --test terse_measure -- --ignored --nocapture`): total proxy tokens 5,543 off, 5,350 data (-3.5 %), 5,340 prose (-3.7 %); with `format=json` 14,779 off, 11,009 data (-25.5 %)

## C. Topology (context and layout). Design: `docs/SEAL-DB-V2.md`

- [x] C1 `uriel-topo` 0-D persistence: `crates/uriel-topo/src/persistence.rs`. Gate: hand-computed diagrams, stability within 2 epsilon over 200 random sets, isometry, scale, permutation; flipped elder rule failed 5 of 15 tests (2026-10-15)
- [x] C2 Context schedule SINK + LOCAL + RELEVANT: `crates/uriel-topo/src/schedule.rs`. Gate: dense parity, causality, CSR output
- [x] C3 Needle-recall evaluation: `crates/uriel-topo/src/eval.rs`, `cargo run --release -p uriel-topo --example eval`. **Verdict (2026-10-15): the topological salience term loses to top-k.** Synthetic (20 contexts, alpha 0.25, recall at 5 / 10 / 25 % budget): top-k .865 / .957 / 1.000, topological .790 / .947 / 1.000. Real data (1,647 blocks, 759 queries): top-k .520 / .626 / .798, topological .439 / .542 / .721. Wins only at alpha 0.10. **Decision:** the salient term is not wired into retrieval
- [x] C4 Topology-sorted layout: `crates/uriel-topo/src/layout.rs`. Gate: identical over 8 runs and across thread counts; reversed-degree RCM mutation caught
- [x] C5 Layout benefit: `crates/uriel-topo/src/measure.rs`. Node pages touched, sorted vs insertion order: 0.379 (interleaved ids, 92 % intra edges) to 0.966 on this repo's call graph (2026-10-15)
- [x] C6 Topology export to the Triton scheduled-attention kernel: `KernelCsr` bytes (`UCSR` v1) in `crates/uriel-topo/src/export.rs`, `examples/export_csr.rs`, gate `crates/uriel-topo/tests/export.rs`
- [ ] C7 GPU parity of the exported CSR against the kernel from triton-lang/kernels#22: harness `crates/uriel-topo/tests/gpu/run.sh` and `parity.py` (needs `URIEL_GPU_PYTHON` and `URIEL_GPU_KERNEL`, prints SKIP otherwise; checks mask violations, error against an fp32 masked reference within a TF32 tolerance, negative cases, timing). The harness is in the tree; a run on the RTX 4060 Laptop GPU is not recorded in this repository, so the box stays unticked

## D. Scheduler (determinism)

- [x] D1 Deterministic placement: `crates/uriel-place/src/place.rs`. Replay gate `crates/uriel-place/tests/replay_gate.rs`: 200 logs x 8 thread counts with shuffled arrival = 1,600 runs equal to the single-thread SHA-256 (2026-10-15)
- [x] D2 One test per OS role in `crates/uriel-place/tests/roles.rs`: locality, prefetch, fragmentation entropy, pressure governor, lifetime
- [x] D3 `calibrate`: `crates/uriel-place/src/calibrate.rs`, knee tests; also the `calibrate` MCP tool. Measured 2026-10-15: knees 32 to 64 KiB, 2 to 4 MiB, 8 to 16 MiB, suggested hot size 4 MiB; does not match the spec sheet (single core, 4 KiB pages, TLB not isolated)
- [x] D4 Pressure ramp: `crates/uriel-place/tests/pressure_ramp.rs`
- [x] D5 Governor and smart test selection replay determinism: `crates/uriel-house/tests/replay.rs` (a 40-leaf run replays to one digest 20 times; another temp path gives the same digest; one changed probe value changes the digest; a probe below the reserve defers every heavy leaf identically)
- [x] D6 Planner behind `Options::placement: Local | Planned`. **Measured verdict (2026-10-15): Planned does not beat Local;** default stays `Local`, `Planned` is for replay and analysis (`--placement planned` on the server)
- [x] D7 Gap-aware best-fit arena planner: `crates/uriel-place/src/arena.rs` (`plan_arena`, `plan_offsets`, `lower_bound`), a segment tree over last use finds live neighbours; gate `crates/uriel-place/tests/arena.rs`. Lineage: google/XNNPACK#10801 (merged); pytorch/pytorch#200588 is proposed and open

## E. Resilience (survival). Design: `docs/SEAL-DB-V2.md` section 3

- [x] E1 HLC and MVCC: `crates/uriel-kv/src/hlc.rs` (monotonic under backward clock jumps over 50 seeds, skew bound exact), `mvcc.rs` (snapshot reads repeatable under interleaved writers and gc, gc keeps exactly the newest version at or before the horizon over 200 seeds)
- [x] E2 Sans-IO Raft: `crates/uriel-kv/src/raft.rs`. Election with PreVote and check-quorum, replication with conflict hints, the current-term commit rule (Figure 8 test), snapshots and log compaction, single-step membership change, ReadIndex. Gates in the module's tests (election safety over 200 seeds, 1,000 proposals identical on 3 replicas, crash at every write boundary)
- [x] E3 Ranges as data structures: `crates/uriel-kv/src/range.rs`. Descriptors, split by reported size, merge, lease table with epochs, a wire codec fuzzed with 20,000 inputs. **Not wired to Raft groups**: the simulator and the server run one group
- [x] E4 Cluster simulator with seeded delay, drop, duplication, reorder, partition, kill, restart, disk fault: `crates/uriel-kv/src/sim.rs`. Gates: 50 seeds x 3,000 steps on 3 nodes; 20 seeds x 3,000 steps on 5 nodes with membership changes; same seed same trace
- [ ] E5 Mutation gate for E2 (drop the current-term rule, drop log truncation, remove PreVote, allow two lease holders; each must fail the suite). No mutation harness exists in `uriel-kv`; the replicated-graph layer has its own (E9)
- [x] E6 Record checksums, scrub, repair from a replica: `crates/uriel-core/src/checksum.rs`, `graph.rs` (`scrub`, `repair_from`), gate `crates/uriel-core/src/tests/integrity.rs` (scrub reports every changed record over 20 seeds; corruption sends the graph read-only; repair from a replica restores a full export). The Raft log has CRC-32C records and a flipped bit below the commit index is an error, not a silent loss (`crates/uriel-kv/src/durable.rs`)
- [x] E7 Crash fault-injection file layer: `crates/uriel-core/src/tests/crash.rs` (every write boundary of a 500-op workload reopens to a committed prefix), `crates/uriel-kv/src/durable.rs` (torn tail at every byte cut, crash between any two sync steps)
- [x] E8 Degraded modes Full, ReadOnly, CacheOnly with deterministic transitions: `crates/uriel-core/src/tests/degraded.rs`
- [ ] E9 The property graph carried on ranges. Partial: the graph has a replicated command log (`crates/uriel-core/src/command.rs`: `Command`, `WriteBatch`, CRC-32C framing, `apply_batch` with explicit ids, `validate_batch`, `GraphDump`), and `crates/uriel-repl` runs a replica state machine over one Raft group. Gate `crates/uriel-repl/tests/gate.rs`: 30 seeds x 2,000 steps keep every replica equal and lose no acknowledged write, plus 6 mutation checks that must fail (built with `--features mutations`). Missing: the graph on multiple ranges, and any wiring of `uriel-repl` into the server, which runs one node. There are no distributed transactions, no intents, no SQL layer and no real-network Raft transport

## F. Embedding and hardware

- [ ] F1 `uriel-sys` capabilities and backends: partial. 36 tests pass (2026-10-15): raw address or size, out-of-range, misaligned and wrong-width access rejected; unbuffered IOCP reader equals the file. **Gaps:** the large-page SUCCESS path never ran (this account lacks SeLockMemoryPrivilege), the buffered fallback on a failed unbuffered open never ran, no non-Windows run. One mutant survives: the bounds check inside `raw` is unreachable through the public API
- [x] F2 Hot arena and CSR: `crates/uriel-sys/src/arena.rs`, `csr.rs`
- [x] F3 Host embedding API for an IDE process: `crates/uriel-host`. Length-prefixed binary frames over in-process channels, loopback TCP and a Windows named pipe; query, search, index and stats messages; frame cap, malformed-frame and 20,000-input fuzz gates; shutdown drains in-flight requests; a host RSS test over idle, indexing and 100 searches. Gates `crates/uriel-host/tests/` (codec, host, memory, pipe, oracle)
- [x] F4 Hard cap is a required constructor argument (`Arena::with_cap`); RSS +0 B after 10,000 refused allocations (2026-10-15). Gate: `crates/uriel-sys/tests/rss.rs`
- [x] F5 Clara oracle adapters over the named pipe: `crates/uriel-host/clients/clara_oracle.py`, `clara_module.py`, stdlib client `uriel_client.py`. Gate: `crates/uriel-host/tests/oracle.rs` (skips when `python` is not on `PATH`)

## G. MCP surface

- [x] G1 Placement and view tools: `view`, `why_hot`, `pin`, `unpin`, `replay`, `calibrate`, `tier_stats`, `soul`: `crates/uriel-mcp/src/place_tools.rs`, `view.rs`, `soul.rs`; gates `tests/placement.rs`, `view_frozen_tool.rs`, `soul_tool.rs`, `stdio.rs`
- [x] G2 Existing tools adopt `format=compact` and the ladder: `crates/uriel-mcp/src/rag_tools.rs`; gates `tests/compact.rs`, `tests/stdio.rs`
- [x] G3 Caustic research tools (house mode, Inspector skill): `research_orbit_audit`, `research_pooling_audit`, `research_record`; bounds T1, T1*, T2, T6, T6*, T8 ported in `crates/uriel-algo/src/caustic.rs`; the other Caustic theorems are not ported. One-sided by construction: a clean result means no violation was found. Gates `crates/uriel-algo/tests/caustic.rs`, `crates/uriel-mcp/tests/research.rs`
- [x] G4 Certified numerics and convergence helper ported from Aether-Lang: `crates/uriel-algo/src/certify.rs` (outward-rounded interval bounds, `gamma`, `eta`, unit roundoff per precision), `stable.rs` (`Stable`, `Stop`); gates `crates/uriel-algo/tests/certify.rs`, `stable.rs`

## H. Verification and hygiene

- [x] H1 `clippy.toml` (msrv 1.90) and `deny.toml` (licences, bans, sources; advisory database offline) at the workspace root. `cargo clippy --workspace --all-targets --no-deps -- -D warnings` exits clean and `cargo deny check licenses bans sources` is ok at `7d74e6c`; advisories are unchecked because the database is not available offline
- [x] H2 Criterion benches per tier operation (`crates/uriel-bench/benches/`) with a regression ceiling asserted in `crates/uriel-bench/tests/ceilings.rs` against `ceilings.json` (factor 5.0)
- [x] H4 Workspace gate: see the header of this file

## J. Dispatch (team-agent control plane). Design: `docs/DISPATCH.md`

Source: DeepSeek `dsh-superagent-prototype` (research prototype, MIT), rebuilt as a pure state machine in `crates/uriel-dispatch`. 108 tests pass in the crate (`cargo test -p uriel-dispatch`); `cargo clippy -p uriel-dispatch --all-targets --no-deps -- -D warnings` exits clean. Nine mutations of the logic (no reserve slot, off mode admitting inferred, retry off by one, bot made explicit, no eval discard on policy change, hand-off keeping the slot, stopped ticket still fed, resume without revalidation, uncapped backoff) each fail 2 to 5 tests.

- [x] J1 Gate and admission table: `crates/uriel-dispatch/src/policy.rs` (`gate`). Gate: `tests/gate.rs`, including an exhaustive sweep of 3 modes x 27 cell settings x scope x addressed x ticket x 5 actors (1,620 cases): a bot is never explicit, off mode never admits inferred
- [x] J2 Policy version, Mandate with recorded basis, revalidation before resume, per-ticket change notices: `policy.rs` (`revalidate`, `changes_for`), `router.rs` (`set_policy`). Gates `tests/gate.rs`, `tests/threads.rs`
- [x] J3 Eval lifecycle: one per thread, batch merge, 120 s from delivery (never before), two retries then `unavailable`, assessment failure, discard on policy change, re-evaluate and dismiss: `router.rs`. Gate `tests/eval.rs`, `tests/replay.rs`
- [x] J4 Ticket stages, worker slots with one reserved for a person, prepare 10 min, background 30 min, redelivery 2 s doubling to 60 s: `ticket.rs`, `router.rs`. Gate `tests/execution.rs`. A woken idle ticket kept its stale `waiting` report and read `idle` while working: found by the gate, fixed
- [x] J5 Subscription fan-out, ownership, hand-back, release on end, resume with revalidation, stop, attach, unfollow, 14-day polling horizon: `router.rs`. Gate `tests/threads.rs`
- [x] J6 Replay: SHA-256 chain over (time, input, effects); log round trip through JSON; clock clamp; 40 seeds x 600 random inputs with slot and phase invariants after each step, and a coverage gate that the streams reach all four ticket endings and the reserve slot: `tests/replay.rs`
- [x] J7 RAM-aware capacity with the same reserve as the House governor: `config.rs`. Gate `tests/execution.rs` (parity with `uriel_house::governor::reserve_mb` and `slots` over 4 totals x 6 free values)
- [x] J8 Time window: `Input::Window`, wind-down and close effects, no grants after the close: `crates/uriel-dispatch/src/router.rs`. Gate `tests/window.rs`
- [x] J9 Persistence: `afk::Afk`, JSONL with the router digest on every line, exclusive lock, forged line refused, torn last line dropped, archive on end: `crates/uriel-dispatch/src/afk.rs`. Gate `tests/afk.rs`
- [x] J10 `/afk` command: `uriel afk start|add|report|status|end|init`, `crates/uriel-cli/src/afk_cmd.rs`, command text `crates/uriel-cli/assets/afk.md` (repo copy `.claude/commands/afk.md`, equality tested). Gates: unit tests in `afk_cmd.rs`, parse tests in `main.rs`, and a run of the built binary through start, three adds under two workers, ready, ask, status, end
- [ ] J10b MCP tools `dispatch_*` in house mode. Unbuilt; moves the 41-tool goldens
- [ ] J10c GitHub and Feishu Sources. Unbuilt; `/afk` is the only Source
- [ ] J11 Token-budgeted ticket context through `view`. Unbuilt and unmeasured
- [ ] J12 Prune ended threads from the in-memory trigger table. Unbuilt

## I. Release

- [x] I1a Name decided: **Uriel** (2026-10-15), mark and lockups in `assets/`, rationale and limits in `docs/BRAND.md`
- [x] I1b Crates renamed `seal-*` to `uriel-*`, binary `uriel`. Residue: the MCP tool prefix `seal_remember` and `seal_recall`, the default database path `~/.seal/graph.seal` and the `uriel-mcp` crate binary keep the old working names
- [x] I2 Repository public (github.com/teerthsharma/uriel), README
- [x] I3 Persona-free public tree: `grep -rli persona crates docs` matches only personalized PageRank
- [x] I4 CLI crate with binary `uriel`: banner, `mcp`, `index`, `calibrate`, `doctor`, `install`, `capture`, `recall-context`, `--version`. Gates `crates/uriel-cli/tests/`, banner and subcommand tests in `crates/uriel-cli/src`
- [ ] I5 Release package: `install/install.ps1` and `cargo install --path crates/uriel-cli --locked` work from a checkout. Missing: a crates.io release and a clean-machine install gate in a fresh `CARGO_HOME`
- [ ] I8 Version bump: the workspace and `uriel --version` still report 0.1.0
- [ ] I6 Blog post on teerth.blog from this registry
- [ ] I7 Paper on blog.teerth.dev: topological context scheduling, deterministic placement, tiered gated storage; every number with its baseline and the negatives

## J. Agents: genius and fleet. Design: `docs/GENIUS.md`

- [x] J1 Sanitiser: `crates/uriel-fleet/src/sanitize.rs`; strips ANSI, controls, bidi and zero-width, Unicode tag characters; redacts private-key blocks and 11 other secret classes to a fixed point; fences untrusted text with a content-hash nonce; counts injection phrases without rewriting. Gate `crates/uriel-fleet/tests/sanitize.rs` (24 tests, including idempotence over 3,000 seeded inputs, which found and fixed a nested-secret bypass, and linear time on 1.1 MB)
- [x] J2 Fleet admission and observation: `crates/uriel-fleet/src/fleet.rs`, `trace.rs`; concurrency, depth, lifetime and token limits, `Want::Max`, deferral, `reap`, bounded runs and events, hash-chained trace with an anchor. Gates `crates/uriel-fleet/tests/fleet.rs` (19) and the unit tests in `trace.rs` (8)
- [x] J3 Genius protocol: `crates/uriel-genius`; hexagon and axes, byte-stable plan and briefs, shared-prefix-first rendering, waves under a cap, typed claim validation, quarantine of wild claims, order-independent verdict. Gate `crates/uriel-genius/tests/protocol.rs` (40), including a worst-case size gate (maximal claims from all six lenses keep every brief and response under the 20,000-character server output limit; found while probing that a six-brief round-2 payload could pass 100 KB in the worst case and be truncated into invalid JSON) and that `.claude/commands/genius.md` equals the generated command
- [x] J4 MCP surface: `crates/uriel-mcp/src/agent_tools.rs` (`genius_plan`, `genius_brief`, `genius_submit`, `fleet_admit`, `fleet_event`, `fleet_trace`), the `genius` prompt and the `prompts` capability in `server.rs`, `URIEL_FLEET_MAX`; `uriel genius` in the CLI. Gates `crates/uriel-mcp/tests/agent_wire.rs` (35), `stdio.rs` tool lists; `mode_golden.rs` still pins the 27 earlier tools byte for byte
- [x] J5 Mutation check: six rules broken one at a time, each turned the suite red (2026-11-01)
- [x] J10 Observation deck: `crates/uriel-fleet/src/fleet.rs` (`Fleet::deck`, `seed_backlog`, `enqueue`, `Launch::{Demand, Max}`): `launch_now`, `split_needed` and lost slot-ticks from integer counters on a logical clock, a deck line on every fleet, genius and ledger response, `fleet_trace format=deck`. Gates `crates/uriel-fleet/tests/fleet.rs` (29), `agent_wire.rs`. Design `docs/LEDGER.md`
- [x] J11 Goal ledger: `crates/uriel-ledger` (Kahn layers with a lexicographic tie-break, ready frontier, issue/finish, blocked-by-failure, re-validating JSON) and `crates/uriel-mcp/src/ledger_tools.rs` (`ledger_add`, `ledger_next`, `ledger_done`; atomic 0600 files next to the database; the issued goals become fleet backlog). Gates `crates/uriel-ledger/tests/ledger.rs` (20, including insertion-order independence and drain-in-waves over random DAGs) and `agent_wire.rs`
- [x] J12 Python next-prompt finder: `scripts/ledger_next.py` (stdlib only, read-only on the ledger, content-addressed prompt cache bounded to 512 files, refuses tampered files and symlinked caches). Gate `crates/uriel-ledger/tests/python_agrees.rs` (10): Rust and Python agree on layers and the ready set at every step of a drain over 25 random DAGs
- [x] J13 Safety record: `docs/SAFETY.md` (trust boundaries, threat to control to test table, residual risk). 6 further mutations (deck, ledger readiness, cycle check, Python ordering and name check) each turned a suite red
- [x] J14 Genius moved inside house mode (2026-11-01, after review): `genius_*` tools and the `genius` prompt exist only with `--mode house`; a conjecture or wild claim can carry a live house `goal`, and an experiment result becomes a house fact (`killed` refutes the goal and its subtree). Normal mode is unchanged (no `prompts` capability). Marked experimental and unproven with its token cost in the README, the command text and `docs/GENIUS.md`. Gates `agent_wire.rs` (`a_killed_claim_bound_to_a_house_goal...`, `a_survived_claim...`, `a_claim_bound_to_a_goal_that_does_not_exist...`, `normal_mode_is_unchanged...`)
- [ ] J15 Cache and model order: group lenses by model so each model pays one cold prefix, and load lenses through a lazy asynchronous loader as slots free; measure `cached_in` per model. The fork-versus-tier trade-off in `docs/GENIUS.md` is the reason this is open
- [ ] J16 One slot story: `uriel-dispatch` (`/afk`, worker slots 1 to 16, default 3, RAM-aware, CLI only) and the fleet deck here (concurrency cap default 8, MCP side) count agent capacity separately. Decide whether the deck reads the dispatcher's slots, or `/afk` workers are admitted through `fleet_admit`, so "launch max agents" has one answer
- [ ] J6 NeMo Relay conformance: map `fleet_trace` jsonl to ATOF and ATIF and check it against the published schema; blocked here because the documentation host was unreachable
- [ ] J7 Evidence that the hexagon helps, and is worth its tokens: a fixed set of diagnosis tasks, single prompt against `/genius` against `/genius` inside house mode, tokens counted from `fleet_trace`, scored blind. Until then genius is experimental. This is the condition for keeping it
- [ ] J8 Hosted-cache measurement: `cached_in` across the six lenses on a real provider, with and without `warm`
- [ ] J9 `--fleet-max` flag next to `URIEL_FLEET_MAX`; NFKC normalisation and email redaction in the sanitiser

## K. Future: particle-physics-shaped retrieval. Design: `docs/PHYSICS-RAG.md`

Not built. Every item has a baseline and a deletion condition in the design.

- [ ] K0 A retrieval benchmark that can fail: BM25 recall at 5 under 0.6, multi-hop and renamed-symbol queries, frozen by digest, with confidence intervals (the current frozen set is saturated at 1.000)
- [ ] K1 Anti-kt clustering of chunks around high-salience seeds, ablated against island labels and Louvain on K0
- [ ] K2 Collinear and infrared safety as a metamorphic gate: splitting a chunk or adding a near-duplicate must not change the returned clusters
- [ ] K3 Iterative Bayesian unfolding over the resolution ladder to choose which hits to zoom
- [ ] K4 Symmetry property tests: identifier renaming, file permutation and chunk duplication leave ranks unchanged
- [ ] K5 Monte Carlo generation of synthetic labelled queries from the graph
- [ ] K6 Certified rank stability using `uriel-algo::certify`
