# Revamp: the MCP surface, assessed by deduction and fixed

Each finding below was deduced from the code and, where it could be, reproduced against the built server before it was fixed. Every fix has a gate in `crates/uriel-mcp/tests/revamp.rs` or `src/guard.rs`; a mutation that undoes the fix fails 2 tests.

## Findings

| # | Weakness (evidence) | Fix |
|---|---|---|
| 1 | **A notification could write.** `handle_message` ran the method before checking for an id, so a `tools/call` sent without an id executed `create_entities` and discarded the reply. Reproduced: `read_graph` then showed the entity. | A request method sent as a notification is ignored; only `notifications/*` is handled without an id. |
| 2 | **Unbounded input line.** The reader used `lines()`. A client sending a long line without a newline grew memory past the 50 MiB budget the rest of the server defends. | `read_bounded`: lines over 8 MiB are dropped without being buffered, the client gets one -32600 error, the next request is served. |
| 3 | **`tools/list` cost 6.5k tokens (normal) and 10.4k (house) before any work.** 55 % of it was schema, with `additionalProperties:false`, empty `required`, a duplicated annotation title and a 300-byte `format` description repeated per tool. | Adaptive listing: the core tools first, groups reveal as they are used (below). Schemas are slimmed in this mode. Measured at start, list plus instructions: normal 6.6k to 2.6k tokens (-61 %), house 10.6k to 3.0k (-71 %). `--tools full` keeps the old list byte for byte. |
| 4 | **`--token-limit` was named tokens, counted characters, and cut anywhere.** A cut gave no way to read the rest, so the model re-ran the query and paid twice. | Cuts land on a line boundary and keep the remainder under a handle; `more(h=N)` returns the next page. The remainder store is capped at 2 MiB, oldest first. Pages rebuild the full text exactly (tested). |
| 5 | **A repeat read cost the full answer again** (`get_neo4j_schema`, `read_neo4j_cypher`, `read_graph`, the memory searches, `tier_stats`). Only `rag_*` and `view` had session dedup. | A repeated read of 200 tokens or more whose result is unchanged returns `=same (N tokens sent earlier; fresh=true resends)`. A changed result is sent whole; errors and small results never stub. |
| 6 | **Result size was bounded only after the result was built.** A Cypher read or `read_graph` serialised every row before the character cut. | At most 200 rows are serialised; the rest are counted (`+N rows not shown; add LIMIT or SKIP`, or `omitted` in JSON). `read_graph` caps entities the same way. |
| 7 | **No notion of how much a session had already spent.** | A session ledger with two thresholds (`--soft-tokens` 40k, `--hard-tokens` 100k, 0 disables): past them every output cap drops to a half and then a quarter of the limit, once-only notice in text results, never inside JSON. `usage` reports tokens, calls, stubs, cuts and the listed groups. |
| 8 | **Capabilities said `listChanged:false`; tool visibility could not follow the work.** | `listChanged:true` in adaptive mode and one `notifications/tools/list_changed` when a group is revealed. Unlisted tools still work. |

## Activation: threshold and sequence

- **Core, always listed:** `get_neo4j_schema`, `read_neo4j_cypher`, `rag_index`, `rag_search`, `rag_expand`, `seal_remember`, `seal_recall`, `view`, `more`, `usage`, `tools_enable`; in house mode also `house_need` and `dispatch_open`.
- **Groups, listed on demand:** `write`, `memory`, `place`, `rag` (feedback, soul, session_reset), `house`, `research`, `dispatch`.
- **Sequential:** `house_need` reveals `house` and `research`; `dispatch_open` reveals `dispatch` and `dispatch_close` hides it again; calling any tool of a group reveals the rest of its group; a failed entry call reveals nothing. `tools_enable(group[, off])` does it by hand.
- **Threshold:** the soft and hard token thresholds above.

## dispatch_* tools (house mode, writable servers)

`dispatch_open`, `dispatch_add`, `dispatch_report`, `dispatch_status`, `dispatch_close` drive the same log as `uriel afk`, so a session started in one can be read in the other. They are house-only (`wrong_mode` otherwise) and hidden with `--read-only`.

## Counts

Full list: 30 tools in normal mode (27 + `more`, `usage`, `tools_enable`), 51 in house mode (+11 house, +3 research, +7 dispatch: open, plan, check, add, report, status, close). Listed at start in adaptive mode: 11 and 13.

## Token counting and the reducer (second pass)

- **A real counter.** The repo counted `characters / 4`. Against the recorded real cl100k counts of 1,042 frozen outputs that rule is off by 28 % on average and undercounts the total by 29 %, so every threshold built on it fired late. `uriel_terse::count_tokens` is a character-class estimator (word runs, digit runs, newlines, punctuation, non-ASCII) fitted on those counts: 3.3 % mean error, total within 3 %. Gated by `crates/uriel-terse/tests/tokenizer.rs` over 24 recorded samples with their real counts. The ledger, the repeat stub and the reducer use it. Limits: one corpus (this repository's own compact outputs), cl100k only, and the fit is on interleaved samples, not a separate hold-out.
- **The reducer.** A fixed pipeline on every successful tool output, on by default with adaptive tools (`--reduce auto|on|off`, `URIEL_REDUCE`): `trim` (trailing blanks and blank runs) then `data` (the lossless terse encoding of JSON results of 300 tokens or more, kept only when it saves 15 % or more and decodes back to the same value). Exact tools (`rag_expand`, `more`, `terse_stats`) and an explicit `format=json` are never reshaped. `usage` reports the tokens each stage saved. Idempotent (tested).
- **Measured** on the 1,980 recorded outputs, by the new counter: JSON results shrink 26.2 % (`view`), 29.6 % (`soul`), 36.8 % (`rag_search`), 37.5 % (`seal_recall`); the compact text results are already minimal and change by 0 %. Reproduce: `REDUCE_MEASURE_DIR=<dump> cargo test --release -p uriel-mcp --test reduce_measure -- --ignored --nocapture` after `token_dump`.

## Not done

- The counter is an estimator, not a tokenizer: no vocabulary, so a model other than cl100k can differ.
- The first adaptive `tools/list` still includes `rag_search` (2.8k bytes) and `view`, the two largest schemas; trimming them is a retrieval-quality call, not a mechanical one.
- Memory write tools still echo the written entities back.
- The reducer has two stages. A stage that rewrites meaning (dropping empty fields) was left out on purpose.
