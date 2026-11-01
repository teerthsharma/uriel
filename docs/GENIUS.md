# Genius and the fleet

Two features, one idea: an agent is only as good as the structure around it, and structure belongs in code, not in a prompt. `genius` structures how a model reasons; `fleet` structures how many models run and what is seen of them.

## 1. What `/genius` does

`/genius <idea>` runs the idea through six lenses placed on a hexagon, then hands the result to the session model, which sits at the centre and **tests the strange parts instead of throwing them away**.

The working definition: a hallucination is an unlabeled conjecture. A model says "the cache is cold because compaction reorders the prefix" with the same voice it uses for `file:line` facts, and the reader cannot tell them apart. Genius makes the label mandatory and enforces it in code. A strange idea is accepted as a `wild` claim if, and only if, it carries the observation that would kill it, the cheapest way to look, and a cost of S or M. A crazy idea that cannot be cheaply tested is refused, because that is the one that is a hallucination.

### The thinkers, and why they are the vertices

| Vertex | Thinker | Operation | Failure it covers |
|---|---|---|---|
| `abduce` | Peirce | Abduction: from a surprising fact, the hypothesis that makes it a matter of course | Stopping at the first explanation |
| `analogize` | Koestler (bisociation), Gentner (structure mapping) | Carry a relation from a far field back | Staying inside the problem's own field |
| `question` | Socrates | Elenchus: expose assumed definitions and contradicting premises | A well-answered wrong question |
| `refute` | Popper | Name the observation that would prove it wrong, and prefer the test most likely to fail | Looking only for confirmation |
| `eliminate` | Holmes, Bayes | Rule out the impossible, then take the base-rate favourite | Novel mechanisms where a common one fits |
| `monitor` | Flavell; Nelson and Narens | Object level versus meta level: monitoring (up) and control (down) | Confident and wrong; unknown unknowns |

The cycle order is chosen so the three **opposite pairs** are three real tensions:

- `abduce` against `refute`: generate against destroy.
- `analogize` against `eliminate`: far field against base rate.
- `question` against `monitor`: interrogate the question against audit the answerer.

In round 2 each lens sees its two neighbours (refinement along the rim) and its opposite (tension across an axis), and nothing else. The verdict counts attacks across each axis, so a lens pair that never disagrees is itself information.

### The round structure

1. **Round 1, diverge.** Six independent briefs. Same shared text, different lens block.
2. **Round 2, cross-examine.** Each lens receives three neighbours' round-1 claims as fenced, sanitised data and must target at least one with `support` or `attack`. An attack must carry evidence or a falsifier.
3. **Centre, review.** The server computes a verdict. The session model walks `test_queue`, which lists **wild claims first** (they are the ones at risk of being thrown away), then conjectures, cheaper tests before dearer within each group, and `test_queue_total` says how many are queued beyond the ten shown. It runs the tests and reports `survived` or `killed` with evidence.

### Output limits are part of the protocol

The server cuts any tool response at 20,000 characters (`--token-limit`). A round-2 payload with all six briefs, each carrying three neighbours' claims, can pass 100 KB in the worst case (each shown claim was about 0.7 KB, 24 of them per brief, six briefs), and a cut-off JSON document silently breaks a run. So `genius_plan` returns metadata only, and `genius_brief {plan, vertex, round}` returns one brief per call as plain text to be used verbatim as the agent's prompt. Shown claims, the verdict's lists and evidence are capped, and a test builds maximal claims for every lens and asserts that every brief and every response stays under the limit. If you lower `--token-limit` below that, the protocol is not guaranteed to fit.

### What is deterministic and what is not

| Deterministic (pure functions, each with a test) | Not deterministic |
|---|---|
| The plan id, the briefs, the waves, the round-2 briefs | What a model writes in answer to a brief |
| What the validator accepts and rejects, and the typed error code | Whether a model repairs a rejected claim |
| The verdict, and its digest, whatever order lenses finish in | Whether the session model runs the tests it is shown |

The server cannot force a model to think. It can refuse everything that is not shaped like thinking: no falsifier, no evidence, no cheap test, no engagement with the other lenses. The model has to produce the structure to make progress.

### Statuses, and why nothing is ever "true"

`corroborated` (survived a test with evidence), `grounded` (an observation with evidence), `open`, `contested` (attacked, untested), `quarantined` (a wild claim not yet tested), `refuted` (killed by a test). There is no `confirmed`. This is Popper's asymmetry, and the same rule as house mode's Inspector tools, which can prove a claim impossible and never confirm one.

### The KV-cache and tier tricks, with their limits

- **Shared prefix first.** Every brief is `shared text + lens block`, and the shared text is byte-identical for the six lenses. A round-2 brief extends its own round-1 brief byte for byte. On the example idea the shared part is 1,153 bytes (about 287 tokens) and 73 % of the bytes of the six briefs. Self-hosted prefix caches (vLLM and SGLang are the usual examples) are the target; as far as I know they match at block granularity rather than with a per-model minimum, which I have not measured here.
- **Provider minimums apply.** A hosted cache has a minimum cacheable length per model. 287 tokens is probably under it, so `tier` mode should not be expected to hit a hosted cache. This was not measured against a hosted API.
- **`warm`.** A hosted cache entry only exists once the first response has begun, so six simultaneous calls all miss. `warm` runs the first lens alone. It costs one sequential step and only pays if the prefix is cacheable.
- **`fork` mode.** `genius_brief {plan}` returns the shared text once, into the session's context; `genius_brief {plan, vertex}` returns each short lens block, and the lenses are forks of that context. The prefix cache is then the session's whole cached context, which is large enough to count. The cost is that a fork runs on the session's own model, so there is no smaller tier. This is the real trade: **cache or tier, not both.**
- **Tier.** In `tier` mode lenses run on a model strictly smaller than the session (the same contract the `/seal` driver already uses), and only the centre runs on the large one. If no smaller model exists, the session answers the briefs itself, in order.

### What it does not do

- It does not make a small model smarter. Six small lenses help to the extent that their errors differ, and that is untested here. There is no benchmark in this repository showing that genius beats a single prompt. Treat the structure as a discipline for exploring and auditing, not as an accuracy claim.
- Evidence strings are recorded, not verified. `file:line` can be checked with `rag_expand`; the server does not do it for you.
- The CLI (`uriel genius`) prints the plan and briefs for any model without MCP. Validation and the verdict need the MCP server, because they are state.

## 2. The fleet: launch, observe, sanitise

The server does not start agents (that is the harness's job; see the two-modes table). It does the three things a harness cannot be trusted to do for itself.

### Admission: "launch max agents"

`fleet_admit {run, max: true}` grants exactly as many agents as four limits allow, and says which one bound:

| Limit | Default | Stops |
|---|---|---|
| Concurrency | 8 (`URIEL_FLEET_MAX`, 1 to 256) | A fan-out larger than the machine or the rate limit |
| Depth | 2 | A fork bomb: an agent that spawns agents that spawn agents |
| Lifetime total | 64 per run | A loop that keeps re-launching |
| Tokens | 2,000,000 per run | A runaway; further admission is denied and an overrun mark is written |

Ids are sequential and never reused. Over-cap requests are partly granted with the rest reported as `deferred`, so the caller runs the remainder in waves. Genius computes its waves from the same cap. `reap` frees agents that never reported, so a crashed harness cannot leave the fleet full for ever.

### Observation

Every admission, model call, tool call, mark and end is an event in a per-run **hash chain**: each event carries the hash of the one before it, so an edited, deleted or reordered event is detected (`chain_ok` in the summary). The event cap drops the oldest events but keeps an anchor, so memory is bounded and the chain still verifies. There is no wall clock in any decision; `t_ms` is stored if the caller supplies one.

`fleet_trace {format: jsonl}` exports one JSON object per line with fields named after the agent-trajectory style of NeMo Relay's observability plugin: scope start and end with an `agent` category, `llm` and `tool` categories, parent links for nested agents, token counts including cached input. **This is shaped for translation, not claimed to conform**: the NeMo Relay documentation host was not reachable from the build environment, so the mapping to the ATOF schema was designed from its public description and has not been checked against the spec. Checking it is the first follow-up.

### Sanitisation

Applied to everything an agent produces before it is stored, traced, echoed to another agent or used to build a prompt. Never applied to `rag_expand`, which stays byte exact.

| Layer | Handles |
|---|---|
| Strip | ANSI CSI and OSC sequences; C0 and C1 controls except newline and tab; carriage returns; bidi overrides and zero-width characters; Unicode tag characters and supplementary variation selectors (ASCII smuggling) |
| Redact | Private-key blocks (an unterminated one is redacted to the end), Anthropic, OpenAI, NVIDIA, GitHub, AWS, Google, Slack and Hugging Face keys, JWTs, `Bearer` tokens, `password=`-style assignments. Run to a fixed point so a secret nested in another match cannot hide |
| Bound | Hard input cap before any scan; per-field caps; output never exceeds its cap |
| Fence | Untrusted text re-entering a prompt is wrapped in `<<uriel-data N untrusted>>` where `N` is a hash of the content, so the content cannot contain its own closing fence. Forged fence markers inside content are neutralised and counted |
| Flag | Injection phrases are **counted, not removed**. A phrase list is not a defence, so the count only marks the text `suspect` |

Properties with tests: sanitising twice equals sanitising once over 3,000 seeded adversarial inputs; 2,000 random byte strings never panic and leave no control character; a megabyte of near-miss prefixes scans in linear time.

Known gaps, stated rather than hidden:

- No Unicode normalisation (NFKC), so look-alike characters pass.
- Redaction is pattern-based. A secret with no known prefix and no `key=` shape passes.
- Email addresses and other personal data are not redacted.
- An injection that reads as ordinary prose is not detectable here. The defence is structural: text from an agent can only ever be a typed claim that the validator accepts or refuses, and it enters later prompts only inside a fence.
