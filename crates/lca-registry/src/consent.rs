//! The consent surface (FR-DIST-7's comparison basis, FR-PERM-2's sentences):
//! the canonical grant hash, the consent lines a manifest earns, the update-widens
//! comparison, and the extension-name rule the install tree joins onto disk.
//!
//! Everything here is a *statement about a manifest*: none of it touches disk or
//! the network, which is why it reads as the consent contract on its own.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::Error;

// ---------------------------------------------------------------------------
// The approved-grant hash (FR-DIST-7's comparison basis)
// ---------------------------------------------------------------------------

/// Canonical text for a TOML value: tables sorted by key at every depth,
/// so two manifests declaring the same grants in a different written
/// order hash the same, and any changed value hashes differently.
fn canonical(value: &toml::Value) -> String {
    match value {
        toml::Value::Table(table) => {
            let mut keys: Vec<&String> = table.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|key| format!("{key}={}", canonical(&table[key])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        toml::Value::Array(array) => {
            let inner: Vec<String> = array.iter().map(canonical).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

/// Hash of a manifest's declared grant set: exactly what the consent
/// screen showed, nothing else (SRDD's extension-tree paragraph: ad hoc
/// grants live in the user grant store and stay out of this hash).
pub fn grant_hash(manifest: &str) -> Result<String, Error> {
    let parsed: toml::Value = manifest
        .parse()
        .map_err(|err| Error::Invalid(format!("manifest does not parse: {err}")))?;
    let capabilities = parsed
        .get("capabilities")
        .cloned()
        .unwrap_or(toml::Value::Table(toml::map::Map::new()));
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(canonical(&capabilities).as_bytes())
    ))
}

// ---------------------------------------------------------------------------
// Consent text (capabilities catalog: the manifest is the consent surface)
// ---------------------------------------------------------------------------

/// One consent line per declared capability, in the catalog's words.
pub fn consent_lines(manifest: &str) -> Result<Vec<String>, Error> {
    let parsed: toml::Value = manifest
        .parse()
        .map_err(|err| Error::Invalid(format!("manifest does not parse: {err}")))?;
    // ADR-0030: the package's `resources/` bag, shown as kind counts. A
    // data-only extension has no capabilities but still shows its bag.
    let resource_line = resource_consent(&parsed);
    let Some(table) = parsed.get("capabilities").and_then(|c| c.as_table()) else {
        return Ok(resource_line.into_iter().collect());
    };
    let mut lines = Vec::new();
    let reason = |table: &toml::Value, key: &str| {
        table
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    for (name, value) in table {
        match name.as_str() {
            "fs" => {
                let mut scopes = Vec::new();
                let mut writable = false;
                if let Some(table) = value.as_table() {
                    let mut keys: Vec<&String> = table.keys().collect();
                    keys.sort();
                    for scope in keys {
                        let mode = table[scope].as_str().unwrap_or("read");
                        if mode == "read-write" {
                            writable = true;
                        }
                        scopes.push(match mode {
                            "read-write" => format!("{scope} (read and write)"),
                            other => format!("{scope} ({other})"),
                        });
                    }
                }
                lines.push(format!(
                    "Files: {}. {}",
                    scopes.join(", "),
                    if writable {
                        "It can read and write those files."
                    } else {
                        "It can read those files."
                    }
                ));
            }
            "process" => lines.push(format!(
                "{} Each command still asks for your approval.",
                reason(value, "reason")
            )),
            "pty" => lines.push(format!(
                "{} This gives it an interactive terminal session.",
                reason(value, "reason")
            )),
            "net" => {
                if let Some(hosts) = value.get("hosts").and_then(|h| h.as_array()) {
                    let names: Vec<String> = hosts
                        .iter()
                        .filter_map(|h| h.as_str())
                        .map(|h| h.replace("*.", "any subdomain of "))
                        .collect();
                    lines.push(format!("Connect to {}. HTTPS only.", names.join(", and ")));
                }
            }
            "net-local" => lines.push(
                "Connect to a device on your local network, your own machine, \
                 or your private tailnet."
                    .to_string(),
            ),
            "oauth" => lines.push(
                "Open a browser sign-in and receive the response on a local port.".to_string(),
            ),
            "credentials" => lines.push("Store and read its own saved credentials.".to_string()),
            // Gh #170: an environment variable is often a secret, so the line
            // names every key. "Read some variables" would be a consent screen
            // that says nothing - the same reasoning that refuses a wildcard
            // grant.
            "env" => {
                let mut keys: Vec<String> = value
                    .get("keys")
                    .and_then(|keys| keys.as_array())
                    .map(|keys| {
                        keys.iter()
                            .filter_map(|key| key.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                keys.sort();
                if keys.is_empty() {
                    lines.push(
                        "Read named environment variables, none of which are granted yet."
                            .to_string(),
                    );
                } else {
                    lines.push(format!(
                        "Read {} from your environment.",
                        keys.iter()
                            .map(|key| format!("`{key}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            "completion" => lines.push(format!(
                "{} This lets it ask the current model for a response.",
                reason(value, "reason")
            )),
            // ABI 0.6 (gh #77): nested tool calling through the host. The arm
            // was missing while the WIT, the host manifest parser, and
            // `docs/capabilities.md` all knew it, so installing any package
            // that declared it failed at this match (gh #307). The
            // schema-vocabulary guard test now fails at the growth point
            // instead: a capability added to the schema without an arm here
            // turns `every_schema_capability_has_a_consent_line` red.
            "tools" => lines.push(format!(
                "{} This lets it list, activate, and call other tools in the session.",
                reason(value, "reason")
            )),
            "ui" => lines.push(
                "Show a segment in the status line and content in the side panel.".to_string(),
            ),
            // Gh #363 / ADR-0049: the inbound HTTP grant. The reach is stated
            // plainly - "on this machine" is not "private to LCA", and a user
            // approving a local port should know that any program on it can
            // connect, not just this agent. The port is not named here because
            // it is not known until the session starts (the fallback may land
            // elsewhere); the session's own notice names the one bound.
            "serve-http" => lines.push(format!(
                "{} Serve a web interface on this machine, reachable by any program on it.",
                reason(value, "reason")
            )),
            other => {
                return Err(Error::Invalid(format!(
                    "unknown capability `{other}` in the manifest"
                )));
            }
        }
    }
    if let Some(line) = resource_line {
        lines.push(line);
    }
    Ok(lines)
}

/// The consent line for a manifest's declared resource kinds, when any.
fn resource_consent(parsed: &toml::Value) -> Option<String> {
    let kinds: Vec<String> = parsed
        .get("resources")
        .and_then(|v| v.as_array())?
        .iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect();
    if kinds.is_empty() {
        return None;
    }
    Some(format!(
        "Ship data files ({}). These are read by the extension, not code it runs.",
        kinds.join(", ")
    ))
}
/// The extension identifier rules the loader's manifest parser enforces
/// (`lca_ext_host::Manifest::parse`), mirrored at the install tree so the
/// name - taken from an attacker-influenced manifest and then joined onto
/// the filesystem - can never escape the tree. See [`crate::InstallTree::install`].
pub fn validate_extension_name(name: &str) -> Result<(), Error> {
    let ok = name.starts_with(|c: char| c.is_ascii_lowercase())
        && (2..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.contains("--")
        && !name.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "`{name}` is not a valid extension name (lowercase letters, digits, and single hyphens; 2-64 chars)"
        )))
    }
}
// ---------------------------------------------------------------------------
// Capability comparison for the update prompt (FR-DIST-7)
// ---------------------------------------------------------------------------

fn capability_map(manifest: &toml::Value) -> BTreeMap<String, String> {
    manifest
        .get("capabilities")
        .and_then(|c| c.as_table())
        .map(|table| {
            table
                .iter()
                .map(|(name, value)| (name.clone(), canonical(value)))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether updating to `new_manifest` would apply a capability the
/// approved set does not already cover exactly: every new declaration
/// must already be there, byte for byte (FR-DIST-7's prompt rule).
/// Narrowing or removing a grant never prompts; the consent shown at
/// install time stays what governs.
pub fn update_widens_grants(approved_manifest: &str, new_manifest: &str) -> Result<bool, Error> {
    let approved: toml::Value = approved_manifest
        .parse()
        .map_err(|err| Error::Invalid(format!("approved manifest does not parse: {err}")))?;
    let new: toml::Value = new_manifest
        .parse()
        .map_err(|err| Error::Invalid(format!("new manifest does not parse: {err}")))?;
    let old = capability_map(&approved);
    let incoming = capability_map(&new);
    Ok(incoming
        .iter()
        .any(|(name, value)| old.get(name) != Some(value)))
}
