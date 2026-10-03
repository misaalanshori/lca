//! The request-path consent for an endpoint host the provider's manifest
//! does not cover (gh #29, QA-004).
//!
//! `OPENAI_BASE_URL` pointed at a host outside the manifest's fixed hosts
//! used to dead-end: every request came back `matches no granted pattern`
//! until an interactive `/login` happened to attach the ad hoc grant, so a
//! scripted run could not configure an endpoint at all. The pieces already
//! existed - the grant store's `net_patterns` (ADR-0022), the login-time
//! grant modal - they simply never fired on the request path.
//!
//! This module is that firing. One function, [`endpoint_consent`](crate::net_consent::endpoint_consent), called
//! by both modes before the turn's first request, going through
//! [`lca_permissions::authorize`] - the same seam the model's tool commands
//! ask through, so this builds no second prompt path:
//!
//! - **interactive**: the modal names the exact host (FR-PERM-16); `allow`
//!   persists it per project (FR-PERM-18/19) and takes effect without a
//!   restart, so neither the next turn nor the next process asks again.
//! - **headless**: there is no modal, so the answer is denial and the
//!   caller exits 4 with [`denied_message`](crate::net_consent::denied_message), naming the host and the fix.
//! - **`--allow-host <host>`**: [`attach_allow_hosts`](crate::net_consent::attach_allow_hosts) puts the pattern in
//!   the session set for this run only and records it once, like a `once`
//!   answer.
//!
//! A `deny` rule refuses without prompting (ADR-0039's rules-first
//! precedence stands), and yolo mode answers this prompt like every other
//! (ADR-0042), recorded the same way a human answer is.

use std::path::Path;
use std::sync::{Arc, Mutex};

use lca_permissions::{Action, GrantStore, PermissionPrompt, authorize};
use lca_protocol::{FORMAT_VERSION, PermissionDecision, Record};
use lca_session::{Session, SessionStore};

/// What the request path found for one endpoint host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointConsent {
    /// Already covered - a persisted grant, this run's `--allow-host`, or
    /// an allow rule. Nothing was asked.
    Granted,
    /// The prompt allowed it, persisted per project (FR-PERM-18/19): no
    /// later turn and no later process asks about this host again.
    Allowed,
    /// A rule or the answer refuses it. The caller must not proceed.
    Denied,
}

/// Ask for `host`'s consent once, on the request path.
///
/// The prompt is `prompt` - the turn's own permission prompt, so a headless
/// run answers by denying and the interface opens its normal modal. The
/// decision is recorded in the session like every other grant decision
/// (a prompted answer, a rule denial, or a yolo answer), because this call
/// sits below the turn loop, which records the tool gates it answers
/// itself and never sees this one.
pub fn endpoint_consent(
    host: &str,
    grants: &Arc<Mutex<GrantStore>>,
    cwd: &Path,
    prompt: &mut dyn PermissionPrompt,
    store: &SessionStore,
    session: &Session,
) -> EndpointConsent {
    let action = Action::Net {
        host: host.to_string(),
    };
    // The lock is taken for this check only, exactly as the turn loop
    // takes it for its own authorize call.
    let mut guard = crate::lock(grants);
    // Rules first: a `deny` refuses without prompting and outranks a grant
    // the store already holds (ADR-0039).
    if guard.rule_denied(cwd, &action) {
        record(store, session, &action, PermissionDecision::Denied, None);
        return EndpointConsent::Denied;
    }
    // Covered already: a grant this project holds (including this run's
    // `--allow-host`, which `net_patterns` reports with the persisted set).
    if guard
        .net_patterns(cwd)
        .iter()
        .any(|pattern| pattern == host)
    {
        return EndpointConsent::Granted;
    }
    match authorize(&mut guard, cwd, &action, None, prompt) {
        Ok(outcome) => {
            // A prompted answer, a yolo answer, and a rule denial all
            // belong in the log (ADR-0042: approve everything must never
            // mean forget everything).
            if outcome.prompted || outcome.yolo || outcome.denied_by_rule {
                let decision = if outcome.stored_pattern.is_some() {
                    PermissionDecision::Always
                } else if outcome.allowed {
                    PermissionDecision::Once
                } else {
                    PermissionDecision::Denied
                };
                record(store, session, &action, decision, outcome.stored_pattern);
            }
            if outcome.allowed {
                EndpointConsent::Allowed
            } else {
                EndpointConsent::Denied
            }
        }
        Err(err) => {
            tracing::error!(%err, "permission store error during the endpoint consent");
            EndpointConsent::Denied
        }
    }
}

/// Attach every `--allow-host` pattern for this run and record each once.
///
/// The flag is a one-run grant by design: the pattern joins the session
/// set (never `grants.json`), so a run without it asks - or exits 4 -
/// again. Invalid patterns are a usage error, which the flag's own parser
/// already turned into exit 2; this re-check keeps that guarantee if the
/// function is reached another way.
pub fn attach_allow_hosts(
    grants: &Arc<Mutex<GrantStore>>,
    cwd: &Path,
    hosts: &[String],
    store: &SessionStore,
    session: &Session,
) -> Result<(), String> {
    let mut guard = crate::lock(grants);
    for host in hosts {
        guard
            .attach_session_net_pattern(cwd, host)
            .map_err(|err| err.to_string())?;
        let action = Action::Net { host: host.clone() };
        // The `once` shape: allowed for this run, nothing persisted.
        record(store, session, &action, PermissionDecision::Once, None);
    }
    Ok(())
}

/// What headless mode prints when the answer is denial: the host and the
/// fix, not a subsystem's name for the failure.
pub fn denied_message(host: &str) -> String {
    format!(
        "error: the endpoint host {host} is not granted for this project, and a \
         non-interactive run cannot approve it\n\
         \x20   run `lca` interactively once and approve the prompt, or pass \
         `--allow-host {host}` to allow it for this run only"
    )
}

/// Append one `permission` record: a grant decision made during the
/// session (`docs/session-log-format.md`).
fn record(
    store: &SessionStore,
    session: &Session,
    action: &Action,
    decision: PermissionDecision,
    pattern: Option<String>,
) {
    let record = Record::Permission {
        v: FORMAT_VERSION,
        ts: lca_session::now_ms(),
        id: lca_session::new_record_id(),
        action: action.display(),
        decision,
        pattern,
    };
    if let Err(err) = store.append(session, record) {
        tracing::error!(%err, "cannot record permission decision");
    }
}

/// The endpoint host configured **through the environment** and nothing
/// else: the path QA-004 is about, and the one with no prompt anywhere
/// else in the product.
///
/// A base URL sitting in the provider's credential namespace came from
/// `/login`, which already offers the ad hoc grant prompt for exactly that
/// host (FR-PERM-16, `tui/login.rs`), so asking again would be the second
/// prompt for one decision - and `env` matching a stored host is precisely
/// that case. An environment host the stored endpoint does *not* match has
/// never been offered, so it is consented to here.
pub fn env_configured_host(data: &Path) -> Option<String> {
    let base = std::env::var("OPENAI_BASE_URL")
        .ok()
        .filter(|value| !value.is_empty())?;
    let host = host_from_base(&base)?;
    match stored_endpoint_host(data) {
        Some(stored) if stored == host => None,
        _ => Some(host),
    }
}

/// The host of a base URL (`https://host/v1`): lowercased, port stripped,
/// `None` for the manifest's own default endpoint.
fn host_from_base(base: &str) -> Option<String> {
    let rest = base.split("://").nth(1).unwrap_or(base);
    crate::ad_hoc_host_from_authority(rest)
}

/// The base URL stored in the provider's credential namespace - what
/// `/login` wrote.
fn stored_endpoint_host(data: &Path) -> Option<String> {
    let path = data.join("credentials").join("openai-compatible.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let base = value.get("base_url")?.as_str()?.to_string();
    host_from_base(&base)
}

/// The flag parser for `--allow-host`: the same `net` vocabulary the
/// grant store validates, so a bad pattern is a usage error (clap exits 2)
/// rather than a surprise at request time.
pub fn parse_allow_host(text: &str) -> Result<String, String> {
    lca_permissions::parse_net_pattern(text)
        .map(|_| text.to_string())
        .map_err(|err| err.to_string())
}
