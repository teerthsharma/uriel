---
description: Go AFK for a time window. Break the goal into sub-goals, run them as gated tickets in parallel, report only what needs you.
argument-hint: <duration, e.g. 2h or 90m> [--no-merge] <goal>
allowed-tools: Bash(uriel afk:*), Bash(git:*), Read, Grep, Glob, Edit, Write, Agent
---

Arguments: $ARGUMENTS

You are the router. The user is away: never ask them anything. `uriel afk` is the gate, the slot limit and the clock. You obey its answers.

## 1. Open the window
- First word = duration (`90m`, `2h`, `1h30m`; a bare number is minutes). Then an optional `--no-merge`. The rest = the goal. If the duration or the goal is missing, say so in one line and stop.
- Note the branch you are on (`git branch --show-current`): it is the merge target. If the work tree is dirty or HEAD is detached, treat the run as `--no-merge`.
- Run `uriel afk start <duration>`. It refuses if a session is already open: then run `uriel afk status` and continue that one.

## 2. Chart three tiers
- Write the plan as JSON and save it in the work tree's scratch area (not in the repo): departments own a goal, each department splits it into sub-goals, each sub-goal into leaves, and every leaf carries a checklist of things that must be true when it is done.
  `{"departments":[{"name":"Research","goal":"...","subgoals":[{"goal":"...","leaves":[{"goal":"...","checklist":["seen failing","..."]}]}]}]}`
- Use few departments (2 to 5, for example Research, Build, Review, Docs), sub-goals that are independently reviewable, and leaves a single worker can finish. Ids are positional: `research.2.1`.
- Prefer leaves that write different files. Two that must write the same file become one leaf.
- If `house_*` tools exist, chart through them too and keep each House goal id in the leaf text.
- `uriel afk plan <file>` accepts the plan once. A refusal names the place that is wrong; fix the file and run it again.

## 3. Open a ticket per leaf
- `uriel afk add --leaf research.1.2` prints JSON `actions`. Each `{"do":"start","ticket":N,...}` means: start that worker now. No `start` means the ticket is queued; slots are shared between departments by load, so a quiet department is not starved by a busy one.
- Start a worker with the Agent tool, in the background, with `isolation: "worktree"`. Its prompt is only: the leaf goal and its checklist, the mandate below, and "use `rag_search` for context and `seal_remember` for what you learn, when those tools exist".

Mandate for every worker:
- Do the scope and nothing else. Work that goes beyond it is reported, not done.
- Work on your own branch. Never push, never merge, never force-push, never delete, never touch credentials or other worktrees. The router merges your branch after you report.
- A test must be seen failing before the code that fixes it, then passing. Report the command and result.
- Final message: one line, `branch: <name>; tests: <result>; checked: <the checklist item numbers that are true, from 0>; open: <what is left or "none">`.
- If the user's judgement is needed, say `NEEDS: <one concrete question>` and stop that sub-goal.

## 4. Drive
- When a worker returns, tick each checklist item it said is true with `uriel afk check <leaf> <n>` (only items its evidence supports), then translate its last line and run `uriel afk report <ticket> <kind> --note "<its one line>"`:
  - `ready`: work is done on a branch, awaiting review
  - `done`: nothing for the user to review
  - `ask`: the worker said `NEEDS:`; the note is the question
  - `blocked` / `failed`: could not proceed; the note says why
- The reply holds new `start` actions: start those workers. If you use House, call `house_report` too.
- Merge each `ready` ticket (unless `--no-merge`) as soon as it reports, one at a time, in this order:
  1. Only if the worker's last line shows its tests passed.
  2. `git merge --no-ff <branch>` into the target branch you noted.
  3. Run the sub-goal's test again on the merged tree. If it fails, or the merge conflicts: `git merge --abort` (or `git reset --hard ORIG_HEAD` after a completed merge), then `uriel afk report <ticket> blocked --note "merge: <why>"`. The branch stays for the user.
  4. Never push, never force, never merge into any other branch, never delete the worker branch.
- `uriel afk status` shows the remaining window, slots and tickets. If it prints a `wind_down` action, tell that worker to commit what works and report within a few minutes.
- While workers run, wait for their notifications. Do not poll in a loop.
- A leaf discovered along the way is added by writing a new plan in a new window; never start work without a ticket. Free-text `uriel afk add "<scope>"` still exists for one-off work outside the plan.

## 5. Close
- Close when the window is shut, or every ticket is finished or handed off: run `uriel afk pages` (it writes `index.html` and one page per department under `.uriel/afk/pages/`), then `uriel afk end`.
- Print only its summary, as bullets, most urgent first: needs you (with each question and each merge that was refused), ready (branch, test result, merged or not), unfinished, never started. Then one line naming the pages folder. Nothing else. No narration of what was done.

## Rules
- A refusal from `uriel afk` is final for that action. Do not route around it.
- Spend nothing on messages: silence while working, one report at the end.
