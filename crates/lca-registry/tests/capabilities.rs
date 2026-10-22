//! The capability vocabulary (gh #307): `consent_lines` must handle
//! EXACTLY the names the manifest schema declares, so a capability added
//! to the normative source without a consent arm fails here — at the
//! vocabulary's growth point — instead of at a user's install.
//!
//! Split from `registry.rs` for the workspace file ceiling (gate 11).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The normative capability vocabulary: the `capabilities` property names
/// in the manifest schema (per the SRDD, the schema is the manifest's
/// source of truth). Reading it in a test keeps every mirror honest at the
/// vocabulary's growth point.
fn schema_capability_names() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas/extension-manifest.schema.json");
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read schema"))
            .expect("schema parses");
    let mut names: Vec<String> = schema["properties"]["capabilities"]["properties"]
        .as_object()
        .expect("capabilities properties")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// A minimal manifest declaring exactly `capability` with a reason.
fn manifest_declaring(capability: &str) -> String {
    match capability {
        "fs" => "[capabilities.fs]\nsrc = \"read\"\n".to_string(),
        "net" => "[capabilities.net]\nhosts = [\"api.example.com\"]\n".to_string(),
        "net-local" | "pty" | "ui" | "credentials" | "oauth" => {
            format!("[capabilities.{capability}]\n")
        }
        other => format!("[capabilities.{other}]\nreason = \"a reason\"\n"),
    }
}

// Verifies: gh #307 (the drift class) - `consent_lines` must handle
// EXACTLY the schema's capability vocabulary. The schema is the normative
// manifest source, so a capability added there without a consent arm turns
// this red at the vocabulary's growth point instead of at a user's install.
#[test]
fn every_schema_capability_has_a_consent_line() {
    let names = schema_capability_names();
    assert!(
        names.len() >= 10,
        "the vocabulary should not shrink silently: {names:?}"
    );
    for name in &names {
        let manifest = format!(
            "name = \"p\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\ndescription = \"x\"\n\n{}",
            manifest_declaring(name)
        );
        let lines = lca_registry::consent_lines(&manifest).unwrap_or_else(|err| {
            panic!("schema capability `{name}` must have a consent line: {err}")
        });
        assert!(
            !lines.is_empty() && lines.iter().all(|line| !line.is_empty()),
            "`{name}` produces a non-empty consent sentence: {lines:?}"
        );
    }
}

// Verifies: gh #307 - the mirror holds the other way too: a capability the
// schema does NOT declare is still refused, so the wildcard arm stays.
#[test]
fn a_capability_outside_the_schema_vocabulary_is_still_refused() {
    let err = lca_registry::consent_lines(
        "name = \"x\"\n[capabilities.not-a-real-capability]\nreason = \"r\"\n",
    )
    .expect_err("a non-schema capability is refused");
    assert!(
        err.to_string().contains("not-a-real-capability"),
        "the error names the capability: {err}"
    );
}

// Verifies: gh #307 AC1/AC2 - a `[capabilities.tools]` manifest produces
// the documented consent sentence instead of `unknown capability`.
#[test]
fn the_tools_capability_has_its_consent_sentence() {
    let manifest = r#"name = "tool-caller"
version = "1.0.0"
abi = "0.6"
worlds = ["tool"]
description = "x"

[capabilities.tools]
reason = "Chains a second opinion."
"#;
    let lines = lca_registry::consent_lines(manifest).expect("tools is a known capability");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        lines[0],
        "Chains a second opinion. This lets it list, activate, and call other tools in the session."
    );
}

// Verifies: gh #170 - the `env` consent line names every key, because an
// environment variable is often a secret and the list is the whole gate. A line
// that said "read some variables" would be a consent screen that says nothing,
// the same reasoning that refuses a wildcard grant.
#[test]
fn the_env_consent_line_names_every_key() {
    let manifest = "name = \"p\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\ndescription = \"x\"\n\n[capabilities.env]\nkeys = [\"ZEBRA_KEY\", \"ALPHA_KEY\"]\nreason = \"Reads the keys its documentation names.\"\n";
    let lines = lca_registry::consent_lines(manifest).expect("a consent line exists");
    let env_line = lines
        .iter()
        .find(|line| line.contains("environment"))
        .expect("the env capability has a line");
    for key in ["ZEBRA_KEY", "ALPHA_KEY"] {
        assert!(
            env_line.contains(key),
            "the consent line must name `{key}`, since the names are what the user \
             is approving: {env_line}"
        );
    }
    // An empty grant is a declaration that grants nothing (the gh #302
    // `hosts = []` precedent), and must not claim otherwise.
    let empty = "name = \"p\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\ndescription = \"x\"\n\n[capabilities.env]\nkeys = []\nreason = \"Nothing yet.\"\n";
    let lines = lca_registry::consent_lines(empty).expect("parses");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("environment") && line.contains("none")),
        "an empty declaration grants nothing and says so: {lines:?}"
    );
}

/// Gh #363 / ADR-0049: the `serve-http` consent line must state the reach
/// honestly.
///
/// "On this machine" is not "private to LCA": any program on the machine can
/// connect to a loopback port, and a web page the user has open can POST to it.
/// A line that implied otherwise would be a consent screen that understates what
/// is being approved, which is worse than no line at all - the whole capability
/// model depends on people reading these.
#[test]
fn the_serve_http_consent_line_names_the_reach_and_leads_with_the_reason() {
    let manifest = "name = \"p\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"serve-http\"]\ndescription = \"x\"\n\n[capabilities.serve-http]\nreason = \"Serves the browser mirror of this session.\"\n";
    let lines = lca_registry::consent_lines(manifest).expect("a consent line exists");
    let line = lines
        .iter()
        .find(|line| line.contains("web interface"))
        .unwrap_or_else(|| panic!("the serve-http capability has a line: {lines:?}"));
    // The reason leads, so the user reads what it is FOR before what it reaches.
    assert!(
        line.starts_with("Serves the browser mirror of this session."),
        "the reason must lead the line: {line}"
    );
    // The reach, in words a user can evaluate: not "local" but "on this
    // machine, reachable by any program on it".
    assert!(
        line.contains("on this machine") && line.contains("any program"),
        "the line must not imply the port is private to LCA: {line}"
    );
}
