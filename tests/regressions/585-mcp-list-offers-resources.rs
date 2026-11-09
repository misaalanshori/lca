//! NFR-24 guard for gh #585: `lca mcp list` reports a server's resources from the
//! connection, never from the config entry.
//!
//! ## The defect
//!
//! `CliMcp::list` built each row from `ServerStatus` (live facts) plus the config
//! entry, and took `resources` from the entry side:
//!
//! ```text
//! fn transport_of(entry: &ServerEntry) -> (String, bool) {
//!     EntryKind::Stdio { .. } => (format!("stdio: …"), false),
//!     EntryKind::Http { .. }  => (format!("http: {url}"), false),
//! }
//! ```
//!
//! Both arms returned `false`, so `ServerReport.resources` was `false` for every
//! server on every run, and `print_list` could never print `  offers resources`
//! while `--json` said `"resources": false`. Measured side by side on one fixture
//! and one moment: `pi 1.3.0` printed `offers resources`, LCA printed nothing - and
//! LCA was the one making a positive claim, because a `false` in a report reads as
//! "I asked and the answer was no".
//!
//! A function that reads a config file cannot know whether a server offers
//! resources: that is a fact about a **handshake**. Returning it from there is how
//! it came to be a constant.
//!
//! ## What this pins
//!
//! The behaviour is in `crates/lca-cli/tests/mcp_cli.rs`:
//! `list_reports_offers_resources_from_the_connection` drives the real binary over
//! three servers in one invocation - `echo` (advertises `capabilities.resources`),
//! `probe` (advertises only tools), `ghost` (never connects) - and asserts both
//! renderings of one report: the text form says `offers resources` inside the
//! `echo` row only, and `--json` says `true`, `false`, `false` with the field
//! present on all three rows. It fails on the pre-fix tree (the text assertion for
//! `echo`) and it fails on two wrong fixes: `resources: false` (the old defect)
//! and `resources: status.tools > 0` (the plausible mistake - tool count is not
//! resource capability).
//!
//! What is left here is the claim no behavioural row can state: **the flag's
//! source is the connection.** A future refactor could derive it from the entry
//! again and still pass the behavioural row on a fixture whose entry type happens
//! to imply resources - or, worse, could pass by accident on the day the fixture's
//! entry and its handshake agree. The structure is the discipline:
//!
//! 1. `transport_of` returns a description only. A bool in that signature is the
//!    defect re-entering by the same door.
//! 2. `ServerStatus::offers_resources` is computed from `self.live`, not from
//!    `self.entry` - so a server with no connection cannot report anything but
//!    `false`, and `false` there stays "not asked".
//! 3. `list` copies that field; it does not recompute it from anything else.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::Path;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("read {rel}: {err}"))
}

const CLI: &str = "crates/lca-mcp/src/cli.rs";
const NATIVE: &str = "crates/lca-mcp/src/native.rs";

/// The source lines of one `fn` body, sliced from its signature to the first
/// line that starts at column zero after it (the next item).
fn item(source: &str, signature: &str, file: &str) -> String {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` exists in {file}"));
    let rest = &source[start..];
    let end = rest
        .match_indices('\n')
        // Skip the signature line itself, then stop at the first top-level item.
        .skip(1)
        .map(|(at, _)| at)
        .find(|at| rest[at + 1..].starts_with("fn ") || rest[at + 1..].starts_with("pub fn "))
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

// Verifies: gh #585 - a transport description describes a transport.
//
// The second return value was the whole defect: a function that only reads
// `entry.kind` cannot answer "does this server offer resources", so the value it
// returned was a constant. If a future change needs to name a transport, it may
// add a field or a method - what it may not do is answer a connection question
// from a config entry again.
#[test]
fn transport_of_returns_a_description_and_nothing_else() {
    let source = read(CLI);
    let signature = source
        .lines()
        .find(|line| line.starts_with("fn transport_of"))
        .unwrap_or_else(|| panic!("`transport_of` exists in {CLI}"));
    assert!(
        signature.contains("-> String {") || signature.contains("-> String\n"),
        "`transport_of` returns a description only; a bool beside it is how the \
         constant `false` got there (gh #585): {signature}"
    );
    assert!(
        !signature.contains("bool"),
        "no second return value: {signature}"
    );
    let body = item(&source, "fn transport_of", CLI);
    assert!(
        !body.contains("offers_resources"),
        "the description does not consult resource capability - it reads a config \
         file, so it cannot (gh #585):\n{body}"
    );
}

// Verifies: gh #585 - the fact is read from the connection that was made.
//
// `ManagedServer::status` is the one place a live fact becomes a reported one.
// `tools` was already derived there; `offers_resources` now sits beside it, from
// the same `live`. Deriving it from `self.entry` would compile and would report
// something - just not the server's answer.
//
// The comparison squeezes out whitespace first: this asserts **where the value
// comes from**, and a line break inside the expression is formatting, not
// behaviour. A row that fails on `cargo fmt`'s decisions would be teaching the
// wrong lesson.
#[test]
fn offers_resources_is_computed_from_the_live_connection() {
    let source = read(NATIVE);
    let body = item(&source, "fn status(&self) -> ServerStatus", NATIVE);
    let squeezed: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let flag = squeezed
        .split("offers_resources:")
        .nth(1)
        .unwrap_or_else(|| panic!("`status` sets `offers_resources`: {body}"));
    assert!(
        flag.contains("self.live") && flag.contains("LiveServer::offers_resources"),
        "the flag comes from `self.live` and the handshake's own predicate: {body}"
    );
    assert!(
        !flag.contains("self.entry"),
        "a config entry cannot answer this: `entry` describes what to connect to, \
         not what the connection said (gh #585): {body}"
    );
    // And the accessor it uses exists on `LiveServer` for both transports.
    let accessor = item(&source, "pub(crate) fn offers_resources", NATIVE);
    assert!(
        accessor.contains("LiveServer::Stdio") && accessor.contains("LiveServer::Http"),
        "both transports answer the same predicate: {accessor}"
    );
}

// Verifies: gh #585 - `list` copies the live field, and the text and JSON
// renderings read one value.
//
// The alternative - recomputing `resources` in `list` from the entry or from the
// tool count - is the mistake that looks equivalent and is not: a server can offer
// resources with zero tools, and a server can offer tools with no resources.
#[test]
fn list_copies_the_field_rather_than_recomputing_it() {
    let source = read(CLI);
    let body = item(&source, "pub fn list(&self, budget:", CLI);
    assert!(
        body.contains("resources: status.offers_resources,"),
        "`list` copies the reported flag verbatim: no entry lookup, no tool count, \
         no second computation (gh #585): {body}"
    );
    assert!(
        !body.contains("resources: false") && !body.contains("resources: status.tools"),
        "a constant or a proxy is exactly the defect (gh #585): {body}"
    );
}
