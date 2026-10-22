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
// terminal and corrupted the screen. Notices ride the transcript
// since gh #239; the one-visual-line-per-line rule rides with them.
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
// Open-line defect (GitHub issue #18): retired with gh #239 — its
// subject, the dock notice row, no longer exists. Transient
// confirmations float as toasts and fade on `tick` (covered in
// `chat_toast_tests`); history appends, so no row can go stale.
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
// GitHub issue #11: reasoning runs toggled by keybinding only; a click
// on a reasoning-run row toggles that run (alt-screen only).
#[path = "gh11-click-toggle-thinking.rs"]
mod gh11_click_toggle_thinking;
// GitHub issue #12: the markdown pre-parse transform seam (pi's
// registerMarkdownTransformer), no WIT growth.
#[path = "gh12-markdown-transform-seam.rs"]
mod gh12_markdown_transform_seam;
// GitHub issue #23 (decision: print it): one dim key-hint line on the
// first frame; headless never prints it.
#[path = "gh23-startup-key-hint.rs"]
mod gh23_startup_key_hint;
// GitHub issue #14: the SDK host-facing approval surface
// (`Session::with_permission_prompt`; default declines and records).
#[path = "gh14-sdk-permission-prompt.rs"]
mod gh14_sdk_permission_prompt;
// GitHub issue #10 (criterion 1, plus `#rgb`): the `system` theme
// derives from the detected terminal palette; oklch/style() deferred.
#[path = "gh10-system-theme.rs"]
mod gh10_system_theme;
// GitHub #109: `-p` is a boolean print flag, messages are positional
// (pi's invocation shape); `--prompt` stays as the legacy spelling.
#[path = "gh109-cli-invocation.rs"]
mod gh109_cli_invocation;
// GitHub #111: headless honors `--model`, `-c`, `-r`.
#[path = "gh111-headless-threading.rs"]
mod gh111_headless_threading;
// GitHub #152: the tracing subscriber installs, rotates bounded, and
// records grant-store failures.
#[path = "gh152-tracing-subscriber.rs"]
mod gh152_tracing_subscriber;
// GitHub #39: read truncation (line budget + offset guidance) and image
// semantics (resize per profile, no-vision note, unknown unchanged).
#[path = "gh39-read-truncation-images.rs"]
mod gh39_read_truncation_images;
// GitHub #116: `edit` refuses non-UTF-8 files instead of corrupting them.
#[path = "gh116-edit-refuses-non-utf8.rs"]
mod gh116_edit_refuses_non_utf8;
// GitHub #96: the panic hook restores the terminal on every build profile.
#[path = "gh96-panic-hook-restores-terminal.rs"]
mod gh96_panic_hook_restores_terminal;
// GitHub #95: the data-vs-docs drift net fires on every drift shape it
// claims to catch, and passes clean.
#[path = "gh95-drift-net-selftest.rs"]
mod gh95_drift_net_selftest;
// GitHub #179: the antigravity 0.9.0 wire alignment (re-auth,
// last_execution_id, schema normalization, fallback routing).
#[path = "gh179-antigravity-realigns.rs"]
mod gh179_antigravity_realigns;
// GitHub #212: an open extension panel ate its own toggle, trapping
// `/exit` behind it.
#[path = "gh212-panel-toggle-traps-exit.rs"]
mod gh212_panel_toggle_traps_exit;
// GitHub #394: the `/tree` navigator's layout math marched a long linear
// session off the right edge (one gutter per ancestor). The four fixes and
// their frame-level guards live here and in `chat_tree_tests.rs`.
// GitHub #369: the OSC 9;4 progress indicator was written once per turn
// (terminals watchdog it away after ~10-15 s), and OSC 7501 tab status
// covered turns only - a background `/compact`, a `!`/`!!` run, and the
// install-consent prompt were invisible to a tab bar.
// GitHub #426: `docs/headless.md` documented `queue-queued` with a `mode`
// and nothing emitted it, so a headless script could not tell "queued and
// later injected" from "never queued at all". Pins that the drift guard's
// exception is gone and one emitter serves both delivery modes.
#[path = "369-osc-keepalive-and-status-coverage.rs"]
mod gh369_osc_keepalive_and_status_coverage;
#[path = "394-tree-layout-math.rs"]
mod gh394_tree_layout_math;
#[path = "426-queue-queued-is-emitted.rs"]
mod gh426_queue_queued_is_emitted;
// GitHub #281: the mechanical compaction fallback decoded every carried
// summary with a hardcoded `compaction` kind, so the host's
// `previous-summary` marker records rendered empty and vanished.
#[path = "281-mechanical-compaction-carries-summaries.rs"]
mod gh281_mechanical_compaction_carries_summaries;
// GitHub #283: Copilot's live model discovery stamped `context_window: 0`
// on every row, discarding the curated table's real windows - and 0 means
// "unknown and never compacts", so a session could grow until the provider
// rejected it. Guards the resolver and the sandbox hand-off, which drift
// independently because the shipped provider is a WASM component.
#[path = "282-enterprise-hosts-offered.rs"]
mod gh282_enterprise_hosts_offered;
#[path = "283-discovered-models-keep-their-windows.rs"]
mod gh283_discovered_models_keep_their_windows;
// GitHub #297: the `/login` picker hid every subscription provider whose
// sign-in is its own `provider-identity` flow rather than a declared preset.
#[path = "297-login-lists-every-provider.rs"]
mod gh297_login_lists_every_provider;
// Gh #297's other half: the picker reads `[login] identity = true`, so the
// six manifests must actually declare it (and key-only packages must not).
#[path = "297-providers-declare-their-sign-in.rs"]
mod gh297_providers_declare_their_sign_in;
// GitHub #401: three queue-UX gaps - the abort rescue is pinned and now
// announces + merges the draft, a submit during a compaction parks and
// flushes when it ends, and the dequeue key is discoverable in the dock.
#[path = "401-queue-rescue-and-compaction.rs"]
mod gh401_queue_rescue_and_compaction;
// GitHub #81: an induced panic writes the crash file with its rows.
#[path = "gh81-crash-log-writes-a-file.rs"]
mod gh81_crash_log_writes_a_file;
// GitHub #253: a compacted session survives the Display read (live
// drive: turns + /compact + /rename listed "No sessions yet").
#[path = "253-display-read-of-compacted-log.rs"]
mod display_read_of_compacted_log;
// GitHub #255: the export document imports back (export wrote a JSON
// document, import wanted raw JSONL).
#[path = "255-export-import-round-trip.rs"]
mod export_import_round_trip;
// GitHub #239: the dock catch-all is retired — command results land
// in the transcript, transient confirmations float as toasts.
#[path = "239-dock-notice-retired.rs"]
mod dock_notice_retired;
// GitHub #215: credential filenames read silently under the fatigue
// cut (live risk: `.env` exfiltrated with no confirmation).
#[path = "215-sensitive-read-escalation.rs"]
mod sensitive_read_escalation;
// GitHub #259: allow/deny rules never applied to in-workspace reads
// (live proof: a stored deny watched the read run `ok` with no record).
#[path = "259-rules-apply-to-in-workspace-reads.rs"]
mod rules_apply_to_in_workspace_reads;
// GitHub #345: git global flags hid the subcommand from the egress and
// config checks, so a trusted folder auto-approved `git --no-pager push`.
#[path = "345-git-flags-bypass-egress-classifier.rs"]
mod git_flags_bypass_egress_classifier;
// GitHub #346: a workspace symlink pointing outside auto-approved an
// out-of-workspace read under folder trust.
#[path = "346-symlink-escape-auto-approve.rs"]
mod symlink_escape_auto_approve;
// GitHub #347: `python3 -c`/`node -e`/... auto-approved arbitrary inline
// code under folder trust.
#[path = "347-interpreter-inline-eval-auto-approve.rs"]
mod interpreter_inline_eval_auto_approve;
// GitHub #341: `{file:}` substitution followed symlinks, so a workspace
// link into `~/.lca/credentials/` read the secret at config-load time.
#[path = "341-file-substitution-symlink-bypass.rs"]
mod file_substitution_symlink_bypass;
// GitHub #343: debug redaction matched keys exactly, so OAuth
// `access_token`/`id_token`/`code` leaked to `debug-wire.log` in the clear.
#[path = "343-debug-redaction-oauth-keys.rs"]
mod debug_redaction_oauth_keys;
// GitHub #342: debug logs were created world-readable (umask 022).
#[path = "342-debug-log-owner-only.rs"]
mod debug_log_owner_only;
// GitHub #293: `run_logout` left preset profile secrets and cached
// models on disk; it must be the exact inverse of the login writes.
#[path = "293-logout-purges-profile-secrets.rs"]
mod logout_purges_profile_secrets;
// GitHub #349: an option-injected git url/rev reached `git clone`/
// `git checkout` unvalidated, and `--upload-pack` runs a command.
#[path = "349-git-source-option-injection.rs"]
mod git_source_option_injection;
// GitHub #263: `@file` truncation cut at a fixed byte count, panicking
// when the 1 MiB bound fell inside a multi-byte UTF-8 character.
#[path = "263-at-file-truncation-utf8-boundary.rs"]
mod at_file_truncation_utf8_boundary;
// GitHub #358: truncated WebP headers direct-indexed past the 16-byte
// magic proof, panicking the image widget in the render path.
#[path = "358-truncated-webp-no-panic.rs"]
mod truncated_webp_no_panic;
// GitHub #311: `session gc` deleted live tool-result image attachments
// because its reachability walker never collected `ToolResult.images`.
#[path = "311-gc-keeps-live-tool-result-images.rs"]
mod gc_keeps_live_tool_result_images;
// GitHub #307: `consent_lines` had no `tools` arm, so any package
// declaring that capability failed install with "unknown capability".
#[path = "307-tools-capability-install-consent.rs"]
mod tools_capability_install_consent;
// GitHub #287: `[limits] memory_mb` was assigned to the byte field raw
// (a 64-byte limiter), and `[limits]` was ignored without `[capabilities]`.
#[path = "287-manifest-limits-memory-unit.rs"]
mod manifest_limits_memory_unit;
// GitHub #334: a cache breakpoint on every user message broke Anthropic's
// four-breakpoint cap, and tool-final requests got no tail anchor at all.
#[path = "334-cache-breakpoint-cap.rs"]
mod cache_breakpoint_cap;
// GitHub #335: assistant `text` serialized before `thinking`, which
// Anthropic rejects on replay.
#[path = "335-thinking-blocks-precede-text.rs"]
mod thinking_blocks_precede_text;
// GitHub #276: `Usage` was assigned whole at every stream accumulator, so
// a split usage report (`message_start` prompt half + `message_delta`
// output half) wiped the prompt counters and auto-compaction never fired.
#[path = "276-partial-usage-merge.rs"]
mod partial_usage_merge;
// GitHub #291: the stdio MCP reader took the next line as the reply, so a
// server notification desynchronized the stream permanently.
#[path = "291-mcp-stdio-notification-desync.rs"]
mod mcp_stdio_notification_desync;
// GitHub #299: a mid-session skill match injected a System-role message,
// which every provider builder hoists into the vendor's root system prompt
// — a KV-cache bust in both directions.
#[path = "299-skill-injection-preserves-root-system.rs"]
mod skill_injection_preserves_root_system;
// GitHub #296: project memory's pin never froze an empty snapshot, so a
// mid-session `learn` re-read the file and mutated `messages[0]`.
#[path = "296-memory-pin-freezes-empty-snapshot.rs"]
mod memory_pin_freezes_empty_snapshot;
// GitHub #265: login option routing ignored the provider, so a key meant
// for one provider was stored in another's credential namespace.
#[path = "265-login-option-provider-scoping.rs"]
mod login_option_provider_scoping;
// GitHub #302 (0.6.x half): an extension's own `net` request had no
// interactive consent path - webfetch's `hosts = []` died on the
// declaration gate, and the deny below it never offered the ad hoc grant.
#[path = "302-webfetch-interactive-consent.rs"]
mod webfetch_interactive_consent;
// GitHub #348: a catalog id from configuration was joined onto the cache
// path unvalidated, so a trusted project's `.lca/config.toml` could write
// attacker-chosen bytes outside `catalogs/`.
#[path = "348-catalog-id-traversal.rs"]
mod catalog_id_traversal;
// GitHub #264: `HeadlessSink::on_event` had no arm for
// `TurnEvent::ExtensionEvent`, so the `extension-event` row the headless docs
// promise fell into the catch-all and never reached a script.
#[path = "264-extension-event-dropped-from-json.rs"]
mod extension_event_dropped_from_json;
// GitHub #357: `/formatter run` walked the whole workspace with no
// exclusions, reformatting `target/**` build output and stat'ing every file
// under `.git/objects` through the capability boundary.
#[path = "357-formatter-skip-vcs-and-build-roots.rs"]
mod formatter_skip_vcs_and_build_roots;
// GitHub #350: `read_archive` decompressed zip members with no read
// ceiling, so a tiny archive could expand to gigabytes in RAM.
#[path = "350-archive-read-ceiling.rs"]
mod archive_read_ceiling;
// GitHub #344: binary MCP resource payloads staged in a shared
// world-writable `/tmp` directory with predictable names.
#[path = "344-mcp-resource-staging-private.rs"]
mod mcp_resource_staging_private;
// GitHub #337: per-chunk lossy decoding corrupted multi-byte codepoints
// split across TCP chunks.
#[path = "337-sse-utf8-chunk-boundary.rs"]
mod sse_utf8_chunk_boundary;
// GitHub #309: the embedding Session dropped data_dir and lacked resume,
// extension attachment, and cancellation.
#[path = "309-sdk-resume-extensions-cancel-datadir.rs"]
mod sdk_resume_extensions_cancel_datadir;
// GitHub #310: `send` rebuilt the tool executor and agent config every turn,
// wiping read tracking and the cache-divergence cell.
#[path = "310-sdk-persists-tools-and-cache-state.rs"]
mod sdk_persists_tools_and_cache_state;
// GitHub #313: session migration rewrote the log non-atomically and named
// backups at second resolution.
#[path = "313-session-migrate-atomic-and-backup.rs"]
mod session_migrate_atomic_and_backup;
// GitHub #317: `ScratchDir::new` shared one path per name+pid, so parallel
// tests deleted each other's directories.
#[path = "317-scratch-dir-parallel-uniqueness.rs"]
mod scratch_dir_parallel_uniqueness;
// GitHub #312: the HTML export emitted `messagemessage-*` classes and an
// unescaped image media type.
#[path = "312-html-export-classes-and-media-escape.rs"]
mod html_export_classes_and_media_escape;
// GitHub #314: the WASM provider guest omitted two capability
// delegations, so WASM silently did less than native.
#[path = "314-subscription-wasm-cap-delegations.rs"]
mod subscription_wasm_cap_delegations;
// GitHub #315: a 400 `invalid_grant` refresh left dead tokens in place.
#[path = "315-refresh-invalid-grant-purges.rs"]
mod refresh_invalid_grant_purges;
// GitHub #316: the WASM model listing dropped per-model extras.
#[path = "316-wasm-model-extras-survive.rs"]
mod wasm_model_extras_survive;
// GitHub #272: `KNOWN_KEYS` / `resolved()` drifted from the config
// vocabulary - a dead environment override and seven unreported settings.
#[path = "272-config-key-vocabulary.rs"]
mod config_key_vocabulary;
// GitHub #273: unknown `config.toml` keys were silently ignored and the
// `[provider]` retry keys were applied twice.
#[path = "273-unknown-config-key-loud.rs"]
mod unknown_config_key_loud;
// GitHub #272/#273 class fix: the config vocabulary mirrors
// (`Config` fields, `resolved()`, `KNOWN_KEYS`, `TABLE_KEYS`, the doc
// key table) must agree, so the next omission fails a test.
#[path = "272-273-config-drift-guard.rs"]
mod config_drift_guard;
// GitHub #338: the Responses builder dropped user images and the usage
// mapper hardcoded `cache_read: 0`.
#[path = "338-responses-images-and-cache-read.rs"]
mod responses_images_and_cache_read;
// GitHub #359: the Responses driver dropped its trailing buffer at EOF and
// stayed silent on a zero-frame 200 body.
#[path = "359-sse-eof-flush-and-zero-frame.rs"]
mod sse_eof_flush_and_zero_frame;
// GitHub #318: `process_wait`/`pty_wait` left the handle in the table, so
// every normal exit leaked a child/PTY entry.
#[path = "318-process-and-pty-wait-remove-handles.rs"]
mod process_and_pty_wait_remove_handles;
// GitHub #319: the loopback OAuth reply was malformed HTTP/1.1 and the
// query decoder widened bytes as Latin-1 instead of UTF-8.
#[path = "319-oauth-loopback-http-and-utf8-query.rs"]
mod oauth_loopback_http_and_utf8_query;
// GitHub #320: `state_write` truncated in place, so a crash mid-write lost
// the extension's state.
#[path = "320-state-write-atomic.rs"]
mod state_write_atomic;
// GitHub #321: `truncate_head_tail` always kept the first and last line,
// so a single oversized line blew past `max_bytes`.
#[path = "321-truncate-head-tail-long-lines.rs"]
mod truncate_head_tail_long_lines;
// GitHub #322: the Win32 Job Object calls ignored their failure returns, so
// the process-tree-kill guarantee could be silently absent.
#[path = "322-windows-job-object-checked.rs"]
mod windows_job_object_checked;
// GitHub #305: a duplicated `ToolCallStart` renamed and cleared the open
// call, so `ToolCallEnd` emitted a call stitched from fragments across the
// reset boundary and the host ran it.
#[path = "305-duplicate-tool-call-start-discarded.rs"]
mod duplicate_tool_call_start_discarded;
// GitHub #306: `validate_against_schema` skipped union `type` arrays and
// never inspected `enum`.
#[path = "306-schema-union-types-and-enums.rs"]
mod schema_union_types_and_enums;
// Shared doubles for the #275 / #277 rows (declared once: `ExtensionDispatch`
// has no defaults, and clippy refuses a module loaded twice).
#[path = "doubles_275.rs"]
mod doubles_275;
// GitHub #274: the curated price table billed cache reads at the input rate
// and the turn loop left the per-bucket cost fields at zero, so a cache
// saving always measured as `$0.00`.
#[path = "274-pricing-cache-read-discount-and-breakdown.rs"]
mod pricing_cache_read_discount_and_breakdown;
// GitHub #275: `stable_cap` was computed before the context transform and
// frozen at the compaction summary afterwards.
#[path = "275-stable-cap-post-transform-and-compaction.rs"]
mod stable_cap_post_transform_and_compaction;
// GitHub #277: overflow recovery emitted `Error` and `TurnEnded` before it
// tried to recover, so a recovered turn continued after `TurnEnded`.
#[path = "277-overflow-recovery-single-turn-ended.rs"]
mod overflow_recovery_single_turn_ended;
// GitHub #356: the provider bridge flattened structured `StreamFailure`
// values to text and re-derived the class by substring heuristics.
#[path = "356-provider-bridge-preserves-error-class.rs"]
mod provider_bridge_preserves_error_class;
// GitHub #355: the self-update fetch paths (`latest_release`,
// `install_binary`, and the catalog fetch) called `plain_get` with no
// timeout and no size cap, so a stalled feed hung `lca update` and a
// hostile mirror could exhaust memory before verify-then-move ran.
#[path = "355-update-fetch-is-bounded.rs"]
mod update_fetch_is_bounded;
// GitHub #292: `provider-presets.toml` declared a canonical environment
// variable per service and `api_key_for` read only the two universal
// names, so exporting the documented variable left the user told to
// authenticate. The guard drives the real request builder and also pins the
// failure the fix must not introduce: another service's key must never be
// sent to this endpoint.
#[path = "292-preset-env-vars-authenticate.rs"]
mod preset_env_vars_authenticate;

// GitHub #269: `--mode rpc` built the Agent without the nested completion
// backend, so RPC compaction degraded to the mechanical excerpt summary.
#[path = "269-rpc-nested-backend.rs"]
mod rpc_nested_backend;
// GitHub #271: the system prompt advertised every built-in tool regardless of
// `--no-tools` / `-t` / `--exclude-tools`, contradicting the request.
#[path = "271-prompt-advertises-effective-tools.rs"]
mod prompt_advertises_effective_tools;
// GitHub #298: the WASM guest hand-wrote a shortened copy of the `question`
// tool's schema with `items` missing, so a WASM-mode model was told
// `questions` was an untyped array and answered with plain strings - which
// the guest's own parser then rejected. The guard pins exact schema parity
// between the two delivery modes and the double rejection of the shape the
// truncated schema invited.
#[path = "298-question-wasm-schema-parity.rs"]
mod question_wasm_schema_parity;
// GitHub #300: `Scanner::feed` never advanced past a matched span, so one
// occurrence re-fired on every later delta for the rest of the 4 KiB window
// (and across turn boundaries); and the WASM guest read workspace stream
// rules without the native path's `project_trusted` gate, so an untrusted
// repository could inject rules. Both defects, with the trust one written as
// a security assertion on the rule set in force.
#[path = "300-stream-guard-refire-and-trust.rs"]
mod stream_guard_refire_and_trust;
// GitHub #295: `discover_models` did `entry.get("id")?.as_str()?` inside its
// loop, so one non-conforming catalog row (`{"id":null}`, a notice object, a
// numeric id) returned `None` for the whole payload and collapsed a
// several-hundred-model gateway catalog to the preset's three curated rows.
// The guard pins that the good rows survive, that the drop is counted rather
// than swallowed, and that a wholly malformed payload still falls back.
#[path = "295-openrouter-catalog-resilience.rs"]
mod openrouter_catalog_resilience;
// GitHub #278: `net_close_response` drops a response from the host's handle
// table, so reading `retry-after` afterwards was `NotFound` and the server's
// backoff intent was silently lost. anthropic closed first and read second;
// openrouter and openai-compatible never read the header at all. The guard
// drives all three through a capability that models the real lifecycle, which
// is the thing the shipped doubles could not do.
#[path = "278-retry-after-read-before-close.rs"]
mod retry_after_read_before_close;
// GitHub #285: `hash_edit` demanded 3-char content hashes that only `annotate`
// produced, and `annotate` had no production caller - so the tool could never
// be called. Its writes also bypassed `ToolExecutor`'s read tracker. The guard
// pins the read -> annotate -> edit loop, the pass-through cases that keep the
// hook from corrupting results, and records honestly that the tracker half
// needs a `crates/lca-tools` API this cycle may not add.
#[path = "285-hash-edit-uncallable.rs"]
mod hash_edit_uncallable;
// GitHub #280: `build_request` extracted every System message into
// `systemInstruction` and then serialized the same messages into `contents`
// again, because the role match's `_ => "user"` arm caught
// `MessageRole::System`. The preamble was billed twice on every turn, arrived
// framed as something the user had said, and occupied the leading `contents`
// slot so the cacheable prefix shifted whenever the instructions changed. The
// guard pins the serialized shape (not a role match), the merge-instead-of-drop
// behaviour for extra System messages, and that the tool-turn signature stayed
// a sibling of `functionCall`.
#[path = "280-antigravity-system-prompt-duplicated.rs"]
mod antigravity_system_prompt_duplicated;
// GitHub #308: three defects in one OCI fetch path. The post-401 retry ignored
// redirects, so the normal ghcr blob-pull shape (401 -> token -> 307 to the
// CDN -> 200) died as "HTTP 307 after authorization"; the token parse read only
// `token` and ignored the spec's `access_token`; and the client had no proxy
// support, no byte cap and no timeout - so a hostile or stalled registry was
// accumulated in full before the digest could run, inverting the
// digest-before-install guarantee. The guard drives the public path against a
// scripted loopback registry, because each defect lives in the sequence of
// requests rather than in one function's return value.
#[path = "308-oci-pull-auth-and-bounds.rs"]
mod oci_pull_auth_and_bounds;
// GitHub #427: the committed `extensions/conformance/fixtures/component.wasm`
// went stale against ABI 0.6 (commit 2ccf187 added the hook worlds to wit/ and
// the source without rebuilding any fixture; the orphan twin added in
// df3663e had no consumer to notice). Installing it failed with "no exported
// instance named 'lca:ext/hook-message-end@0.6.0'" - a link error that looks
// like a host bug. The guard pins both halves of the mechanism: no fixture
// without a consumer naming it, and every fixture links on the current host.
#[path = "427-conformance-fixture-keeps-current.rs"]
mod conformance_fixture_keeps_current;

// Gh #443: `/bug`, `/export`, `/import` were implemented and advertised but
// absent from `slash_commands`, so the router refused them. The named guard
// pins the registration/dispatch/help triangle; the class guards live beside
// the code they scan.
#[path = "443-three-commands-reachable.rs"]
mod three_commands_reachable;

// Gh #403 gap 1: a transcript replacement must not keep the previous
// transcript's selection (a wrong-data path, not just a stale highlight).
// The seam is the transcript generation; the guard pins that it stays one.
#[path = "403-stale-selection-cleared.rs"]
mod stale_selection_cleared;

// Gh #391: collapsed previews keep the tail, the timer ticks and freezes,
// and `!`/`!!` are distinguishable without colour.
#[path = "391-tool-card-preview-and-timer.rs"]
mod tool_card_preview_and_timer;

// Gh #388 gap 2: the dock keeps its row count across a turn, and
// `Compacting` is a first-class separator state with its reason.
#[path = "388-dock-height-and-compacting.rs"]
mod dock_height_and_compacting;
// GitHub #436: `hash_edit` wrote through `Capabilities::fs_write`, which never
// consulted the `ReadTracker` the built-in `edit` checks - so
// `tool.edit_requires_read` had a hole exactly one extension wide. The issue
// left this as an owner call between making the tracker govern (A) and
// documenting an anchor-guarded exception (B). The guard pins the evidence that
// B is false: a 3-char line hash carries no file identity (an anchor from one
// file resolves in another) and a change-then-revert revives a stale anchor, so
// an anchor proves "some read", never "a recent read of this file".
#[path = "436-hash-edit-read-tracker.rs"]
mod hash_edit_read_tracker;

// Gh #388 gap 4: `$EDITOR` strips exactly one trailing terminator (and a
// BOM), so an edit never adds a blank line the user did not type.
#[path = "388-editor-trailing-newline.rs"]
mod editor_trailing_newline;

// Gh #389 (closes #330): the footer's git branch is right in every repository
// shape, costs no I/O per frame, and stops printing zeroes.
#[path = "389-footer-git-truth.rs"]
mod footer_git_truth;

// Gh #397 (tri-state slice): a project override can be seen and cleared -
// `inherit` is the key's absence, and trust still gates it.
#[path = "397-tri-state-project-override.rs"]
mod tri_state_project_override;
// GitHub #302 (final half): `extensions/webfetch` implemented its tool only on
// the native path because `wit/world-tool.wit` did not import `lca:host/net` -
// so in WASM mode the tool could not reach the network at all. The world now
// carries that import, which is a breaking contract change but additive for
// OLD artifacts (the host satisfies an import whether the guest calls it or
// not, the gh #77 rule). The guard proves both halves of that claim: a tool
// guest built before the import still loads and runs, and the import grants
// nothing by itself - a manifest without `[capabilities.net]` still refuses.
#[path = "302-tool-world-net-import.rs"]
mod tool_world_net_import;

// Gh #400 (type-while-loading): the terminal is claimed before the startup
// load, so typing and quit keys work from the first frame.
#[path = "400-type-while-loading.rs"]
mod type_while_loading;

// Gh #387 gaps 1 and 2: the `system` palette's panels are derived from the
// probed background (not copied from it), and COLORFGBG 7/8 classifies like pi.
#[path = "387-system-panels-and-scheme.rs"]
mod system_panels_and_scheme;

// Gh #288: `Manifest::parse` validates every `worlds` entry against one
// vocabulary table, so a typo is refused with a suggestion instead of installing
// as an extension that matches no world.
#[path = "288-unknown-world-is-refused.rs"]
mod unknown_world_is_refused;

// Gh #286: the synchronous call path hands work to one long-lived driver per
// extension instead of spawning a thread per call, with cancellation, trap
// isolation, and shutdown still pinned.
#[path = "286-no-thread-spawn-per-call.rs"]
mod no_thread_spawn_per_call;

// Gh #336: a JSON-RPC error with `id: null` surfaces the server's own words on
// both MCP selectors instead of LCA's guess, with notifications and foreign ids
// unchanged.
#[path = "336-null-id-error-surface.rs"]
mod null_id_error_surface;

// Gh #279: the antigravity extension's advertised description agrees with what its
// code offers - the manifest, the catalog row, and the provider doc all say the
// same thing as `is_selectable`.
#[path = "279-description-matches-code.rs"]
mod description_matches_code;

// Gh #261: `/permissions deny read <literal>` compiles into a pattern the matcher
// can compare, so the read it names is actually refused.
#[path = "261-deny-rule-actually-matches.rs"]
mod deny_rule_actually_matches;

// Gh #459: the MCP error path reads a live child's stderr without waiting, so a
// silent failure lands `Failed` instead of hanging on `Connecting`.
#[path = "459-stderr-tail-never-blocks.rs"]
mod stderr_tail_never_blocks;

// Gh #412: `/persona` applies a bundle of model, thinking, loadout and
// instructions atomically, or nothing at all.
#[path = "412-persona-bundle-atomic.rs"]
mod persona_bundle_atomic;

// Gh #411: the diagnostic bundle is scrubbed before it is written, and the user
// inspects it first - while the session log on disk is never touched.
#[path = "411-bug-bundle-redaction.rs"]
mod bug_bundle_redaction;

// Gh #410: a key hint resolves from the live keybinding manager, so a rebind
// reaches every label - and the lying `key_text` free function is gone for good.
#[path = "410-key-hints-resolve-live.rs"]
mod key_hints_resolve_live;

// Gh #447: the real-terminal harness's readiness primitives exist in one place,
// the class they close is routed through them, and their negative behaviour (a
// pane that never settles must fail, not hang) is still pinned.
#[path = "447-readiness-primitives.rs"]
mod readiness_primitives;
// GitHub #449: two `conformance_diff` rows compiled the WASM fixture more than
// once, so they sat at ~1.6x nextest's default cap and timed out on a loaded
// runner. The measurement put the cost in `wasmtime::component::Component::new`
// (~6 s a compile in a test build), on a fresh *or* reused engine alike, and
// nextest forks a process per row - so the only reducible part was the
// redundant loads inside the two rows, and sharing them removed the need for
// any budget raise at all. The guard pins the compile count per row (a
// structural property, not a stopwatch, which would re-create the fragility).
#[path = "449-conformance-rows-compile-once.rs"]
mod conformance_rows_compile_once;
// GitHub #303 (NFR-24): `SharedPrompt` is the object every capability engine
// holds, and its `ask_annotated` was the trait default - so it dropped tool
// hints and called plain `ask`, and a user approving an MCP call never saw the
// server's `destructive`/`read-only` declaration. The delegation landed in
// `ba7f7fce`; this guard covers what that row does not: a *denying* prompt
// stays denying under every hint (ADR-0047's no-bypass line, which the landed
// allowing-prompt row cannot prove), plus the empty slot, the `None` shape, the
// lock discipline, and the class sweep that found `CallbackPrompt` in `lca-sdk`
// still relying on the default.
#[path = "303-shared-prompt-forwards-annotations.rs"]
mod shared_prompt_forwards_annotations;
// GitHub #294: a preset's declared `env` names were parsed and never read
// (fixed in e51dcab5, gh #292), its `supports_reasoning` declaration was inert
// because the reasoning gate read a static environment-derived setting instead
// of the resolved profile's preset, and the `ollama`/`lmstudio` presets could
// not discover models because the manifest declared `net` for one internet host
// and `net` refuses a loopback address by design (FR-PERM-13). The manifest now
// carries `net-local` for exactly `localhost` and `127.0.0.1` - not a CIDR, not
// `*.local` - and the guard asserts the declared set, because a too-broad grant
// would pass a behaviour-only row.
#[path = "294-preset-env-and-local-discovery.rs"]
mod preset_env_and_local_discovery;
// GitHub #365: inline *program text* carried as a positional argument
// auto-approved in a trusted folder. gh #347 policed flag-carried inline eval
// (`python3 -c`) through `INTERPRETER_EVAL_FLAGS`; `awk 'BEGIN{system(...)}'`,
// `sed 's/x/y/e'` and friends carry the same unfile-able code positionally and
// were never classified. The refuse-set is extended the way #347 extended it -
// a known program-text interpreter reviews unless its program arrives as an
// in-workspace file - because a "does this parse as a program" heuristic cannot
// be proven and fails open. Enumerating the cases also found a sharper hole in
// the same family: a denied program behind a wrapper (`timeout 5 curl ...`)
// auto-approved, since the wrapper is the program and `curl` was just an
// unclassified argument.
#[path = "365-inline-program-text.rs"]
mod inline_program_text;
// GitHub #170: a sandboxed WASM extension got an empty `WasiCtx`, so
// `std::env::var` always answered `NotPresent` inside a component while a
// native-linked extension read the host environment directly - the same
// provider worked compiled-in and broke as a `.wasm`. The fix is a real
// capability (#170 is a p2 capability addition, so it carries the catalog's
// seven obligations: ADR, catalog entry, schema section, consent text, denial
// behaviour, conformance entry, threat-model row). The model is *declared exact
// names forwarded at instantiation*, enforced by context construction rather
// than by a check, needing no new WIT import because `wasi:cli/environment`
// already exists. Wildcards are refused for the same reason `net`'s bare `*`
// was; an unset key is absent rather than `Some("")`; declared is not granted;
// and the ambient environment is never forwarded, not even `PATH`.
#[path = "170-env-capability.rs"]
mod env_capability;
// GitHub #363: an extension had no way to *receive* an HTTP request. `net` and
// `net-local` point outward, and `oauth`'s listener exists for one redirect and
// serves a page the host wrote, so a browser UI, a webhook receiver, or a
// documentation viewer could not be built as a WASM extension at all. The fix
// is a capability whose *grant is an export*: the host owns the loopback
// listener, routes `/<session-id>/<extension-name>/`, strips the prefix, and
// calls the extension's handler with a request record. The direction is
// inverted relative to every other capability, which is why the obligation
// rows here are wire-level: the bind address, the mandatory token, the `Host`
// rebinding check, the stripping contract, the port fallback, the no-free-port
// degradation, and clean shutdown. ADR-0049 records why loopback plus a token
// is the minimum rather than loopback alone: a page the user already has open
// can POST to 127.0.0.1 without a preflight.
#[path = "363-inbound-http-capability.rs"]
mod inbound_http_capability;
// GitHub #418 Phase G: the `/mcp` manager modal, and the boundary it sits on.
// Bare `/mcp` was a flat text block; it is now a server list, a per-server detail
// with the actions that state supports, a tool inspector, an exposure picker, and
// an OAuth sign-in screen with a paste field for the SSH case.
//
// The rows here are cross-crate on purpose. `lca-ui`'s own module has the screen
// journeys; what no single crate can check is the contract *between* them: the
// picker's four levels are the config parser's four, the documented transient
// statuses are the function's, the manager's modal types carry data rather than
// display instructions (ADR-0047's boundary, expressed where a test can read it),
// and the scriptable verbs still answer with a notice rather than a widget.
#[path = "418-mcp-manager-modal.rs"]
mod mcp_manager_modal;
// GitHub #404 gap 1: nested calls must not flood the transcript. A tool that
// makes ten nested calls produced eleven cards: gh #77 delivered the linkage
// (`ToolCall.parent_call_id`, `ToolResult.nested`) but nothing in the interface
// read it, so a codemod script that greps twenty times buried its own result
// under twenty cards.
//
// The rendering rows live beside the code (`transcript::nested_tests`). These
// guard the part a single crate cannot see: that the interface *consumes* both
// halves of the linkage at all (a perfect checklist fed by nothing is the state
// the tree was in for the issue's whole life), that the collapsed bound exists
// and keeps the tail (#391's rule reused, not re-derived), and that every child
// state has a glyph as well as a colour (NFR-28).
#[path = "404-nested-call-flood.rs"]
mod nested_call_flood;
// GitHub #464: the footer's `tok/s` cell reported 5,263,158 on a live session.
// The meter opened its window in the interface thread, which drains a bounded
// channel in a burst, so a finished response's deltas all landed within
// microseconds of each other; hidden reasoning tokens were in the numerator
// while their time was not. The authoritative span is now stamped where the
// stream is consumed, carried on the call's own `Usage` extras, and the
// interface divides provider tokens by it. The rendering rows (impossible
// number, zero span, the stale reading) live beside the code in
// `lca-ui/src/footer/meter.rs`.
#[path = "464-tokens-per-second-plausible.rs"]
mod tokens_per_second_plausible;
