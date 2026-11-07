# Graph database

Uriel keeps its graph in one embedded file, with no server process of its own. The graph is a property graph: nodes and relationships carry labels, types and properties. Writes run in ACID transactions with a single writer and many readers. Queries are openCypher, and the MCP tools expose them, along with a memory graph for notes and facts. This page describes the storage model, the file format, the memory tiers, the supported query subset, the memory tools, the server settings that affect the database, and the locking rules as of v0.2.9.

Every statement below comes from the source file named next to it. Numbers come from `PLAN.md` or a test, and the page says which.

## Contents

- [Model](#model)
- [Transactions and epochs](#transactions-and-epochs)
- [Constraints and indexes](#constraints-and-indexes)
- [File format and upgrade](#file-format-and-upgrade)
- [Memory tiers](#memory-tiers)
- [Memory budget](#memory-budget)
- [Query language](#query-language)
- [Memory tools](#memory-tools)
- [Cypher and inspection tools](#cypher-and-inspection-tools)
- [Database location](#database-location)
- [Locking](#locking)
- [Backup](#backup)
- [Limits](#limits)
- [See also](#see-also)

## Model

Node and relationship records are keyed by id. A node has a set of labels and a property map. A relationship has one type, a source node, a destination node and a property map. Both record kinds are encoded in `crates/uriel-core/src/codec.rs`.

Property values are one of nine kinds: null, boolean, 64-bit integer, 64-bit float, string, byte array, list, map, and a vector of 32-bit floats (the value tags in `crates/uriel-core/src/codec.rs`). Label, relationship type and property key names must be non-empty and must not contain a NUL byte (`crates/uriel-core/src/error.rs`, `InvalidName`).

The record decoder checks every length against the bytes left and refuses nesting deeper than 128 levels. Truncated or trailing bytes return a corruption error, not a panic (`codec.rs`, `Reader`).

The graph lives in one `redb` file. These tables are defined in `crates/uriel-core/src/tables.rs`:

| Table | Key | Holds |
|---|---|---|
| `nodes` | node id | node record |
| `rels` | relationship id | relationship record |
| `adj_out`, `adj_in` | source (or destination) id, type, relationship id | the other end of each relationship, so a neighbour lookup is a range scan |
| `label_idx` | label, node id | label index |
| `type_idx` | type, relationship id | relationship-type index |
| `prop_idx` | label, key, value, node id | one shared table for every property index |
| `counts` | `L` label or `R` type | live entity counts |
| `indexes`, `constraints` | label and key | index and constraint catalog |
| `meta` | name | format version, next ids, counts, epoch |
| `node_sums`, `rel_sums` | record id | CRC-32C checksum of each record (`crates/uriel-core/src/checksum.rs`) |

## Transactions and epochs

- `Graph::begin_read` returns a snapshot. It never blocks on a writer (`crates/uriel-core/src/lib.rs`, module docs).
- `Graph::begin_write` returns the one writer. It waits until any other write transaction has committed or been dropped (`crates/uriel-core/src/graph.rs`, `begin_write`).
- A commit is atomic and durable. Dropping a `WriteTx` without `commit` rolls back everything in it (`lib.rs`, module docs).
- If a storage error happens halfway through a write transaction, the transaction is poisoned. It can only be dropped, and `commit` returns `TransactionPoisoned` (`error.rs`).
- `Graph::epoch` counts effective commits. It goes up by one per commit that changed something; a commit that changed nothing does not advance it (`graph.rs`, `epoch`).
- `Graph::on_commit` registers a callback that receives the new epoch and the labels and relationship types the commit touched. Callbacks run after the commit is visible, in epoch order, while later writers wait. A callback must not start a write transaction (`graph.rs`, `on_commit`).

## Constraints and indexes

Constraints and indexes are created with Cypher schema commands, run through `write_neo4j_cypher`. The parser accepts these forms (`crates/uriel-cypher/src/parser.rs`, `parse_schema`):

```cypher
CREATE INDEX person_name FOR (p:Person) ON (p.name)
CREATE CONSTRAINT person_email FOR (p:Person) REQUIRE p.email IS UNIQUE
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.name IS NOT NULL
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.id IS NODE KEY
SHOW INDEXES
SHOW CONSTRAINTS
DROP INDEX person_name IF EXISTS
DROP CONSTRAINT person_email IF EXISTS
```

The executor maps the constraint kinds onto two core kinds (`crates/uriel-query/src/schema.rs`):

| Cypher | Core kind | Meaning |
|---|---|---|
| `IS UNIQUE` | `Unique` | no two nodes with the label share the value |
| `IS NOT NULL` | `Exists` | every node with the label has the property |
| `IS NODE KEY` | `Unique` and `Exists` | both |

Constraints are enforced in the write path. A violating write returns `ConstraintViolation` and rolls back. Creating a constraint also fails when existing data already violates it (`error.rs`, `ConstraintViolation`; the rollback test is `crates/uriel-core/src/tests/constraints.rs`, cited in `PLAN.md` Phase 0).

Limits on schema commands: relationship targets and multi-property targets return `Unsupported` ("exactly one property is supported"). A unique constraint's backing index cannot be dropped alone; drop the constraint instead (`error.rs`, `IndexBacksConstraint`). The `RANGE`, `TEXT` and `POINT` keywords on `CREATE INDEX` are parsed, but nothing in `crates/uriel-query/src` reads them, so all indexes behave the same.

## File format and upgrade

The format version is stored in the `meta` table under `format`. The current build writes and reads version 3 (`crates/uriel-core/src/graph.rs`, `FORMAT_VERSION`).

| Version | Constant | What it is |
|---|---|---|
| 1 | `LEGACY_FORMAT_VERSION` | original layout |
| 2 | `SUMLESS_FORMAT_VERSION` | layout without the checksum tables |
| 3 | `FORMAT_VERSION` | current; records have CRC-32C checksums |

When a read-write open finds version 1 or 2, `init_unguarded` clears the `node_sums` and `rel_sums` tables and writes version 3 in the same transaction. A file with any other version returns `UnsupportedFormat { found, expected }`, which reads "unsupported storage format version N (this build reads version 3)" (`graph.rs`, `init_unguarded`; `error.rs`).

**The upgrade is one-way.** Once a read-write open has stamped version 3, a build that only reads an older version refuses the file. `PLAN.md` entry A2 records that the previous release cannot read an upgraded file. That entry says "format 1 to 2"; the current code stamps version 3 and accepts versions 1 and 2. Copy the file before the first open with a new build (see [Backup](#backup)).

A read-only open goes through an overlay. The overlay keeps its writes in memory over the file (`crates/uriel-core/src/overlay.rs`, `patches`), so a read-only open does not rewrite the file.

Checksums: each node and relationship record has a CRC-32C checksum (`checksum.rs`). `Graph::scrub` checks them, and `Graph::repair_from` repairs from a replica (`graph.rs`).

Failure modes (`graph.rs`, `Mode`, `ModeCause`):

| Mode | Entered when | Effect |
|---|---|---|
| `Full` | normal | reads and writes |
| `ReadOnly` | a write hits an I/O error, or scrub finds corruption | writes return `ReadOnly`; the graph reopens the file, or falls back to a read-only overlay |
| `CacheOnly` | a read hits an I/O error | disk reads return `Unavailable` ("cold data is unavailable") |

Each mode change is logged as a `ModeEvent` with its cause.

## Memory tiers

Reads go through three tiers. Each tier is a copy of the one below it, except the last, which is the file.

| Tier | Holds | Encoding | Bound |
|---|---|---|---|
| T0 hot | decoded node records and whole adjacency lists | decoded | `hot` bytes and `hot_entries` (`crates/uriel-core/src/tier.rs`) |
| T1 warm | the same entries | lz4 blob in RAM | `warm` bytes |
| T2 cold | the `redb` file | each value tagged raw or zstd level 19 | the file |

The codec is `crates/uriel-tier/src/codec.rs`. A value is zstd-compressed at level 19 only when it is at least `cold_threshold_bytes` (256 by default, `graph.rs`, `Options::new`) and the compressed form is smaller. Otherwise it is stored raw behind a one-byte tag. Values written before the tags existed decode unchanged (`codec.rs`, test `legacy_untagged_values_decode_unchanged`).

**Read path.** A read checks T0, then T1. A T1 hit that T0 admits is removed from T1. A miss reads the file and increments `disk_reads`.

**Admission gate (TinyLFU).** A candidate enters T0 only if its estimated access count is strictly higher than the count of the entry it would evict. Ties are rejected (`crates/uriel-tier/src/gate.rs`, `admit`; test `ties_are_rejected`). The counts come from a sketch with a sample size of ten times the capacity; when the sample fills, all counts are halved (`crates/uriel-tier/src/sketch.rs`). The gate is off under `--placement planned`, where the placement plan decides instead (`tier.rs`, `TierState::new`).

**Warm promotion.** A candidate the gate rejects is still written to T1 if its estimated count is at least 2 (`tier.rs`, `WARM_THRESHOLD`, `fill`).

**Heat.** `crates/uriel-tier/src/controller.rs` keeps heat counters per key class: node and adjacency. Heat halves every ten times the hot entry count. The `Index` class exists in the type, but no cache key maps to it (`tier.rs`, `CacheKey::class`).

**Demotion.** An entry evicted from T0 is compressed and moved to T1. Nothing is ever demoted to T2; the file is the source of truth.

**Writes.** A commit removes every touched key from T0 and T1 before and after it runs (`tier.rs`, `begin_commit`, `finish_commit`). Writes do not fill the caches. The `invalidated` counter in `tier_stats` counts the entries removed this way.

**Budget enforcement.** T0 and T1 share one byte budget, the sum of their two shares. When the total is over budget, warm entries are dropped first (least recently used first), then hot entries (`uriel-tier/src/controller.rs`, `Budget::shed`; `tier.rs`, `enforce_budget`).

Byte counts are estimates from a weight function (`tier.rs`, `weight`). They are not allocator RSS (`PLAN.md`, A4).

### Measured (from PLAN.md)

Dated measurements from `PLAN.md`, entries A6 and A7. They are not re-run for this page.

| Measure | Result | Source |
|---|---|---|
| 20k nodes of repository text, lz4 (T1) | 22,260,000 logical bytes to 13,978,613 (1.59x) | A6, 2026-10-15 |
| 20k nodes, zstd-19 (T2 codec) | 22,260,000 to 10,532,683 (2.11x) | A6, 2026-10-15 |
| redb file, same corpus | 33,689,600 to 16,846,848 (2.00x) | A6, 2026-10-15 |
| Zipf(1.0) hit rate, gate on and off | 65.8 % and 61.2 % (plain reads); 63.1 % and 60.4 % with a 20k-node scan burst | A6, 2026-10-15 |
| Read latency, median of 1M reads, T0 / T1 / T2 | 242 ns / 1,018 ns / 3,882 ns (T2 page-cached) | A6, 2026-10-15 |
| Peak working set of the full server | 52.7 MiB against a 60 MiB gate | A7, at commit `7d74e6c` |

The gate's benefit is a hit-rate result on a skewed workload, not a general speed-up. A scan-heavy workload with no skew shows little gain.

## Memory budget

`--memory-mib` sets one budget for the whole server. The default is 50 MiB and the minimum is 16 MiB (`crates/uriel-mcp/src/config.rs`; `MemoryBudget::MIN_BYTES`, `crates/uriel-core/src/budget.rs`). The budget is split by per-mille shares (`budget.rs`):

| Share | Per mille | Default at 50 MiB | Used for |
|---|---|---|---|
| reserve | 250 | 12.5 MiB | unmanaged reserve (`PLAN.md` A7) |
| redb page cache | 250 | 12.5 MiB | `Options::cache_bytes`, the file's page cache |
| RAG index | 240 | 12 MiB | the `rag_*` tools' index (`PLAN.md` A7) |
| hot (T0) | 80 | 4 MiB | `Options::hot_bytes`; entries = hot bytes / 2048, at least 16 (2,048 entries at the default) |
| warm (T1) | 80 | 4 MiB | `Options::warm_bytes` |
| query cache | 80 | 4 MiB | the read-result cache in `crates/uriel-query/src/lib.rs` |
| session | 20 | 1 MiB | session dedup (`PLAN.md` B3) |

The shares sum to 1,000. The default column is the share applied to 52,428,800 bytes. It is arithmetic from `budget.rs`, not a separate measurement.

## Query language

Queries run through `crates/uriel-query`. The parser is `crates/uriel-cypher`, and its AST carries a `Display` that re-parses to the same tree. A syntax error returns `syntax error: ...`.

### Clauses

The executor dispatches every clause type (`crates/uriel-query/src/exec.rs`, the `match clause` block):

| Clause | Notes |
|---|---|
| `MATCH`, `OPTIONAL MATCH` with `WHERE` | pattern matching |
| `UNWIND` | list expansion |
| `CREATE` | one relationship type per relationship; directed only; no variable length (`write.rs`) |
| `MERGE` with `ON CREATE SET` and `ON MATCH SET` | single pattern part |
| `SET`, `REMOVE` | property and label targets |
| `DELETE`, `DETACH DELETE` | |
| `WITH`, `RETURN` | `DISTINCT`, `ORDER BY`, `SKIP`, `LIMIT`; `RETURN` only at the end |
| `CALL proc(...) YIELD` | procedures, below |
| `CALL { ... }`, `CALL (vars) { ... }` | subqueries |
| `UNION`, `UNION ALL` | every part must end with `RETURN` and share column names |
| `EXPLAIN`, `PROFILE` prefixes | `EXPLAIN` returns the plan; `PROFILE` reports row counts (`lib.rs`) |

### Patterns

Node patterns take labels and a property map. Relationship patterns take one or more types separated by `|`, a variable, a property map, and a length range (`*`, `*2`, `*1..3`, `*..5`, `*2..`). Directions are `->`, `<-` and undirected `-`. `shortestPath` and `allShortestPaths` are supported for a single relationship pattern (`matcher.rs`).

### Expressions

Literals (decimal, hex and octal integers; floats; strings; booleans; `null`); parameters `$name`; property access; index and slice; `NOT`, `AND`, `OR`, `XOR`; comparisons; `IN`; `STARTS WITH`, `ENDS WITH`, `CONTAINS`, `=~`; `IS [NOT] NULL`; label tests `n:Label`; arithmetic including `^`; `CASE`; list and map literals; list comprehensions; pattern comprehensions; pattern predicates; `EXISTS { ... }`; `all`, `any`, `none`, `single`; `reduce`; `count(*)`.

### Functions

Function names are matched in lower case (`funcs.rs`). Aggregates: `count`, `collect`, `min`, `max`, `sum`, `avg`, `stdev`, `stdevp`, `percentilecont`, `percentiledisc`.

Scalar and list functions: `coalesce`, `id`, `elementid`, `labels`, `type`, `keys`, `properties`, `size`, `length`, `nodes`, `relationships`, `rels`, `startnode`, `endnode`, `head`, `last`, `tail`, `range`, `tointeger`, `tofloat`, `tostring`, `toboolean`, `tolower`, `toupper`, `trim`, `ltrim`, `rtrim`, `replace`, `split`, `substring`, `left`, `right`, `reverse`, `abs`, `ceil`, `floor`, `round`, `sqrt`, `exp`, `log`, `sin`, `cos`, `tan`, `sign`, `pi`, `e`, `rand`, `timestamp`, `exists`, `isempty`, `nullif`.

An unknown function returns `Unknown function '<name>'`.

### Procedures

Procedure names are matched case-insensitively (`crates/uriel-query/src/procs.rs`). An unknown procedure returns "There is no procedure with the name ... registered".

| Procedure | Arguments | Yields |
|---|---|---|
| `db.labels()` | none | `label` |
| `db.relationshipTypes()` | none | `relationshipType` |
| `db.propertyKeys()` | none | `propertyKey` |
| `db.schema.visualization()` | none | `nodes`, `relationships` |
| `seal.pagerank(config)` | `dampingFactor`, `maxIterations` | `nodeId`, `score` |
| `seal.betweenness()` | none (default config) | `nodeId`, `score` |
| `seal.wcc()` | none | `nodeId`, `componentId` |
| `seal.louvain()` | none (default config) | `nodeId`, `communityId` |

The four `seal.*` procedures read the whole graph into a compressed sparse row projection first (`procs.rs`, `project_all`), so their cost grows with the graph. Only `seal.pagerank` reads its `config` map; the others use the algorithm defaults.

```cypher
CALL seal.pagerank({dampingFactor: 0.85, maxIterations: 50})
YIELD nodeId, score
RETURN nodeId, score ORDER BY score DESC LIMIT 10
```

### Rejected

| Input | Result | Source |
|---|---|---|
| a write clause (`CREATE`, `MERGE`, `SET`, `REMOVE`, `DELETE`, `DETACH DELETE`) in read mode | `read-only access mode forbids write operation: <clause>` | `lib.rs`, `execute`; `analyze.rs` |
| `CREATE` with zero or several relationship types | runtime error "CREATE requires exactly one relationship type" | `write.rs` |
| `CREATE` with variable length | runtime error | `write.rs` |
| `CREATE` with an undirected relationship | runtime error "only directed relationships are supported in CREATE" | `write.rs` |
| `SET` or `REMOVE` target that is not a property (for labels, use `n:Label`) | `unsupported` | `write.rs` |
| index or constraint on a relationship type | `unsupported: relationship indexes and constraints` | `schema.rs` |
| index or constraint on several properties | `unsupported` | `schema.rs` |
| `shortestPath` with more than one relationship pattern | `unsupported` | `matcher.rs` |
| `CALL` of an unregistered procedure | semantic error | `procs.rs` |
| unknown function | semantic error | `funcs.rs` |

The `read_neo4j_cypher` tool runs in read mode, so any write clause is refused there (`crates/uriel-mcp/src/tools.rs`, `run_cypher`).

Read results are cached, keyed by the query text and its parameters, in a cache sized by the query-cache share. A commit invalidates the cached results that depend on the labels and relationship types it touched. Statements that call `rand()` or `timestamp()` are never cached (`lib.rs`, module docs).

## Memory tools

The memory tools keep notes in the same graph. Each entity is a node with the label `Memory` plus a label for its `type`. Its `observations` are a list property. Each relation is a relationship whose type is the relation type. A unique constraint on `Memory.name` is created when the database opens (`crates/uriel-mcp/src/db.rs`, `CREATE_CONSTRAINT`; `crates/uriel-mcp/src/memory.rs`). A failure to create it is printed to stderr and does not stop the server.

| Tool | Arguments | Writes | Returns |
|---|---|---|---|
| `create_entities` | `entities`: array of `{name, type, observations}` | yes | the entities written |
| `create_relations` | `relations`: array of `{source, target, relationType}` | yes | the relations written |
| `add_observations` | `observations`: array of `{entityName, observations}` | yes | the observations newly added per entity |
| `delete_entities` | `entityNames`: array of names | yes | `deleted_entities` count; relations go with them |
| `delete_observations` | `deletions`: array of `{entityName, observations}` | yes | `deleted_observations` count |
| `delete_relations` | `relations`: array of `{source, target, relationType}` | yes | `deleted_relations` count |
| `read_graph` | none | no | every entity and the relations among them |
| `search_memories` | `query`: text | no | entities whose name, type or observation contains the text (case-insensitive) |
| `find_memories_by_name` | `names`: array | no | entities with those exact names |

`create_entities` upserts: an existing name keeps its id, its observations are merged in order without duplicates, and its label is replaced when the type changes. `create_relations` requires both entities to exist, and reports the missing names otherwise ("entities not found: ..."). Type names and relation types must be non-empty and must not contain a backtick or a control character (`memory.rs`, `quote_ident`).

Read results are capped at 200 entities (`crates/uriel-mcp/src/guard.rs`, `MAX_ROWS`). A capped result carries `omitted` and a note to use `search_memories` or `find_memories_by_name`.

```json
{"entities": [{"name": "Ada Lovelace", "type": "Person", "observations": ["wrote the first published algorithm"]}]}
```

## Cypher and inspection tools

| Tool | Arguments | Writes | Notes |
|---|---|---|---|
| `get_neo4j_schema` | none | no | labels, relationship types, properties, indexes and constraints as JSON |
| `read_neo4j_cypher` | `query`, `params`, `format` | no | write clauses are refused; rows are compact by default |
| `write_neo4j_cypher` | `query`, `params`, `format` | yes | returns update counters: nodes and relationships created and deleted, properties set, labels added and removed |
| `tier_stats` | `format` | no | hot and warm counters, `disk_reads`, `bypassed`, `gate_rejected`, `invalidated`, `budget_bytes`, `stored_bytes`, `logical_bytes`, `compression_ratio` (`place_tools.rs`) |
| `calibrate` | `budget_secs` (default 8, maximum 20) | no | measures the latency staircase on this machine; writes `calibration.json` next to the database file, except with `--in-memory` or `--read-only` |
| `why_hot` | `node` | no | explains a node's region; needs `--placement planned` |
| `pin`, `unpin` | `node` | yes | pin or unpin the node's region; need `--placement planned` |
| `replay` | `events`, `regions`, `region_size`, `hot_bytes`, `warm_bytes`, `plan_every` | no | replays a placement event log and returns a SHA-256 digest of the decisions; the same log gives the same digest |

Under the default `--placement local`, `why_hot`, `pin` and `unpin` do not act. They return a note that no plan exists, and `pin` and `unpin` report `applied: false` (`place_tools.rs`, `NO_PLAN`).

`replay` takes at most 100,000 events and 65,536 regions, and it refuses work above 8,000,000 region decisions (`place_tools.rs`, `MAX_EVENTS`, `MAX_REGIONS`, `MAX_REPLAY_WORK`).

Tool output is cut at `--token-limit` characters (default 20,000) on a line boundary. The `more` tool returns the rest (`crates/uriel-mcp/src/config.rs`, usage text).

## Database location

The default path comes from `default_db_path` in `crates/uriel-mcp/src/config.rs`:

- Windows: `%USERPROFILE%\.seal\graph.seal`
- Linux and macOS: `~/.seal/graph.seal`

Set the path with `--db`. The `uriel` binary takes the same flag after its `mcp` subcommand (`crates/uriel-cli/src/commands.rs`, `install_command_line`).

```bash
uriel mcp --db ~/.uriel/graph.uriel
```

```powershell
uriel mcp --db "$env:USERPROFILE\.uriel\graph.uriel"
```

A read-write server creates the parent directory if it is missing (`crates/uriel-mcp/src/db.rs`, `open_waiting`). A read-only server whose file does not exist serves an empty in-memory graph and prints a message to stderr (`db.rs`, `open_once`).

## Server settings that affect the database

| Flag | Default | Effect |
|---|---|---|
| `--db <PATH>` | `~/.seal/graph.seal` | the database file |
| `--read-only` | off | hides and refuses every write tool; opens the file read-only |
| `--in-memory` | off | a throwaway database; nothing is saved |
| `--memory-mib <N>` | 50 (minimum 16) | the memory budget above |
| `--placement <M>` | `local` | `planned` turns on the placement plan and `pin`, `unpin`, `why_hot` |
| `--idle-release-secs <N>` | 30 | close the database after N idle seconds; 0 keeps it open |
| `--format <F>` | `compact` | default result format of `read_neo4j_cypher` and the RAG tools |

Sources: `crates/uriel-mcp/src/config.rs` (defaults and usage text).

## Locking

This section describes the locking behaviour of v0.2.9. It is the part most likely to change, so it is kept apart from the rest of the page.

**Exclusion.** The file has one `redb` lock. A second process that opens the same file gets `DatabaseAlreadyOpen`, which the core reports as `Error::is_already_open` (`crates/uriel-core/src/error.rs`). How a read-only open interacts with that lock is not checked on this page (see [Limits](#limits)).

**Waiting.** `crates/uriel-mcp/src/db.rs` retries only on that error:

- The total wait is `LOCK_WAIT`, 3 seconds.
- Retries start after 25 ms and double each time, up to 400 ms. Each sleep is capped at the time left.
- Any other open error fails at once with `cannot open database: ...`.

**The error.** When the wait runs out, the tool returns an error result with this text, using the real path:

```text
the graph <path> is locked by another uriel process (waited 3.0 s); retry the call in a few seconds
```

The server stays up, and the next call tries again (`crates/uriel-mcp/tests/lock_lifetime.rs`, `a_tool_call_on_a_held_database_is_an_error_result_naming_the_lock_and_the_server_lives`). At this layer the error is a message string, not a typed value. The typed check is `Error::is_already_open` in the core.

**Concurrent calls.** The database handle is opened under a slot lock. A second tool call in the same server waits on that lock while the first one opens the file, and so shares its 3-second limit.

**Idle release.** When the server has been idle for `--idle-release-secs` (default 30), it drops the open handle. That closes the file, so another process can open it. The next tool call reopens the file (`crates/uriel-mcp/src/server.rs`, `idle_window`, `release`; `crates/uriel-mcp/tests/lock_lifetime.rs`, `an_idle_server_releases_the_lock_and_the_next_call_reopens`). `--idle-release-secs 0` keeps the file open for the life of the server. An in-memory database is never released.

**Consequence.** Two Claude sessions that point at the same file cannot both hold it. The second one waits up to 3 seconds, then gets the error above. With idle release on, the first session gives the file up after 30 idle seconds.

## Backup

Copy the file only when no process holds it. A running server holds it while it is open, and an idle server holds it until its idle release fires. Stop the MCP client session (which ends the server), or wait past the idle window with the server still running, then copy:

```bash
cp ~/.uriel/graph.uriel ~/backups/graph-$(date +%F).uriel
```

```powershell
Copy-Item "$env:USERPROFILE\.uriel\graph.uriel" "$env:USERPROFILE\backups\graph-$(Get-Date -Format yyyy-MM-dd).uriel"
```

Copy the file before the first open with a new build, because the format upgrade is one-way (see [File format and upgrade](#file-format-and-upgrade)).

The Rust API can also export the whole graph as a command batch with `Graph::export_dump` and restore it with `Graph::restore_dump` (`crates/uriel-core/src/graph.rs`). No command in `crates/uriel-cli` exposes this export, so the copy route is the one a user runs.

## Limits

- **Single node.** The shipped server runs the graph on one node. Replication and distributed transactions are in `docs/SEAL-DB-V2.md` section 3 and are not wired to the server (`docs/SEAL-DB-V2.md`, "What this is not (yet)").
- **Schema coverage.** Relationship and multi-property indexes and constraints are refused. Index kinds are not used.
- **Byte counts.** Tier sizes are estimates, not allocator RSS.
- **Gate benefit.** The gate protects the hot set only when access is skewed (`docs/SEAL-DB-V2.md`, "Limits").
- **Read-only locking.** Whether a read-only open can coexist with a writer's `redb` lock, and whether it ever writes the file, was reasoned from `overlay.rs` and not run.

## See also

- [`../README.md`](../README.md): what Uriel is and how to install it.
- [`../PLAN.md`](../PLAN.md): the registry of built and open work, with the dated measurements cited above (entries A2, A6, A7).
- [`SEAL-DB-V2.md`](SEAL-DB-V2.md): the design of the tiers, the token ladder and the resilience layers.
- [`DESIGN.md`](DESIGN.md): the original crate map. It is partly historical.
