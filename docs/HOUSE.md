# House mode

House mode is the part of the uriel MCP server for work that is disputed or heavy. A Need (what a project requires to be complete) is charted by three fellows into one goal tree. The harness validates that tree, schedules its leaves under a RAM-aware governor, and accepts a goal as `reached` only when its test was red on the first recorded run and green on the latest. Every state change is also appended as one JSON line to a board event log. The server spawns no agents: the calling session dispatches the batches the harness returns.

## Contents

- [What house mode is for](#what-house-mode-is-for)
- [Starting house mode](#starting-house-mode)
- [The Need, the goal tree and the differential](#the-need-the-goal-tree-and-the-differential)
- [House tools (`house_*`)](#house-tools-house_)
- [Inspector tools (`research_*`)](#inspector-tools-research_)
- [Dispatch tools (`dispatch_*`)](#dispatch-tools-dispatch_)
- [The governor and RAM-aware scheduling](#the-governor-and-ram-aware-scheduling)
- [Board events: `house-events.jsonl`](#board-events-house-eventsjsonl)
- [The flake guard and the result cache](#the-flake-guard-and-the-result-cache)
- [Tool counts by mode](#tool-counts-by-mode)
- [See also](#see-also)

## What house mode is for

Use house mode when one question has competing theories, when several agents return findings that may contradict each other, or when test and build jobs are heavy enough that they must share a machine. Normal mode is for cached context and cited answers. House mode is for keeping a disputed question as a graph, checking every claim of "done" against a test, and deciding how many jobs may run at once.

The main pieces, from `crates/uriel-house/src/lib.rs`:

| Piece | Source | Role |
|---|---|---|
| Harness facade | `crates/uriel-house/src/house.rs` | Needs, charts, merge, next batch, report, facts, leap, status |
| Validator | `crates/uriel-house/src/validate.rs` | Rules 1 to 10 on each chart and on the merged tree |
| Governor | `crates/uriel-house/src/governor.rs` | Slot math, command wrapping, cost learning |
| Smart tests | `crates/uriel-house/src/smart.rs` | Result-cache keys, flake quarantine, test selection |
| Event sink | `crates/uriel-house/src/sink.rs` | Appends board events to `house-events.jsonl` |
| MCP wiring | `crates/uriel-mcp/src/house_tools.rs`, `research_tools.rs`, `dispatch_tools.rs` | Tool schemas and routing |

## Starting house mode

The server has exactly two modes. Pick one at start:

```bash
uriel mcp --db ~/.uriel/graph.uriel --root /path/to/project --mode house
```

```bash
export URIEL_MODE=house
uriel mcp --db ~/.uriel/graph.uriel --root /path/to/project
```

```powershell
uriel mcp --db "$HOME\.uriel\graph.uriel" --root "C:\src\myproject" --mode house
```

```powershell
$env:URIEL_MODE = "house"
uriel mcp --db "$HOME\.uriel\graph.uriel" --root "C:\src\myproject"
```

To register it with Claude Code, put the same arguments after `--`; the server name is your choice:

```bash
claude mcp add uriel-house -s user -- uriel mcp --db ~/.uriel/graph.uriel --root /path/to/project --mode house
```

Rules, from `crates/uriel-mcp/src/config.rs` and `crates/uriel-mcp/src/tools.rs`:

- `--mode` beats `URIEL_MODE`. A value other than `normal` or `house` is a usage error.
- A normal-mode server neither lists nor accepts a `house_*`, `research_*`, `dispatch_*` or `genius_*` tool. A call returns a typed error with `"error": "wrong_mode"` and a hint to restart with `--mode house`.
- `--root` must be an existing directory. The `house_*` tools resolve file paths (`reads`, `writes`, covered files) against it.

### Server flags that matter for house mode

| Flag | Default | Effect |
|---|---|---|
| `--mode <normal\|house>` | `normal` (or `URIEL_MODE`) | Selects the tool set |
| `--db <PATH>` | `~/.seal/graph.seal` | Graph database. House state is stored in it |
| `--root <PATH>` | current directory | Project root for house file paths |
| `--events <PATH>` | `<root>/house-events.jsonl` | Board event log |
| `--config <PATH>` | `~/.seal/config.json` if the file exists | JSON file with model slots and governor settings |
| `--read-only` | off | Hides every tool with `writes: true` |
| `--tools <adaptive\|full>` | `adaptive` | `full` lists every tool at start; `adaptive` lists the core and reveals groups when they are called |
| `--idle-release-secs <N>` | `30` | Closes the database after N idle seconds so another uriel process can open it; `0` never closes |

The default config path is derived from the default database path, so `--db` does not move it (`crates/uriel-mcp/src/config.rs`, `default_config_path`).

### Environment variables

| Variable | Effect | Source |
|---|---|---|
| `URIEL_MODE` | `normal` or `house` when `--mode` is absent | `config.rs` |
| `SEAL_DR_HOUSE_MODEL` | Overrides the `dr_house` model slot after the config file is read | `crates/uriel-house/src/config.rs` (`DR_HOUSE_ENV`) |
| `SEAL_MCP_FAKE_PROBE` | `FREE_MB,TOTAL_MB` replaces the real RAM probe. Used by tests | `crates/uriel-mcp/src/services.rs` |

## The Need, the goal tree and the differential

### Needs

A **Need** is one project's requirement, given with acceptance criteria (`house_need`). The first Need is `G0`. A later Need is `N2.G0`, `N3.G0` and so on, and every goal id under it carries the same prefix (`N2.G1`, `N2.G1.1`). Several Needs share one database and one governor (`README.md`, "House mode").

Most tools take an optional `need`. When it is absent the tool acts on the newest Need that is not complete. `house_status` names the Need it describes and the newest open one (`current`).

### Goal ids and the tree

Goal ids are positional: `G1.2` is the second child of `G1`. Ids sort naturally, so `G1.2` comes before `G1.10` (`crates/uriel-house/src/types.rs`, `GoalId`). `G0` is the virtual root of a Foreman chart.

Each goal has a status:

| Status | Meaning | Source |
|---|---|---|
| `pending` | Not started | `types.rs`, `GoalStatus` |
| `active` | Dispatched to an agent by `house_next` | `house.rs`, `next_in` |
| `reached` | Proven: RED-first gate passed (see below) | `house.rs`, `report_reached` |
| `blocked` | Cannot proceed. A refuted goal is blocked with reason `refuted` | `house.rs` |
| `retired` | Replaced or refuted; never runs again | `types.rs` |

A leaf carries a tier. `sonnet` leaves run as **residents** and `haiku` leaves as **nurses** (`types.rs`, `Tier::role`). The token caps per leaf are 40,000 for a resident and 8,000 for a nurse (`types.rs`, `Tier::token_cap`). A nurse leaf must be read-only: its class must be one of `read`, `grep`, `run_test`, `collect` or `format` (`validate.rs`, rule 5).

### The three fellows (the differential)

Three fellows chart the same Need from three stances. Each submits its own chart with `house_chart`. The merge is deterministic.

| Fellow | Stance | What the chart contains | Merge rule |
|---|---|---|---|
| Foreman | Structure | The tree, root cause first | Backbone of the merged tree |
| Chase | Failure modes | Guard goals (`guards`) that name the goal they protect | Each failure mode becomes a test leaf under the goal it guards |
| Cameron | Alternatives | Alternative routes (`alt_of`), each with a cost | Each set of alternatives keeps the cheapest route that no recorded fact refutes |

Sources: the fellow descriptions in `crates/uriel-mcp/src/house_tools.rs` and the merge rules in `README.md`, "House mode". The cost of an alternative is `tokens + secs * alt_secs_weight`, with `alt_secs_weight` 20.0 by default (`crates/uriel-house/src/config.rs`).

A chart is accepted or rejected as a whole. A rejected chart returns its violations by rule, so only that fellow has to chart again. A fellow with three or more rejections on one Need is listed in `house_status.escalated`.

### Validation rules

`validate.rs` checks each chart on its own, and the merged tree after merge. A violation carries its rule number, the fellow who must change the chart, and the goal.

| Rule | Check |
|---|---|
| 1 | Parent links form a tree; `needs` form a DAG; no duplicate ids |
| 2 | Depth at most 6 (`MAX_DEPTH`) and fan-out at most 7 (`MAX_FAN_OUT`) |
| 3 | A non-leaf has at least two children |
| 4 | A leaf has a tier, a test command and an estimate |
| 5 | A haiku leaf is read-only (class in `read`, `grep`, `run_test`, `collect`, `format`) |
| 6 | Token caps per tier |
| 7 | Two leaves that write the same file are ordered |
| 8 | Test names are distinct within a Need; a name bound in another Need is refused |
| 9 | Every path a leaf reads or writes exists, or is declared in `creates` |
| 10 | A Chase node names `guards`; a Cameron node names `alt_of` |

Rule `0` marks a merge conflict: a goal id outside the Need's namespace, a missing backbone, or a re-chart required (`types.rs`, `Violation`; `house.rs`, `rule0`).

### Facts and refutation

A **fact** is a verified statement recorded with `house_fact`. Its `by` defaults to `Wilson`, the fact checker. A fact that carries `refutes` kills the named goal and its whole subtree at once: the goal is blocked with reason `refuted`, and when a merged tree exists the re-merge runs immediately (`house.rs`, `add_fact_in`). If that re-merge fails validation, the fact is kept and `house_status.merge_stale` is `true` until the next `house_merge` reports why.

### Re-charts and the leap

Each accepted chart by the fellow that owns a refuted goal spends one re-chart attempt. `max_rechart` (default 2) is the number of attempts allowed before the goal is blocked as a **leap candidate** (`config.rs`; `house.rs`, `submit_chart`).

`house_leap` gives a leap candidate one dispatch to **Dr House**, a single leap at maximum effort for a dead subtree. The dispatch names the `dr_house` model, the `dr_house_fallback` model, effort `max` and a 300-second box (`house.rs`, `leap_in`). A goal gets one leap; a second call errors. A goal that is not a leap candidate returns `{"leap": false, ...}` and no dispatch.

### RED-first gate

`house_report` accepts `reached` only when all of the following hold (`house.rs`, `report_reached`):

1. The goal's bound test is the test named in the evidence, and the evidence status is `green`.
2. The test is not quarantined by the flake guard.
3. The test has at least one recorded run, the first recorded run was `red` and the latest is `green`.

For a **leaf**, the first-run check is strict. A **parent** is reached only when all its children are reached; its evidence is the bound test of its first live leaf descendant, and only the latest-run-green check applies to it.

A refused report returns `outcome: unproven` (or `rejected`) and the call errors with the reason; the goal stays `active`.

## House tools (`house_*`)

Eleven tools, all in `crates/uriel-mcp/src/house_tools.rs`. Parameters marked **required** must be present. Tools marked *writes* are hidden under `--read-only`.

| Tool | Writes | Required | Optional | Purpose |
|---|---|---|---|---|
| `house_need` | yes | `project`, `goal`, `acceptance` (array of strings) | | State a Need; returns its goal id |
| `house_chart` | yes | `fellow`, `chart` | `need` | Validate and store one fellow's chart |
| `house_merge` | yes | | `need` | Merge the stored charts into one validated tree |
| `house_fact` | yes | `fact` | `need` | Record a verified fact; a `refutes` fact kills a goal and its subtree |
| `house_next` | yes | | `need` | Return the next batch of dispatchable leaves inside the governor |
| `house_report` | yes | `goal`, `status` | `need`, `evidence` | Report a goal `pending`, `blocked` or `reached` |
| `house_test_run` | yes | `test`, `status` | `need`, `secs`, `peak_ram_mb`, `covered_files` | Record one test run |
| `house_leap` | yes | `goal`, `cause` | `need` | Give a dead leaf-candidate goal its one Dr House leap |
| `house_status` | no | | `need` | JSON snapshot of the Need, goal counts, lists and facts |
| `house_select_tests` | no | `files` (at least one) | `need` | Tests to run after the given files changed |
| `house_config` | no | | | Model slots, governor settings, root, event log, config file |

### Parameters in detail

**`house_need`**: `project` is the name of the patient (the project); `goal` is the Need itself; `acceptance` lists the acceptance criteria. Returns `{goal, project, acceptance}` with the count of criteria.

**`house_chart`**: `fellow` is one of `Foreman`, `Chase`, `Cameron`. `chart` is an object with `goals` (array, required) and optionally `need`, `fellow`. Each goal may carry `id`, `parent`, `text`, `leaf`, `tier` (`sonnet` or `haiku`), `class`, `needs`, `writes`, `reads`, `creates`, `test` (`{name, cmd, class}`), `est` (`{ram_mb, secs, tokens}`), `guards` and `alt_of`. A `need` argument that conflicts with `chart.need` is an error. A rejection returns `{"rejected": true, "count": N, "violations": [...]}`.

**`house_merge`**: returns the merge report: `goals`, `leaves`, `alt_groups`, `retired` (goal and reason pairs), `pending_rechart`, `leap_candidates`.

**`house_fact`**: `fact` is an object with `source` (file:line or other provenance) and `text` (both required), and optionally `id` (assigned as `F<n+1>` when absent), `by` (default `Wilson`) and `refutes` (a goal id). The schema allows no other keys. Returns `{recorded, id}`.

**`house_next`**: returns a batch. Each dispatch carries `goal`, `model`, `role` (`resident` or `nurse`), a `prompt_pack` (Need text, acceptance criteria, goal text, ancestor texts, `reads`, `writes`, `creates`, `class`, `guards`), a `test` (name, command, class), `limits` (`CARGO_BUILD_JOBS` and the wrapped command), `cost_mb`, `class` and `integration` (true for a parent's integration run). Dispatched goals become `active`. The batch also carries `governor` (see [The governor](#the-governor-and-ram-aware-scheduling)) and `complete`, which is true once the root is reached. Calling `house_next` before a merge errors.

**`house_report`**: `status` is `pending`, `blocked` or `reached`. `reached` requires `evidence`: `{test, status, output}` with `status` `red` or `green`. Accepted and recorded outcomes return the outcome. Unproven and rejected outcomes return an error whose body is the same JSON, and the goal does not change.

**`house_test_run`**: records one run of a bound test. `status` is `red` or `green`. `secs` (default 0) is wall-clock time. `peak_ram_mb` (default 0) is the measured peak RSS and feeds the cost model. `covered_files` lists the files the test covers. Runs feed the RED-first gate, the result cache, the flake guard and the cost model.

**`house_leap`**: `goal` must be a leap candidate. Returns the leap dispatch (`goal`, `model`, `fallback_model`, `effort`, `timeout_secs`, `nurses`, `input`). The `input` carries the dead theory, its subtree, the cause of death and the facts that refute the subtree.

**`house_status`**: returns `need`, `merged`, `merge_stale`, `complete`, `goals` (counts: `total`, `pending`, `active`, `reached`, `blocked`, `retired`), `current`, `needs` (one entry per Need), `goal_list`, `active`, `retired`, `pending_rechart`, `leap_candidate`, `leap_candidates`, `quarantined`, `escalated`, `alt_groups` and `facts`.

**`house_select_tests`**: `files` lists the files that changed. Returns the tests in order, each with `test`, `cmd`, `level` (`L0`, `L1` or `L2`), `action` (`run`, `reuse_green` or `quarantined`), `last_failed` and `est_secs`. Affected leaf tests (`L0`) come first, then the integration tests of their parents (`L1`). `L2`, the full suite, is produced only when `allow_full_suite` is `true` (default `false`, command `cargo test --workspace`). Within a level, recently failed tests come first, then the cheapest.

**`house_config`**: takes no arguments. Returns `models`, `governor`, `settings`, `root`, `events`, `config_file` and `dr_house_env` (the name of the Dr House variable, `SEAL_DR_HOUSE_MODEL`).

### Model slots

The harness maps each role to a model string. Defaults in code (`crates/uriel-house/src/config.rs`, `ModelSlots::default`):

| Slot | Role | Default in code |
|---|---|---|
| `fellows` | Foreman, Chase, Cameron | `opus` |
| `residents` | Sonnet leaves | `sonnet` |
| `nurses` | Haiku leaves | `haiku` |
| `wilson` | Fact checker | `opus` |
| `inspector` | Auditor | `opus` |
| `dr_house` | The one leap | `opus` |
| `dr_house_fallback` | Used if the `dr_house` dispatch fails | `opus` |

The config file replaces any key it sets. `SEAL_DR_HOUSE_MODEL` replaces `dr_house` after the file is read. The `wilson` and `inspector` slots are stored and reported by `house_config`, but no code path in this repository dispatches to them; the harness uses `residents`, `nurses`, `dr_house` and `dr_house_fallback` (`house.rs`).

### Configuration file

`--config` takes a JSON file with the keys `allow_full_suite`, `full_suite_cmd`, `reserve_floor_mb`, `reserve_pct`, `heavy_cap`, `max_agents`, `cargo_jobs`, `alt_secs_weight`, `max_rechart`, `cost_window` and `models` (the seven slots above). Absent keys keep their defaults; unknown keys are ignored; `project_root` is never read from the file (`crates/uriel-house/src/config.rs`, `HouseConfig::from_file`). A missing or malformed file is an error at start.

## Inspector tools (`research_*`)

Three tools, in `crates/uriel-mcp/src/research_tools.rs`. Their descriptions name them as the Inspector's skill. The server does not check who calls them; the Inspector role is a convention in the tool text and in the `inspector` model slot.

The audits apply one-sided floors from the Caustic bounds (orbit bounds T1, T1\*, T6, T6\*, T8 and pooling bound T2), implemented in the `uriel_algo::caustic` module that the file imports. **They can prove a claim impossible. They can never confirm one.** A clean result means only that no violation was found, and the `note` field says so.

| Tool | Writes | Required | Optional | Purpose |
|---|---|---|---|---|
| `research_orbit_audit` | no | `injective` and one of `relation` or `edge_type` | `admissible`, `claimed_errors`, `claimed_precision`, `claimed_recall` | Test claims about a relation against the orbit floors |
| `research_pooling_audit` | no | `recoveries` and one of `relation` or `edge_type` | | Test recovery claims against the T2 ceiling of 1/k |
| `research_record` | yes | `tool`, `arguments`, `goal` | | Run one audit and write an `AUDITED` edge from a goal |

### Parameters in detail

**Relation input.** Either `relation` (an array of `{entity, answer}`, at most 10,000 entries) or `edge_type` (read the relation from graph edges of that type). Not both. An entity given two different answers is skipped and counted under `skipped`. Entity and answer strings are limited to 512 bytes each.

**`research_orbit_audit`**:
- `injective` (required boolean): the caller's premise that distinct entities have distinct correct answers. Every bound depends on it. `false` claims nothing: the result has `bound: "none"` and an `inapplicable` entry.
- `admissible` (array of strings, at most 20,000): the set of correct answers without the pairing. The starred bounds need at least as many distinct values as entities.
- `claimed_errors` (integer ≥ 0): the most wrong answers the claimant admits to. With no claim at all, it defaults to 0.
- `claimed_precision` and `claimed_recall` (numbers in [0, 1]): `claimed_recall` needs `admissible`.

Returns `violated`, `bound`, `n`, `orbits`, `admissible_orbits`, `claims`, `checks` (one per bound with its floor and claim), `inapplicable`, `witnesses` (at most 20, each listing at most 20 entities), `witnesses_truncated`, `skipped` and `note`.

**`research_pooling_audit`**: `recoveries` is a non-empty array of `{entity, recovery}`, where `recovery` is in [0, 1] and `entity` must be in the relation. An entity whose answer is shared by k entities cannot be recovered with probability above 1/k. Returns `violated`, `bound: "T2"`, one check per claimed entity with its ceiling and block size, and witnesses.

**`research_record`**: `tool` is `research_orbit_audit` or `research_pooling_audit`. `arguments` is the input of that audit. `goal` must match a `Goal` node by its `gid` property, or the call errors. It runs the audit, then writes an `Audit` node keyed by the tool name and the SHA-256 of its input, and an `AUDITED` edge from the goal with the verdict `violation` or `no_violation_found` and a `why` field (the tool name and the input, truncated at 2,000 characters). The write is idempotent per input. The result includes `recorded: {goal, audit, verdict}`.

## Dispatch tools (`dispatch_*`)

Seven tools, in `crates/uriel-mcp/src/dispatch_tools.rs`, backed by the `uriel-dispatch` crate. They open a time window for unattended work. Sub-goals become gated tickets, and a limited number of workers run them. The design, its gates and its caps are in [DISPATCH.md](DISPATCH.md).

| Tool | Writes | Required | Optional | Purpose |
|---|---|---|---|---|
| `dispatch_open` | yes | `duration` | `workers` (1 to 16, default 3) | Open a window: `90m`, `2h` or `1h30m`, between 1 minute and 24 hours |
| `dispatch_plan` | yes | `plan` | | Set the department plan once per window |
| `dispatch_check` | yes | `leaf`, `index` | `done` (default `true`) | Tick or untick one checklist item of a leaf |
| `dispatch_add` | yes | `leaf` or `scope` | `goal_id` | Gate and open a ticket for a plan leaf or a one-line scope |
| `dispatch_report` | yes | `ticket`, `kind` | `note` | Report a worker's outcome and free its slot |
| `dispatch_status` | yes | | | Window left, slots, tickets and their stage |
| `dispatch_close` | yes | | | End the window and return the summary |

Notes from the registration and code:

- `dispatch_plan` takes `plan: {departments: [{name, goal, subgoals: [{goal, leaves: [{goal, checklist: [string]}]}]}]}`. Ids are positional (`research.2.1`). The caps (12 departments, 24 sub-goals per department, 48 leaves per sub-goal, 32 checklist items, 300 characters per text) are stated in `docs/DISPATCH.md`.
- `dispatch_add` returns `{ticket, actions}`. An action with `start` means start a worker now; no action means the ticket is queued.
- `dispatch_report`'s `kind` is one of `ready`, `done`, `ask`, `blocked`, `failed` or `waiting`. `ready` means awaiting review; `ask` means the question needs the user.
- `dispatch_open` returns `{open, minutes, workers, closes}`. The workers value in the response is clamped to 1 to 16.
- `dispatch_close` returns the summary and the archive path of the log, and hides the dispatch group in adaptive listing (`crates/uriel-mcp/src/guard.rs`).
- All seven tools are registered with `writes: true`, so `--read-only` hides all of them, including `dispatch_status`. The window log is `<root>/.uriel/afk/session.jsonl`.

The `Not built` section of `docs/DISPATCH.md` predates these tools; the list above is the registered one.
## The governor and RAM-aware scheduling

`house_next` and the dispatch window both ask the governor how many slots each job class may use. Free RAM is read from the system on every call and is never cached.

### The formula

For a heavy class (unit tests and builds):

```
reserve = max(reserve_floor_mb, ceil(total_ram_mb * reserve_pct / 100))
slots   = 0                                            if free <= reserve
        = min(class_cap, floor((free - reserve) / cost_mb))   otherwise
```

With the defaults, `reserve_floor_mb` is 2048 and `reserve_pct` is 15. `governor.rs` tests the reserve against 16,000 MB total (2,400 MB) and 8,000 MB total (2,048 MB).

### Classes

| Class | Jobs (`JobClass::parse`) | Cap | Cost used before any observation | Reserve applies |
|---|---|---|---|---|
| `llm` | model calls | `max_agents` (20) | 0 MB | No |
| `light` | `read`, `grep`, `collect`, `format` | `max_agents` | 50 MB | No |
| `test_unit` | `test_unit`, `run_test` | `heavy_cap` (6) | 800 MB | Yes |
| `build` | `build` | `heavy_cap` (6) | 1,750 MB | Yes |
| `bench` | `bench` | 1 | 1,750 MB | Yes; runs only when free RAM is above the reserve |

Sources: `crates/uriel-house/src/governor.rs` (`class_cap`, `default_cost_mb`, `slots`) and `types.rs` (`JobClass::parse`). `heavy_cap` is the global cap on heavy jobs. `max_agents` is the cap on concurrently active agents, the in-app subagent cap.

### Deferral reasons

Each leaf that is ready but not dispatched gets a deferral line (`types.rs`, `DeferReason`):

| Reason | Meaning |
|---|---|
| `ram` | Not enough RAM above the reserve |
| `heavy-cap` | The global heavy cap is full |
| `file-lock` | Another active goal holds a file this leaf writes |
| `needs` | A goal this leaf depends on is not reached |
| `bench-exclusive` | A bench is running or draining |
| `agent-cap` | The concurrent agent cap is full |

`house_next` returns the governor report with `free_mb`, `total_mb`, `reserve_mb`, `heavy_active`, `heavy_slots` and the deferrals.

### Cost learning

Each `house_test_run` with `peak_ram_mb` above zero, for a class other than `llm`, adds an observation. A class's cost is the mean of its last `cost_window` observations (default 5); before any observation, the declared default applies (`governor.rs`, `CostModel`).

### Command wrapping (Windows)

Each dispatched test command is wrapped as (one line):

```
set CARGO_BUILD_JOBS={cargo_jobs} && start /belownormal /wait /b {command}
```

This wrapper is Windows-specific (`governor.rs`, `wrap_command`); `limits.env` carries the same `CARGO_BUILD_JOBS` value for any platform. On macOS and Linux, wrap the command yourself.

The probe reads memory through `sysinfo` on every call (`crates/uriel-house/src/probe.rs`); `SEAL_MCP_FAKE_PROBE` replaces it for tests.

## Board events: `house-events.jsonl`

The harness appends one JSON object per line to the event log (`crates/uriel-house/src/sink.rs`, `JsonlSink`), created on the first event. A failed write is ignored, so a board write never fails a harness call.

The default path is `<root>/house-events.jsonl`; override it with `--events`. The live board reads this file. The board page and its `gates.py` checker are not in this repository.

Every event has a `t` field. The kinds the harness emits (`crates/uriel-house/src/house.rs`):

| `t` | Emitted by | Main fields |
|---|---|---|
| `chart` | An accepted chart | `agent` (fellow), `goals` (`id`, `text`, `parent`) |
| `goal` | A goal changes status | `agent`, `id`, `status` (`active`, `reached`, `blocked`, `retired`), `by` (model), `haiku` (0 or 1), `test` |
| `theory` | A goal dies (refuted or retired by cost) | `id`, `text`, `status: "dead"` |
| `route` | A dead theory is replaced | `theory`, `shape` (`reroute` or `reprice`), `to`, `because` |
| `fact` | A fact is recorded | `id`, `by`, `source`, `text`, `refutes` (when present) |
| `test` | A test run is recorded | `agent`, `status`, `name`, `quarantined` (when set) |
| `nurse` | A haiku leaf is dispatched | `agent`, `n`, `model`, `task` |
| `dispatch` | A sonnet leaf, or the Dr House leap, is dispatched | `agent`, `model`, `task` |
| `leap` | A Dr House leap is given | `id`, `goal`, `text`, `minutes`, `model` |
| `done` | The root is reached | `agent` (one event per charting fellow) |

## The flake guard and the result cache

Source: `crates/uriel-house/src/smart.rs`.

- **Result-cache key.** `sha256(test command, content hash of every covered file, Cargo.lock hash)`. A GREEN run with the current key is reused by `house_select_tests` (`reuse_green`). A RED run is never reused.
- **Quarantine.** A test whose result flips while its key is unchanged is quarantined. The guard needs a non-empty set of covered files, because with none an unchanged key proves nothing. A quarantined test cannot prove a goal: `house_report` refuses `reached` with "test is quarantined by the flake guard". `house_status.quarantined` lists these tests, and `house_select_tests` reports them as `quarantined`.

## Tool counts by mode

The counts in `README.md` ("Modes" and "Tools") are what `tools/list` returns with `--tools full`:

| | normal | house |
|---|---|---|
| All tools | 36 | 60 |
| With `--read-only` | 21 | 29 |
| Listed at start (default `--tools adaptive`) | 17 | 22 |

House mode adds 24 tools in the current source, counted from the `name` fields in each module:

| Group | Tools | Count |
|---|---|---|
| `house_*` | `house_need`, `house_chart`, `house_merge`, `house_fact`, `house_next`, `house_report`, `house_test_run`, `house_leap`, `house_status`, `house_select_tests`, `house_config` | 11 |
| `research_*` | `research_orbit_audit`, `research_pooling_audit`, `research_record` | 3 |
| `dispatch_*` | `dispatch_open`, `dispatch_plan`, `dispatch_check`, `dispatch_add`, `dispatch_report`, `dispatch_status`, `dispatch_close` | 7 |
| `genius_*` | `genius_plan`, `genius_brief`, `genius_submit` (experimental; see [GENIUS.md](GENIUS.md)) | 3 |
| Total added | | 24 |

`uriel-mcp` decides visibility in `tools.rs` (`visible`): a tool whose name starts with `house_`, `research_`, `genius_` or `dispatch_` is hidden unless the mode is house. Under `--read-only`, every tool with `writes: true` is also hidden. Of the tools on this page, the five that remain are `house_config`, `house_status`, `house_select_tests`, `research_orbit_audit` and `research_pooling_audit`.

## See also

- [DISPATCH.md](DISPATCH.md): the time-window control plane, slot rules, departments and the `/afk` command
- [GENIUS.md](GENIUS.md): the experimental `genius_*` tools and the `/genius` command
- [LEDGER.md](LEDGER.md): the goal ledger that feeds fleet backlog
- [SAFETY.md](SAFETY.md): trust boundaries and residual risk
- [../README.md](../README.md): install, the mode table and the full tool list
- [../PLAN.md](../PLAN.md): the house and dispatch gates, with the test file for each claim
