# Dispatch: a deterministic router for team agents

`uriel-dispatch` is the control plane of a team coding agent, taken from DeepSeek's `dsh-superagent-prototype` (research prototype, MIT) and rebuilt as a pure state machine. It decides who may start work, what the work may include, which ticket holds which worker slot, and what a person must look at. It spawns nothing and reads no clock: the host feeds it timestamped inputs and carries out the effects it returns.

## Source and what was kept

The prototype's principle is kept as the design rule: take as little human attention as possible, give as much value as possible. Its domain model is kept under the same names, so the two documents read against each other.

| Prototype concept | Here | Gate |
|---|---|---|
| Gate, allowed list, bot list, admission table (3 cells per scope) | `policy::gate` | `tests/gate.rs` |
| Accept mode `off` / `observe` / `auto` | `AcceptMode`; observe records `WouldAccept` | `tests/eval.rs` |
| Policy version, revalidation before resume | `Router::version`, `policy::revalidate`, `changes_for` | `tests/gate.rs`, `tests/threads.rs` |
| Eval: 120 s, 2 retries, third expiry `unavailable` | `Router::timers`, `fail_run` | `tests/eval.rs` |
| Decision: accept, stop, ignore, unavailable (+ attach) | `Verdict`, `Decision` | `tests/eval.rs` |
| Mandate (immutable, basis recorded) | `Mandate`, `Basis` | `tests/gate.rs` |
| Ticket stages, first match wins | `Ticket::stage` | `tests/execution.rs` |
| Worker slots 1 to 16, +1 kept for a person | `Router::settle`, `SlotKind::Reserve` | `tests/execution.rs` |
| Prepare 10 min, background 30 min, redelivery 2 s doubling to 60 s | `RouterConfig`, `timers` | `tests/execution.rs` |
| Subscription fan-out, ownership, hand-back, release on end, resume | `Router::ingest`, `hand_back`, `end_ticket` | `tests/threads.rs` |
| Polling horizon 14 days | `Effect::Unpoll` | `tests/threads.rs` |

## What is different, and why

- **Pure and replayable.** `Router::apply(now, input) -> effects` is the only entry point. The clock is an argument and never goes backwards (`apply` clamps it). Every input and effect is hashed into a SHA-256 chain, so the same log gives the same digest on any machine. The prototype keeps its state in SQLite and reads the wall clock; it cannot make this claim. Gates: 4 seeds x 800 inputs replay to the recorded digest, also through a JSON round trip of the log; one extra input changes the digest.
- **RAM-aware slots.** Capacity is `min(max_workers, ram_slots)` where `ram_slots = (free - reserve) / worker_cost` and `reserve = max(2048 MB, 15 % of total)`, the same reserve as `uriel-house`'s governor (parity gate against `governor::reserve_mb` and `governor::slots`). Pressure shrinks capacity without killing running work and lifts it again when memory returns. The prototype uses a fixed count.
- **Idempotent and total.** A trigger id seen twice is a recorded `Duplicate`. Accepting a trigger twice returns the same ticket. An unknown ticket, a stale eval id or an out-of-order executor call returns `Effect::Rejected` and changes nothing; the random-stream gate drives 40 seeds x 600 inputs, including garbage ids, with slot and phase invariants checked after every step.
- **Observe becomes a ledger.** `observe_report()` counts `WouldAccept` per rule, so the move from `observe` to `auto` rests on numbers instead of reading rows by eye. It reports and does not decide: no threshold is invented.
- **Attention is ordered.** `attention()` lists only what waits for a person, most urgent first (answer, review, blocked or failed, then an unresolved eval outage). A healthy ticket takes no space.
- **Fails closed.** An unknown source, an unknown scope, a silenced source and an unknown or disabled rule all refuse.

## Judgement calls where the prototype's text is silent

Each has a test; change the test with the call.

- A `ready` report is superseded by a person's message on the ticket, like `blocked` and `failed`. Otherwise a ticket the executor is already working on again would still show as "awaiting review".
- A woken idle ticket drops its stale `waiting` report, so it reads `working` and not `idle` while its turn runs.
- A stop needs a non-bot person and any admitted strength. An unlisted person's stop is admitted in `observe` and `auto` and refused in `off`.
- A bot or CI message does not wake a ticket that waits on a person (`answer`, `review`, `blocked`); it is delivered and read on the next turn. It does wake an `idle` ticket.
- Only a person's message with an authority subscription (`Origin` or `Attached`) is owned by the executor; a `Context` subscription delivers and never owns.
- `Basis::Followup` is produced by `gate(.., has_ticket = true)`; the router calls that only to admit a stop, because every other follow-up on a live ticket reaches the executor without a gate.

## /afk: the trigger is a command, not an issue

The prototype listens to GitHub and Feishu. Here the Source is the person at the keyboard: typing `/afk 2h <goal>` is an explicit, addressed request from the one allowed identity, and the same gate that judges a GitHub comment judges it.

```
/afk 2h <goal>
  -> uriel afk start 2h            window opens, log created
  -> chart sub-goals               House (house_need, house_next) when present, else the model, max 12
  -> uriel afk add --goal-id G1.2  gate -> eval -> accept -> ticket -> worker slot, or queued
  -> Agent (background, worktree)  one worker per `start` action, under the mandate in the command
  -> uriel afk report N ready|ask|blocked|failed|done
  -> uriel afk end                 one summary: needs you, ready, unfinished, never started
```

| Part | Where | What it guarantees |
|---|---|---|
| Slash command | `crates/uriel-cli/assets/afk.md`, copied to `.claude/commands/afk.md`; `uriel afk init` installs it for any project or globally | A test fails if the two copies differ |
| Policy | `afk::afk_policy` | Accept mode `off`, one allowed identity, no bots: nothing inferred is ever admitted, so the agent acts only on what you typed |
| Window | `Input::Window`, `Effect::WindDown`, `Effect::WindowClosed` | After the close no slot is granted; running tickets get one wind-down notice and may still report; extending the window releases the queue |
| Slots | `RouterConfig::max_workers` (`--workers`, 1 to 16, default 3), `uriel afk` feeds free RAM on every call | A burst of sub-goals runs three at a time, fewer when memory is short, the rest wait |
| Log | `afk::Afk`, `.uriel/afk/session.jsonl`, an exclusive file lock | Every line carries the router digest after that step. A forged or edited line is refused with its line number; a half-written last line from a crash is dropped; a second process waits for the lock |
| Summary | `Afk::summary` | Sorted by what a person must do: needs you (with each question), ready, unfinished, never started, done. The log is archived as `.done` |

House keeps the goal tree, the RED-first test gate and the RAM governor for heavy test jobs; dispatch adds who may ask, how many workers run, the time window and the attention list. A goal id passed with `--goal-id` links a ticket to its House leaf.

Not enforced by code, only by the mandate text in the command: a worker staying inside its scope and its branch. The host cannot sandbox a subagent; run the agent under a dedicated OS account, as the prototype also warns.

## Not built

- No MCP tools. The command drives the `uriel afk` CLI; `dispatch_*` tools in house mode would move the tool-count goldens (41), so they are their own change.
- No GitHub or Feishu Source, and no worker runner of its own: workers are Claude Code subagents started by the command.
- Triggers are kept in memory for the life of the router. Pruning ended threads is not done.
- No token budget on what a ticket receives. The natural fit is the `view` ladder; it is not measured, so it is not claimed.
