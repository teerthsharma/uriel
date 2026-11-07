# Agent features

Uriel has six features for working with a team of coding agents and with its own memory. `genius` plans a multi-lens exploration of one idea and validates the claims it gets back. `fleet` admits agents against fixed limits and keeps a hash-chained trace of their calls. `ledger` keeps a small graph of dependent goals and issues the ones that are ready. `afk` runs a timed work window with gated tickets and a summary of what needs a person. `page` draws the deterministic HTML pages that `afk` uses. The Claude Code hooks keep the context that compaction would otherwise lose.

The CLI commands and the hooks never start an agent. The MCP tools record state and make decisions; the harness or the model starts the work and reports back.

The genius tools are experimental. No benchmark in this repository shows that genius beats a single prompt, and it runs only in house mode. Read [GENIUS.md](GENIUS.md) first.

## Contents

1. [Where each feature runs](#1-where-each-feature-runs)
2. [Genius](#2-genius)
3. [Fleet](#3-fleet)
4. [Ledger](#4-ledger)
5. [afk](#5-afk)
6. [page](#6-page)
7. [Claude Code hooks](#7-claude-code-hooks)
8. [Known limits](#8-known-limits)
9. [See also](#see-also)

## 1. Where each feature runs

| Feature | Surface | Mode | Source |
|---|---|---|---|
| `genius_plan`, `genius_brief`, `genius_submit` | MCP tools and the `genius` prompt | House only. Other modes answer `wrong_mode`. | `crates/uriel-mcp/src/agent_tools.rs`, `crates/uriel-mcp/src/tools.rs` (`house_only`) |
| `uriel genius` | CLI. Prints the plan without a server. | Any | `crates/uriel-cli/src/commands.rs` (`genius`) |
| `fleet_admit`, `fleet_event`, `fleet_trace` | MCP tools | Normal and house | `crates/uriel-mcp/src/agent_tools.rs` |
| `ledger_add`, `ledger_next`, `ledger_done` | MCP tools | Normal and house | `crates/uriel-mcp/src/ledger_tools.rs` |
| `scripts/ledger_next.py` | Python script. Reads a ledger file and never writes it. | Any | `scripts/ledger_next.py` |
| `uriel afk` | CLI | Any | `crates/uriel-cli/src/afk_cmd.rs` |
| `uriel page` | CLI | Any | `crates/uriel-cli/src/page_cmd.rs` |
| `uriel capture`, `uriel recall-context`, `uriel recall` | CLI. The first two run as hooks. | Any | `crates/uriel-cli/src/capture.rs`, `crates/uriel-cli/src/recall.rs` |

Tools marked `writes: true` are hidden when the server runs read-only. The `ledger_*` tools are marked that way. The `fleet_*` and `genius_*` tools are not, so they stay visible in read-only mode.

## 2. Genius

Genius runs one idea through six lenses placed on a hexagon. Each lens returns claims in a fixed shape. The server validates them, compares them across lenses and returns a verdict. The session model then tests the claims the verdict queues. The method and its reasons are in [GENIUS.md](GENIUS.md).

### 2.1 The six lenses

| Vertex | Thinker | Operation |
|---|---|---|
| `abduce` | Peirce | Find the hypothesis that makes a surprising fact a matter of course |
| `analogize` | Koestler, Gentner | Carry a relation back from a distant field |
| `question` | Socrates | Expose assumed definitions and contradicting premises |
| `refute` | Popper | Name the observation that would prove the idea wrong |
| `eliminate` | Holmes, Bayes | Rule out the impossible, then take the base-rate favourite |
| `monitor` | Flavell; Nelson and Narens | Audit the answer: monitoring and control |

The vertex names come from `crates/uriel-genius/src/conductor.rs` and the schema in `crates/uriel-mcp/src/agent_tools.rs`. The thinkers are from [GENIUS.md](GENIUS.md). In round 2, each lens sees its two neighbours and its opposite, and nothing else.

### 2.2 Protocol flow

1. `genius_plan` compiles the idea. The plan id is deterministic.
2. For each wave, `fleet_admit` grants agents. `genius_brief` returns each lens brief as plain text, to be used verbatim as the agent's prompt.
3. The session reports each agent's usage with `fleet_event`, then calls `genius_submit` with the agent's JSON and the agent id. A rejected submission returns an error and a `submit_rejected` mark on that agent. The error goes back to the same agent for one repair.
4. After the sixth round-1 submission, round 2 opens. Fetch round-2 briefs with `genius_brief` and `round: 2`.
5. After the sixth round-2 submission, the response carries the verdict. Its `test_queue` lists wild claims first, then conjectures. The session runs the tests and reports results with `genius_submit` and an `experiment` array.

Responses are capped at 20,000 characters by default (`--token-limit`, per [GENIUS.md](GENIUS.md)). This is why `genius_plan` returns metadata only and `genius_brief` returns one brief per call.

### 2.3 `genius_plan`

The response carries the plan id (`plan`), the protocol name (`uriel-genius/1`), `mode`, `waves` (the vertices to run together under the agent cap), the fleet `limits` and a `deck` line. It also seeds six units of backlog into the fleet run named by the plan id.

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `idea` | string | yes | none | The idea, question or surprising fact |
| `mode` | `"tier"` or `"fork"` | no | `"tier"` | `tier`: one full brief per lens, for a smaller model. `fork`: the shared text once plus six short lens blocks, for forks of the current context. |
| `warm` | boolean | no | `false` | Run the first lens alone first, so a provider prompt cache holds the shared prefix |

### 2.4 `genius_brief`

Returns one brief as plain text. In `fork` mode, round 1, call it once without `vertex` for the shared text, then once per vertex.

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `plan` | string | yes | none | Plan id from `genius_plan` |
| `vertex` | string | no | none | One of the six vertices. Omit for the shared text in `fork` mode. |
| `round` | `1` or `2` | no | `1` | Round 2 opens after the sixth round-1 submission |

### 2.5 `genius_submit`

Validates and records one lens's claims, or a batch of experiment results. Everything is sanitised first (section 3.5). After the sixth round-1 submission the response includes the round-2 briefs. After the sixth round-2 submission it includes the verdict. When `agent` is given with claims, the response includes a `fleet` object showing the agent was released. When experiments are given for claims bound to a house goal, it includes a `house` list.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `plan` | string | yes | Plan id |
| `round` | `1` or `2` | no | Must match the open round |
| `vertex` | string | no | The lens that produced the claims |
| `claims` | array of objects | no | The lens's claims (section 2.6). A conjecture or wild claim may carry `goal`. |
| `unknowns` | array of strings | no | What the lens could not determine |
| `experiment` | array of objects | no | Results for the verdict's `test_queue`: `{claim, outcome, evidence}`. `outcome` is `survived` or `killed`. Send it instead of `claims`, once both rounds are complete. |
| `agent` | string | no | Fleet agent id from `fleet_admit`. Its slot is released and the submission is traced. |
| `need` | string | no | House Need id. A claim's `goal` is checked against it. Default: the newest open Need. |

The schema marks only `plan` as required. The server checks the other fields when they are present.

### 2.6 Claim shape

Rules from `crates/uriel-genius/src/claim.rs`:

| Field | Rule |
|---|---|
| `id` | `<vertex>.c<N>` in round 1, `<vertex>.d<N>` in round 2. `N` is 1 or 2 digits, no leading zero. |
| `kind` | `observation`, `conjecture`, `wild` or `question` |
| `text` | Required. Capped at 400 (`TEXT_BYTES`). |
| `falsifier` | Required for `conjecture` and `wild`. Capped at 240 (`FIELD_BYTES`). |
| `test` | Required for `conjecture` and `wild`. Capped at 240. |
| `cost` | `S`, `M` or `L`. Required for `conjecture` and `wild`. |
| `confidence` | Number from 0 to 1. Default 0.5. |
| `evidence` | Array of strings. At most 6, each capped at 240. Required for `observation`. |
| `targets` | Array of claim ids. Round 2 only. Requires `stance`. |
| `stance` | `support` or `attack`. Round 2 only. An `attack` needs evidence or a falsifier. |
| `goal` | House goal id such as `G1.2`. Only on `conjecture` and `wild`. Alphanumeric and dots, 1 to 24 characters. |

A `wild` claim may not have cost `L`, and its confidence may not exceed 0.8. A lens may send at most 8 claims. Refusals name the claim and the field.

### 2.7 Statuses

The verdict gives each claim one status (`crates/uriel-genius/src/verdict.rs`): `corroborated` (a test survived), `grounded` (an observation with evidence), `open`, `contested` (attacked, untested), `quarantined` (a wild claim not yet tested), `question`, and `refuted` (a test killed it). There is no `confirmed`. A test can kill a claim or support it; nothing is proven true.

### 2.8 Binding claims to house goals

When a conjecture or wild claim carries `goal`, the server checks it against the Need's live goals. An unknown, retired or already refuted goal is refused before anything is stored. When an experiment reports on a bound claim, the server records a house fact `GX.<claim id>` with `by` set to `genius`. A `killed` outcome refutes the goal and blocks its subtree. A `survived` outcome records the fact and refutes nothing. Reaching a goal still needs a RED then GREEN test through `house_test_run`, because a hypothesis test is not a code test ([GENIUS.md](GENIUS.md), section 1a).

### 2.9 The `uriel genius` command

Prints the plan and the briefs for any model. Validation and the verdict need the server, because they hold state.

| Flag | Default | Meaning |
|---|---|---|
| `<idea words>` | none | The idea. Trailing words are joined with spaces. Required unless `--print-command`. |
| `--mode tier\|fork` | `tier` | Brief layout |
| `--warm` | off | Run the first lens alone first |
| `--max-agents N` | `8` | Agent cap used to compute the waves. Range 1 to 256. |
| `--json` | off | Print the plan as JSON. Cannot be combined with `--print-command`. |
| `--print-command` | off | Print the `/genius` slash command text and exit. No idea is needed. |

Without `--json`, the output is the conductor text, a line `plan <id> mode <mode> waves <waves>`, then the briefs: one per vertex in tier mode, or the shared text and six lens blocks in fork mode.

```bash
uriel genius "why does the prefix cache go cold after compaction"
uriel genius --mode fork --warm --max-agents 4 "why does the prefix cache go cold after compaction"
uriel genius --print-command > .claude/commands/genius.md
```

`--print-command` writes to standard output and does not install the file. The text starts with a `description` and `argument-hint: <idea>` header and ends with `Idea: $ARGUMENTS`. In PowerShell, use `uriel genius --print-command | Out-File -FilePath .claude\commands\genius.md -Encoding utf8`.

### 2.10 The `genius` prompt

In house mode the server exposes a prompt named `genius`. It takes one string argument, `idea`, and returns the conductor text with the idea attached. In other modes it returns an error that says to restart with `--mode house` (`crates/uriel-mcp/src/server.rs`).

## 3. Fleet

The fleet counts admissions and keeps a trace. The server does not start agents. A harness asks with `fleet_admit`, starts the agents, reports their calls with `fleet_event`, and releases each one with `kind: end`.

### 3.1 Limits

| Limit | Default | Setting | Stops |
|---|---|---|---|
| Concurrency | 8 | env `URIEL_FLEET_MAX`, 1 to 256, read at server start | A fan-out larger than the machine or the rate limit |
| Depth | 2 | not settable through the server | An agent that spawns agents that spawn agents |
| Lifetime total | 64 per run | not settable through the server | A loop that keeps launching |
| Tokens | 2,000,000 per run | not settable through the server | A runaway run. Once spent, admission is denied and an overrun mark is written. |
| Events | 4,096 per run | not settable through the server | Memory. The oldest events drop, and an anchor keeps the chain verifiable. |
| Runs | 16 | not settable through the server | Too many open runs |

Sources: `crates/uriel-fleet/src/fleet.rs` (`Limits::default`) and `crates/uriel-mcp/src/agent_tools.rs` (`FLEET_ENV`). The event and run figures come from the code; the docs do not state them. Ids are sequential and never reused.

```bash
URIEL_FLEET_MAX=4 uriel mcp --db ~/.uriel/graph.uriel
```

```powershell
$env:URIEL_FLEET_MAX = "4"; uriel mcp --db "$HOME\.uriel\graph.uriel"
```

### 3.2 `fleet_admit`

Asks to launch agents. Over-cap requests are partly granted and the rest are deferred, so the caller runs the remainder in waves. The response has the granted ids, `denied_by` (the limit that bound, if any), `limits` and `deck`.

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `run` | string | yes | none | Run id. Use the genius plan id so the trace and plan line up. |
| `count` | integer ≥ 0 | no | `1` | How many agents you want |
| `max` | boolean | no | `false` | Ask for as many as the limits allow. Overrides `count`. |
| `parent` | string | no | none | Fleet agent id of the spawning agent |
| `role` | string | no | `"agent"` | What the agents do, e.g. `lens` |
| `tier` | string | no | `""` | Model tier you will run them on, e.g. `small` |

`denied_by` is one of `concurrency`, `depth`, `total` or `token_budget` (`crates/uriel-fleet/src/fleet.rs`).

### 3.3 `fleet_event`

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `run` | string | yes | none | Run id |
| `agent` | string | yes, except for `reap` | none | Fleet agent id |
| `kind` | string | yes | none | `llm`, `tool`, `mark`, `end` or `reap`. `end` releases the slot. `reap` cancels every running agent of the run. |
| `name` | string | no | `""` | Model or tool name, or the mark |
| `tokens_in`, `tokens_out`, `cached_in` | integer ≥ 0 | no | `0` | Input, output and prompt-cache-served input tokens |
| `t_ms` | integer ≥ 0 | no | none | Caller timestamp. Stored, never used for decisions. |
| `attrs` | object of strings | no | none | Short attributes. Secrets and control bytes are removed before storing. |
| `status` | `done`, `failed` or `cancelled` | no | `done` | For `kind: end` |
| `note` | string | no | `""` | For `kind: end` |

Responses carry the event, a `sanitised` report for `llm`, `tool` and `mark` events, and a `deck` line. `reap` returns `reaped`, the number of agents cancelled.

### 3.4 `fleet_trace`

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `run` | string | yes | none | Run id |
| `format` | `summary`, `jsonl` or `deck` | no | `summary` | `summary`: counts by state, tokens, limits, `chain_ok` and the head hash. `jsonl`: every event, one JSON object per line. `deck`: the observation deck as JSON. |

Each event is hash-linked to the one before it, so an edited, deleted or reordered event breaks `chain_ok`. `jsonl` uses the schema `uriel.fleet.v1` (`crates/uriel-fleet/src/trace.rs`). It is shaped for translation to an agent-trajectory format and is not claimed to conform to it ([GENIUS.md](GENIUS.md)).

### 3.5 Sanitisation

Applied to everything an agent produces before it is stored, traced, echoed to another agent or used in a prompt. Never applied to `rag_expand`.

| Layer | Handles |
|---|---|
| Strip | ANSI sequences; C0 and C1 controls except newline and tab; bidi overrides; zero-width characters; Unicode tag characters and variation selectors |
| Redact | Private-key blocks; keys from Anthropic, OpenAI, NVIDIA, GitHub, AWS, Google, Slack and Hugging Face; JWTs; `Bearer` tokens; `password=`-style assignments. Run to a fixed point. |
| Bound | Input cap before any scan; per-field caps |
| Fence | Untrusted text re-entering a prompt is wrapped in `<<uriel-data N untrusted>>`, where `N` is a hash of the content. Forged markers are neutralised and counted. |
| Flag | Injection phrases are counted, not removed. A count marks the text `suspect`. |

Known gaps, as [GENIUS.md](GENIUS.md) states them: no Unicode normalisation, so look-alike characters pass; redaction is pattern-based, so a secret with no known prefix and no `key=` form passes; email addresses and other personal data are not redacted; an injection written as ordinary prose is not detected. The defence is structural: agent text becomes only a typed claim, and it enters later prompts only inside a fence.

### 3.6 The deck line

Every `fleet_*`, `genius_*` and `ledger_next` response ends with a `deck` line computed by the server:

```text
deck underutilised: launch 3 now (running 5/8, backlog 3, lost 214‰ of slot-ticks)
deck underutilised: launch 2 now, split work into 3 more independent units (running 1/6, backlog 2, ...)
deck saturated: launch none (running 8/8, backlog 4, ...)
```

These examples are from [LEDGER.md](LEDGER.md). `launch_now` is the smallest of free slots, lifetime total left and backlog, and zero once the token budget is spent. The deck cannot launch anything. An agent that ignores it shows up in the lost-slot counter.

### 3.7 Error codes

Errors are `{"error": "<code>", "message": "..."}`. Fleet and genius handlers return codes such as `unknown_run`, `admit_failed`, `event_failed`, `reap_failed`, `unknown_goal`, `house`, `wrong_mode` and `unknown_tool` (`crates/uriel-mcp/src/agent_tools.rs`, `crates/uriel-mcp/src/tools.rs`).

## 4. Ledger

The ledger holds tiny goals with dependencies. `ledger_next` issues the goals whose dependencies are done. A failed goal blocks everything downstream of it.

### 4.1 `ledger_add`

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `ledger` | string | yes | Name: 1 to 64 characters from `A-Z a-z 0-9 _ . : -`. Also the fleet run id that the ledger's backlog feeds. |
| `goals` | array, at most 64 | yes | Goal objects (below) |

Each goal object:

| Key | Type | Rule |
|---|---|---|
| `id` | string | Required |
| `title` | string | Required. One line, at most 200 bytes. |
| `detail` | string | Optional, at most 600 bytes |
| `needs` | array of strings | Optional, at most 16 goal ids. Each must exist in the ledger or in the same batch. |

A ledger holds at most 256 goals. The whole batch is refused on a cycle, an unknown dependency, a self-dependency or a changed redefinition. Goals are append-only; an identical goal added again is counted as `unchanged`.

### 4.2 `ledger_next`

| Parameter | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `ledger` | string | yes | none | Ledger name |
| `limit` | integer ≥ 0 | no | free slots of the fleet run with this name, or the concurrency limit if that run does not exist | Ready goals to issue |

Returns `issued` (a list of `{id}`), `summary`, `deck`, `file` and `prompts`. `prompts` is the command `python3 scripts/ledger_next.py <file> --issued` when the ledger has a file on disk. Issued goals are queued as fleet backlog.

### 4.3 `ledger_done`

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `ledger` | string | yes | Ledger name |
| `id` | string | yes | Goal id, issued by `ledger_next` |
| `outcome` | `done` or `failed` | yes | Result |
| `result` | string | no | One or two sentences, at most 400 bytes. It is passed, fenced, into the prompts of dependent goals. |

### 4.4 Goal states and order

A goal is `pending`, `issued`, `done` or `failed`. `blocked` is derived: a pending goal with a failed goal among its transitive dependencies. A ready goal is pending with all dependencies `done`. The order is a topological sort (Kahn's algorithm) with ids sorted inside each layer, so the same goals give the same layers whatever order they were added in (`docs/LEDGER.md`, `crates/uriel-ledger/src/ledger.rs`).

### 4.5 Files on disk

The ledger is saved as JSON after every write, in a `uriel-ledger` folder beside the database: `<database folder>/uriel-ledger/<name>.json`. The schema name is `uriel.ledger.v1`. Writes go to a temporary file in the same folder, then rename.

With the database at `~/.uriel/graph.uriel`, the ledger `release` is `~/.uriel/uriel-ledger/release.json`. On Windows, with the database at `$HOME\.uriel\graph.uriel`, it is `$HOME\.uriel\uriel-ledger\release.json`.

- Prompts written by `scripts/ledger_next.py` are cached in `prompt-cache/`, next to the ledger, named by the SHA-256 of their content. The cache is bounded to 512 files ([LEDGER.md](LEDGER.md)).
- The file is set to mode 0600 on Unix only (the code sets it under `cfg(unix)`).
- A ledger file over 1 MiB is refused on load. The server keeps at most 16 ledgers open.
- Loading re-validates the file, so a hand-edited ledger must still pass the same checks.
- With no writable database folder, the ledger stays in memory, and `prompts` becomes a note instead of a command (`crates/uriel-mcp/src/ledger_tools.rs`).

### 4.6 `scripts/ledger_next.py`

A read-only function from a ledger file to the next prompts. It re-implements the layering and the ready rule on its own, and a differential test checks that it agrees with the Rust code at every step of a drain over 25 random graphs ([LEDGER.md](LEDGER.md)).

```bash
python3 scripts/ledger_next.py ~/.uriel/uriel-ledger/release.json
python3 scripts/ledger_next.py ~/.uriel/uriel-ledger/release.json --issued --limit 8
```

```powershell
python scripts\ledger_next.py "$HOME\.uriel\uriel-ledger\release.json" --issued
```

| Flag | Default | Meaning |
|---|---|---|
| `<ledger.json>` | none | The ledger file |
| `--issued` | off | Render prompts for issued goals. Without it, the script previews ready goals. |
| `--limit N` | `64` | How many goals to render |
| `--cache DIR` | none | Prompt cache directory |
| `--no-cache` | off | Do not read or write the cache |

Each prompt opens with the same header, so a prefix cache can reuse it. Then comes the goal, then its finished dependencies' results, each inside a content-hash fence.

## 5. afk

`afk` runs a timed work window. The `/afk` slash command charts the work into departments, leaves and checklists, and the `uriel afk` CLI gates each ticket, limits the workers and keeps the log. The design is in [DISPATCH.md](DISPATCH.md).

`uriel afk` does not start workers. The `/afk` command starts them with the Agent tool. [DISPATCH.md](DISPATCH.md) also describes `dispatch_*` MCP tools that mirror some of these steps; this page does not cover them.

### 5.1 Global flag and subcommands

The global `--file PATH` sets the log. Its default is `.uriel/afk/session.jsonl`, relative to the current folder.

| Command | Arguments | Output |
|---|---|---|
| `uriel afk start <duration> [--workers N]` | Duration such as `2h`, `90m` or `1h30m`. `--workers` defaults to 3, range 1 to 16. | `{started, owner, minutes, workers, closes, file}`. Refuses while a session is open. |
| `uriel afk add [--goal-id ID] <text...>` | Free text for one-off work. `--goal-id` links the ticket to a House leaf. | `{ticket, actions}`. A `{"do":"start",...}` action means start that worker now. No action means the ticket is queued. |
| `uriel afk add --leaf LEAF` | Leaf id such as `research.1.2`. Cannot be combined with text. | Same, opened inside the leaf's department |
| `uriel afk plan <spec.json>` | Plan file (5.4). Accepted once per window. | `{leaves, departments}` |
| `uriel afk check <leaf> <index> [--undo]` | Ticks checklist item `<index>`, counted from 0 | `{leaf, index, done, plan}` |
| `uriel afk report <ticket> <kind> [--note TEXT]` | `kind`: see below | `{actions}`. New `start` actions are the next workers to launch. |
| `uriel afk status` | none | Window, slots, tickets, and any `wind_down` action |
| `uriel afk pages [--out DIR]` | `DIR` defaults to `.uriel/afk/pages` | `{dir, pages}` with each file's digest |
| `uriel afk end` | none | Summary sorted by what needs a person, and `log`, the archived path |
| `uriel afk init [--project]` | Without `--project`, installs `afk.md` into `~/.claude/commands`. With it, into `./.claude/commands`. | `{installed, use}` |

Report kinds: `ready` (work is on a branch, awaiting review), `done` (nothing for the user to review), `ask` (the note is a question for the user), `blocked` and `failed` (the note says why), and `waiting`. `waiting` is accepted by the CLI, and neither `assets/afk.md` nor DISPATCH.md describes it.

### 5.2 The slash command

`uriel afk init` installs `crates/uriel-cli/assets/afk.md`. The arguments are a duration, an optional `--no-merge`, and the goal. The text tells the router to:

1. Open the window with `uriel afk start`, then write a plan and run `uriel afk plan`.
2. Open one ticket per leaf with `uriel afk add --leaf`, and start a worker, in the background and in its own worktree, for each `start` action.
3. Tick checklist items with `uriel afk check`, then report each result with `uriel afk report`.
4. Merge each `ready` branch with `git merge --no-ff` into the branch current at start, one at a time, and re-run the sub-goal's test on the merged tree. A conflict or a failing test aborts the merge and marks the ticket `blocked`.
5. Run `uriel afk pages` and `uriel afk end`, then print the summary.

`--no-merge` is handled by the command text, not by the CLI. The text also turns merging off for a dirty tree or a detached HEAD, and forbids pushing, force-pushing and deleting branches. A test in `crates/uriel-cli/src/afk_cmd.rs` checks that the repository's `.claude/commands/afk.md` matches the file the binary installs.

Uriel does not sandbox workers. The mandate limits their scope, and [DISPATCH.md](DISPATCH.md) recommends running agents under a dedicated operating-system account.

### 5.3 Log, policy and slots

- The log (`.uriel/afk/session.jsonl`) stores the router digest after each step. An edited or forged line is refused with its line number. A half-written last line is dropped. An exclusive file lock makes a second process wait.
- The policy accepts triggers from one identity and no bots. The agent acts only on what the person typed.
- After the window closes, no slot is granted. Running tickets get one wind-down notice and may still report.
- Slots are shared between departments by load. A department with a deficit is served first, and no slot stays idle while work is queued ([DISPATCH.md](DISPATCH.md), `dept::distribute_ranked`).
- The summary lists needs-you items first, then ready, unfinished, never started and done. The log is then archived as `.done`.

### 5.4 Plan file

`uriel afk plan` takes JSON. Each department owns a goal and splits into sub-goals; each sub-goal splits into leaves; each leaf has a checklist.

```json
{"departments":[
  {"name":"Research","goal":"Find the fix","subgoals":[
    {"goal":"Reproduce","leaves":[
      {"goal":"Failing test","checklist":["seen failing","minimal"]}]}]},
  {"name":"Build","goal":"Land it","subgoals":[
    {"goal":"Implement","leaves":[
      {"goal":"Code","checklist":["compiles"]}]}]}
]}
```

Ids are positional, `<department>.<sub-goal>.<leaf>`. The first leaf of the department named `Research` is `research.1.1`. The tests in `afk_cmd.rs` use lower-case department names. Caps: 12 departments, 24 sub-goals per department, 48 leaves per sub-goal, 32 checklist items per leaf, and 300 characters per text field (`crates/uriel-dispatch/src/dept.rs`). Every leaf needs at least one checklist item. A tampered plan line in the log is refused.

### 5.5 Pages

`uriel afk pages` writes `index.html`, the board, and one `department-<slug>.html` per department. The same log always gives the same bytes. The pages are drawn with the `uriel page` toolkit (section 6). The pages have no scripts and no external requests.

## 6. page

`uriel page` draws one self-contained HTML file from a JSON spec. The output has no JavaScript, no external requests, no web fonts and no timestamps unless the spec contains a date. The toolkit's API is in [`crates/uriel-page/API.md`](../crates/uriel-page/API.md).

| Command | Arguments | Output |
|---|---|---|
| `uriel page build <spec.json> [--out FILE]` | Without `--out`, HTML goes to standard output. With it, the file is written (its folder is created) and `{file, bytes, digest}` is printed. | HTML or JSON |
| `uriel page demo [--out DIR]` | `DIR` defaults to `docs/pages`. Writes `index.html` and `department-research.html`. | `{dir, pages}` |

```bash
uriel page build board.json --out site/board.html
```

```powershell
uriel page build board.json --out site\board.html
```

The spec's top-level keys are `title` (required), `subtitle`, `chart_id`, `ward`, `attending`, `theme`, `nav`, `blocks` and `footer`. Block types are `vitals`, `table`, `checklist`, `progress`, `timeline`, `notes`, `section`, `columns` and `rule`. Unknown fields are errors, and the input is capped at 1 MiB. Limits: 6 nesting levels, 500 blocks, 2,000 characters per string, 256 KiB of text, 500 table rows, 5,000 cells. Links may be relative, `#anchors`, `http(s)` or `mailto:`. Other schemes fail validation. Field-level detail is in the API file above.

## 7. Claude Code hooks

These hooks keep what compaction would lose. Before a compaction, `uriel capture` stores the transcript as fractions of about 600 tokens, `ctx/<session>/<n>.md`. When a compacted session starts, `uriel recall-context` returns the summary and the most relevant fractions. The full design is in [claude-code-hooks.md](claude-code-hooks.md).

### 7.1 Settings block

Merge this into `~/.claude/settings.json` (on Windows, `%USERPROFILE%\.claude\settings.json`). Use the same `--db` in every hook and every `uriel recall` call (section 7.5).

```json
{
  "hooks": {
    "PreCompact": [
      { "hooks": [ { "type": "command", "command": "uriel capture --db ~/.uriel/graph.uriel" } ] }
    ],
    "SessionStart": [
      {
        "matcher": "compact",
        "hooks": [ { "type": "command", "command": "uriel recall-context --capture --db ~/.uriel/graph.uriel --budget 1200" } ]
      }
    ],
    "SessionEnd": [
      { "hooks": [ { "type": "command", "command": "uriel capture --db ~/.uriel/graph.uriel" } ] }
    ]
  }
}
```

Copied from [claude-code-hooks.md](claude-code-hooks.md). `--capture` matters: hooks of one event run in parallel, and two commands would race for the database lock. One command opens the database once.

### 7.2 `uriel capture`

Reads the hook payload from standard input and stores the transcript. Runs on `PreCompact` and `SessionEnd`.

| Flag | Default | Meaning |
|---|---|---|
| `--db PATH` | `~/.seal/graph.seal` (see 7.5) | Database to write |
| `--session ID` | from the payload's `session_id`, else the transcript file name | Session key |
| `--transcript PATH` | from the payload's `transcript_path` | Transcript to read |

The payload fields read are `session_id`, `transcript_path`, `hook_event_name`, `trigger`, `source` and `cwd`. Input is capped at 1 MiB, and the transcript at 512 MiB. A line over 2 MiB is skipped.

Stored text is user and assistant text, tool names with a 160-character input summary, and tool results cut to 600 characters. Thinking blocks are skipped. Each fraction holds about 600 tokens (4 characters per token, so about 2,400 characters) and carries 60 tokens of overlap. It opens with a one-line topic, the first 120 characters of its first substantive text. Re-running stores nothing new; a grown transcript rewrites only its tail. One run writes at most 256 fractions, and a scan keeps at most 8,192.

### 7.3 `uriel recall-context`

Runs at `SessionStart` with matcher `compact`. It prints one hook JSON object:

```json
{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "..."}}
```

| Flag | Default | Meaning |
|---|---|---|
| `--db PATH` | `~/.seal/graph.seal` (see 7.5) | Database to read |
| `--budget N` | `1200` | Token budget, clamped to 1 to 8,000 |
| `--capture` | off | Store the compaction summary first, under the same database open |

The newest summary comes first, then the best fractions for the recent topic. The query is the compaction summary, or without one the last three user prompts plus the folder name. Output is capped at 24 KiB. With nothing to return, or with the database held by another process, it prints `{}`.

### 7.4 `uriel recall`

Searches captured context from a shell. It opens the database read-only and never writes or upgrades it. It is not a hook.

| Flag | Default | Meaning |
|---|---|---|
| `<query words...>` | none | Required unless `--expand` is given |
| `--db PATH` | `~/.seal/graph.seal` (see 7.5) | Database to read |
| `--budget N` | `1200` | Token budget, clamped to 1 to 8,000 |
| `--k N` | `8` | Hits to consider, clamped to 1 to 64 |
| `--expand ID` | none | Print the stored text of one chunk. Cannot be combined with query words. |

```bash
uriel recall --db ~/.uriel/graph.uriel --budget 1200 --k 8 release tag commit
uriel recall --db ~/.uriel/graph.uriel --expand 123
```

```powershell
uriel recall --db "$HOME\.uriel\graph.uriel" --expand 123
```

The first form prints `// file:start-end #chunk` citations at the resolution ladder (top hit in full, then the next level, then one-line gists) under the budget. The second prints one chunk. Output is plain text, capped at 24 KiB, and identical for identical input. No match prints `uriel recall: no match` to standard error and exits 0.

### 7.5 Database path

**Warning.** Without `--db`, `uriel capture`, `uriel recall-context` and `uriel recall` use `default_db_path()`, which resolves to `~/.seal/graph.seal` (`crates/uriel-mcp/src/config.rs`). `uriel mcp` uses the same default when `--db` is omitted (`crates/uriel-cli/src/main.rs`). `capture` and `recall-context` open the database for writing, so a run without `--db` can change that file. Always pass `--db`.

The hook examples in [claude-code-hooks.md](claude-code-hooks.md) write to `~/.uriel/graph.uriel`, but its recall examples read `~/.uriel/ctx.uriel`. The commands read and write whatever file they are given, so the hooks and the recall searches see each other only when they share one path. Pick one path and use it in all three places.

Hooks never block a session. Both hook commands exit 0 on every error, write one line to standard error, and read at most 512 MiB of transcript. A hook that finds the database locked by the MCP server prints one line and leaves the database alone.

## 8. Known limits

- The genius token cost (10 to 40K tokens per run) is an estimate from byte counts in [GENIUS.md](GENIUS.md), not a measurement. No benchmark here shows that genius beats a single prompt.
- The protocol checks structure. It cannot make a model reason well. Six small lenses help only to the extent that their errors differ, and that is untested.
- Evidence strings are recorded, not verified. `rag_expand` can check a `file:line`; the server does not.
- The fleet does not start agents and does not enforce that an agent stays in its scope.
- The deck cannot launch anything. It makes idle capacity visible in every response.
- Dispatch triggers and threads are kept in memory for the life of the router. Pruning ended threads is not done ([DISPATCH.md](DISPATCH.md)).
- [DISPATCH.md](DISPATCH.md) says the afk router has no MCP tools, yet it also lists `dispatch_*` tools. The server source contains `dispatch_tools.rs`, which this page did not read, so the status of those tools is not stated here.
- No measurement of recall quality appears in the documents read for this page.

## See also

- [GENIUS.md](GENIUS.md): the genius method, its reasons and its experimental status
- [LEDGER.md](LEDGER.md): the goal ledger, the observation deck and the prompt script
- [DISPATCH.md](DISPATCH.md): the router behind `uriel afk`, departments and pages
- [claude-code-hooks.md](claude-code-hooks.md): the hook settings and the context-recall design
- [`crates/uriel-page/API.md`](../crates/uriel-page/API.md): the page toolkit's spec and limits
