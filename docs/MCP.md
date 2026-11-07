# Uriel MCP server reference

The MCP server is the part of Uriel that a model client talks to. It is a stdio server that speaks JSON-RPC 2.0, one message per line, over the seal graph database. A client starts it as a child process (`uriel mcp`, or the `uriel-mcp` binary), sends `initialize`, then calls tools such as `rag_search`, `read_neo4j_cypher` or `view`. The server keeps answers small: results are cut to a character limit with a handle to fetch the rest, repeated large reads are replaced by a short reference, and tools a session has not used are hidden until needed. This page is the reference for the protocol, the flags, the modes and the errors. The design of the graph and retrieval is in the [README](../README.md).

Source for everything here: `crates/uriel-mcp/src/` (`server.rs`, `config.rs`, `guard.rs`, `session.rs`, `tools.rs`, `reduce.rs`, `render.rs`, `compact.rs`, `terse.rs`) and the tests in `crates/uriel-mcp/tests/` (`stdio.rs`, `modes.rs`, `mode_config.rs`).

## Contents

- [Transport and framing](#transport-and-framing)
- [Initialize and capabilities](#initialize-and-capabilities)
- [Tool listing: adaptive and full](#tool-listing-adaptive-and-full)
- [Modes](#modes)
- [Read-only](#read-only)
- [Output formats](#output-formats)
- [Terse encoding](#terse-encoding)
- [Session dedup and session_reset](#session-dedup-and-session_reset)
- [Token limits](#token-limits)
- [Errors](#errors)
- [Flags and environment](#flags-and-environment)
- [Client configuration](#client-configuration)
- [Troubleshooting](#troubleshooting)
- [See also](#see-also)

## Transport and framing

- **Transport.** stdio only. The server reads standard input and writes standard output. It exits with status 0 on end of input (`eof_on_stdin_exits_cleanly`, `stdio.rs`).
- **Framing.** Each message is one line of JSON terminated by `\n`. Each reply is one line. Blank lines are skipped. Bytes that are not valid UTF-8 are replaced, not rejected (`server.rs`, `serve`).
- **Stdout carries only JSON-RPC.** The test `stdout_is_only_json_rpc` checks that every stdout line parses as a JSON-RPC response. Do not write other output to the same pipe.
- **Line limit.** A line may be up to 8 MiB (`MAX_LINE_BYTES`, `guard.rs`, 8,388,608 bytes, newline included). A longer line is read to its end and discarded. The server then replies with error `-32600` and the message `Request line exceeds 8388608 bytes and was dropped`, with `id` null.
- **Batches.** A JSON array is a batch. The reply is one array line with one entry per request. Notifications produce no entry, and a batch made only of notifications produces no line. An empty array is an error, `-32600 Empty batch` (`batch_requests_answered_as_array`, `server.rs`, `reply`).
- **Which messages get a reply.**
  - A message with an `id` (string or number) always gets a reply, unless it is a response from the client (it has `result` or `error` and no `method`), which is ignored.
  - A message without an `id` gets no reply. Methods starting with `notifications/` are accepted and ignored. Any other method without an `id` is dropped silently, even if unknown (`unknown_notification_gets_no_reply`).
  - `jsonrpc` must be the string `"2.0"`, and `id` must be a string or a number. A `null` id is rejected.

## Initialize and capabilities

`initialize` negotiates the protocol version. The server supports `2025-06-18` and `2024-11-05` (`SUPPORTED_VERSIONS`, `server.rs`). A requested version it does not know gets `2025-06-18`, the latest.

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"example","version":"0"}}}
```

The result contains:

| Field | Value |
|---|---|
| `protocolVersion` | The negotiated version. |
| `serverInfo` | `name` `uriel-mcp`, `title` `seal graph database`, `version` from the crate version. |
| `capabilities.tools.listChanged` | `true` in adaptive listing, `false` in full listing. |
| `capabilities.prompts.listChanged` | `false`, and the `prompts` capability is present only in house mode. |
| `instructions` | A text that names the mode (`normal mode` or `house mode`), the read-only state, and, in adaptive listing, the tools listed at start. With `--terse` other than `off` it also describes the terse encoding. |

The server does not advertise `resources` (`initialize_handshake_content`). `ping` returns `{}`.

After any reply that changes which tool groups are listed, the server writes one notification line, `{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}`. This happens only in adaptive listing (`take_notification`, `guard.rs`).

## Tool listing: adaptive and full

`tools/list` depends on `--tools`:

- **`adaptive` (default).** The listing starts with the core tools and reveals a group when it is first used (see below). Each schema is slimmed: `additionalProperties: false` and empty `required` arrays are removed, and each description is cut to its first sentence, at most 160 characters (`slim`, `first_sentence`, `tools.rs`). The `format` property is described as `compact (default) or json`.
- **`full`.** Every tool visible in the mode is listed, with full descriptions and schemas (`list_json_with`, `tools.rs`).

Every listed tool carries `annotations` with `readOnlyHint` (true for tools that do not write) and `openWorldHint: false`.

Adaptive listing does not block calls. A tool that is not yet listed still works when called (`with_notes` says so in the instructions).

### Groups

Tools belong to a group (`group_of`, `guard.rs`). Five groups are enableable by name, and the core group is always listed.

| Group | Tools |
|---|---|
| `write` | `write_neo4j_cypher` |
| `memory` | `create_entities`, `create_relations`, `add_observations`, `delete_entities`, `delete_observations`, `delete_relations`, `read_graph`, `search_memories`, `find_memories_by_name` |
| `place` | `why_hot`, `pin`, `unpin`, `replay`, `calibrate`, `tier_stats` |
| `rag` | `rag_feedback`, `soul`, `session_reset` |
| `house` | `house_*` except `house_need` |
| `research` | `research_*` |
| `dispatch` | `dispatch_*` except `dispatch_open` |
| `core` | Every other tool, plus `house_need` and `dispatch_open`. Always listed. |

A group is listed when:

- a tool of that group is called, whether the call succeeds or fails;
- a successful `house_need` also opens `house` and `research`, and a successful `dispatch_open` opens `dispatch` (`opens`, `note_call`);
- the client calls `tools_enable`.

A successful `dispatch_close` hides `dispatch` again (`closes`). A failed `dispatch_open` does not open it (`a_failed_entry_call_does_not_open_its_group`).

### tools_enable

`tools_enable` lists or hides a group. It is available in both listing modes.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `group` | string | yes | One of `memory`, `write`, `place`, `rag`, `house`, `research`, `dispatch`. `core` is not accepted. |
| `off` | boolean | no | `true` hides the group again. Default `false`. |

The result is JSON: `{"group": "...", "listed": true, "changed": true, "tools": [...]}`, where `tools` names the tools of that group visible in this server. The call fails with a tool error if the group has no tools in this server, for example `house` in normal mode, or `write` with `--read-only`, because the error names the mode and the read-only state.

### Other guard tools

Three tools are always present in both modes and in both listing styles, `more`, `usage` and `tools_enable`. They are explained in [Token limits](#token-limits) and [Session dedup](#session-dedup-and-session_reset) and in the `tools_enable` section above.

## Modes

The server has two modes, chosen at start. The mode is set with `--mode` or the `URIEL_MODE` environment variable. The flag wins when both are given (`the_flag_beats_the_environment_in_both_directions`). An empty `URIEL_MODE` means normal.

| | `normal` (default) | `house` |
|---|---|---|
| Tools in `tools/list` with `--tools full` | 36 | 60 |
| Tools listed at start (adaptive) | 17 | 22 |
| Extra tools | none | the 11 `house_*`, the 3 `research_*`, the 3 `genius_*` and the 7 `dispatch_*` (24 in total) |
| Prompts | none (`prompts/list` returns an empty list) | `genius` |

The counts are the ones in the [README](../README.md#modes). The house extra of 24 is checked by `house_lists_every_normal_tool_plus_twenty_four_more` (`mode_config.rs`). The mode name is also part of the scaffold fingerprint, so the two modes never share a cached prompt prefix (`the_scaffold_fingerprint_differs_by_mode_and_is_stable_within_one`).

A normal server neither lists nor accepts a house-only tool. A house-only name is any name starting with `house_`, `research_`, `genius_` or `dispatch_` (`house_only`, `tools.rs`). Calling one returns a tool result with `isError: true` whose text is this typed object:

```json
{"error":"wrong_mode","tool":"house_next","mode":"normal","requires":"house","hint":"restart the server with --mode house (or URIEL_MODE=house)"}
```

The `tool`, `mode` and `requires` fields change with the call. A name that matches no tool at all, such as `research_nope`, is a `-32602` error in both modes instead.

### The genius prompt

In house mode, `prompts/list` returns one prompt, `genius`, with one required argument, `idea`. The idea is sanitised and cut to 2,000 bytes (`GENIUS_IDEA_BYTES`). `prompts/get` returns one user message containing the protocol prompt. In normal mode `prompts/get` returns `-32602` with the message `prompt 'genius' requires house mode: restart with --mode house (or URIEL_MODE=house)`.

The `genius` prompt is marked experimental in its own description and is not benchmarked. [`docs/GENIUS.md`](GENIUS.md) describes it.

## Read-only

`--read-only` hides every tool whose schema marks it as writing, and refuses calls to them. The refusal is a plain tool error, not a typed object:

```text
tool 'write_neo4j_cypher' is disabled: the server was started with --read-only
```

`read_neo4j_cypher` rejects write clauses, including `MERGE` and `DETACH DELETE`, and writes nothing (`read_tool_rejects_merge_and_delete_too`). In house mode, read-only keeps `research_orbit_audit`, `research_pooling_audit`, `house_status`, `house_select_tests` and `house_config`, and drops `research_record` and the eight house tools that change state (`read_only_house_keeps_the_audits_and_drops_every_writer`).

The README gives the resulting counts: 21 tools in normal read-only mode and 29 in house read-only mode, under `--tools full`.

## Output formats

`--format compact|json` sets the default encoding for three tools: `rag_search`, `seal_recall` and `read_neo4j_cypher`. A call can override it with the `format` argument. `json` is the exact v1 output. The default is `compact` (`Format::Compact`, `config.rs`).

### Compact

Compact output is a table. The first line is `#` and the row count. Then come optional alias lines, then the column names, then one line per row, with cells separated by tabs (`encode_table`, `compact.rs`):

| Syntax | Meaning |
|---|---|
| `#N` | N rows. A ` +` suffix means the table uses run markers or dictionary entries. |
| `@a=prefix` | Alias definition. A value `@a rest` expands to `prefix` followed by `rest`. Aliases are `a` to `z`, then `A` onward. |
| `^` | The cell is equal to the cell in the same column of the row above. |
| `$a=value` and `$a` | Dictionary definition and reference (only in tables with the ` +` flag). |
| empty cell | `null`. Trailing empty cells on a row are omitted. |
| `'` prefix | The text after it is a string. Used for strings that would otherwise read as a number, boolean, `@`, `~`, `'` or sign. |
| `~` prefix | The rest is nested JSON for an array or object. |
| `\t \n \r \\` | Escapes inside strings. |

The encoder tries the plain form and three variants with run markers and dictionary entries, and keeps the one with the lowest estimated token count (`encode_table`). A result that is one JSON object is a one-row table (`render.rs`). A result that does not fit the table shape, such as mixed rows, is returned as JSON.

Cypher results are capped at 200 rows (`MAX_ROWS`). With more rows the compact form ends with the line `+N rows not shown (cap 200); add LIMIT or SKIP`. The JSON form is `{"rows": [...], "omitted": N, "note": "..."}`.

## Terse encoding

`--terse off|data|prose` (default `off`) applies a second, lossless encoding to results. It is separate from the compact table format above.

- **`off`.** Results are returned as produced.
- **`data`.** A result that parses as JSON is packed with the `uriel-terse` data encoding. The packed text is kept only when it has fewer estimated tokens than the original and decodes back to the same value (`pack`, `terse.rs`). The server's instructions describe the encoding so a model can read it. The `rag_expand` and `terse_stats` results are never reshaped.
- **`prose`.** Also shortens the wording of stored memory hits (`remembered`, `terse.rs`) and of documentation sections (`.md`, `.markdown`, `.txt`, `.rst`) in the `soul` guide (`prose_sections`). Stored data is never changed.

With `--terse` other than `off` the server adds one tool, `terse_stats`. It reports `terse`, `terse_before_tokens`, `terse_after_tokens` and `saved_ratio` for the session. The README reports the measured effect: at default formats the saving is a few per cent, and larger for `format=json` callers.

## Session dedup and session_reset

Six tools are repeatable: `get_neo4j_schema`, `read_neo4j_cypher`, `read_graph`, `search_memories`, `find_memories_by_name` and `tier_stats` (`REPEATABLE`, `guard.rs`). When one of them returns the same result, with the same arguments, a second time in the same session, and the result is at least 200 estimated tokens (`MIN_STUB_TOKENS`), the second reply is a reference:

```text
=same (640 tokens sent earlier; fresh=true resends)
```

The number is the estimate for the earlier result. A call with `"fresh": true` skips the check and resends the full result. The argument is ignored when the server builds the key, so a fresh call and a normal call share one entry. The table holds 512 entries (`REPEAT_ENTRIES`); the entry recorded longest ago is evicted first. Error results are never stubbed.

The retrieval tools have their own dedup: a chunk already delivered in the session comes back as a `#<id>` reference with a flag, and a changed chunk comes back with its text. The README describes this under "Session dedup". `session_reset` is the tool that clears it (`session_*` tools are routed to `rag_tools`, `tools.rs`).

## Token limits

Output size has four controls: `--token-limit`, the handle that `more` reads, the soft and hard session budgets, and the reducer.

### --token-limit and cutting

`--token-limit N` (default `20000`, a positive integer) is a limit in characters, not tokens. The server cuts any result that is longer. The cut lands on a newline when one lies at or beyond half of the cap, and on a character boundary otherwise (`cut_index`, `guard.rs`). The cut text ends with this notice, where `R` and `T` are estimates and `ID` is the handle:

```text
... [truncated: ~R of ~T tokens left; more(h=ID) continues, or refine the query]
```

The rest of the result is kept in memory, up to 2 MiB in total (`SPILL_BYTES`). The oldest kept rest is evicted first. A rest larger than 2 MiB is dropped and the notice says `dropped as too large to keep; refine the query`.

### more

`more` returns the next part of a cut result.

| Argument | Type | Required | Meaning |
|---|---|---|---|
| `h` | integer | yes | The handle from the notice. |
| `chars` | integer | no | Page size in characters. Default is the current output cap. Clamped to between 1 and the cap. |

When the handle is spent, evicted or never existed, `more` returns a tool error: `no stored remainder for h=N: it was read to the end, evicted, or never existed; rerun the query`.

### Session pressure: soft and hard tokens

The server counts the tokens it has sent in the session, using the estimate from `uriel-terse` (`count_tokens`), not a model tokenizer. Two thresholds change the character cap:

| Pressure | Condition | Cap |
|---|---|---|
| `normal` | sent < soft | `--token-limit` |
| `tight` | sent >= soft | the larger of half the limit and 2000 characters, never above the limit |
| `critical` | sent >= hard | the larger of a quarter of the limit and 800 characters, never above the limit |

With the default limit of 20000 characters, `tight` caps output at 10000 and `critical` at 5000 (`caps_shrink_with_pressure_and_never_exceed_the_limit`). A threshold of `0` never triggers (`--soft-tokens 0` or `--hard-tokens 0`). When the hard threshold is not zero, `--soft-tokens` must not exceed it.

When the pressure level rises, the next plain-text result ends with one notice per level:

```text
[budget: ~N tokens sent this session; outputs now capped near X tokens, more continues]
```

Here `X` is the new character cap divided by four. Results that are JSON objects or arrays do not get the notice, so they stay parseable.

### usage

`usage` reports the state of these counters. It takes no arguments and returns:

| Field | Meaning |
|---|---|
| `tokens_sent` | Estimated tokens sent in the session. |
| `calls` | Tool calls handled. |
| `pressure` | `normal`, `tight` or `critical`. |
| `soft`, `hard` | The thresholds in use. |
| `listed` | Groups currently listed. |
| `repeats_stubbed` | Results replaced by `=same`. |
| `cut_results` | Results that were cut. |
| `kept_bytes` | Bytes held for `more`. |
| `reducer` | `on`, `tokens_saved`, and `by_stage` with `trim` and `data`. |

### Reducer (--reduce)

The reducer changes results without losing information. It applies to every successful result, before the cut and the repeat check (`shape`, `guard.rs`).

- **`trim` stage.** Removes trailing spaces on each line and collapses runs of blank lines to one (`reduce.rs`).
- **`data` stage.** Packs JSON results with the terse data encoding, but only when `--terse` is `off` and the call does not ask for `format=json`. It is skipped when the terse encoding is already on, so the two never stack.

`--reduce auto` (the default) turns the reducer on with adaptive listing and off with `--tools full`. `on` and `off` force it. `usage` reports the tokens it saved.

## Errors

The server uses two kinds of error and never mixes them.

**JSON-RPC errors** are objects with `code` and `message` in place of `result`. They mean the request itself was wrong.

| Code | Name | When |
|---|---|---|
| `-32700` | Parse error | The line is not JSON. `id` is null. |
| `-32600` | Invalid request | Not a JSON object; `jsonrpc` is not `"2.0"`; `id` is not a string or number; a request has no `method`; an empty batch; a line over 8 MiB. |
| `-32601` | Method not found | A method the server does not implement, such as `does/not/exist`. |
| `-32602` | Invalid params | `tools/call` without a string `name`; an unknown tool name; a `prompts/get` for an unknown prompt or in normal mode. |

Example:

```json
{"jsonrpc":"2.0","id":3,"error":{"code":-32602,"message":"Unknown tool: drop_everything"}}
```

**Tool errors** are normal results with `isError: true`. The server is still healthy. The text of the single content item is either a message or, for a mode mismatch, the typed `wrong_mode` object shown in [Modes](#modes). Other tool errors include a missing or empty required argument, a Cypher syntax error, a read-only refusal, and a `more` handle that is not stored.

```json
{"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"'query' is required and must be a non-empty string"}],"isError":true}}
```

A successful call is `{"content":[{"type":"text","text":"..."}],"isError":false}`.

## Flags and environment

`uriel-mcp` reads its flags from the command line. Each value can be given as `--flag value` or `--flag=value`. A flag that takes no value rejects `=value`. A bad value, an unknown flag, or a missing value exits with status `2`, prints the problem on standard error, and writes nothing to standard output. `-h` and `-V` print and exit with status 0. The README's `uriel mcp` line lists a subset of these flags; this table is the complete set the `uriel-mcp` binary accepts (`USAGE` and `parse_args_all`, `config.rs`).

| Flag | Default | Environment | Meaning |
|---|---|---|---|
| `--db <PATH>` | `~/.seal/graph.seal` (`USERPROFILE`, then `HOME`) | none | Database file. The parent directory is created. Linux and macOS: `~/.uriel/graph.uriel`. Windows PowerShell: `$HOME\.uriel\graph.uriel`. |
| `--in-memory` | off | none | Use a throwaway database. Nothing is kept after exit. |
| `--read-only` | off | none | Hide and refuse write tools. See [Read-only](#read-only). |
| `--mode <normal\|house>` | `normal` | `URIEL_MODE` | Tool set. See [Modes](#modes). The flag wins over the variable. |
| `--tools <adaptive\|full>` | `adaptive` | `URIEL_TOOLS` | Tool listing. See [Tool listing](#tool-listing-adaptive-and-full). |
| `--token-limit <N>` | `20000` | none | Cut results at N characters. N must be positive. |
| `--reduce <auto\|on\|off>` | `auto` | `URIEL_REDUCE` | Output reducer. `auto` is on with adaptive listing. |
| `--soft-tokens <N>` | `40000` | none | Tokens sent before the `tight` cap. `0` never. |
| `--hard-tokens <N>` | `100000` | none | Tokens sent before the `critical` cap. `0` never. Must be at least `--soft-tokens` unless `0`. |
| `--terse <off\|data\|prose>` | `off` | `URIEL_TERSE` | Lossless result encoding, and for `prose` shorter wording. Adds `terse_stats` when not `off`. |
| `--format <compact\|json>` | `compact` | none | Default encoding for `rag_search`, `seal_recall` and `read_neo4j_cypher`. |
| `--root <PATH>` | the current directory at start | none | Project root for the `rag_*` and `house_*` tools. |
| `--events <PATH>` | `<root>/house-events.jsonl` | none | House event log. |
| `--config <PATH>` | `~/.seal/config.json` if present (help text) | none | JSON file with model slots. |
| `--memory-mib <N>` | `50` (minimum `16`) | none | Memory budget in MiB, split between the caches, the RAG index and the session. |
| `--placement <local\|planned>` | `local` | none | `planned` keeps a placement model that `why_hot` and `pin` read and steer. |
| `--idle-release-secs <N>` | `30` (`0` never) | none | Close the database after N idle seconds so another process can open it. The next call reopens it. |
| `-h`, `--help` | | | Print usage and exit 0. |
| `-V`, `--version` | | | Print the version and exit 0. |

One environment variable has no flag. `URIEL_STABLE_PREFIX` set to `off`, `0` or `false` makes the JSON result of `view` and `soul` use the plain serialisation. By default it begins with a byte-stable scaffold preamble and carries `prefix_id` and `stable_prefix_bytes` (`USAGE`, `config.rs`).

Each `URIEL_*` variable is read only when its flag is absent. An empty variable counts as absent. A value that is not one of the allowed words is a usage error that names the variable (`a_bad_flag_or_environment_value_is_an_error_that_names_its_source`).

## Client configuration

### Claude Code

Register normal mode under the name `uriel`, and house mode under the name `seal`, as the README does. Both names can be registered, but they open the same database file, which has one writer, so enable one at a time.

```bash
claude mcp add uriel -s user -- uriel mcp --db ~/.uriel/graph.uriel
claude mcp add seal -s user -- uriel mcp --db ~/.uriel/graph.uriel --mode house
```

```powershell
claude mcp add uriel -s user -- uriel mcp --db $HOME/.uriel/graph.uriel
claude mcp add seal -s user -- uriel mcp --db $HOME/.uriel/graph.uriel --mode house
```

`uriel install` registers the server for you and prints the command if `claude` is not on the path.

### Generic mcpServers JSON

Most MCP clients read a file with a top-level `mcpServers` object. The entry runs the binary over stdio. This is the usual shape for such files; check your client's documentation for its exact file location.

```json
{
  "mcpServers": {
    "uriel": {
      "command": "uriel",
      "args": ["mcp", "--db", "/home/you/.uriel/graph.uriel"],
      "env": {
        "URIEL_TOOLS": "adaptive",
        "URIEL_TERSE": "off"
      }
    }
  }
}
```

On Windows, escape each backslash in the database path:

```json
"args": ["mcp", "--db", "C:\\Users\\you\\.uriel\\graph.uriel"]
```

For house mode, add `"--mode", "house"` to `args`, or set `"URIEL_MODE": "house"` in `env`. The flag and the variable have the same effect (`the_environment_selects_the_mode_when_no_flag_is_given`).

## Troubleshooting

**A tool returns `wrong_mode`.** The server is in normal mode. Restart it with `--mode house` or `URIEL_MODE=house`, or use the house registration from [Claude Code](#claude-code). The hint in the error names the same fix.

**A tool is missing from `tools/list`.** In adaptive listing, a group appears after its first use. Call the tool directly, which works before it is listed, or call `tools_enable` with its group name. `--tools full` lists everything from the start.

**The server used a different database than expected.** Without `--db`, the server opens `~/.seal/graph.seal`, not `~/.uriel/graph.uriel`. Pass `--db` explicitly in every client configuration.

**A second client cannot open the database.** The database has one writer. Keep one server running per database file, or let the idle server release it sooner with a smaller `--idle-release-secs`. The default is 30 seconds, and `0` disables the release.

**A result ends with `[truncated ...]`.** Ask for the rest with `more` and the `h` in the notice. To see more at once, raise `--token-limit`. A Cypher result that says `+N rows not shown` needs a `LIMIT` or `SKIP` in the query.

**A repeated read returns `=same`.** The server has already sent that result in this session and it has not changed. Pass `"fresh": true` in the arguments to get the full result again.

**Outputs get shorter partway through a session.** A `[budget: ...]` notice means the session passed a pressure threshold. Call `usage` to see `tokens_sent` and `pressure`. Raise `--soft-tokens` and `--hard-tokens`, or set them to `0` to turn the caps off.

**The server exits with status 2.** A flag or value is wrong. Standard error names the flag, for example `--mode expects normal or house, got 'warp'`. Standard output is empty in this case.

**A request reports `Request line exceeds 8388608 bytes`.** The line was dropped and the server kept running. Split a large payload, such as a big write batch, into several smaller calls.

**A call to `write_neo4j_cypher` or a memory write is refused.** The server was started with `--read-only`. Remove the flag for a server that should write.

**The client shows a banner or text where JSON is expected.** Standard output must carry only JSON-RPC lines. `uriel mcp` never writes the banner (README), so check that the command in the client configuration is `uriel mcp` and not a wrapper that prints its own output.

**`prompts/get` for `genius` says it requires house mode.** The prompt exists only in house mode. Restart with `--mode house`.

## See also

- [README](../README.md): what the graph and retrieval do, the evidence and the tool counts.
- [Claude Code hooks](claude-code-hooks.md): capture and recall of compacted sessions, which use the same database.
- [Genius](GENIUS.md): the `genius` prompt and the house-mode abduction protocol.
- [Ledger](LEDGER.md): the goal ledger behind `ledger_*`.
- [Dispatch](DISPATCH.md): the `dispatch_*` tools in house mode.
- [Safety](SAFETY.md): the threat model for the agent tools and the controls that the tests check.
- [Revamp](REVAMP.md): the tool groups and adaptive listing in more detail.
- [Plan](../PLAN.md): what is built and what is staged.
