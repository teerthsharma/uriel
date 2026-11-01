# Safety model: genius, fleet, ledger

Scope: the code added for `genius_*`, `fleet_*`, `ledger_*`, `scripts/ledger_next.py` and the `/genius` command. It is a hardening record, not a sandbox: this server never starts, stops or isolates an agent.

## Trust boundaries

| Input | Trusted? | Why |
|---|---|---|
| Anything an agent writes (claims, results, event names and attributes) | No | A model can be steered by what it read |
| A ledger file on disk, the prompt cache | No | Another process, or an earlier bug, may have written it |
| The idea text | Partly | The user typed it, but it is embedded in six prompts, so it is sanitised and fenced |
| The MCP client on stdio | Yes, as the parent process | There is no network listener |
| `URIEL_FLEET_MAX` | Yes, as the operator's setting | Anyone who can set the environment can set the cap |

## Threats, controls and the test that fails without the control

| Threat | Control | Gate |
|---|---|---|
| A secret in an agent's output reaches a trace, a ledger, a verdict or a later prompt | Sanitiser on every ingress path, redaction to a fixed point | `each_known_token_family_is_redacted`, `a_token_nested_inside_a_keyword_bearing_identifier_is_still_redacted`, `secrets_and_control_bytes_in_observation_attributes_never_reach_the_trace`, `ledger_text_is_sanitised_before_it_is_stored_or_returned` |
| Terminal escapes, bidi overrides, zero-width or tag characters hide text from the reader | Stripped and counted | `ansi_csi_and_osc_sequences_are_removed_and_counted`, `bidi_overrides_and_zero_width_characters_are_removed...`, `unicode_tag_characters_that_smuggle_ascii_are_removed` |
| Prompt injection through agent output | Output can only be a typed claim the validator accepts; text that re-enters a prompt is fenced with a content-hash nonce; forged fences neutralised; injection phrases only flag | `the_fence_nonce_is_a_function_of_the_content...`, `a_dependency_result_reaches_the_next_prompt_only_inside_a_fence`, `forged_fence_markers_inside_data_are_neutralised_and_counted` |
| A hallucination is presented as fact | No claim without falsifier and cheap test; wild claims quarantined; no status says true | `a_wild_claim_must_be_cheap_to_test_and_never_near_certain`, `untested_wild_claims_are_quarantined...` |
| Runaway fan-out: fork bomb, retry loop, token burn | Concurrency, depth, lifetime and token limits; `reap` for crashed harnesses | `a_fork_bomb_of_recursive_spawns_stops_at_depth_two_by_default`, `the_lifetime_total_binds_even_when_slots_are_free`, `the_token_budget_stops_further_admission...` |
| Capacity left idle while work waits | The observation deck: `launch_now`, `split_needed`, and lost slot-ticks, all arithmetic on counters | `a_request_over_the_cap_leaves_a_backlog_and_the_deck_says_to_launch_as_slots_free`, `the_ledger_frontier_feeds_the_deck_which_asks_for_exactly_the_free_slots_and_refills_as_agents_finish` |
| Path traversal through a run, plan or ledger name | One id grammar (`A-Z a-z 0-9 _ . : -`, 1 to 64), checked before any path is built | `run_ids_outside_the_safe_alphabet_are_rejected`, `ledger_errors_are_typed_and_names_cannot_escape_the_directory` |
| A tampered or hostile state file | Re-validated on load (schema, ids, caps, unknown needs, cycles, control characters); the Python reader refuses with exit 2 and writes nothing | `a_tampered_ledger_file_is_refused_instead_of_trusted`, `a_tampered_or_hostile_file_is_refused_with_exit_2_and_writes_nothing` |
| Cache poisoning or a symlink redirect | Prompt files are named by the hash of their content; a symlinked cache directory is refused; files 0600, directory 0700; atomic write by temp file and rename | `a_symlinked_cache_directory_is_refused`, `cache_files_are_private_to_the_owner`, `prompts_are_cached_by_content...` |
| Unbounded memory or disk | Caps: 16 runs, 4,096 events per run, 16 plans, 16 ledgers, 256 goals, 64 per batch, 16 needs, 1 MiB per file, 1 MiB hard input to the sanitiser, 512 cache files | `caps_on_goals_batch_and_needs_hold`, `the_event_cap_bounds_memory...`, `the_prompt_cache_is_bounded...`, `at_most_sixteen_ledgers_are_open_at_once` |
| A response cut at the server's output limit breaks the protocol | One brief per call, capped lists, a worst-case size gate | `the_worst_case_brief_and_verdict_fit_the_default_server_output_limit` |
| An edited, deleted or reordered audit event | Hash chain with an eviction anchor | `editing_any_field_of_any_event_breaks_the_chain...`, `deleting_an_event_from_the_middle...`, `reordering_two_events...` |
| A write tool runs on a read-only server | Ledger tools write files, so they are hidden and refused under `--read-only` | `the_six_new_tools_are_listed_in_both_modes_and_survive_read_only` |
| A cycle or a changed redefinition corrupts the plan | A batch is validated whole on a copy, then committed; goals are append-only | `a_cycle_is_rejected_and_leaves_the_ledger_unchanged` |

## Practices applied

- `#![forbid(unsafe_code)]` in the three new crates; no `unwrap`, `expect` or `panic!` in their non-test code (checked by grep; one invariant `expect` was removed).
- Typed errors with stable codes; the MCP layer returns `{"error": code, "message": ...}`.
- Fail closed: an unterminated private-key block is redacted to the end of the text; an unknown schema, state or id is rejected rather than coerced.
- Validate on load, not only on write.
- No wall clock in any decision: replays and incident reconstruction give the same answer.
- No new third-party dependency: the lockfile gains only the three path crates. `cargo deny` was not run because it is not installed in this environment.
- Property tests with seeded generators (sanitiser idempotence over 3,000 inputs, random byte strings, random DAGs drained in waves) and a differential test between the Rust ledger and the Python reader over 25 DAGs and every step of a drain.
- Mutation checks: 12 load-bearing rules were broken one at a time and each turned its suite red.
- Clippy with warnings denied on every new package.

## Residual risk, stated plainly

- **Not a sandbox.** The server can show that agents are idle; it cannot start or stop one.
- **The deck is advice with a measurement.** It makes under-use visible on every response and the conductor text tells the model to obey it, but a model that ignores it is only caught by the lost-slot counter.
- **Redaction is pattern based.** A secret with no known prefix and no `key=` shape passes. There is no NFKC normalisation and no email or personal-data redaction.
- **Injection is not detectable in general.** The defence is structural (typed claims, fences), not a phrase list.
- **The hash chain is tamper-evident, not tamper-proof.** Someone who can rewrite process memory can recompute it. Quote the head hash outside the process (the conductor does).
- **Ledger and cache files are readable by the owning user.** Results can be sensitive; treat the database directory like the database.
- **`URIEL_FLEET_MAX` is unauthenticated.** It is an operator knob, not a security boundary.
- **The Python reader needs `python3`.** The differential gate fails loudly if it is missing.

## Operator checklist

1. Run the server `--read-only` when the ledger is not needed.
2. Keep the database directory private (the files inside are written 0600).
3. Set `URIEL_FLEET_MAX` to what the machine and your rate limits allow, not to 256.
4. Quote `fleet_trace` head hashes in anything you rely on.
5. Prune or delete `prompt-cache/` after a sensitive run.
