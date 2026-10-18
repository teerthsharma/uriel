# Claude Code hooks: keep what compaction forgets

When Claude Code compacts a conversation, the window shrinks to a summary and the
detail is gone. These hooks hand the transcript to Uriel first. Every turn is cut
into fractions of about 600 tokens, run through the RAG and stored in the graph as
`ctx/<session>/<n>.md`, so `rag_search` and `seal_recall` can still find a sentence
from turn 3 after the window no longer holds it. When the new window starts, the
most relevant gists come back as `file:line` citations that `rag_expand` zooms.

Nothing here blocks a session: both commands exit 0 on every error, write one line
to stderr, and read at most 512 MiB of transcript per call.

## Settings

Merge into `~/.claude/settings.json`. Replace the paths with your `uriel` binary and
database.

```json
{
  "hooks": {
    "PreCompact": [
      {
        "hooks": [
          { "type": "command", "command": "uriel capture --db ~/.uriel/graph.uriel" }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "compact",
        "hooks": [
          { "type": "command", "command": "uriel recall-context --capture --db ~/.uriel/graph.uriel --budget 1200" }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          { "type": "command", "command": "uriel capture --db ~/.uriel/graph.uriel" }
        ]
      }
    ]
  }
}
```

## What each hook does

| Hook | Command | Effect |
| --- | --- | --- |
| PreCompact | `uriel capture` | Stores the transcript about to be summarised. Re-running stores nothing new; a grown transcript rewrites only its tail. |
| SessionStart, matcher `compact` | `uriel recall-context --capture` | Stores the compaction summary as `ctx/<session>/summary-<n>.md` under one database open, then prints the hook JSON with the summary first and the gists nearest the recent topic. |
| SessionEnd | `uriel capture` | Stores whatever the last compaction did not see. |

`--capture` matters: hooks of one event run in parallel, and two separate commands
would race for the database lock. One command takes the lock once.

## Behaviour worth knowing

- The database has one writer. If the MCP server holds it, the hook prints one
  stderr line and leaves the database untouched; `recall-context` then prints `{}`.
- Output is bounded: at most 24 KiB of `additionalContext`, and never more tokens
  than `--budget` (default 1200, ceiling 8000).
- Stored text is the user and assistant text, tool names with a short input
  summary, and tool results cut to 600 characters. Thinking blocks are skipped.
- Each fraction opens with a one-line topic (the first 120 characters of its first
  substantive user or assistant text), so the one-line gist of a hit says what it is
  about; the next line carries the turn range and the record timestamps, never the
  wall clock, so the same transcript always produces the same database content.
- Re-capturing replaces fractions in place (same `ctx/<session>/<n>.md`), and drops
  the ones past the end of a transcript that has since become shorter.
- After a compaction the recall query is the compaction summary; without one it is
  the last three user prompts. The newest summary comes first, then the best fractions.

## Searching captured context from a shell

The MCP servers keep their own databases and do not read the ctx database. Search it
with `uriel recall`, which opens it read-only (never writes, never upgrades it):

```
uriel recall --db ~/.uriel/ctx.uriel --budget 1200 --k 8 release tag commit
uriel recall --db ~/.uriel/ctx.uriel --expand 123
```

The first form prints `// file:start-end #chunk` citations with the resolution ladder
(top hit at L1, then L2, then one-line L3 gists) under the token budget; the second
prints the whole stored text of one cited chunk. Output is plain text, bounded to
24 KiB, and identical for identical input. If a hook holds the database at that
moment, the command prints one stderr line and exits 0.
