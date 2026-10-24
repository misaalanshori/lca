//! Regression guards, part 1 of 4.
//!
//! `mod.rs` is the index and hit gate 11's 1,200-line ceiling purely by being
//! append-only - one entry per shipped defect. These entries are split by
//! generation, not by meaning: nothing inside changed, only which file the `mod`
//! lives in. New guards append to the last part.

// Composer-polish fold-in (no issue): a slash command typed mid-turn
// leaked to the model as text; commands dispatch as commands now.

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
