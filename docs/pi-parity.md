# Pi-parity manifest (RM-001, #93)

The living checklist for "matches pi". Pi tree per `docs/parity-baseline.md`:
pin `v1.0.0-25-ga276dabe5`, live oracle `~/gits/pi` (1.0.0+ era).

Harness: `tests/pi-parity/` — run `cargo nextest run -E 'test(pi_parity)'`.
Every case carries a `// Verifies: pi:<anchor>` comment; anchors name the
pi file and section, never pi internals.

## Green pins (24)

| Pi anchor | LCA test(s) | What matches |
|---|---|---|
| `packages/coding-agent/docs/session-format.md#sessionheader` | `pi_parity_session_log_is_json_lines_with_header_first` | JSONL log, header record first |
| `packages/coding-agent/docs/json.md#framing-and-process-io` | `pi_parity_jsonl_framing_is_lf_delimited` | strict LF framing, no CR |
| `packages/coding-agent/docs/session-format.md#session-version` | `pi_parity_partial_tail_is_discarded_with_prefix_kept`, `pi_parity_unknown_record_type_is_skipped_not_fatal` | corrupt tail keeps prefix; unknown types skip with a warning |
| `packages/coding-agent/docs/sessions.md#manage-conversation-context` | `pi_parity_compaction_appends_marker_and_keeps_originals` | compaction appends a summary; originals stay on disk |
| `packages/coding-agent/test/cache-stats.test.ts` (healthy / full-miss / compaction-reset) | `pi_parity_cache_healthy_turns_have_no_waste`, `pi_parity_cache_full_miss_bills_the_previous_prompt`, `pi_parity_cache_baseline_resets_on_compaction` | missed-token math: full read = 0; full miss = previous prompt (105k in the replayed fixture); reset on compaction |
| `packages/coding-agent/docs/settings.md` (noise floor) | `pi_parity_cache_noise_floor_swallows_small_misses` | 1024-token floor |
| `packages/coding-agent/src/core/cache-stats.ts` (no model-switch exemption; silent providers) | `pi_parity_cache_model_change_does_not_reset`, `pi_parity_cache_silent_provider_has_no_measurable_waste` | switches are counted; no signal means no measurable waste |
| `packages/coding-agent/docs/json.md` + `docs/cli-integration.md` | `pi_parity_headless_exit_code_map`, `pi_parity_exit_code_values_are_stable` | exit-code map and frozen values 0–6 |
| `packages/coding-agent/docs/json.md#agent-and-turn-events` | `pi_parity_usage_envelope_carries_cache_fields`, `pi_parity_turn_records_usage_with_cache_fields` | usage carries input/output/cache buckets + cost, on the wire and on the record |
| `packages/coding-agent/docs/settings.md` + `docs/cli.md` (`--thinking`) | `pi_parity_thinking_vocabulary_matches_pi`, `pi_parity_thinking_rejects_unknown_levels` | `off/minimal/low/medium/high/xhigh/max`; unknown refused |
| `packages/coding-agent/docs/settings.md` (`shellPath`) | `pi_parity_shell_ladder_resolves_an_interpreter` | always an explicit interpreter |
| `packages/coding-agent/docs/codemode.md` (`bash` result shape) | `pi_parity_shell_result_reports_exit_status`, `pi_parity_tool_result_marks_truncation` | non-zero exit is data with its code; over-limit results are marked |
| `packages/coding-agent/docs/environment-variables.md` + `docs/cli.md` | `pi_parity_config_precedence_flag_beats_env_beats_file` | flags > env > project > user > defaults |
| `packages/coding-agent/docs/security.md` | `pi_parity_zero_prompt_mode_is_explicit_never_default` | prompts by default; yolo is opt-in |
| `packages/coding-agent/test/agent-session-retry.test.ts` | `pi_parity_retryable_error_retries_then_succeeds`, `pi_parity_fatal_error_keeps_the_session_open` | retryable retries with an announcement; fatal errors leave the session open |

Two deliberate measurement differences (match here / differ here):

- **Dollar rates.** Pi prices a miss against a model catalog (e.g. full-miss
  re-bill at write price minus a cache-read fallback). LCA prices it from
  the turn's own cost buckets only, so `missed_tokens` matches pi exactly
  while `missed_cost` is LCA's own number. The harness pins tokens exactly
  and asserts cost is positive, never the catalog figure.
- **Session defaults.** Pi defaults `defaultThinkingLevel` to `"medium"`;
  LCA defaults `thinking` to unset (the provider chooses). The harness pins
  the shared vocabulary and the refusal of unknown levels, never the default.

## Settled divergences (documented, never pinned against)

Yolo-over-no-prompts, provider profiles over one-provider, theme roles over
raw colors, fork-directories over in-file branching (until #37 lands),
`session-start` record shape over pi's header shape, LCA's headless event
vocabulary over pi's `agent_start/message_start/...` stream, and the
read-before-edit staleness guard over pi's blind edits (gh #117: kept,
but off by default behind `tool.edit_requires_read`, so the default
behavior is pi parity). A parity case
that fails because of one of these is a wrong case, not a failing product.

## Red witnesses (4, `#[ignore]`-gated)

Run with `cargo nextest run -E 'test(pi_parity)' --run-ignored`. Each fails
ONLY on its not-yet-built behavior. ignore-gating (rather than hard red) is
deliberate: CI runs this target on every push, so hard-red witnesses would
hold main red; the gate stays meaningful while the milestones own the work.

| Witness | Owner | Behavior it waits for |
|---|---|---|
| `pi_parity_compaction_uses_token_budget_trigger` | #36 (EFG-001, RM-012/13) | `compaction.reserveTokens` / `keepRecentTokens` budget trigger |
| `pi_parity_builtin_bash_tool_name` | #39 (EFG-005) | pi-named `bash` tool |
| `pi_parity_session_tree_branches_in_file` | #37 (EFG-002, RM-011) | in-file `id`/`parentId` branching |
| `pi_parity_rpc_mode_exists` | #56 (PG-004) | `--mode rpc` + RPC protocol |
