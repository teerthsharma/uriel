# The goal ledger and the observation deck

## The problem

An agent harness that decides by itself how many agents to launch tends to launch too few: each model call is cheap to defer and nothing measures the idle capacity. Two small deterministic pieces fix that without asking a model to be diligent.

## The ledger: tiny goals with dependencies

`ledger_add` keeps a bounded graph of tiny goals (at most 256, 16 dependencies each). The order is a **topological sort** (Kahn's algorithm): layer 0 is every goal with no dependency, layer 1 is every goal whose dependencies are all in layer 0, and so on. Inside a layer, ids sort lexicographically, so the same ledger always gives the same layers whatever order the goals were added in (a property test shuffles insertion order over random DAGs).

| Call | Effect |
|---|---|
| `ledger_add` | Adds goals. The whole batch is refused on a cycle, an unknown dependency, a self-dependency or a changed redefinition; goals are append-only, and adding an identical goal again is a no-op |
| `ledger_next` | Issues the ready frontier: pending goals whose dependencies are all `done`, earliest layer first. The default count is the free agent slots of the fleet run with the ledger's name, so it asks for exactly as much work as can start now |
| `ledger_done` | Finishes an issued goal as `done` or `failed`. A failed goal blocks everything downstream of it |

State is `pending`, `issued`, `done`, `failed`; `blocked` is derived. The ledger is saved as JSON next to the database (`uriel-ledger/<name>.json`, mode 0600, written to a temp file and renamed) and re-validated whenever it is loaded.

## Python finds the next prompts

`scripts/ledger_next.py <ledger.json> [--issued] [--limit N] [--cache DIR]` is a pure function from a ledger file to the next prompts. It never writes the ledger. It re-implements the layering and the ready rule independently of the Rust code, and a differential test checks that the two agree at every step of a drain over 25 random DAGs.

- Default mode previews the ready goals; `--issued` renders the goals the server has handed out.
- Every prompt opens with the same header (the shared prefix, so a prefix cache can reuse it), then the goal, then the results of its finished dependencies, each inside a content-hash fence.
- Prompts are cached under `prompt-cache/` next to the ledger, named by the SHA-256 of their content, so identical prompts are reused byte for byte. The cache is bounded to 512 files, refuses a symlinked directory, and its files are 0600.

## The observation deck

`fleet_trace {format: "deck"}`, and a `deck` line on every `fleet_*`, `genius_*` and `ledger_next` response:

```text
deck underutilised: launch 3 now (running 5/8, backlog 3, lost 214‰ of slot-ticks)
deck underutilised: launch 2 now, split work into 3 more independent units (running 1/6, backlog 2, ...)
deck saturated: launch none (running 8/8, backlog 4, ...)
```

All of it is integer arithmetic on counters the server already has:

- `backlog` is the number of ready units waiting for an agent. The ledger adds the goals it issues; genius adds its six lenses (and six more for round 2); a `fleet_admit` that could not be fully granted leaves the remainder.
- `launch_now = min(free slots, lifetime total left, backlog)`, and zero when the token budget is spent.
- `split_needed` appears only under the `max` policy (set by `fleet_admit {max: true}` and sticky): when free slots exceed the backlog, the deck says how many more independent units the work must be split into to saturate the cap.
- `lost` counts, per logical tick, the slots that were free and launchable while work was waiting. Time is the number of fleet calls, not the wall clock, so the number replays exactly.

Honest limit: the deck cannot launch anything. It makes under-use impossible to miss, puts the exact number in every response, and the conductor text tells the model to obey it first. A model that ignores it shows up in the lost-slot counter, not as a blocked call.
