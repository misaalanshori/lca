//! Regression guards, part 3 of 4.
//!
//! `mod.rs` is the index and hit gate 11's 1,200-line ceiling purely by being
//! append-only - one entry per shipped defect. These entries are split by
//! generation, not by meaning: nothing inside changed, only which file the `mod`
//! lives in. New guards append to the last part.

// Gh #459: the MCP error path reads a live child's stderr without waiting, so a
// silent failure lands `Failed` instead of hanging on `Connecting`.
#[path = "459-stderr-tail-never-blocks.rs"]
mod stderr_tail_never_blocks;

// Gh #405: macOS screenshot path variants and streaming-text safety, plus guards
// for the two behaviours the issue says are already at parity with pi.
#[path = "405-platform-paths-and-stream-safety.rs"]
mod platform_paths_and_stream_safety;

// Gh #402: a cache miss names its cause, once per cause, with counted-by-design
// worded as cost rather than as a defect.
#[path = "402-a-miss-names-its-cause.rs"]
mod a_miss_names_its_cause;

// Gh #489: the RPC loop must poll its turn future in place; a `take()` inside a
// `select!` branch drops the turn and the session's state with it.
#[path = "489-rpc-turn-future-is-never-dropped.rs"]
mod rpc_turn_future_is_never_dropped;

// Gh #481: from `$HOME` the user config is not a project config - the project
// layer does not exist when the two paths name the same file.
#[path = "481-user-config-is-not-a-project-config.rs"]
mod user_config_is_not_a_project_config;

// Gh #484: every extension that reads the environment declares it, or its read is
// gated out of the sandboxed build - the class-level audit behind #461 and this.
#[path = "484-every-env-read-is-declared.rs"]
mod every_env_read_is_declared;

// Gh #476: a capability call made from inside a runtime poll returns instead of
// unwinding - `/mcp` → Sign in used to kill the whole TUI session.
#[path = "476-a-capability-call-never-unwinds-a-ui.rs"]
mod a_capability_call_never_unwinds_a_ui;

// Gh #454: `ToolAnnotations` decodes the camelCase spelling every real payload
// uses, and still accepts the snake_case one this tree's fixtures wrote.
#[path = "454-annotations-decode-in-the-spelling-they-are-sent.rs"]
mod annotations_decode_in_the_spelling_they_are_sent;

// Gh #461: the provider declares the environment it reads, so the WASM guest sees
// it - and the hardcoded workaround that existed without it stays gone.
#[path = "461-workaround-deleted-capability-took-over.rs"]
mod workaround_deleted_capability_took_over;

// Gh #463: from `$HOME` the project-extension directory is the data home, so the
// scan read the install tree and wrongly warned for every installed extension.
#[path = "463-data-home-is-not-a-project.rs"]
mod data_home_is_not_a_project;

// Gh #458: a flag whose value is a program (or a shell string) reviews -
// `vim -c ':!id'`, `tar --to-command=id`, and the four sibling tar flags the
// enumeration found.
#[path = "458-program-valued-flags-review.rs"]
mod program_valued_flags_review;

// Gh #467: the discovery origin fallback parses instead of pattern-matching, so
// the last resort can succeed, and its branch is covered by a row.
#[path = "467-origin-fallback-reachable.rs"]
mod origin_fallback_reachable;

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

// GitHub #462: the tab-switch refusal defeated the primary reason tabs exist.
// #209 specified that switching tabs while an agent generates in Tab 1 does not
// cancel Tab 1's turn - it keeps streaming in the background, its item pulses,
// and an unread badge appears when it settles. The shipped implementation
// refused `open_tab`, `select_tab`, and `close_tab` mid-turn instead, and
// `chat_tab_tests::tab_moves_refuse_mid_turn` pinned that refusal as expected
// behaviour (sanctioned to change by this cycle's brief).
//
// The turn now belongs to the tab: `begin_turn` pins the owner, every event is
// routed by it, a background tab carries its own transcript and session totals,
// and `Ctrl+C` cancels only what the reader can see. The behaviour rows live
// beside the code (`chat::tests::tab_tests`); this file guards the wiring at
// the seams - the key handler, the tab bar, and the per-session accounting.
#[path = "462-background-turns-across-tabs.rs"]
mod background_turns_across_tabs;

// GitHub #456: the native provider path assembled its capability grants by hand
// and let `..Default::default()` absorb what it forgot, dropping `net_local`
// (and `net_declared`) while the WASM host copied both. A provider declaring
// `[capabilities.net-local]` therefore worked as a `.wasm` and silently failed
// as a built-in - the worse half of the #170 class, because a capability went
// *missing* rather than a feature. #294 had just added exactly that grant to
// `openai-compatible` for its `ollama`/`lmstudio` presets, so the native build's
// local-preset discovery was broken at the time this was filed.
//
// The fix is one shared constructor (`Manifest::capability_grants`) so a new
// grant field fails to compile rather than being silently omitted. The
// deliverable is the equality row: native and WASM assembly compared field by
// field over a manifest that populates every capability the schema allows.
#[path = "456-capability-parity-across-delivery-modes.rs"]
mod capability_parity_across_delivery_modes;

// GitHub #477: the pty read loop had no yield and treated the first "nothing
// yet" as the end of the stream. Two defects, one site, and the second one is
// what macOS CI caught: `echo pty-ok` exited 0 and its output was simply
// missing (`"exit 0 "`). The host answered "the session ended" from a single
// syscall's view; the conformance guest looped on that answer with no wait, at
// a measured 584,352 calls per second.
//
// The fix moves the answer to where the facts are - a read says done only when
// the child has exited *and* the buffer is drained, and waits when nothing is
// coming yet - so a caller written the naive way (loop until `None`) is
// correct by construction.
#[path = "477-pty-read-waits-and-drains.rs"]
mod pty_read_waits_and_drains;

// GitHub #406: the text clipboard write spawned a helper per copy
// (`pbcopy`/`clip`/`wl-copy`/`xclip`/`xsel`) while `arboard` - already a direct
// `lca-cli` dependency, already doing the *image* read - sat linked and unused.
// On a machine with none of those five (most Linux boxes, every container) a
// copy degraded to OSC 52 or failed outright for no reason. The fix is seam
// routing: one in-process backend, the helper ladder deleted, and a bound on the
// read so a hung X11 selection owner cannot freeze the interface.
#[path = "406-clipboard-through-the-linked-backend.rs"]
mod clipboard_through_the_linked_backend;

// GitHub #496: a compaction that happens while the user is in the session
// rendered nothing. `apply_turn_event` mapped `CompactionStarted |
// CompactionEnded` to `{}` as "reconstruction events for headless/rpc; rendered
// elsewhere" - but the live path is exactly where the user is when a threshold
// or overflow compaction fires, and the transcript they are reading has just
// been summarized underneath them. The replay path was already correct, so the
// fix adds a bare marker (reason and success, no summary) on the live path and
// pins replay so the working half cannot be traded away.
#[path = "496-live-compaction-marker.rs"]
mod live_compaction_marker;

// GitHub #409's first slice: a plan-mode badge over the regions that already
// exist. The issue is three features (badge, live todo widget, parallel subagent
// fleet capsules) with an `abi-0.7-breakage` label for a possible dedicated dock
// region; this takes the badge alone, driven by an extension through the
// existing `ui` world, because that is the smallest thing that proves the
// pattern. The guard here is the repo-level half - no ABI change, no new widget
// kind - because a behaviour row cannot see a region being added to make a badge
// nicer. The render rows live in `extensions/ui-example/tests/plan_badge.rs`,
// where the extension is first-class.
#[path = "409-plan-mode-badge.rs"]
mod plan_mode_badge;

// GitHub #407 pattern 3: `!vim`, `!git add -p` and `!... | less` hung, because
// every `!`/`!!` command went through `spawn_direct`'s pipes. A fullscreen
// program on a pipe either refuses or blocks for keystrokes that never arrive -
// a hang, with the interface still drawing so the user cannot tell slow from
// wedged. The fix is detection (a small explicit list, a `!i` opt-in, and a
// pipe-tail check, per pi's `isInteractiveCommand`) plus a handoff that reuses
// `$EDITOR`'s stop/run/start rather than growing a second one. Patterns 1 and 2
// of the issue stay open.
#[path = "407-interactive-shell-tty.rs"]
mod interactive_shell_tty;

// GitHub #502: the `@`-file fuzzy walk starved the match it should find. Its
// budget counted **matches** (`out.len() > 200`), so a wide shallow directory at
// the root spent the whole budget on scattered-letter hits and the walk never
// descended to a nested file the query named - `@mai` listed
// `home/.lca/private/mcp` and friends and never `src/main.rs`, while bare `@` did
// list it. The matcher was right; the walk order was wrong. The fix bounds
// **work** (visited entries and directories, each level sorted, breadth-first)
// rather than discovery, which also makes the result deterministic instead of
// `read_dir`-dependent.
#[path = "502-fuzzy-walk-budget.rs"]
mod fuzzy_walk_budget;

// GitHub #398 (gap 1): the sign-in URL over SSH/tmux. A 250-character OAuth URL
// wraps across three or four rows in the login modal; dragging the mouse across
// them grabs the `\n`s at the soft-wrap seams plus the modal's padding and `│`
// borders, and `OSC 8` click-through is unavailable exactly where this matters.
// The URL is the only way to finish the login, so the user had to reconstruct it
// by hand. Gap 1 fixes the two halves that matter: a one-key copy of the exact
// unwrapped URL with in-place `Copied URL to clipboard` feedback, and a short
// clickable label gated on the terminal actually forwarding `OSC 8`. Gaps 2-5
// (cumulative login history, expand-during-confirm, credential provenance, lazy
// themed text) stay open and are listed in the batch report.
#[path = "398-oauth-url-copy.rs"]
mod oauth_url_copy;

// GitHub #408: the trigger-provider seam (the in-process half). These rows read the
// source because the promise is about *shape*: a later cycle could pass every
// behavioural row while turning the decorator into a registry, unguarding the
// trigger validator, or letting the chain own `@`. The WIT import and the
// `autocomplete.trigger` capability are the deferred half (see the report).
#[path = "408-trigger-provider-seam.rs"]
mod trigger_provider_seam;

// GitHub #393: a compaction summary collapses to one self-describing card instead
// of printing a wall of prose the reader has already watched happen. These rows
// guard the shape - the default stays collapsed, the stored record does not
// change, the card reuses the tool-card override lifecycle rather than growing a
// second one, and the live and replay paths build the same card. A branch summary
// gets the same treatment; before this it rendered nowhere at all.
#[path = "393-compaction-card-collapses.rs"]
mod compaction_card_collapses;

// GitHub #395 (gap 1): threaded forks in `/resume`. The picker was a flat
// chronological list, so a fork family - the ordinary case after a few `/fork`s -
// showed the parent and its children as unrelated rows and the only ordering was
// last-write time. It is a tree now: parent heads a block, children hang under it
// with `│/├─/└─` connectors, families sort by their **subtree's** latest activity
// (a busy fork lifts its parent), a query flattens to the scored list and clearing
// it restores the tree. The linkage is ADR-0046's `fork-point` record's
// `parent_session` - **not** pi's `parentSessionPath`, which this design has no
// field for - already read into `SessionSummary::parent_session` by the store, so
// no storage change was needed. Gaps 2-5 (streaming progress + touch preservation,
// hierarchy colours, safe delete, scope toggle / rename / full-text) stay open and
// are listed in the batch report.
#[path = "395-fork-tree.rs"]
mod fork_tree;

// GitHub #396: the `/model` search ranks the real model above the proxies that
// resell it, and `/thinking` gains the save-as-default that `/model` already had.
// These rows guard the shapes a behaviour row cannot see: that the search no longer
// matches the display label, that the ranking is scored and stably sorted, that the
// thinking default uses the one persistence path, and that both pickers advertise
// the save from their live bindings.
#[path = "396-model-search-and-thinking-default.rs"]
mod model_search_and_thinking_default;

// GitHub #399: the owner-approved logo and the three-line startup header. The
// brief treats the logo as a **supplied asset**, so these rows guard the three ways
// a supplied asset silently stops being supplied: the grid gets prettified, the
// logo's three colours drift into the theme (the owner was explicit they are icon
// colours), or the Apple_Terminal fallback turns back into a guess. Behaviour rows
// live beside their modules - `logo_tests`, `startup_header_tests`,
// `startup_header_wired_tests`, and `terminal_tests`.
#[path = "399-logo-and-startup-header.rs"]
mod logo_and_startup_header;

// GitHub #266: a disabled extension runs nothing, and stays disabled however late
// it is registered. The primary half (MCP spawns nothing) landed as `16fc841e`
// with behavioural rows in `crates/lca-cli/tests/mcp_enablement.rs`; this file
// guards the seam that keeps the *next* registration path honest
// (`registry::register_gated`), plus the ordering property that makes disablement
// real rather than reported.
#[path = "266-disabled-extension-runs-nothing.rs"]
mod disabled_extension_runs_nothing;

// GitHub #331: the fullscreen tab bar takes a row from the transcript window, and
// four sites each computed that window themselves - three of them without the tab
// term - so clicks landed a row off, the top stepper never fired, the scroll extent
// was one short, and tooltips fired a row low. The behaviour rows live in
// `crates/lca-ui/src/chat_offset_tests.rs`; this file guards the *design* (one
// function, conditional on the renderer), because four green symptoms still leave a
// fifth copy of the arithmetic waiting to be written.
#[path = "331-tab-row-offset.rs"]
mod tab_row_offset;

// GitHub #329: the Extension Hub filter typed `[` on unhandled keys. The arm scanned
// raw input bytes for the first printable character, so an ANSI sequence's second
// byte (`[`) became text. The field now takes parsed text
// (`printable`/`paste_text`), the path every other inline field already used - which
// is why they were immune. Behaviour rows live in `chat_hub_tests.rs`; this file
// guards the shape and proves the character-blacklist "fix" stays rejected.
#[path = "329-hub-filter-takes-parsed-text.rs"]
mod hub_filter_takes_parsed_text;

// GitHub #332: `/reload` re-read the key map and replaced `Chat::keybindings`, while
// `Chat::editor` kept the map it was built with - one fact with two owners and nothing
// keeping them equal. The instance was already fixed by gh #410's single setter; what
// this file guards is the *seam*, so the next place that replaces the map cannot forget
// again: the field is no longer `pub`, exactly one function writes it, that function
// updates all three holders, and the editor holds no parsed-binding cache that would
// need its own invalidation. Behaviour rows: `crates/lca-ui/src/chat_reload_keys_tests.rs`.
#[path = "332-reload-rebinds-editor.rs"]
mod reload_rebinds_editor;

// GitHub #371 bugs 1 and 2: the fullscreen-free renderer recorded the caret's
// *requested* row while the terminal clamped the result at the viewport top, so the
// next frame's diff started from a screen row that never existed and the differential
// rendering desynchronised permanently; and the painted caret closed reverse video with
// a full `\x1b[0m`, stripping the style of everything after it. Behaviour rows live in
// `main_screen.rs` and `widgets/editor.rs`; this file guards the two shapes that would
// let either come back. Bugs 3 (overlay marker ownership) and 4 (`show_hardware_cursor`)
// stay open on the issue.
#[path = "371-caret-row-and-sgr-close.rs"]
mod caret_row_and_sgr_close;

// Gh #388 gap 1: the separator's status and its `↑ N more` badge coexist on one
// row instead of overwriting each other.
#[path = "388-status-badge-coexistence.rs"]
mod status_badge_coexistence;

// Gh #372 bug 1: a toast must never wipe the scrollback. The painter's parameter
// is a screen slice by contract, and the scrollback path slices the frame out.
#[path = "372-toast-does-not-wipe-scrollback.rs"]
mod toast_does_not_wipe_scrollback;

// Gh #373 bug 1: a spliced overlay lands at its column, not at the end of the row's
// text; both seams go through the style-tracking instrument.
#[path = "373-splice-pads-to-column.rs"]
mod splice_pads_to_column;

// Gh #375 gaps 2 + 4: the scrollbar column keeps the block's background (the
// 1-cell hole down a shaded block's right edge), and plain Home/End belong to the
// editor while the viewport's top/bottom move to Ctrl+Home/Ctrl+End.
#[path = "375-scrollbar-background-and-home-split.rs"]
mod scrollbar_background_and_home_split;

// Gh #378 gaps 1-2: the width fast path allocates on nothing and declines rather than
// guessing an escape length; an SGR arriving at the slice seam keeps its order.
#[path = "378-fast-path-and-sgr-order.rs"]
mod fast_path_and_sgr_order;

// Gh #377: a binding on a shifted symbol (`?`, `ctrl+?`, `alt+~`) can be written down
// naturally and matches, through one shared rule for both key protocols.
#[path = "377-shifted-symbol-bindings.rs"]
mod shifted_symbol_bindings;

// Gh #383 gap 1: `Ctrl+W` deletes a segment, not a path - one classifier for
// `Alt+Left`, `Alt+Right` and deletion, with punctuation as its own class.
#[path = "383-word-motion-segments.rs"]
mod word_motion_segments;

// Gh #379 gap 1: the terminal colour query is a lifecycle closed by a DA1 sentinel,
// with one recogniser for every reply kind - so a slow link's answer still lands and a
// stray OSC 11 from a child process cannot repaint the palette mid-session.
#[path = "379-colour-query-lifecycle.rs"]
mod colour_query_lifecycle;

// Gh #370 flaw 1: the loop blocks on every source it has, not stdin alone - so a
// streamed token, a shell chunk or a permission request wakes it instead of waiting for
// the 50 ms tick; input is still tried first, and 200 ms of idle still paints nothing.
#[path = "370-loop-wakes-on-every-source.rs"]
mod loop_wakes_on_every_source;

// GitHub #384: table layout measured `natural`/`min_word` on raw Markdown,
// so one linked cell starved every other column.
#[path = "384-table-measures-rendered-cells.rs"]
mod table_measures_rendered_cells;
