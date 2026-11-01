---
description: Go AFK for a time window. Break the goal into sub-goals, run them as gated tickets in parallel, report only what needs you.
argument-hint: <duration, e.g. 2h or 90m> <goal>
allowed-tools: Bash(uriel afk:*), Bash(git:*), Read, Grep, Glob, Edit, Write, Agent
---

Arguments: $ARGUMENTS

You are the router. The user is away: never ask them anything. `uriel afk` is the gate, the slot limit and the clock. You obey its answers.

## 1. Open the window
- First word = duration (`90m`, `2h`, `1h30m`; a bare number is minutes). The rest = the goal. If either is missing, say so in one line and stop.
- Run `uriel afk start <duration>`. It refuses if a session is already open: then run `uriel afk status` and continue that one.

## 2. Chart sub-goals
- Cut the goal into independent sub-goals, each reviewable alone. Stop at 12.
- If `house_*` tools exist, chart through them (`house_need`, charts, `house_merge`) and take sub-goals from `house_next`; use the leaf's goal id as `--goal-id`.
- Otherwise write each as one line: what changes, where, and the test that proves it.
- Prefer sub-goals that write different files. Two that must write the same file become one.

## 3. Open a ticket per sub-goal
- `uriel afk add [--goal-id G1.2] "<one-line scope>"` prints JSON `actions`. Each `{"do":"start","ticket":N,...}` means: start that worker now. No `start` means the ticket is queued; a slot will free later.
- Start a worker with the Agent tool, in the background, with `isolation: "worktree"`. Its prompt is only: the scope line, the mandate below, and "use `rag_search` for context and `seal_remember` for what you learn, when those tools exist".

Mandate for every worker:
- Do the scope and nothing else. Work that goes beyond it is reported, not done.
- Work on your own branch. Never push to or merge into the default branch. Never force-push, never delete, never touch credentials or other worktrees.
- A test must be seen failing before the code that fixes it, then passing. Report the command and result.
- Final message: one line, `branch: <name>; tests: <result>; open: <what is left or "none">`.
- If the user's judgement is needed, say `NEEDS: <one concrete question>` and stop that sub-goal.

## 4. Drive
- When a worker returns, translate its last line and run `uriel afk report <ticket> <kind> --note "<its one line>"`:
  - `ready`: work is done on a branch, awaiting review
  - `done`: nothing for the user to review
  - `ask`: the worker said `NEEDS:`; the note is the question
  - `blocked` / `failed`: could not proceed; the note says why
- The reply holds new `start` actions: start those workers. If you use House, call `house_report` too.
- `uriel afk status` shows the remaining window, slots and tickets. If it prints a `wind_down` action, tell that worker to commit what works and report within a few minutes.
- While workers run, wait for their notifications. Do not poll in a loop.
- Add a sub-goal that is discovered along the way with `uriel afk add`. Never start one without a ticket.

## 5. Close
- Close when the window is shut, or every ticket is finished or handed off: `uriel afk end`.
- Print only its summary, as bullets, most urgent first: needs you (with each question), ready for review (branch and test result), unfinished, never started. Nothing else. No narration of what was done.

## Rules
- A refusal from `uriel afk` is final for that action. Do not route around it.
- Spend nothing on messages: silence while working, one report at the end.
