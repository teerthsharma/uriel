<p align="center"><b>Invented by <a href="https://teerthsharma.vercel.app/">Teerth Sharma</a></b> · <a href="mailto:teerths57@gmail.com">teerths57@gmail.com</a></p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-lockup-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="assets/logo-lockup-light.png">
    <img alt="Uriel: a flame holding a small graph, nodes coloured by memory tier" src="assets/logo-lockup-light.png" width="470">
  </picture>
</p>

<h1 align="center">Uriel</h1>

<p align="center"><b>Fewest tokens in, same information out: a graph engine that schedules what the model sees.</b></p>

<p align="center">An embedded property-graph database, a deterministic placement scheduler and a budgeted retrieval layer, in Rust,<br>
served to a coding model over MCP. The model reads a small, well-lit view; the graph stays the system of record.</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.90%2B-orange?style=flat-square&logo=rust" alt="Rust 1.90+">
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square" alt="License: MIT OR Apache-2.0">
  <img src="https://img.shields.io/badge/platform-Windows%20tested-0078D6?style=flat-square&logo=windows" alt="Windows tested">
  <img src="https://img.shields.io/badge/tests-1%2C441%20passed%20%C2%B7%200%20failed-2ea44f?style=flat-square" alt="1,441 tests passed, 0 failed">
</p>

---

## Abstract

A coding model is usually fed by stuffing documents into its context window until the budget runs out. Uriel treats the window as a *view* computed from a durable graph, and spends engineering on choosing that view rather than on enlarging it. Four layers do the work. A single-writer, many-reader property graph on redb holds the record, with ACID transactions, constraints and an openCypher engine. Under it, a three-tier store (decoded hot entries, lz4 blocks in RAM, zstd-19 values on disk) admits records to RAM only through a TinyLFU frequency gate, so a one-pass scan cannot flush the hot set. Beside it, a placement scheduler is a pure function of an ordered event log: 1,600 replays across 8 thread counts with shuffled arrival produce one SHA-256 per log. On top, the view layer returns retrieval hits on a four-step resolution ladder, encodes result tables compactly with a proven decoder, and replaces repeated chunks with a session reference. Measured on one Windows laptop on 2026-10-15, the store file is 2.00x smaller than with raw values, the gate raises the Zipf hit rate under scan interference from 51.6 % to 60.2 %, and v2 output is 2.55x smaller than v1 in proxy tokens with all 305 citations preserved. Three results are losses and are reported as such: the token bar was 3x and was missed; hybrid retrieval recalls 0.880 at 5 where plain full-text BM25 recalls 1.000; and the topological salience term this project set out to test loses to plain top-k on both synthetic and real data, so it was not wired in.

**Keywords:** context scheduling · property graph · openCypher · tiered storage · TinyLFU admission · deterministic replay · resolution ladder · compact encoding · retrieval-augmented generation · 0-dimensional persistent homology · MCP · capability-checked memory

---

## 1. Why: the context window is a view, not the system of record

A database never answers a query by shipping its whole heap to the client. It plans, reads the pages the plan needs, and returns rows. Retrieval for language models mostly does the opposite: chunk the corpus, rank by similarity, and paste the top k into the prompt at full resolution, again on every turn, whether or not the model already saw it. Three costs follow. Tokens are spent on text the model did not need at that resolution. The same chunk is paid for repeatedly within a session. And the prompt becomes the de facto store of what the model knows, which is lossy and unaddressable.

Uriel moves the store back to where stores belong. Code, symbols, tests, decisions and goals live as nodes and edges in an embedded graph with transactions. The model gets a view of that graph, chosen under a token budget, at the lowest resolution that still answers, with every hit carrying an address (`file:line`, chunk id) that `rag_expand` can resolve to the exact original. The name follows from that: Uriel is the angel of light and wisdom, "the light of God", and the engine shines a small, chosen light on a large graph. In the mark, the flame is the budgeted view and the graph inside is the record; node colour encodes the memory tier and nothing else (amber hot, orange warm, slate cold).

Two separate economies are managed, and the project keeps them apart because they do not share a mechanism:

| Economy | What is scarce | Mechanism | Crates |
|---|---|---|---|
| Bytes | RAM and disk | three tiers, per-tier codecs, admission gate, RAM budget | `uriel-core`, `uriel-tier`, `uriel-place`, `uriel-sys` |
| Tokens | the model's context | resolution ladder, compact encoding, session dedup | `uriel-rag`, `uriel-mcp` |

Binary compression does not reduce tokens; a zstd frame is not readable text. Tokens fall only by saying less in text the model can still read, which is why the token levers are textual and the byte levers are binary.

---

## 2. Install

Requirements: Rust 1.90 or newer (the workspace `rust-version`), and Windows for the tested path. The installer script needs `cargo` on `PATH` and exits with an error naming rustup if it is absent.

**One line, from a checkout:**

```powershell
pwsh install/install.ps1
```

`install/install.ps1` runs `cargo install --path crates/uriel-cli --locked --force` from the repository root, locates `uriel.exe` in `~/.cargo/bin` (or on `PATH`), and runs `uriel install`.

**Or by hand:**

```powershell
cargo install --path crates/uriel-cli --locked
uriel install
```

`uriel install` prints the banner, then looks for `claude` on `PATH`. If it finds it, it registers the server for the current user and prints `registered uriel with claude`. If it does not, it prints the exact command to run yourself, with the path quoted when it contains spaces:

```powershell
claude mcp add uriel -s user -- <path-to-uriel.exe> mcp
```

The database defaults to `~/.seal/graph.seal` (`%USERPROFILE%\.seal\graph.seal` on Windows); every subcommand that touches it takes `--db <path>`.

**Irreversible on upgrade:** opening a format-1 database with this release rewrites it to format 2 (compressed records at rest). The previous release cannot read an upgraded file. Copy the file first if you may roll back.

---

## 3. Quick start

```powershell
uriel                 # banner (truecolor flame; plain when NO_COLOR is set or stdout is not a TTY)
uriel --version       # banner, then "uriel 0.1.0"
uriel index .         # chunk and index a directory into the graph
uriel calibrate       # measure this machine's memory-latency staircase and suggest a hot size
uriel doctor          # PASS/WARN line per check: db writable, free RAM, lock-memory privilege
uriel mcp             # stdio MCP server; writes JSON-RPC only, never the banner
```

| Command | Flags | What it does |
|---|---|---|
| `uriel` / `uriel banner` | | Prints the logo banner. Colour mode is chosen from `NO_COLOR`, `COLORTERM`, `TERM` and whether stdout is a terminal; if ANSI cannot be enabled on the console it falls back to plain. |
| `uriel index <path>` | `--db` | Indexes a directory and reports files scanned, indexed, unchanged, skipped and removed, chunks added and removed, and the total. Re-running is incremental: unchanged files are not re-chunked. A path that is not a directory is an error. |
| `uriel calibrate` | `--secs` (default 4.0, clamped to 0.05 to 20) | Pointer-chases buffers from 4 KiB to 256 MiB, prints each latency knee as `from -> to (xratio)` and a suggested hot-tier size. |
| `uriel doctor` | `--db` | Three checks: the db directory is writable (by writing and removing a probe file), at least 1 GiB of RAM is available, and whether `SeLockMemoryPrivilege` is granted (needed for large pages; see §6). |
| `uriel mcp` | `--db`, `--root`, `--read-only`, `--format compact\|json`, `--placement local\|planned` | Runs the MCP server on stdin/stdout. Defaults: compact format, `Local` placement, a 20,000-token response limit. The banner is never written on this path, in any colour mode, which a test asserts byte-for-byte. |
| `uriel install` | | Registers the server with Claude Code, as in §2. |

A typical session against an indexed repository: the model calls `rag_search` with a budget, gets the top hit at a readable resolution and the rest as one-line gists with addresses, calls `rag_expand` on the one it needs, and on a repeat search receives a session reference instead of the same text. Structural questions go through `read_neo4j_cypher` against the same graph.

---

## 4. Architecture

Four pillars, bottom to top. Each is a separate crate boundary, so each can be tested without the others.

```
                    ┌─────────────────────────────────────────────────────────┐
  model  <── MCP ── │  VIEW       resolution ladder · compact encoding ·      │  uriel-mcp, uriel-rag
                    │             session dedup · 29 tools                    │
                    ├─────────────────────────────────────────────────────────┤
                    │  SCHEDULER  placement = f(ordered event log)            │  uriel-place
                    │             fixed-point scores · logical clock · gate   │
                    ├─────────────────────────────────────────────────────────┤
                    │  GRAPH DB   T0 hot ─miss→ T1 warm (lz4) ─miss→ T2 cold  │  uriel-core, uriel-tier
                    │             TinyLFU admission · RAM budget · Cypher     │  uriel-cypher, uriel-query
                    ├─────────────────────────────────────────────────────────┤
                    │  DURABLE    redb file, zstd-19 values, ACID, one writer │  uriel-core
                    │             (replication layer: in progress, uriel-kv)  │
                    └─────────────────────────────────────────────────────────┘
```

### 4.1 Durable

The record is a redb file. `uriel-core` implements a property graph on it: nodes with labels and properties, typed relationships, indexes, unique and existence constraints enforced on the write path (a violation rolls the transaction back), and ACID transactions with one writer and many readers. Every commit fires callbacks carrying the epoch and the set of labels and relationship types it touched; the query result cache and the T0 tier both invalidate from that set, exactly, rather than flushing.

T2 is authoritative. Writes go to T2 first; RAM tiers are caches of it. So a crash between tiers cannot lose a committed write by construction, and the differential test (§5.6) checks that reads equal writes through every tier against a `HashMap` model.

Values above a size threshold are stored with a one-byte codec tag and compressed with zstd level 19; small values stay raw because the header would cost more than it saves. The tag is what lets a format-1 database still read and write after upgrade.

### 4.2 Graph database with tiers and a gate

```
 read ─→ T0 HOT   decoded records, bounded by entry count       242 ns median
           │ miss
           ▼
         T1 WARM  lz4 blocks in RAM, bounded by a byte budget   1,018 ns median
           │ miss
           ▼
         T2 COLD  redb file, zstd-19 values                     3,882 ns median (page-cached)
```

**The gate.** A record enters T0 only if its estimated access frequency beats that of the entry it would evict. Frequencies come from a count-min sketch with periodic halving (TinyLFU):

```math
\text{admit}(x) \iff \hat f(x) > \hat f(\text{victim}), \qquad \hat f(x) = \min_{i} \, C_i[h_i(x)], \qquad C \leftarrow \lfloor C/2 \rfloor \text{ every } W \text{ increments}.
```

A 100,000-key scan touches each key once, so no scanned key out-votes a hot key and the hot set survives. Records are admitted on reads only, never on writes.

**Three control loops, one job each:** the tier controller (heat per key class: node, adjacency list, index page; promote T2→T1 on frequency, demote T0→T1 on eviction, never to T2), the gate (admission per cache), and the budget (a hard cap on T0 + T1 bytes; the lowest-heat entries leave first). The T1 byte budget was never exceeded over 100,000 random operations; the byte counts are estimates of payload, not allocator RSS.

**Queries.** `uriel-cypher` is a hand-written openCypher lexer and parser (Neo4j 5 dialect) whose AST prints back to source and re-parses identically. `uriel-query` executes it: aggregation, `MERGE`, variable-length paths, `EXPLAIN`, procedures, and a result cache invalidated by commit tags. `uriel-algo` runs PageRank, Louvain and Kruskal on immutable CSR projections. `uriel-cache` adds a distributed W-TinyLFU cache with a consistent-hash ring, gossip membership and epoch invalidation.

### 4.3 Deterministic scheduler

`uriel-place` decides which regions are hot, warm or cold as a pure function of an ordered event log: integer fixed-point scores, a logical clock instead of wall time, and a serialisable copy of the TinyLFU sketch (checked equal to the `uriel-tier` sketch on 480 keys). Shuffling arrival across threads does not change the output (§5.4): it depends on the log and nothing else. Checkpoints serialise the full state; replay from a checkpoint equals replay from scratch.

The scheduler covers the operating-system roles a placement policy is expected to play, one test each: locality, prefetch, fragmentation entropy, a pressure governor (hot bytes stay within budget at all 7 steps of a pressure ramp, pins survive below Critical, recovery restores the hot set), and lifetime. `uriel calibrate` measures the machine's latency staircase so the hot size comes from this machine, not from a spec sheet (§5.5 shows why that matters).

`uriel-core` can drive its tiers from the planner (`Options::placement = Planned`) or from its local per-read policy (`Local`, the default). §6 explains why the default stayed `Local`.

`uriel-topo` sits beside the scheduler: 0-dimensional persistence over block centroids (elder rule), a causal context schedule of `SINK + LOCAL + RELEVANT` blocks emitted as CSR, and a topology-sorted node layout (a reverse Cuthill-McKee ordering) that is identical across 8 runs and across rayon and std thread counts. The sink-plus-local shape follows StreamingLLM's attention sinks: keep the first blocks and the most recent window, then spend the remaining budget on relevance.

### 4.4 View

What the model reads, three levers, each with a decoder or an exact-original path:

1. **Resolution ladder.** Every chunk carries four resolutions. `rag_search` packs the budget top-down; `rag_expand(chunk, level)` zooms one hit.

   | Level | Contents | Target size |
   |---|---|---|
   | L3 | one-line gist: kind, name, signature | about 12 tokens |
   | L2 | extractive compressed memory | about 1/16 of the chunk |
   | L1 | compressed memory at 1/4 | about 1/4 |
   | L0 | the original text | 1 |

   `pack_ladder` is never over budget across 200 random budgets and never drops a hit before every hit has been reduced to L3. Chunks indexed before the ladder upgrade lazily.

2. **Compact encoding.** Result tables come back as one header and value lines, with no repeated keys, a per-response alias table for long repeated strings (`@a=crates/uriel-rag/src/`), `^` run markers and a `$a=` value dictionary for repeated cells, line ranges as `32-36`, and default fields omitted. `format=json` remains available everywhere. The decoder round-trips 2,000 random tables, still decodes 2,000 old-format tables, survives 20,000 adversarial inputs per decoder without a panic, and refuses expansion bombs.

3. **Session dedup.** A chunk already delivered in this session returns as `#1547` plus a changed-since marker. `session_reset` clears it.

Retrieval itself (`uriel-rag`) follows the shape of CLaRa and drops the trained parts: an extractive compressor (signatures, doc comments, salient lines) instead of a learned one; hybrid BM25 plus a 256-dimensional hashed n-gram embedding fused by reciprocal rank instead of search in a learned latent space; 1 to 2 hop graph expansion over callers, tests and decisions; and a utility prior that raises chunks cited in a reached goal and lowers chunks retrieved but unused, a bandit-style stand-in for CLaRa's differentiable top-k. No GPU, no model download, deterministic.

### 4.5 Charter harness

`uriel-house` is the planning harness that built this repository, shipped as library code and as MCP tools. It has three tiers, **planner**, **resident** and **worker**, and the harness itself is deterministic Rust with no opinions:

- **Planners** chart a goal (a *need* with acceptance criteria) into a tree of sub-goals from three stances: structure (root cause, then parts), failure modes (guard goals that become tests) and alternative routes (each with an estimated cost). The harness merges the three charts by fixed rules: the structure tree is the backbone, every failure-mode node becomes a test leaf under the goal it guards, and each set of alternatives keeps exactly one member, the cheapest not refuted by a recorded fact, with the losers retired under a recorded reason.
- **The validator** rejects a chart that is not a tree, whose `needs` form a cycle, deeper than 6 or wider than 7, with a one-child parent, with a leaf lacking a tier, test command or estimate, with two unordered leaves writing the same file, or with a path that does not exist and is not declared as created. Each rule is a unit test.
- **Residents** take leaves that need judgment or code (token cap 40,000 per leaf); **workers** take mechanical leaves (read, grep, run a named test, collect, format; no writes; cap 8,000).
- **A goal counts as reached only if its bound test was RED on its first recorded run.**
- **The governor** runs on every scheduling call, never once at start:

```math
\text{slots}(c) = \min\!\left(\text{cap}(c),\ \left\lfloor \frac{\text{free\_ram} - \text{reserve}}{\text{cost}(c)} \right\rfloor\right), \qquad \text{reserve} = \max(2\ \text{GB},\ 0.15 \cdot \text{total\_ram}),
```

  with heavy classes (unit tests at 800 MB, builds at 1,500 to 2,000 MB) sharing a global cap of 6, and heavy slots dropping to 0 below the reserve. Measured peak RSS of finished jobs updates each class cost as a moving average.

Role persona slots are empty strings by design: the harness schedules by tier, stance and cost, and carries no characters. The gate that checks the public tree for this (registry item I3) is not yet ticked.

### 4.6 Crates

| Crate | One line |
|---|---|
| `uriel-core` | Embedded property-graph storage on redb: values, ACID transactions, indexes, constraints, commit callbacks, T0/T1/T2 tiers and `tier_stats()`. |
| `uriel-tier` | Tier policy with no I/O: count-min sketch, TinyLFU gate, hot cache, tagged lz4/zstd codecs, tier controller, RAM budget. |
| `uriel-place` | Deterministic placement as a pure function of an ordered event log, checkpoints, and the cache-latency calibrator. |
| `uriel-cypher` | Hand-written openCypher (Neo4j 5 dialect) lexer, parser and typed AST with a Display round trip. |
| `uriel-query` | Tree-walking Cypher executor over `uriel-core` with a tag-invalidated result cache. |
| `uriel-algo` | Graph algorithms on immutable CSR projections: PageRank, Louvain community detection, Kruskal MST. |
| `uriel-cache` | Distributed cache: W-TinyLFU local cache, consistent-hash ring, gossip membership, TCP cache protocol. |
| `uriel-rag` | CLaRa-inspired trainless retrieval: extractive compression, resolution ladder, hybrid BM25 + vector search, graph expansion, utility prior, frozen eval corpus. |
| `uriel-topo` | 0-D persistence of block centroids, causal context schedules, needle-recall evaluation, topology-sorted layout. |
| `uriel-mcp` | stdio MCP server: Neo4j-compatible Cypher, memory, RAG and harness tools; compact encoding and session dedup. |
| `uriel-house` | The charter harness: chart validation and merge, goal scheduling under the resource governor, RED-first gating. |
| `uriel-sys` | The only crate with `unsafe`: capability-checked memory regions, large pages, hot arena with CSR layout, unbuffered IOCP reads, memory metering. |
| `uriel-kv` | Sans-IO resilient KV core: HLC, MVCC, Raft, ranges and a deterministic cluster simulator (in progress, §7). |
| `uriel-host` | Embedding API for a host process such as an IDE: the engine in-process, a framed binary protocol over loopback TCP or in-process channels (in progress, §7). |
| `uriel-bench` | Criterion benches per tier operation and a self-calibrating latency regression gate (in progress, §7). |
| `uriel-cli` | The `uriel` binary: banner, `mcp`, `index`, `calibrate`, `doctor`, `install`, `--version`. |

---

## 5. Results

Every number below is taken from the project registry (`PLAN.md`, code-verified pass of 2026-10-15), each with the gate that produced it. Machine for all runs: **Intel Core i7-14700HX, 16 GB RAM, Windows, 2026-10-15, release builds.** Each block states its baseline; a number without a baseline is not reported.

### 5.1 Storage at rest

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 STORAGE · 20,000 nodes of this repository's text · uriel-core tier_stats
 i7-14700HX · 16 GB · Windows · 2026-10-15 · release
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 payload, logical bytes          22,260,000 B     1.00x   baseline
 payload, lz4 (T1 codec)         13,978,613 B     1.59x
 payload, zstd-19 (T2 codec)     10,532,683 B     2.11x
 ─────────────────────────────────────────────────────────────────────
 redb file, values raw           33,689,600 B     1.00x   baseline
 redb file, zstd-19 values       16,846,848 B     2.00x   smaller
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

The codec decoder survived 20,000 fuzzed inputs. Ratios depend on the corpus: this is Rust source and prose from one repository, and code compresses differently from prose. The 1.5x disk bar set before building was written for the RAG index, v2 against v1; that comparison is not in the registry, so the 2.00x above is not counted against it (§6).

### 5.2 Hit rates, gate on vs off

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 T0 HIT RATE · Zipf(1.0) · 20,000 nodes · 1,000 hot · 200,000 reads
 i7-14700HX · 16 GB · Windows · 2026-10-15 · release
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 workload                        gate ON    gate OFF    delta
 plain Zipf                       65.8 %     61.2 %     +4.6 pt
 + one scan read per read         60.2 %     51.6 %     +8.6 pt
 + a 20,000-key scan burst        63.1 %     60.4 %     +2.7 pt
 oracle ceiling                   71.4 %
 ─────────────────────────────────────────────────────────────────────
 scan pair · 100-key hot set through a 100,000-key scan
   hot keys kept                  100/100      0/100
 scan pair · uriel-place gate     4/4          0/4
 hot-read hit rate · uriel-core · 100k-cold scan vs 100-node hot set
                                  99.7 %     49.9 %  (always-admit)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 bar: gate on higher than gate off under a scan, on a fixture   PASSED
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

The gate is worth most exactly when scans interleave with hot reads (+8.6 points) and least on a pure skewed stream (+4.6). Under a plain Zipf stream the gated cache sits 5.6 points under the oracle. A mutation that admits every record failed 5 tests in `uriel-tier`. The gate protects the hot set only when access is skewed; uniform access gives it nothing to protect.

### 5.3 Tier latencies

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 READ LATENCY · median of 1,000,000 reads per tier
 i7-14700HX · 16 GB · Windows · 2026-10-15 · release
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 T0 hot   decoded record               242 ns     1.0x   baseline
 T1 warm  lz4 block in RAM           1,018 ns     4.2x
 T2 cold  redb + zstd-19             3,882 ns    16.0x   page-cached file
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

T2 was measured with the file in the OS page cache, so 3,882 ns is decode-plus-lookup cost, not a cold disk read.

### 5.4 Replay determinism

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 REPLAY · uriel-place replay_gate · SHA-256 of placement output
 i7-14700HX · 16 GB · Windows · 2026-10-15
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 200 logs × 8 thread counts, shuffled arrival   1,600 runs
 runs equal to the single-thread digest         1,600 / 1,600
 distinct digests across the 200 logs           200
 checkpoint replay equal to scratch replay      200 / 200
 single-event perturbations changing digest     156 / 200
 ─────────────────────────────────────────────────────────────────────
 uriel-core Planned, 5,000-op log, 1 vs 8 reader threads
   tier_stats, T0/T1 keys, hot regions, event log    identical
 Placer::plan at 1,250 regions    82 ms  →  0.63 ms   (was quadratic)
   output identical on 3,000 replayed streams, SHA-256 83ec0d57…2339
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

The control for "identical" is the 200 distinct digests: a hash that collapsed every log to one value would also replay identically. The remaining 44 of 200 single-event changes altered no placement decision, so an unchanged digest there is correct, not a blind spot; the test does not distinguish the two cases on its own.

### 5.5 Latency staircase (`uriel calibrate`)

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 POINTER-CHASE LATENCY vs WORKING SET · 12.26 s run · single core, 4 KiB pages
 i7-14700HX · 16 GB · Windows · 2026-10-15
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 4 – 32 KiB       1.18 ns  ▏
 64 – 256 KiB     3.2  ns  ▎          knee 1:  32 KiB →  64 KiB
 1 MiB            4.1  ns  ▎
 2 MiB            4.4  ns  ▍
 4 MiB           13.7  ns  █          knee 2:   2 MiB →   4 MiB
 8 MiB           15.0  ns  █▏
 16 MiB          72    ns  █████▋     knee 3:   8 MiB →  16 MiB
 32 MiB          92    ns  ███████▏
 64 – 256 MiB    96–100 ns ███████▊
 ─────────────────────────────────────────────────────────────────────
 suggested hot size                4 MiB
 spec sheet for comparison         28 MB L2, 33 MB L3
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

The measured knees do not sit at the advertised cache sizes; see §6.

### 5.6 Token economy

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 PROXY TOKENS · v1 vs v2 output, same 20 fixed queries, same run
 tokenizer: cl100k proxy (not Claude's tokenizer)
 i7-14700HX · 16 GB · Windows · 2026-10-15
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 tool                     v1 tokens    v2 tokens    ratio
 rag_search                  23,959        9,255    2.59x
 read_neo4j_cypher           17,655        7,038    2.51x
 total                       41,614       16,293    2.55x
 citations preserved                                305 / 305
 ─────────────────────────────────────────────────────────────────────
 bar (set before building): ≥ 3x fewer          MISSED by 2,422 tokens
                                                (14.9 %; target 13,871)
 ─────────────────────────────────────────────────────────────────────
 repeated-value shortcut, Cypher alone          2.10x → 2.51x
 identical repeat search (session dedup)
   bytes                                        7.4x smaller
   cl100k proxy                                 905 → 196 (4.62x)
   bar ≥ 5x                                     passes by chars/4,
                                                misses by cl100k
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

The claim is a ratio against v1 output for the same query, never an absolute count of Claude tokens.

### 5.7 Retrieval recall

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 RECALL@5 · frozen corpus: 207 files, 3,534 chunks, 100 queries
 corpus SHA-256 pinned; a drifted corpus is refused
 i7-14700HX · 16 GB · Windows · 2026-10-15 · two runs
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 arm                                    run 1     run 2
 BM25 over full chunks (baseline)       1.000     1.000
 hybrid BM25 + vector                   0.880     0.890
 hybrid through pack_ladder             0.880     0.890
 ─────────────────────────────────────────────────────────────────────
 compression of memories                12.29x    (target 16x)
 index size                             3.21 MB vs 3.31 MB
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

The ladder costs no recall (equal on both runs). Hybrid retrieval costs 12 points of recall against the boring baseline; see §6. An earlier, unfrozen measurement over 100 doc-comment queries on this repository gave the same 0.880 vs 1.000.

### 5.8 Memory bounds, layout and the server

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 HARD RAM CAP · uriel-sys Arena::with_cap(256 MiB), 1 MiB slabs touched
 i7-14700HX · 16 GB · Windows · 2026-10-15
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 reserve only (control)                     RSS +12,288 B
 filled to the cap                          RSS +268,460,032 B (24,576 B over 256 MiB)
 after 10,000 refused allocations           RSS +0 B
 private bytes over cap                     +528,384 B
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 MCP SERVER · uriel-mcp release binary, real-binary run on this repository
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 tools                                      29
 index of this repository                   133 files → 2,538 chunks in 0.2 s
 binary                                     4.72 MiB
 idle RSS                                   10.3 MiB
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 NODE PAGES TOUCHED · topology-sorted vs insertion order (lower is better)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 interleaved ids, 92 % intra-community edges     0.379
 60 % intra                                      0.668
 contiguous ids                                  0.728
 30 % intra                                      0.816
 this repository's call graph                    0.966
 adjacency lines, same graph                     1.007
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

### 5.9 Correctness gates

| Gate | Result | Control |
|---|---|---|
| Workspace | 1,441 passed, 0 failed, 6 ignored across 83 test binaries | `cargo clippy --workspace --all-targets --no-deps -- -D warnings` clean on the same run |
| Cypher parser | 303 passed, incl. 4 generators × 5,000 fuzz cases | Display round trip re-parses identically |
| Cypher executor | 38 passed, incl. a 105-row TCK-style table | |
| Result cache | 9 tests | 6 fail with invalidation disabled |
| Graph algorithms | 138 passed | karate-club values match networkx |
| Tier differential | 5 seeds × 2,000 ops, T0 = 3 entries, T1 = 1 KiB | a `HashMap` model; 18 tier tests also run under both placement modes (36 runs) |
| T0 exact invalidation | only touched ids evicted; rollback invalidates nothing | a stub `remove_key` failed 6 tests (RED seen) |
| 0-D persistence | hand-computed diagrams; stability within 2ε over 200 random sets; isometry, scale, permutation | a flipped elder rule failed 5 of 15 tests |
| Topology layout | identical over 8 runs and across thread counts | a reversed-degree RCM mutant was caught |
| Hot arena and CSR | 100 random graphs equal a plain adjacency model; 100,000 allocations never pass the cap; 64-byte alignment | 4 mutants caught |
| Capabilities | 36 tests; 20,000 random accesses match a bounds model | raw address, out-of-range, misaligned and wrong-width access rejected |
| Charter harness | 103 passed, incl. a 40-leaf simulation under free RAM of 6,000, 2,000 and 5,000 MB | |
| MCP server | 120 passed | compact (12) and stdio (51) suites |

---

## 6. What we got wrong

These are first-class results. Each one changed a default or a plan.

**Topological salience loses to top-k.** The project started from the hypothesis that 0-dimensional persistence over block centroids would find salient context that relevance ranking misses. The needle-recall evaluation says no. At the default noise level (alpha 0.25, 20 synthetic contexts), recall at 5 / 10 / 25 % budget is .865 / .957 / 1.000 for plain top-k against .790 / .947 / 1.000 for the topological arm. On real data (1,647 blocks, 759 queries) the gap widens: top-k .520 / .626 / .798, topological .439 / .542 / .721, and turning the salient term off reproduces top-k exactly. The only setting where it wins is alpha 0.10 (margins +0.335 / +0.620 / +0.365). Both arms beat the floor (recency .025 / .085 / .250, random .047 / .092 / .250), so the evaluation can tell arms apart. **Decision:** the salient term is not wired into retrieval; the shipped schedule is `SINK + LOCAL + RELEVANT`.

**Planned placement costs 4 to 24x per read and does not beat Local.** Zipf(1.0), 20,000 nodes, 1,000 hot, 200,000 reads, hit rate plain / +scan per read / +20k burst: Local with the gate 65.8 / 60.2 / 63.1 %; Planned with region size 1 66.3 / 63.8 / 64.8 %; Planned with region size 16 50.5 / 42.7 / 45.4 %. Region size 1 buys 0.5 to 3.6 points; region size 16 loses 15.3 to 17.7. Per-read cost is 216 ns for Local against about 5,200 ns for Planned at N = 128 (24x) and 983 ns at N = 1,024 (4.6x). **Decision:** the default stays `Local`; `Planned` exists for replay and analysis.

**Retrieval recall sits below plain full-text BM25.** On the frozen corpus, hybrid BM25 + vector retrieval recalls 0.880 / 0.890 at 5 on two runs where BM25 over full chunks recalls 1.000 / 1.000. The hybrid arm searches compressed memories; BM25 over the full chunks keeps 12 points more. Compression reached 12.29x against a 16x target, and the index is not meaningfully smaller (3.21 MB vs 3.31 MB). The pre-set bar of 1.5x fewer bytes on disk for the RAG index, v2 against v1, has no v1-vs-v2 measurement in the registry, so it is unmeasured, not passed. The CLaRa-shaped design is kept for its token savings, not for recall.

**The token bar was missed: 2.55x against 3x.** After the repeated-value shortcut, v2 is 2,422 proxy tokens (14.9 %) short of the bar. `rag_search` is 57 % of v2 output, and 249 of its 294 hits are already L3 gists plus a citation line; the citation line is now the cost. Unique identifiers in Cypher output cannot be shrunk by this scheme at all. Session dedup also misses its own 5x bar by the cl100k proxy (4.62x) while passing it by chars/4, which is a reminder that the proxy choice moves verdicts.

**Calibration knees do not match the spec sheet.** The measured knees fall at 32→64 KiB, 2→4 MiB and 8→16 MiB, and the suggested hot size is 4 MiB, against an advertised 28 MB L2 and 33 MB L3. The probe runs on one core with 4 KiB pages and does not isolate the TLB, so a knee may reflect TLB reach rather than cache capacity. The number is what one core sees, which is what the scheduler uses; it is not a cache-size measurement.

**The large-page path was never exercised.** This account lacks `SeLockMemoryPrivilege` (`whoami /priv`), so only the fallback to normal pages (`NormalPages` / `PrivilegeMissing`) has run. The buffered fallback taken when an unbuffered open fails has never run either. Nothing in `uriel-sys` has run on a non-Windows OS. One mutant survives: the bounds check inside the raw layer is unreachable through the public API, so no test can kill it.

**Topology-sorted layout barely helps on real code.** Pages touched fall to 0.379 of insertion order on a planted graph with interleaved ids, but on this repository's call graph the ratio is 0.966, and on adjacency lines it is 1.007, slightly worse. Louvain found 18 communities where 8 were planted.

**Replay perturbation is weaker than it looks.** Only 156 of 200 single-event changes alter the digest. The other 44 change no decision, but the test cannot by itself separate "no decision changed" from "a change was missed".

**The quadratic planner.** `Placer::plan` was quadratic, 82 ms at 1,250 regions; it is now 0.63 ms with identical output on 3,000 replayed streams. It shipped quadratic first.

**The format upgrade is one-way.** Compressed records at rest bump the database format from 1 to 2 on open, and the previous release cannot read the result.

---

## 7. Status

The registry counts **38 of 64** boxes ticked at the code-verified pass of 2026-10-15. A box is ticked only when its gate was written, seen failing, and then passed, with the run cited.

| Area | Built and gated | Not built, or partial |
|---|---|---|
| Foundations | property graph, constraints, commit callbacks, openCypher parser and executor, result cache, graph algorithms, distributed cache, trainless RAG, charter harness, MCP server (29 tools) | |
| Storage tiers | sketch and gate, zstd-19 at rest with format-1 compatibility, T0 exact invalidation, T1 under budget, differential test, `tier_stats` | |
| Token tiers | resolution ladder, compact encoding, session dedup, recall with the ladder, repeated-value shortcut, frozen eval corpus, compact format adopted by existing tools | token bar (missed, §6); compact per-hit citations to close the gap |
| Topology | 0-D persistence, context schedule, needle-recall eval (negative), layout, layout benefit | |
| Scheduler | deterministic placement, OS roles, calibration, pressure ramp, planner behind `Options::placement` | replay determinism for the harness governor and test selection |
| Resilience | | HLC and MVCC partial (gc-horizon and snapshot-repeatability tests missing); sans-IO Raft, ranges, cluster simulator and the Raft mutation gate not gated; WAL checksums, scrub and self-heal; crash fault injection at every write boundary; degraded modes; the graph carried on ranges |
| Embedding and hardware | hot arena and CSR, hard cap as a required constructor argument | capabilities partial (large-page success path, buffered fallback and non-Windows never ran); host embedding API not gated |
| MCP surface | | tools for placement and views: `why_hot`, `pin`, `view`, `replay`, `calibrate` |
| Hygiene | workspace gate | `clippy.toml` and `deny.toml`; Criterion benches with an asserted regression ceiling |
| Release | name and mark | crate rename gate; CLI snapshot gates; release package with a clean-machine install; persona-free public pack |

Code is moving ahead of the registry. `uriel-kv` now contains Raft and range modules, `uriel-host` and `uriel-bench` exist with their own tests, and the `uriel` CLI ships the subcommands in §3, but none of those boxes is ticked until its gate is cited, so none of it is claimed here. The cluster simulator file is empty. Scrub and crash-injection results do not exist yet, and so are not reported. The property graph runs on a single node; `uriel-kv` does not carry it. The registry also holds one unreconciled pair: the workspace gate records clippy clean, while the HLC/MVCC entry records 6 clippy errors in the `uriel-kv` MVCC test target.

---

## 8. Design rules

1. **Deterministic.** Same input log, same output, independent of thread count and wall clock, and each such claim has a test: placement replay (1,600 runs), layout (8 runs and across thread pools), and `Planned` tier state (1 vs 8 readers).
2. **Bounded.** Every cache has a cap that is a constructor argument, not a default: `Arena::with_cap` has no unlimited form, the T1 byte budget held over 100,000 random operations, and refused allocations add 0 bytes of RSS. The resource governor computes slots from free RAM on every call.
3. **`unsafe` is quarantined in `uriel-sys`.** Every other crate is `#![deny(unsafe_code)]` (the CLI is `#![forbid(unsafe_code)]`). Inside `uriel-sys`, `unsafe` exists only under `src/raw/`, 28 lines.
4. **Capabilities.** Hardware is reached only through validated range objects. No caller passes a raw address or size; out-of-range, misaligned and wrong-width accesses are rejected, and 20,000 random capability accesses match a bounds model.
5. **No panics on adversarial bytes.** Every decode path is bounds-checked and returns an error: 20,000 fuzzed inputs for the record codec, 20,000 adversarial inputs per compact decoder, an expansion-bomb guard, and 4 × 5,000 generated Cypher cases.
6. **Gate first, seen failing.** A gate is written and watched failing before the work lands; mutation checks (admit-always gate, flipped elder rule, disabled cache invalidation, reversed-degree RCM, stub `remove_key`) show the tests can fail.
7. **Evidence over assertion.** A number states what it was compared against, baselines are included, and losses stay in the registry and in §6.
8. **No comments in code.** Test names carry the meaning; `PLAN.md` carries scope, results and misses.

---

## 9. Prior art and credits

| Work | What Uriel takes from it | What it does not claim |
|---|---|---|
| **CLaRa** (Apple), [arXiv 2511.18659](https://arxiv.org/abs/2511.18659) | The inspiration: compress documents to about 1/16, retrieve in the compressed space, let the generator read compressed memories, expand on demand. | No learned compressor, no continuous latent reasoning, no differentiable top-k, no parity with CLaRa's benchmarks. The utility prior is a bandit-style stand-in for joint training. |
| **StreamingLLM**, Xiao et al., "Efficient Streaming Language Models with Attention Sinks" | The sink-plus-local shape of the context schedule: keep the first blocks and the recent window, spend the rest on relevance. | Applied to retrieval scheduling, not to attention or a KV cache. |
| **Raft**, Ongaro and Ousterhout, "In Search of an Understandable Consensus Algorithm" | The target for `uriel-kv`: election, replication, the current-term commit rule, PreVote, snapshots, single-step membership, verified sans-IO in simulation. | Not yet gated (§7); verified in simulation only, never on real network faults. |
| **TinyLFU** (Einziger, Friedman, Manes) | Frequency-sketch admission with periodic aging, the gate in `uriel-tier`, `uriel-place` and `uriel-cache`. | |
| **CockroachDB** layering | Transaction, distribution, replication and storage layers with HLC and MVCC, as the plan for `uriel-kv`. | No compatibility or performance claim. |
| [**redb**](https://github.com/cberner/redb) | The embedded ACID key-value store under `uriel-core`. | |
| [**zstd**](https://github.com/facebook/zstd) | Level-19 compression for T2 values, through the `zstd` crate. | |
| [**lz4_flex**](https://github.com/PSeitz/lz4_flex) | The T1 codec, built with its safe encoder and decoder. | |

Further credits: the openCypher project for the query language, networkx for the karate-club reference values, the Reverse Cuthill-McKee ordering behind the layout, and the elder rule of persistent homology behind `uriel-topo`.

---

## License

Dual-licensed under **MIT OR Apache-2.0**, as declared in the workspace `Cargo.toml`.
