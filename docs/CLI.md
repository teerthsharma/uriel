# uriel command-line reference

`uriel` is the command-line front end of the Uriel graph database. It starts the stdio MCP server (`uriel mcp`), indexes directories into a graph database, searches captured Claude Code context, runs the Claude Code hook commands, keeps the AFK (time-boxed) session log, renders HTML pages from JSON, and prints the planning prompt for the `genius` protocol. This page documents the `uriel` binary built from `crates/uriel-cli`, version 0.2.9.

## Contents

- [Invocation and exit codes](#invocation-and-exit-codes)
- [Environment variables](#environment-variables)
- [Default database path](#default-database-path)
- [Colour output and NO_COLOR](#colour-output-and-no_color)
- [banner](#banner)
- [mcp](#mcp)
- [The uriel-mcp binary](#the-uriel-mcp-binary)
- [index](#index)
- [calibrate](#calibrate)
- [doctor](#doctor)
- [install](#install)
- [capture](#capture)
- [recall-context](#recall-context)
- [recall](#recall)
- [genius](#genius)
- [afk](#afk)
- [page](#page)
- [See also](#see-also)

## Invocation and exit codes

```
uriel [-V | --version] [COMMAND]
```

With no command, `uriel` prints the banner and exits 0 (`crates/uriel-cli/src/main.rs`, `dispatch`). `uriel banner` prints the same banner. `-V` or `--version` prints the banner, then `uriel 0.2.9`. Errors are written to stderr as `uriel: <message>`.

| Exit | When |
|---|---|
| 0 | Success. Also returned by `capture`, `recall-context` and `recall` on runtime failures, so a hook never blocks a session. Their messages go to stderr. |
| 1 | The command returned an error, for example `index` on a path that is not a directory, or a failed write. |
| 2 | Usage error (unknown flag or command, bad value, missing argument) for any command except the two hook commands. The status comes from the clap parser. |

For `capture` and `recall-context`, a usage error prints one line to stderr and exits 0. The check looks only at the first argument (`is_hook_invocation` in `main.rs`).

A bad `URIEL_MODE`, `URIEL_TERSE`, `URIEL_TOOLS` or `URIEL_REDUCE` value makes `uriel mcp` exit 1, not 2 (`McpArgs::to_config`).

## Environment variables

| Variable | Effect |
|---|---|
| `NO_COLOR` | Any value, including an empty one, makes the banner plain (`mode_from_env`, `main.rs`). |
| `FORCE_COLOR` | A non-empty value other than `0` makes stdout count as a terminal for colour. |
| `COLORTERM` | `truecolor` or `24bit` (any case) selects 24-bit colour; anything else selects 256-colour. |
| `WT_SESSION` | When `COLORTERM` is empty and this is set, colour is treated as `truecolor`. |
| `TERM` | Read, not used. |
| `USERPROFILE`, `HOME` | Build the default database path. `USERPROFILE` is tried first; empty values are ignored. |
| `USER`, `USERNAME` | `afk start` records the first non-empty one as the owner, else `owner`. |
| `PATH` | `install` searches it for `claude`. |
| `URIEL_MODE` | Default `--mode` for `uriel mcp`. `normal` or `house`. |
| `URIEL_TERSE` | Default `--terse` for `uriel mcp`. `off`, `data` or `prose`. |
| `URIEL_TOOLS` | Default `--tools` for `uriel mcp`. `adaptive` or `full`. |
| `URIEL_REDUCE` | Default `--reduce` for `uriel mcp`. `auto`, `on` or `off`. |
| `URIEL_STABLE_PREFIX` | Read by the MCP server (`crates/uriel-mcp/src/services.rs`). `off` (or `0`, `false`) keeps the JSON result of `view` and `soul` in the plain serialisation. |

A flag takes precedence over its variable. An invalid variable value is an error, not a fallback (`resolve` functions in `crates/uriel-mcp/src/config.rs`).

## Default database path

When `--db` is absent, commands use `default_db_path()` (`crates/uriel-mcp/src/config.rs`):

| Platform | Path |
|---|---|
| Windows | `%USERPROFILE%\.seal\graph.seal` |
| Linux and macOS | `$HOME/.seal/graph.seal` |

If neither variable is set, the path is `.seal/graph.seal` relative to the current directory. Commands that use it: `index`, `doctor`, `capture`, `recall-context`, `recall`, `mcp`, and `install` (which registers this path). `afk` does not use the graph database.

## Colour output and NO_COLOR

Only the banner has colour (`crates/uriel-cli/src/banner.rs`). The mode is chosen in this order: `NO_COLOR` set means plain; stdout not a terminal and `FORCE_COLOR` unset means plain; `COLORTERM` of `truecolor` or `24bit` means 24-bit; anything else means 256-colour. On Windows, if the console cannot enable ANSI handling, the banner is plain (`main.rs`). Piped output is plain. Example: `NO_COLOR=1 uriel banner` (PowerShell: `$env:NO_COLOR = "1"; uriel banner`).

## banner

```
uriel banner
```

Prints the banner. No flags. Running `uriel` with no command prints the same output.

## mcp

```
uriel mcp [--db <PATH>] [--root <PATH>] [--read-only]
          [--format compact|json] [--placement local|planned]
          [--mode normal|house] [--terse off|data|prose]
          [--memory-mib <N>] [--token-limit <N>] [--idle-release-secs <N>]
          [--tools adaptive|full] [--reduce auto|on|off]
          [--soft-tokens <N>] [--hard-tokens <N>]
```

Runs the stdio MCP server: JSON-RPC requests on stdin, responses on stdout. Nothing else is written to stdout; there is no banner and no startup line (`mcp_dispatch_never_writes_the_banner`, `main.rs`).

| Flag | Default | Meaning |
|---|---|---|
| `--db <PATH>` | default database path | Graph database file. |
| `--root <PATH>` | current directory | Project root for the `rag_*` and `house_*` tools. |
| `--read-only` | off | Hide and deny all write tools. |
| `--format compact\|json` | `compact` | Result format of `rag_search`, `seal_recall` and `read_neo4j_cypher`. `json` is the exact v1 output. |
| `--placement local\|planned` | `local` | `planned` keeps a placement model fed by the nodes the tools read. |
| `--mode normal\|house` | `normal`, or `URIEL_MODE` | `house` adds the `house_*` and `research_*` tools. A normal server neither lists nor accepts them. |
| `--terse off\|data\|prose` | `off`, or `URIEL_TERSE` | `data` returns JSON results in a compact text encoding; `prose` also shortens prose. Stored data is not changed. |
| `--memory-mib <N>` | `50` | Memory budget in MiB; minimum `16` (clap rejects less). |
| `--token-limit <N>` | `20000` | Output is cut at N characters on a line boundary. Minimum `1`. |
| `--idle-release-secs <N>` | `30` | Close the database after N idle seconds; the next call reopens it. `0` means never. |
| `--tools adaptive\|full` | `adaptive`, or `URIEL_TOOLS` | `adaptive` reveals tool groups as they are used; `full` lists every tool. |
| `--reduce auto\|on\|off` | `auto`, or `URIEL_REDUCE` | Lossless output reducer. `auto` is on when tools are `adaptive`. |
| `--soft-tokens <N>` | `40000` | After N tokens sent in a session, outputs are capped at half the limit. `0` means never. |
| `--hard-tokens <N>` | `100000` | After N tokens sent, outputs are capped at a quarter of the limit. `0` means never. |

Defaults come from `DEFAULT_TOKEN_LIMIT`, `DEFAULT_MEMORY_MIB` and `DEFAULT_IDLE_RELEASE_SECS` (`config.rs`) and the soft and hard defaults (`crates/uriel-mcp/src/guard.rs`). Flag meanings follow the `USAGE` text in `config.rs`.

`uriel mcp` does not check that `--soft-tokens` is at most `--hard-tokens`; the `uriel-mcp` binary does.

```bash
uriel mcp --db ~/.seal/graph.seal --root ~/src/project --mode house --terse data
```

```powershell
uriel mcp --db "$HOME\.seal\graph.seal" --root "C:\Users\seal\Documents\GitHub\uriel"
```

## The uriel-mcp binary

The workspace also builds `uriel-mcp` from `crates/uriel-mcp/src/main.rs`. Shared options have the same names, defaults and value rules in both. The parsers differ: `uriel mcp` uses clap (`McpArgs`), while `uriel-mcp` uses the hand-written `parse_args_all` in `config.rs`.

| | `uriel mcp` | `uriel-mcp` |
|---|---|---|
| `--in-memory` (throwaway database) | not accepted | accepted |
| `--events <PATH>` (house event log) | not accepted; defaults to `<root>/house-events.jsonl` | accepted |
| `--config <PATH>` (JSON model slots) | not accepted | accepted; usage text says it defaults to `~/.seal/config.json` if present |
| `-V` / `--version` | top level only (`uriel --version`); `uriel mcp` does not take it | prints `uriel-mcp <version>` |
| Startup line on stderr | none | `uriel-mcp <version> starting (db: <path>, read-only: <bool>)` |
| Soft and hard token check | not performed | refuses `--soft-tokens` above a non-zero `--hard-tokens` |
| Usage error exit | 2 | 2, with the usage text on stderr |
| Runtime error exit | 1 | 1 |


## index

```
uriel index <PATH> [--db <PATH>]
```

Indexes the files under `PATH` into the graph and prints the result (`crates/uriel-cli/src/commands.rs`, `index`). Re-running is incremental: unchanged files are reported as `files unchanged`. `PATH` must be a directory, or the command exits 1. The parent of `--db` is created if missing.

```
database: <db path>
files scanned: <n>
files indexed: <n>
files unchanged: <n>
files skipped: <n>
files removed: <n>
chunks added: <n>
chunks removed: <n>
total: <n> chunks in <n> files
```

```bash
uriel index ~/src/project --db ~/.seal/graph.seal
```

```powershell
uriel index "C:\Users\seal\Documents\GitHub\uriel\crates" --db "$HOME\.seal\graph.seal"
```

## calibrate

```
uriel calibrate [--secs <S>]
```

Measures memory behaviour for the placement model and prints a suggested hot size (`commands.rs`, `calibrate`). `--secs` defaults to `4.0` and is clamped to 0.05 to 20.0; a non-finite value becomes 0.05. The output lines are `calibrating for <S> s`, `points measured: <n>`, one `knee: <from> -> <to> (x<ratio>)` line per knee (or `knees: none detected`), and `suggested hot size: <size> (<bytes> bytes)`.

## doctor

```
uriel doctor [--db <PATH>]
```

Prints one `PASS` or `WARN` line per check (`commands.rs`, `doctor`):

| Check | PASS when |
|---|---|
| `db path` | The database's folder can be created and a probe file can be written and removed in it. |
| `free RAM` | Available memory is at least 1 GiB. |
| `lock memory privilege` | `SeLockMemoryPrivilege` is present and enabled in the output of `whoami /priv`. Other outcomes are `WARN` with the reason. |

```
PASS  db path: <path> is writable
PASS  free RAM: <size> available of <size>
WARN  lock memory privilege: SeLockMemoryPrivilege not granted to this user
```

`doctor` exits 0 even when a check is `WARN`. Outside Windows, `whoami /priv` is not available, so the privilege check reports `WARN` there (inferred from the code path; not run on another OS).

## install

```
uriel install
```

Prints the banner, then registers this executable as a user-scope Claude Code MCP server named `uriel` (`commands.rs`, `install` and `install_args`):

```
claude mcp add uriel -s user -- <path to uriel> mcp --db <default database path>
```

If `claude` is on `PATH` (on Windows, `claude.exe`, `claude.cmd` or `claude.bat`, searched per directory in that order), the command runs and prints `registered uriel with claude`; a non-zero exit from `claude` makes `install` exit 1. If `claude` is not found, the command line is printed instead, with paths containing spaces in double quotes, and the exit is 0. `install` takes no flags.

## capture

```
uriel capture [--db <PATH>] [--session <ID>] [--transcript <PATH>]
```

A hook command for `PreCompact` and `SessionEnd`. It reads the hook payload from stdin (up to 1 MiB), stores the transcript in the graph, and exits 0 with nothing on stdout (`crates/uriel-cli/src/capture.rs`).

| Flag | Default | Meaning |
|---|---|---|
| `--db <PATH>` | default database path | Database file, created if missing. |
| `--session <ID>` | payload `session_id`, then the transcript's file stem | Session key used in stored names. |
| `--transcript <PATH>` | payload `transcript_path` | Transcript file (JSON Lines). Required if the payload has none. |

What is stored: user and assistant text, short summaries of tool calls (160 characters) and tool results (600 characters), cut into fractions of about 600 tokens (4 characters per token) with a 60-token overlap, as `ctx/<session>/<n>.md`. Compaction summaries go to `ctx/<session>/summary-<n>.md`. Fraction writes are capped at 256 per run. Fractions beyond the end of a shorter transcript are removed.

Capturing the same transcript twice leaves the database unchanged (`crates/uriel-cli/tests/hooks.rs`). Failures, such as no transcript or a locked database, print `uriel capture: <message>` to stderr and exit 0. Nothing is created when there is nothing to capture.

```bash
uriel capture --db ~/.seal/graph.seal --transcript ~/transcripts/s1.jsonl --session s1
```

```powershell
Get-Content payload.json | uriel capture --db "$HOME\.seal\graph.seal"
```

The hook settings are in [claude-code-hooks.md](claude-code-hooks.md).

## recall-context

```
uriel recall-context [--db <PATH>] [--budget <TOKENS>] [--capture]
```

The `SessionStart` hook for the `compact` matcher. It reads the hook payload from stdin and prints one JSON line (`crates/uriel-cli/src/recall.rs`, `run`). It always exits 0.

| Flag | Default | Meaning |
|---|---|---|
| `--db <PATH>` | default database path | Database file. |
| `--budget <TOKENS>` | `1200` | Token budget, clamped to 1 to 8000. |
| `--capture` | off | Store the transcript's compaction summary first, under the same database open. |

On success the output is `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"..."}}`. The query is the latest compaction summary if there is one, otherwise the last three user prompts, plus the current folder name, taken from the last 256 KiB of the transcript. The context is capped at 24 KiB.

The output is `{}` when there is nothing to return, when the database does not exist and `--capture` is off, when the query is empty, and when the database is locked. Failures add one `uriel recall-context: <message>` line to stderr and still print `{}`.

```bash
uriel recall-context --db ~/.seal/graph.seal --budget 1200 --capture
```

```powershell
uriel recall-context --db "$HOME\.seal\graph.seal" --budget 1200 --capture
```

## recall

```
uriel recall [--db <PATH>] [--budget <TOKENS>] [--k <N>] [--expand <CHUNK_ID>] [<QUERY>...]
```

Searches captured context from a shell. It is read-only, never reads stdin, and is not a hook (`recall.rs`, `run_search`).

| Flag or argument | Default | Meaning |
|---|---|---|
| `QUERY...` | required unless `--expand` | Trailing words, joined with spaces. |
| `--db <PATH>` | default database path | Must already exist. |
| `--budget <TOKENS>` | `1200` | Output budget, clamped to 1 to 8000. |
| `--k <N>` | `8` | Hits to consider, clamped to 1 to 64. |
| `--expand <CHUNK_ID>` | none | Print the whole stored text of one chunk. Cannot be combined with query words. |

Without `--expand`, the output gives `// file:start-end #chunk` citations, with the top hit in full detail, the next hits in less detail, and the rest as one-line gists (`HOOKS_HELP` in `main.rs`). Pass a `#chunk` number to `--expand`.

A missing database, a database held by another process (tested on Windows only), and no match each print one line to stderr and exit 0. The database file is not modified (`hooks.rs`).

```bash
uriel recall --db ~/.seal/graph.seal --budget 600 --k 4 tcp drain
uriel recall --db ~/.seal/graph.seal --expand 42
```

```powershell
uriel recall --db "$HOME\.seal\graph.seal" --budget 600 --k 4 tcp drain
uriel recall --db "$HOME\.seal\graph.seal" --expand 42
```

## genius

```
uriel genius [--mode tier|fork] [--warm] [--max-agents <N>] [--json] [--print-command] [<IDEA>...]
```

Compiles an idea into a `genius` plan and prints its prompt and briefs. It calls no model and needs no server (`commands.rs`, `genius`). The method is in [GENIUS.md](GENIUS.md).

| Flag or argument | Default | Meaning |
|---|---|---|
| `IDEA...` | required unless `--print-command` | Trailing words, joined with spaces. |
| `--mode tier\|fork` | `tier` | `tier`: one `BRIEF` section per vertex. `fork`: one `SHARED` section, then one `LENS` section per vertex. |
| `--warm` | off | Passed to the plan options as `warm`; `GENIUS.md` describes it as running the first lens alone first. |
| `--max-agents <N>` | `8` | Concurrency used to compute waves. Range 1 to 256. |
| `--json` | off | Print the plan as JSON. Cannot be combined with `--print-command`. |
| `--print-command` | off | Print the conductor command as Markdown; the idea is ignored. |

The default output is the conductor prompt, a line `plan <id> mode <mode> waves <waves>` (vertex names joined with `+`, waves joined with `>`), then the sections. A refused idea exits 1; a bad `--max-agents` exits 2.

```bash
uriel genius why is the tcp drain slow
uriel genius --mode fork --max-agents 4 --json why is the tcp drain slow
uriel genius --print-command
```

## afk

```
uriel afk [--file <PATH>] <SUBCOMMAND> [ARGS]
```

Keeps the state of an AFK window in a JSON Lines session log, not in the graph database (`crates/uriel-cli/src/afk_cmd.rs`). Each subcommand prints one JSON object. `--file` is global, so it may follow the subcommand; its default is `.uriel/afk/session.jsonl` relative to the current folder.

| Subcommand | Arguments and flags | Output |
|---|---|---|
| `start <DURATION>` | `--workers N`, default `3` | `{started, owner, minutes, workers, closes, file}`. Durations use `h`, `m` and `s` (`2h`, `90m`, `1h30m`). Workers are clamped to 1 to 16 (`afk.rs`). |
| `add [TEXT...]` | `--goal-id ID`, or `--leaf LEAF` | `{ticket, actions}`. `--leaf` opens the named plan leaf and cannot be combined with text. An empty `actions` means the goal was queued, for example on low memory. |
| `plan <SPEC>` | JSON file with `departments` | `{leaves, departments}`. At least one department is required. |
| `check <LEAF> <INDEX>` | `--undo` | Ticks checklist item `INDEX` (counted from 0). Output `{leaf, index, done, plan}`. |
| `report <TICKET> <KIND>` | `--note TEXT` | `KIND` is `ready`, `done`, `ask`, `blocked`, `failed` or `waiting`; anything else is an error. Output `{actions}`. |
| `status` | none | Current state as JSON. |
| `pages` | `--out DIR`, default `.uriel/afk/pages` | `{dir, pages}`: `index.html` and one `department-<name>.html` per department, each with a digest. |
| `end` | none | Removes the session file and archives its log. Output is the summary, with the archive path in `log`. |
| `init` | `--project` | Writes the `/afk` command to `~/.claude/commands/afk.md`, or to `./.claude/commands/afk.md` with `--project`. The home folder comes from `HOME`, then `USERPROFILE`. |

Errors, such as no session, a missing ticket or a bad duration, exit 1. `init` writes the text compiled in from `crates/uriel-cli/assets/afk.md`.

```bash
uriel afk start 2h --workers 4
uriel afk add --goal-id G1.2 write the parser
uriel afk report 3 ready --note "branch afk/g1; tests: ok"
uriel afk end
```

```powershell
uriel afk start 2h --workers 4 --file "$HOME\.uriel\afk\session.jsonl"
uriel afk status --file "$HOME\.uriel\afk\session.jsonl"
```

The plan file format and the slash command's workflow are described in [DISPATCH.md](DISPATCH.md).

## page

```
uriel page build <SPEC> [--out <FILE>]
uriel page demo [--out <DIR>]
```

Renders HTML from a JSON spec (`crates/uriel-cli/src/page_cmd.rs`; the spec format is in `crates/uriel-page`).

| Subcommand | Behaviour |
|---|---|
| `build <SPEC>` | Without `--out`, the HTML goes to stdout. With `--out FILE`, the file is written (its folder is created) and `{file, bytes, digest}` is printed. |
| `demo` | Writes `index.html` and `department-research.html` into `--out DIR`, default `docs/pages`, and prints each file with its digest. |

An unreadable spec exits 1 with `cannot read <path>`. An invalid spec exits 1 with the renderer's message.

```bash
uriel page build ./spec.json --out ./site/index.html
uriel page demo --out ./docs/pages
```

```powershell
uriel page build .\spec.json --out .\site\index.html
uriel page demo --out .\docs\pages
```

## See also

- [claude-code-hooks.md](claude-code-hooks.md): the hook settings for `capture` and `recall-context`.
- [GENIUS.md](GENIUS.md): the protocol behind `uriel genius`.
- [DISPATCH.md](DISPATCH.md): the dispatch layer behind the `afk` commands.

## Config, call, tools, check and install

Documented with the v1 commands.
