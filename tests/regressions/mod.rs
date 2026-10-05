//! Regression tests: one file per defect that reached a release and was
//! fixed, named after its tracking id
//! (`tests/regressions/<id>-<short-slug>.rs`), written in the same change
//! as the fix and before it where the process allowed
//! (`docs/testing-plan.md` section 7, NFR-24).
//!
//! The review report's finding numbers are the tracking ids until the
//! project uses issue numbers. The backfill below covers the critical and
//! high findings from `../lca-issues.md`, which shipped in 0.1.0/0.1.1 and
//! were fixed after the post-release audit.

// `../lca-issues.md` finding 1 (critical): cyclic widget arena.
#[path = "01-widget-cycle.rs"]
mod widget_cycle;
// finding 2 (critical): `ext install` path traversal.
#[path = "02-install-name-traversal.rs"]
mod install_name_traversal;
// finding 3 (high): unreachable `fs private` scope.
#[path = "03-private-scope-reachable.rs"]
mod private_scope_reachable;
// finding 4 (high): extension process/pty prompts auto-denied.
#[path = "04-extension-process-prompt.rs"]
mod extension_process_prompt;
// finding 5 (high): corrupt known record silently skipped.
#[path = "05-corrupt-record-truncation.rs"]
mod corrupt_record_truncation;
// finding 7 (high): DNS-rebinding TOCTOU.
#[path = "07-net-dns-rebinding-pinned.rs"]
mod net_dns_rebinding_pinned;
// released 0.1.3 defect: a cancel could not reach a blocked `oauth.await`.
#[path = "08-cancel-reaches-blocked-oauth-wait.rs"]
mod cancel_reaches_blocked_oauth_wait;
// released 0.1.1-0.1.3 defect: HTTPS `net` requests were rejected as non-http.
#[path = "09-https-net-scheme-rejected.rs"]
mod https_net_scheme_rejected;
// released 0.1.x defect: a plain conversation logged a boundary divergence.
#[path = "10-plain-conversation-no-boundary-warning.rs"]
mod plain_conversation_no_boundary_warning;
// released 0.1.x defect: reasoning glued to the answer, tool lines unnamed.
#[path = "11-tui-turn-rendering.rs"]
mod tui_turn_rendering;
// released 0.1.x defect: `lca resume` listed stale message counts.
#[path = "12-resume-message-counts-fresh.rs"]
mod resume_message_counts_fresh;
// released 0.1.x defect: re-compaction dropped the previous summary.
#[path = "13-compaction-keeps-prior-summary.rs"]
mod compaction_keeps_prior_summary;
// released 0.1.x defect: the display view dropped truncation warnings.
#[path = "14-display-view-keeps-truncation.rs"]
mod display_view_keeps_truncation;
// released 0.1.x defect: a non-SSE body read as a silent empty success.
#[path = "15-malformed-sse-not-silent.rs"]
mod malformed_sse_not_silent;
// released 0.2.0 defect: a forked session listed as 0 messages.
#[path = "16-fork-listing-resolved-count.rs"]
mod fork_listing_resolved_count;
// released 0.2.0 defect: a compaction summary read as an untrusted user note.
#[path = "17-compaction-summary-framed.rs"]
mod compaction_summary_framed;
// latent release-pipeline defect: a bare release build produced no `lca`.
#[path = "18-release-builds-the-cli.rs"]
mod release_builds_the_cli;
// latent nightly-fuzz defect: the fuzz workspace drifted out of sight.
#[path = "19-fuzz-targets-match-the-api.rs"]
mod fuzz_targets_match_the_api;
// cycle-5 driving: a disabled provider must take its presets with it.
#[path = "20-disabled-provider-drops-its-presets.rs"]
mod disabled_provider_drops_its_presets;
// cycle-5 release defect: a WIT change broke the wasm components silently.
#[path = "21-wasm-generate-blocks-map-every-host-import.rs"]
mod wasm_generate_blocks_map_every_host_import;
// cycle-6: the zero-provider state is valid, not fatal (FR-PROV-9).
#[path = "22-zero-provider-is-a-valid-state.rs"]
mod zero_provider_is_a_valid_state;
// cycle-7 driving: a data-only package's digest must cover its bag.
#[path = "23-data-only-package-digest.rs"]
mod data_only_package_digest;
// cycle-7 driving: a disabled package takes its skill pack with it.
#[path = "24-disable-stops-skill-injection.rs"]
mod disable_stops_skill_injection;
// cycle-7 P2: the six release targets have their own build gate.
#[path = "25-release-targets-gate.rs"]
mod release_targets_gate;
// tui-port driving: `net_read_body` buffered a streaming body until EOF, so
// an SSE provider stream painted only at the end (owner issue #4).
#[path = "26-net-read-body-streams.rs"]
mod net_read_body_streams;
// tui-port2 P6: a multi-line notice wrote embedded newlines to the
// terminal and corrupted the screen.
#[path = "27-multiline-notice-render.rs"]
mod multiline_notice_render;
// audit: a steer left the assistant before it with a streaming marker.
#[path = "28-steer-finalizes-assistant.rs"]
mod steer_finalizes_assistant;
// R11: cancelling the permission countdown dropped the responder.
#[path = "29-permission-countdown-keeps-responder.rs"]
mod permission_countdown_keeps_responder;
// Cycle-4 finding S8: the grants overlay was composited past the viewport
// because the resize guard did not list the new picker.
#[path = "30-grants-view-renders-while-open.rs"]
mod grants_view_renders_while_open;
// Cycle-4 audit: an empty-id model from an unparseable probe left the
// session model-less instead of falling back.
#[path = "31-empty-model-id-falls-back.rs"]
mod empty_model_id_falls_back;
// Cycle-4 finding: submit was checked before newline, so legacy Ctrl+J
// submitted instead of inserting a newline.
#[path = "32-ctrl-j-inserts-a-newline.rs"]
mod ctrl_j_inserts_a_newline;
// Released 0.5.0 trust-boundary defect: an overlong OSC 11 hex channel
// could overflow the decoder; headlined in 0.5.1.
#[path = "33-osc11-hex-overflow.rs"]
mod osc11_hex_overflow;
// Cycle-5 dogfood defect: an iteration-limit / cancel abort left a
// dangling tool call, poisoning the session with HTTP 400 on every later
// turn. `assemble` now heals it.
#[path = "34-dangling-tool-call-healed.rs"]
mod dangling_tool_call_healed;
// Windows quarantine defect: the pty child was born on a fresh console, not
// the pseudoconsole, so the pty pipe stayed empty (HPCON value/pointer and
// the child's std handles).
#[path = "35-conpty-pseudoconsole-attached.rs"]
mod conpty_pseudoconsole_attached;
// Windows quarantine defect: on exit the TUI hung joining a reader blocked
// in `ReadFile` (`wait_stdin` never timed out), so no `session-end`.
#[path = "36-console-exit-does-not-hang.rs"]
mod console_exit_does_not_hang;
// Windows display defect: the session-start notice showed the canonical
// `\\?\C:\...` working directory instead of the plain path.
#[path = "37-verbatim-path-not-displayed.rs"]
mod verbatim_path_not_displayed;
// Released 0.5.2 defect (owner's real-terminal report): paste worked in the
// editor but every single-line surface dropped it. Cycle 7's shared paste
// primitive fixes it.
#[path = "38-paste-into-modal-fields.rs"]
mod paste_into_modal_fields;
// Released 0.5.3 defect (GitHub issue #25): `/antigravity.login` opened the
// preset picker instead of the provider's identity flow - the interface
// intercepted every `*.login` before the host's handler could see it.
#[path = "gh25-namespaced-login-routes-to-identity-flow.rs"]
mod gh25_namespaced_login_routes_to_identity_flow;
// Released 0.5.3 defect (GitHub issue #33): exit left the cursor at the
// terminal's restored position instead of on a fresh line below the
// transcript, so the returning shell prompt overwrote LCA's own content.
#[path = "gh33-exit-cursor-parks-below-transcript.rs"]
mod gh33_exit_cursor_parks_below_transcript;
// Released 0.5.3 defect (GitHub issue #24): the capability consent prompt
// confirmed on a bare `y` keystroke - no Enter, on a security surface.
#[path = "gh24-consent-confirm-requires-enter.rs"]
mod gh24_consent_confirm_requires_enter;
// Released 0.5.3 defect (GitHub issue #34): every preset model reported no
// context window, so the footer read `ctx ?` and compaction had no real
// denominator.
#[path = "gh34-curated-context-windows-reach-the-footer.rs"]
mod gh34_curated_context_windows_reach_the_footer;
// Review finding on gh #34 (no issue id): the shipped openai-compatible
// manifest named a resource kind its own bag did not carry, so installing
// it with its bag had failed since the flat-file layout landed.
#[path = "39-shipped-bag-kinds-match-the-manifest.rs"]
mod shipped_bag_kinds_match_the_manifest;
// Open-line defect (GitHub issue #19): a stdout closed early
// (`| head`) made every `println!` panic with a broken pipe.
#[path = "gh19-sigpipe-does-not-panic.rs"]
mod gh19_sigpipe_does_not_panic;
// Open-line defect (GitHub issue #18): a modal close that shrank the frame
// while the notice changed could leave the old notice row on the pane.
#[path = "gh18-stale-notice-row.rs"]
mod gh18_stale_notice_row;
// Released-line defects (GitHub issue #27): a multiline prompt stepped back
// to column 0 on every line after the first, and spaces were dropped at wrap
// boundaries.
#[path = "gh27-editor-rows-keep-the-marker-pad-and-spaces.rs"]
mod gh27_editor_rows_keep_the_marker_pad_and_spaces;
// GitHub issue #28: Arrow Up/Down walked logical lines, so a wrapped prompt
// behaved like Home/End instead of moving row by row.
#[path = "gh28-visual-row-navigation.rs"]
mod gh28_visual_row_navigation;
// GitHub issue #29 (QA-004): an env-configured endpoint host dead-ended
// without an interactive grant; QA-007: a second GrantStore handle opened
// in `load_installed` clobbered the session's state.
#[path = "gh29-env-host-grant-prompt.rs"]
mod gh29_env_host_grant_prompt;
// GitHub issue #20: `meta.json` never carried the model and provider it
// is specified to hold, so a resumed session could not say what the last
// turn ran on.
#[path = "gh20-session-meta-model.rs"]
mod gh20_session_meta_model;
// GitHub issue #31: one `api_key`/`base_url` meant a second login
// overwrote the first, and every picker row wore the crate name instead
// of the service that would bill the call.
#[path = "gh31-provider-profiles.rs"]
mod gh31_provider_profiles;
// GitHub issue #9 (EFG-016/EFG-014): an `edit` result's structured diff
// never reached the transcript, so every edit rendered as a plain tool
// card with the change hidden behind Ctrl+O.
#[path = "gh9-diff-card.rs"]
mod gh9_diff_card;
// GitHub issue #32: a code block could only be drawn four-sided, so a
// terminal selection of copied code always dragged the side pipes along.
#[path = "gh32-codeblock-borders.rs"]
mod gh32_codeblock_borders;
// GitHub issue #35: fullscreen scrolling took the dock with it, there
// was no scrollbar, and no way back to the live bottom but the wheel.
#[path = "gh35-fullscreen-dock.rs"]
mod gh35_fullscreen_dock;
// GitHub issue #30 (EFG-030 + PG-032): `/settings` was a read-only text
// dump; it edits and persists now, on the picker chrome we already had.
#[path = "gh30-settings-selector.rs"]
mod gh30_settings_selector;
// GitHub issue #169: `/compact` on a long session - the summarization
// went out with no generation budget and an open-ended prompt, and a
// failed answer degraded in silence.
#[path = "gh169-compaction-summarization-budget.rs"]
mod gh169_compaction_summarization_budget;
// GitHub issue #17: pi shows `$0.000` unconditionally; the footer shows
// the cost once usage has been measured (a measured zero is informative)
// and stays silent before that.
#[path = "gh17-cost-shown-when-measured.rs"]
mod gh17_cost_shown_when_measured;
// GitHub issue #16: pickers stacked above a tall notice instead of
// anchoring directly above the composer (pi's bottom-anchored shape).
#[path = "gh16-picker-anchors-at-composer.rs"]
mod gh16_picker_anchors_at_composer;
// Composer-polish fold-in (no issue): a slash command typed mid-turn
// leaked to the model as text; commands dispatch as commands now.
#[path = "mid-turn-slash-dispatches-as-command.rs"]
mod mid_turn_slash_dispatches_as_command;
