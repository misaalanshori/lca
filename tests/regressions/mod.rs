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
