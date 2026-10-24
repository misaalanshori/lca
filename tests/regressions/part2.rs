//! Regression guards, part 2 of 4.
//!
//! `mod.rs` is the index and hit gate 11's 1,200-line ceiling purely by being
//! append-only - one entry per shipped defect. These entries are split by
//! generation, not by meaning: nothing inside changed, only which file the `mod`
//! lives in. New guards append to the last part.

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
