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
