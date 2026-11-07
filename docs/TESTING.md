# Testing

Uriel is a graph store, a retrieval layer and a deterministic scheduler, served to AI agents over MCP. This page is for someone who wants to run the tests, add a gate, or check whether a number in the README still holds. It covers the gate method, where each crate's tests live, the slow suites, the lint and supply-chain commands, the CI workflows, the end-to-end audit script, and the recorded results. Results are dated and tied to a commit and a machine. Some recorded checks are failing, and this page says which.

## Contents

- [Quick start](#quick-start)
- [The gate method](#the-gate-method)
- [Test layout by crate](#test-layout-by-crate)
- [Running focused tests](#running-focused-tests)
- [Heavy suites](#heavy-suites)
- [Lint and supply chain](#lint-and-supply-chain)
- [CI workflows](#ci-workflows)
- [End-to-end audit: scripts/audit_mcp.py](#end-to-end-audit-scriptsaudit_mcppy)
- [Recorded results](#recorded-results)
- [Known gaps and failing checks](#known-gaps-and-failing-checks)
- [See also](#see-also)

## Quick start

`Cargo.toml` sets `rust-version = "1.95"`, and `clippy.toml` sets `msrv = "1.95"`. CI builds with Rust 1.97.0. Python 3 is needed only for the audit script, the token evaluation and the optional oracle tests.

```bash
cargo clippy --workspace --all-targets --no-deps -- -D warnings
cargo deny check licenses bans sources
cargo test --workspace --no-fail-fast
```

The first two are the lint and supply-chain commands CI runs. The third is the CI `test` command, whose current state is in [Known gaps and failing checks](#known-gaps-and-failing-checks). These commands are the same on Windows, Linux and macOS.

## The gate method

`PLAN.md` states the method. Every claim is a checkbox, and a box is ticked only when its gate passes. The rules, in the order `PLAN.md` gives them:

1. **Gate first, seen failing.** A gate (a test, a simulation, or a benchmark with a bar) is written and watched failing before the work lands. "A box without a gate is not a box."
2. **Evidence over assertion.** A number states what it was compared against, on which machine and at which commit. Losses are recorded in `PLAN.md`, not dropped.
3. **No comments in code.** Test names carry the meaning.
4. **No panics on adversarial bytes.** Every decode path is bounds-checked and returns an error. `#![deny(unsafe_code)]` applies everywhere except `uriel-sys`. The CLI, `uriel-host`, `uriel-repl` and `uriel-terse` use `#![forbid(unsafe_code)]`.
5. **Determinism is a feature with a test.** The same input log gives the same output, whatever the thread count or wall clock.
6. **Hardware is reached only through capabilities.** Callers pass no raw addresses or sizes. Each unsafe operation takes a validated range object.

### Mutation checks

A mutation check breaks one load-bearing rule on purpose and confirms that the suite turns red. The recorded ones:

| Crate | Mutation | Recorded result | Source |
|---|---|---|---|
| `uriel-tier` | cache admits every key | 5 tests failed (2026-10-15) | `PLAN.md` A1 |
| `uriel-topo` | flipped elder rule in 0-D persistence | 5 of 15 tests failed (2026-10-15) | `PLAN.md` C1 |
| `uriel-topo` | reversed-degree ordering in the layout | caught | `PLAN.md` C4 |
| `uriel-query` | result cache with tag invalidation disabled | 6 of 9 cache tests fail | `PLAN.md`, Phase 0 |
| `uriel-dispatch` | nine logic mutations (no reserve slot, retry off by one, uncapped backoff, and six more) | each fails 2 to 5 tests | `PLAN.md` J (dispatch) |
| fleet, genius, ledger | six rules broken, then six more (deck, ledger readiness, cycle check, Python ordering) | each turned a suite red (2026-11-01) | `PLAN.md` J5, J13 (agents) |
| `uriel-repl` | six seeded mutations behind the `mutations` feature | each caught | `PLAN.md` E9 |
| `uriel-sys` | a bounds check inside `raw` | one mutant survives: the check is unreachable through the public API | `PLAN.md` F1 |

Run the `uriel-repl` mutations with:

```bash
cargo test --release -p uriel-repl --features mutations
```

`uriel-kv` has no mutation harness. `PLAN.md` E5 lists the four mutations it needs (drop the current-term commit rule, drop log truncation, remove PreVote, allow two lease holders) and leaves that box unticked.

## Test layout by crate

The workspace has 23 members (`Cargo.toml`). Integration tests live in each crate's `tests/` directory. Module tests live under `src/`. The table is not exhaustive.

| Crate | Where the tests are | What they cover |
|---|---|---|
| `uriel-core` | `src/tests/*.rs` | Constraints, epochs, tier cache, crash reopening, degraded modes, integrity and scrub, codec, command log |
| `uriel-cypher` | `tests/parse.rs`, `edge.rs`, `fuzz.rs` | Parser, Display round trip, four fuzz generators |
| `uriel-cache` | `tests/cluster.rs` | Distributed cache and gossip membership |
| `uriel-algo` | `tests/` (`caustic`, `certify`, `community`, `islands`, `stable`, and others) | Graph algorithms and certified numerics |
| `uriel-query` | `tests/` (`cache`, `command_log`, `read`, `write`, `tck`, and others) | Executor and cache; `tck.rs` is the table-driven Cypher suite |
| `uriel-mcp` | `tests/` (`stdio`, `modes`, `mode_golden`, `revamp`, `agent_wire`, `compact`, `terse_*`, `prefix_*`, `view_*`, and others), `tests/token_eval/` | The server over stdio, both modes, compact encoding, stable prefix; the Python token harness |
| `uriel-rag` | `tests/` (`frozen`, `eval`, `ladder`, `quantised`, `rerank`, `spill`, and others), `tests/frozen_corpus/` | Retrieval, the resolution ladder, the frozen evaluation corpus |
| `uriel-house` | `tests/` (`house`, `multi_need`, `replay`, `simulation`, `board`, `models`) | The goal-tree harness, governor, replay digests |
| `uriel-dispatch` | `tests/` (`gate`, `eval`, `execution`, `replay`, `threads`, `window`, and others) | The team-agent router: gate table, eval lifecycle, replay, time window |
| `uriel-tier` | `src/codec.rs`, `src/cache.rs` | Tagged codecs, TinyLFU gate, budget |
| `uriel-kv` | module tests in `src/` (`hlc`, `mvcc`, `raft`, `range`, `durable`, `sim`) | HLC and MVCC, sans-IO Raft, the seeded cluster simulator |
| `uriel-repl` | `tests/gate.rs`, `state_machine.rs`, `common/mod.rs` | Replicated graph over one Raft group |
| `uriel-topo` | `tests/` (`eval`, `export`, `layout`, `measure`, `persistence`, `schedule`) | Persistence, schedule, layout, CSR export |
| `uriel-place` | `tests/` (`replay_gate`, `arena`, `calibrate_knees`, `pressure_ramp`, `roles`, and others) | Deterministic placement and replay, arena planner |
| `uriel-sys` | `tests/` (`arena`, `csr`, `reader`, `region`, `rss`) | Capabilities, arena with a hard cap, IOCP reader |
| `uriel-host` | `tests/` (`codec`, `host`, `memory`, `oracle`, `pipe`), `oracle_driver.py` | Frame codec, named pipe, memory, the Clara oracle |
| `uriel-bench` | `tests/ceilings.rs` with `ceilings.json`; `benches/` | Regression ceilings; Criterion benches |
| `uriel-cli` | `tests/hooks.rs`, `tests/rss.rs`; `src/capture_tests.rs` | Compaction hooks, the full-server memory gate |
| `uriel-terse` | `tests/` (`data`, `prose`, `protected`, `tokenizer`, and others) | Lossless compaction, protected spans, token counter |
| `uriel-fleet` | `tests/fleet.rs`, `sanitize.rs` | Admission, observation deck, sanitiser |
| `uriel-genius` | `tests/protocol.rs` | Plan and brief bytes, claim validation, verdict |
| `uriel-ledger` | `tests/ledger.rs`, `python_agrees.rs` | Layers and ready frontier; agreement with `scripts/ledger_next.py` |
| `uriel-page` | `tests/` (`a11y`, `caps`, `determinism`, `docs_pages`, `security`) | Deterministic HTML pages; `docs_pages` keeps `docs/pages/` in step |

Notes:

- `crates/uriel-rag/tests/frozen_corpus/` stores evaluation files with a `.frozen` suffix. They are retrieval data, not tests that run.
- `crates/uriel-host/tests/oracle.rs` skips when `python` is not on `PATH` (`PLAN.md` F5), so a machine without Python does not exercise it.

## Running focused tests

Run one crate while you work on it. Filter by test name after the crate and test target.

```bash
cargo test -p uriel-kv
cargo test -p uriel-core crash
cargo test -p uriel-mcp --test revamp
cargo test -p uriel-cypher --test fuzz fuzz_token_soup_5000
cargo test -j 4 -p uriel-dispatch --test gate
```

`-j` and `--jobs` are cargo's flags for parallel compile jobs. `--test-threads` is the libtest flag for test threads; `--test-threads=1` is what the README uses for the RSS gate.

Use `--release` in three cases:

- For memory and timing tests. The full-server RSS test asserts only in release builds (`crates/uriel-cli/tests/rss.rs`, `if !cfg!(debug_assertions)`).
- For every test whose number the README or `PLAN.md` quotes.
- Not for logic tests. The release profile uses thin LTO and one codegen unit (`Cargo.toml`), which slows compilation.

Measurement tests are `#[ignore]`d, so a plain `cargo test` skips them. Run them by name:

```bash
cargo test --release -p uriel-rag --test eval soul_guide -- --ignored --nocapture
TERSE_MEASURE_ROOT=. cargo test --release -p uriel-mcp --test terse_measure -- --ignored --nocapture
```

Windows PowerShell, for the RSS test with a fixed database directory:

```powershell
$env:URIEL_RSS_DB_DIR = "$env:TEMP\uriel-rss"
cargo test --release -p uriel-cli --test rss -- --nocapture --test-threads=1
Remove-Item Env:URIEL_RSS_DB_DIR
```

## Heavy suites

### Memory: `uriel-cli/tests/rss.rs`

The file builds the release `uriel` binary (`CARGO_BIN_EXE_uriel`) and measures its working set. `BUDGET_MIB` is 50 and is also the `--memory-mib` value the server runs with. `PEAK_LIMIT_MIB` is 60 and `PEAK_TARGET_MIB` is 56, the 4 MiB margin under the limit. `SEARCHES` is 100, `VIEWS` and `SOULS` are 20 each.

Three tests run on every platform, on synthetic databases of 0, 10,000 and 100,000 chunks. Each asserts that idle RSS, peak RSS after one `rag_search`, and settled RSS are at or under 50 MiB. The synthetic text comes from two 40-word tables.

One test runs on Windows only (`#[cfg(windows)]`). It starts the server over the `crates/` tree, records the peak working set after `rag_index`, after 100 `rag_search` calls, after 20 `view` calls and after 20 `soul` calls, and asserts each peak is at or under 60 MiB and at or under 56 MiB.

Environment variables read by the file: `URIEL_RSS_DB_DIR` (directory for the synthetic databases; when set, they are not deleted), `URIEL_CONTROL_BIN` (a second binary measured the same way, as a control), and `URIEL_RSS_MEMORY_MIB` (passed as `--memory-mib` to the full-server run).

The recorded peak is 52.7 MiB at commit `7d74e6c` (`PLAN.md` A7). `crates/uriel-sys/tests/rss.rs` checks that an arena with a hard cap keeps RSS growth under the cap plus a slack.

### Frozen retrieval corpus: `uriel-rag`

`crates/uriel-rag/tests/frozen_corpus/` holds 207 files (`PLAN.md` B5, B7). The query file `eval_queries.json` records the corpus SHA-256, and `run_frozen` refuses a corpus whose digest differs. Tests in `crates/uriel-rag/tests/frozen.rs`:

- `corpus_digest_matches_the_recorded_one` checks the corpus hash against the digest in `eval_queries.json`. **It is failing in the current record**; see [Known gaps](#known-gaps-and-failing-checks).
- `a_drifted_corpus_is_refused` checks that a changed corpus is refused, not scored.
- `query_loader_survives_5000_adversarial_inputs` checks that the loader returns errors, never panics.
- `regenerating_reproduces_the_checked_in_query_file` and `query_file_roundtrips_through_the_loader` check the query file.

Measurement tests in `crates/uriel-rag/tests/eval.rs`, both `#[ignore]`d: `recall_on_the_frozen_corpus` and `soul_guide_coverage_on_the_frozen_corpus`.

### Raft simulator: `uriel-kv`

`crates/uriel-kv/src/sim.rs` is a seeded cluster simulator. Fault levels are constants at the top of the module: `DROP_PER_MILLE` 80, `DUP_PER_MILLE` 30, `MAX_DELAY` 5, `LONG_DELAY` 60, `STALE_LEADER_TICKS` 110, `HEAL_STEPS` 1500, `HEAL_PROOF_STEPS` 400. It covers delay, drop, duplication, reordering, partition, kill, restart and disk fault (`PLAN.md` E4).

- `fifty_seeds_of_three_thousand_steps_on_three_nodes_hold_every_invariant`
- `twenty_seeds_of_three_thousand_steps_on_five_nodes_with_membership_changes_hold_every_invariant`
- `the_same_seed_gives_the_same_trace_and_a_different_seed_gives_another`

```bash
cargo test --release -p uriel-kv
```

### Fuzzers and adversarial decoders

The project rule is that a decoder returns an error on bad bytes and never panics. Each decoder has a test that feeds it generated input.

| Location | Test | Input |
|---|---|---|
| `uriel-cypher/tests/fuzz.rs` | `fuzz_random_bytes_5000`, `fuzz_ascii_punctuation_5000`, `fuzz_token_soup_5000`, `fuzz_mutated_valid_queries_5000` | Four generators, 5,000 cases each |
| `uriel-cypher/tests/fuzz.rs` | `fuzz_seeds_themselves_parse_and_round_trip`, `fuzz_every_prefix_of_every_seed_terminates` | Seed queries and all their prefixes |
| `uriel-core/src/tests/codec.rs` | `record_decoders_survive_20000_random_byte_strings` | 20,000 |
| `uriel-core/src/tests/command.rs` | `decoder_never_panics_on_twenty_thousand_adversarial_inputs` | 20,000 |
| `uriel-kv/src/range.rs` | `decoder_never_panics_on_20000_deterministic_byte_strings` | 20,000 |
| `uriel-tier/src/codec.rs` | `decoders_survive_20000_random_byte_strings` | 20,000 |
| `uriel-topo/tests/export.rs` | `decode_never_panics_on_20000_random_byte_strings` | 20,000 |
| `uriel-host/tests/codec.rs` | `index_decoders_survive_twenty_thousand_adversarial_inputs_with_bounded_output` | 20,000 |
| `uriel-mcp/src/compact.rs` | `decoders_survive_20000_adversarial_inputs_each` | 20,000 per decoder |
| `uriel-mcp/src/compact.rs` | `table_roundtrip_over_2000_random_tables`, `pack_roundtrip_over_2000_random_packs` | 2,000 round trips |

The recorded cypher parser gate is the four 5,000-case generators, 303 tests passed on 2026-10-15 (`PLAN.md`, Phase 0).

### Determinism replay

| Test | What it checks | Source |
|---|---|---|
| `replay_gate` | 200 placement logs x 8 thread counts, shuffled arrival, equal to the single-thread SHA-256 (1,600 runs) | `uriel-place/tests/replay_gate.rs` |
| `replay` | A 40-leaf governor run replays to one digest 20 times; a changed probe changes it | `uriel-house/tests/replay.rs` |
| `replay` | Dispatch digest chain and log round trip | `uriel-dispatch/tests/replay.rs` |

```bash
cargo test --release -p uriel-place --test replay_gate
cargo test --release -p uriel-house --test replay
```

### Benchmarks and ceilings: `uriel-bench`

`crates/uriel-bench/benches/` holds Criterion benches for each tier operation. `crates/uriel-bench/tests/ceilings.rs` asserts the regression ceilings in `tests/ceilings.json`, with a factor of 5.0 (`PLAN.md` H2).

```bash
cargo test -p uriel-bench
cargo bench -p uriel-bench
```

### Other ignored measurement tests

Run each with `-- --ignored --nocapture`: `uriel-algo/tests/smoke.rs` (large), `uriel-algo/tests/islands_measure.rs`, `uriel-core/src/tests/tier.rs` and `placed.rs`, `uriel-mcp/tests/reduce_measure.rs`, and `uriel-mcp/tests/token_dump.rs`. The last needs the Python `tiktoken` package for `real_tokenizer`:

```bash
cargo test --release -p uriel-mcp --test token_dump -- --ignored --nocapture real_tokenizer
```

The cl100k token evaluation is a Python script (`PLAN.md` B4). It needs `tiktoken`:

```bash
python crates/uriel-mcp/tests/token_eval/eval.py --bin target/release/uriel-mcp --out <scratch-dir>
```

On Windows the binary is `target/release/uriel-mcp.exe`.

## Lint and supply chain

`clippy.toml` holds one setting, `msrv = "1.95"`, because the `sysinfo` 0.39 dependency needs Rust 1.95 (`PLAN.md` H1).

```bash
cargo clippy --workspace --all-targets --no-deps -- -D warnings
```

`--no-deps` limits the lint to workspace crates. `-D warnings` makes every warning an error.

`deny.toml` configures `cargo deny`:

| Section | Setting |
|---|---|
| `[advisories]` | `yanked = "deny"` |
| `[licenses]` | Allowed: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-3-Clause, Unicode-3.0, Zlib. Confidence threshold 0.8 |
| `[bans]` | `multiple-versions = "warn"`, `wildcards = "deny"`, path wildcards allowed |
| `[sources]` | Unknown registries and git sources denied; only the crates.io index allowed |

```bash
cargo deny check licenses bans sources
cargo deny check advisories
```

`advisories` needs the advisory database, which was not available offline when `PLAN.md` H1 was recorded.

## CI workflows

Workflows are in `.github/workflows/`.

### `ci.yml`

Triggers: push to `master` and every pull request. An older run of the same ref is cancelled. `CARGO_INCREMENTAL` is `0`.

| Job | Runs on | Toolchain | Command |
|---|---|---|---|
| `check` | ubuntu, windows | 1.97.0 with clippy, rustfmt | `cargo clippy --workspace --all-targets --no-deps -- -D warnings`; `cargo build --workspace --all-targets` |
| `test` | ubuntu, windows | 1.97.0 | `cargo test --workspace --no-fail-fast` |
| `msrv` | ubuntu | 1.95 | `cargo +1.95 check --workspace` |
| `fmt-new` | ubuntu | 1.97.0 with rustfmt | `cargo fmt -p uriel-dispatch -p uriel-page -- --check` |
| `deny` | ubuntu | `cargo-deny-action@v2` | `cargo deny check` (all sections, including advisories) |
| `mcp-token-gate` | ubuntu | 1.97.0 | `cargo build -p uriel-mcp`; `cargo test -p uriel-mcp --test revamp` |
| `docs-pages` | ubuntu | 1.97.0 | `cargo run -p uriel-page --example demo`; `git diff --exit-code docs/pages` |
| `ci-ok` | ubuntu | none | Fails unless all seven jobs above succeeded |

`fmt-new` checks formatting for two crates only. `docs-pages` fails if regenerated sample pages differ from the committed ones, so the page output must be deterministic.

Per `PLAN.md` L6, the workflows had not run on GitHub when that entry was written. The commands were run locally.

### `security.yml`

Triggers: weekly at 05:17 UTC on Mondays (`cron: "17 5 * * 1"`), and manual dispatch. Jobs: `advisories` runs `cargo deny check advisories`; `secrets` runs gitleaks over the full history.

### `release.yml` and `tag-release.yml`

`release.yml` runs on tags matching `v*` and on manual dispatch. It builds `uriel` with `cargo build --release -p uriel-cli --locked` for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc` and `aarch64-apple-darwin`, uses the `stable` toolchain (not pinned), packages each binary with a `.sha256` file, and attaches them to a GitHub release.

`tag-release.yml` runs on push to `master`. It reads the `version` under `[workspace.package]` in `Cargo.toml`, and if the tag `v<version>` does not exist, tags the commit and starts `release.yml` by dispatch.

## End-to-end audit: `scripts/audit_mcp.py`

The audit runs the built binary as a real MCP server over stdio and checks every tool and the CLI. It is stdlib-only Python 3.9 or later. It exits with code 1 if any recorded call failed.

```bash
cargo build --release -p uriel-cli
python scripts/audit_mcp.py
python scripts/audit_mcp.py --binary target/release/uriel --outdir audit-out --keep
```

On Windows PowerShell the default binary is `target\release\uriel.exe`, so `python scripts/audit_mcp.py` works without `--binary`.

| Flag | Default | Effect |
|---|---|---|
| `--binary PATH` | `target/release/uriel` (`uriel.exe` on Windows), under the repository root | The binary to audit. A `uriel-mcp` binary next to it is audited too |
| `--outdir DIR` | `audit-out` under the repository root | Where `audit.json` and `AUDIT.md` are written |
| `--keep` | off | Keep the temporary working directory |
| `--wait-minutes N` | 0 | Poll for the binary for N minutes, 30 seconds between tries |

The script copies the binaries to a temporary directory and runs the copies, so a rebuild during the audit does not change what is tested. It sets `HOME` and `USERPROFILE` to a folder inside that directory, so default database paths point at a scratch copy.

### What it runs

| Phase | Configuration | Coverage |
|---|---|---|
| `normal` | `--mode normal --tools full --format json` | Every listed tool: a scripted sequence of realistic calls, a sample call for each tool not yet covered, and bad arguments (non-object, missing required, wrong types). Protocol probes: malformed JSON, unknown method, `ping`, unknown tool, empty batch, a 9 MB line |
| `house` | `--mode house` | The same, with house, research, genius and dispatch tools. Then reopens the same database |
| `ro-normal`, `ro-house` | `--read-only` added | Write tools must refuse with a `disabled` error |
| `terse+token-limit` | `--terse data --token-limit 150` | Terse output, token limit, `more` continuation, `tools_enable` |
| `mem16` | `--memory-mib 16` | A subset of calls under the 16 MiB minimum budget |
| lock test | two servers on one database | With `--idle-release-secs 0`, the second server is refused with a `locked` error and the first keeps working. With a 2-second idle release, the second server opens the released database |
| list sizes | mode x tools x read-only | Tool count and `tools/list` byte size for all eight combinations |
| CLI | 25 cases, plus 3 for `uriel-mcp` when present | `--help`, `--version`, `banner`, `doctor`, `index`, `recall`, `calibrate`, `genius`, `afk`, `page`, `capture`, `recall-context`, and invalid invocations that must exit non-zero |

A call passes when its result matches its expectation (`ok`, `typed-error`, `protocol-error`, or either). A passing call is marked **odd** when an auto-generated call got a typed error, when an either-expected call got a typed error, or when the call took more than 5,000 ms.

Peak memory is read per call. On Windows the script calls `GetProcessMemoryInfo` for the peak working set. On Linux it reads `VmHWM`. On other systems it samples `ps -o rss=` and keeps the largest sample, which can under-report a short peak.

### Output

Both files go to `--outdir`:

- `audit.json`: every record (phase, tool, arguments, result, latency, response bytes, peak RSS, pass or fail, request and response text), plus lock results, list sizes and CLI results.
- `AUDIT.md`: sections for Summary, Failures, Odd, Lock behaviour, tools/list size, CLI, and one table per phase.

Read `AUDIT.md` first. A clean run shows `none` under Failures and under CLI.

## Recorded results

The README says that unless a row states otherwise, its rows come from one machine (Intel Core i7-14700HX, 15.7 GB RAM, Windows 11) at commit `7d74e6c` on 2026-10-18, release profile.

| Check | Recorded result | Commit and machine | Source |
|---|---|---|---|
| Workspace tests, Windows | 2,040 passed, 3 failed, 9 ignored, across 130 result lines (113 test binaries, 17 doc-test runs). The 3 failures are the `uriel-rag` frozen-corpus digest checks | `7d74e6c`, 2026-10-18 | `PLAN.md` header |
| Workspace tests, Linux | 2,458 passed, 4 failed, 13 ignored. The failures are the 3 digest checks and the `uriel-sys` rss test. The README says they fail the same way on `v0.2.1`, which passes 2,058 with 5 failures | Linux cloud container; commit not stated | README, Evidence |
| Clippy | Exits clean with `-D warnings` | `7d74e6c` | `PLAN.md` header, H1 |
| `cargo deny` | `licenses bans sources` ok; advisories not run offline | `7d74e6c` | `PLAN.md` H1 |
| Full-server peak working set | 52.7 MiB against a 60 MiB limit (idle 9.4, after `rag_index` 36.7, 100 searches 38.0, 20 views 38.1, 20 souls 52.7) | `7d74e6c` | `PLAN.md` A7; README |
| Retrieval recall, frozen corpus | Recall at 5 and 10 on 100 queries over 207 files and 3,534 chunks: hybrid 1.000 and 1.000 | `7d74e6c` | `PLAN.md` B5; README |
| Raft simulation | 50 seeds x 3,000 steps (3 nodes), 20 seeds x 3,000 steps (5 nodes); 124 `uriel-kv` tests pass | `7d74e6c` | README |
| Replicated graph | 30 seeds x 2,000 steps keep replicas equal; 6 mutations caught | `7d74e6c` | README; `PLAN.md` E9 |
| Crash safety | Every write boundary of a 500-operation workload reopens to a committed prefix | `7d74e6c` | README |
| Determinism | 1,600 placement replay runs equal the single-thread digest | 2026-10-15 | `PLAN.md` D1 |
| Token evaluation (`B4`) | 2.74x fewer cl100k proxy tokens against a bar of 3x. **Bar missed** | `7d74e6c`, 2026-10-18 | `PLAN.md` B4 |
| Islands in `view` (`B11`) | Recall at 5: 84/100 off, 92, 94, 94 on at budgets 300, 800, 4,000. Mean proxy tokens 148.6 off, 196.4, 257.6, 274.6 on. **Verdict: off by default** | re-measured 2026-10-18 | `PLAN.md` B11 |
| Soul guide (`B12`) | Gold chunk in the guide for 0, 3, 7 of 100 queries; plain top-by-salience 0, 2, 7. **Verdict: does not beat the plain baseline** | no date given | `PLAN.md` B12 |

The topological salience term in retrieval also has a negative verdict: synthetic recall at 5 is .790 against .865 for top-k, and on real data .439 against .520 (`PLAN.md` C3, `cargo run --release -p uriel-topo --example eval`). The term is not wired into retrieval.

## Known gaps and failing checks

- **The workspace is not green.** The three `uriel-rag` frozen-corpus digest checks fail on the Windows checkout. The recorded digest in `eval_queries.json` is `d0b2c76e…5652`. This checkout hashes to `dec6e990…2dd3` with CRLF line endings and `57438a3d…f2b9` with LF (`PLAN.md` header). The Linux run also fails the `uriel-sys` rss test. `PLAN.md` says the record needs refreshing.
- **The CI `test` job runs the same command as the workspace run,** so it is expected to fail on both platforms until those checks are fixed. The workflows had not run on GitHub when `PLAN.md` L6 was written.
- **The README and `PLAN.md` differ on the Windows run.** The README's row says "2,040 passed, 9 ignored" and does not mention the 3 failures that `PLAN.md` records for the same count. `PLAN.md` is the record of failures.
- **Advisories are not checked locally.** `cargo deny check advisories` needs the database, which was unavailable offline (`PLAN.md` H1). The weekly `security.yml` job is the only recorded advisory check.
- **No mutation harness for `uriel-kv`** (`PLAN.md` E5, unticked).
- **`uriel-sys` has unexercised paths.** The large-page success path never ran, because the account lacks the lock-memory privilege. The buffered fallback after a failed unbuffered open never ran. `PLAN.md` F1 records no non-Windows run, and one mutant survives.
- **The GPU parity harness for the topology CSR has no recorded run.** `crates/uriel-topo/tests/gpu/run.sh` and `parity.py` need `URIEL_GPU_PYTHON` and `URIEL_GPU_KERNEL` and print SKIP without them (`PLAN.md` C7, unticked).
- **The token evaluation missed its bar** (2.74x against 3x). The Cypher part is the gap (`PLAN.md` B4).

## See also

- [`../PLAN.md`](../PLAN.md): the task registry, the gate method, and the dated results this page quotes.
- [`../README.md`](../README.md): the Evidence table with a command for each claim.
- [`DESIGN.md`](DESIGN.md): the original crate map.
- [`SEAL-DB-V2.md`](SEAL-DB-V2.md): storage tiers, tokens and resilience, which the tier and KV gates test.
- [`DISPATCH.md`](DISPATCH.md): the team-agent router behind the dispatch gates.
- [`GENIUS.md`](GENIUS.md): the genius protocol and its fleet limits.
- [`MCP.md`](MCP.md): the tools and modes the audit script exercises.
