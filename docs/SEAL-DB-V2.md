# Seal db v2: tiered, gated, compressed, resilient

Goal in one line: **the fewest tokens into the model and the fewest bytes on
disk and in RAM, without losing the ability to zoom to the exact original, on a
database that keeps working when a replica dies.**

Three separate problems, three separate mechanisms. They share a vocabulary
(tiers, gates, resolution) but not code.

| Problem | Mechanism | Crate |
|---|---|---|
| Bytes in RAM and on disk | Tiered storage with compression and a gated cache | `uriel-core` (storage), `uriel-tier` (policy) |
| Tokens the model reads | Resolution ladder plus compact result encoding plus session dedup | `uriel-rag`, `uriel-mcp` |
| Surviving a failed node | Cockroach-style layers: ranges, leases, Raft, HLC, MVCC | `uriel-kv` |

## 1. Storage tiers (bytes)

```
 read --> T0 HOT   decoded records, bounded by entries         (fastest, smallest)
            | miss
            v
          T1 WARM  compressed blocks in RAM (lz4)              (about 2x denser, ~us to decode)
            | miss
            v
          T2 COLD  the redb file, values compressed (high ratio) (densest, ~ms)
```

- **Gate (admission).** A record enters T0 only if its estimated access
  frequency beats the frequency of the entry it would evict (TinyLFU: a
  count-min sketch with periodic aging). One scan of 100k cold nodes therefore
  does not flush the hot set. Writes go to T2 first and are admitted to T0 only
  on reads.
- **Heat controller.** Each tier owns a heat counter per key class (node,
  adjacency list, index page). A background-free controller runs inline on
  every N operations: promote T2 to T1 when frequency crosses a threshold,
  demote T0 to T1 on eviction, never to T2 (T2 is the source of truth).
- **Compression per tier.** T0 raw. T1 lz4. T2 a higher-ratio codec for values
  above a size threshold, with a one-byte format tag so old uncompressed values
  still read. Small values stay raw because the header costs more than it saves.
- **Hierarchical control.** Three loops, each with one job: the tier controller
  (heat, per node), the gate (admission, per cache), and the budget (a hard cap
  on T0 + T1 RAM; when exceeded the lowest-heat entries are dropped first).
- **Invariants (tests).** Reads equal writes across every tier; a crash between
  tiers loses nothing (T2 is authoritative); T0 + T1 bytes never exceed the
  budget; a one-pass scan does not evict a hot key; a tag-0 legacy value still
  decodes.

## 2. Token tiers (what the model reads)

Binary compression does not reduce tokens. Tokens fall by **saying less in text
the model can still read**. Three levers:

1. **Resolution ladder (hierarchical).** Every chunk stores four resolutions:

   | Level | Contents | Target |
   |---|---|---|
   | L3 | one-line gist: kind, name, signature | about 12 tokens |
   | L2 | compressed memory (today's 16x) | about 1/16 |
   | L1 | compressed memory at 1/4 | about 1/4 |
   | L0 | full original | 1 |

   `rag_search` returns the top hit at L1, the next few at L2, the rest at L3,
   chosen by budget; `rag_expand(chunk, level)` zooms exactly one hit. The model
   asks for resolution instead of receiving everything.
2. **Compact result encoding.** Rows come back as a header once plus value
   lines (no repeated keys), with a per-response alias table for repeated long
   strings (`@a=crates/uriel-house/src/`), line ranges as `32-36`, and default
   fields omitted. `format=json` stays available. The encoding has a decoder,
   and a test proves decode(encode(x)) == x.
3. **Session dedup.** A chunk already delivered in this session comes back as
   `#1547` plus a changed-since marker, not as text again.

Measured as a proxy: tokens counted with a real tokenizer library where
available, otherwise `ceil(chars / 4)`, always labelled which. The claim is a
ratio against the v1 output for the same query, never an absolute count of
Claude tokens.

## 3. Cockroach-type resilience (`uriel-kv`)

CockroachDB's layering, kept honest to what a small Rust crate can verify:

```
 Cypher / SQL
   transaction layer   MVCC versions stamped by a hybrid logical clock (HLC)
   distribution layer  keyspace split into ranges (by size); range descriptors
   replication layer   Raft per range, 3 replicas, leaseholder serves reads
   storage layer       uriel-core / redb, tiered as in section 1
```

- **HLC:** (wall ms, logical counter, node id), monotonic across nodes, bounded
  skew check.
- **MVCC:** a key holds versions by timestamp; reads see a consistent snapshot.
- **Ranges:** split when a range exceeds a size, merge when small; descriptors
  are versioned.
- **Raft (sans-IO):** a pure state machine fed messages and ticks, emitting
  messages and committed entries, so tests drive it deterministically with a
  simulated network and no sockets or threads.
- **Leaseholder:** one replica per range serves consistent reads; the lease
  moves with the leader.
- **Survival tests:** kill a minority, the range still commits; kill the
  leader, a new one is elected and no committed entry is lost; partition and
  heal, logs converge; a restarted replica catches up.

**What exists.** Sans-IO Raft (PreVote, check-quorum, snapshots, single-step
membership, ReadIndex) over a file-backed CRC-32C log, HLC, MVCC, range
descriptors with split, merge and a lease table, and a seeded cluster simulator.
`uriel-core` records each commit as a checksummed command batch, and `uriel-repl`
applies those batches on every replica of one Raft group under the simulator.

**What this is not (yet).** The shipped server runs the property graph on a
single node. Ranges are not wired to Raft groups, there are no distributed
transactions, and Raft has run in simulation only. No claim of CockroachDB
compatibility or performance.

## Success numbers (set before building, to be measured)

| Measure | Bar |
|---|---|
| Proxy tokens, `rag_search` + `read_neo4j_cypher` typical outputs, v2 vs v1 | at least 3x fewer, all citations preserved |
| Bytes on disk for the seal-repo RAG index, v2 vs v1 | at least 1.5x smaller |
| Hot-set hit rate under a scan, gate on vs off | gate on is higher, shown on a fixture |
| `uriel-kv` | every survival test above passes under 50 seeded random fault schedules |

A bar that is missed is reported as missed.

## Limits

Compression ratios depend on the corpus; code compresses differently from
prose. The gate protects the hot set only when access is skewed. A tokenizer
proxy is not Claude's tokenizer. Raft is verified in simulation, not on real
network faults.
