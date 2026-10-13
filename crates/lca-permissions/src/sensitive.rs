//! Sensitive-read escalation (gh #215): credential filenames whose
//! reads prompt even in-workspace, amending FR-PERM-27's fatigue cut.
//! Matching reads the file name only (never the directory), with the
//! crate's own shell glob; exemptions win over patterns.

use std::path::Path;

use crate::rules::wildcard_match;

/// Safe templates that never escalate, even though they share the
/// `.env` shape (checked before every sensitive pattern).
const EXEMPTIONS: &[&str] = &[
    "*.env.example",
    "*.env.sample",
    "*.env.template",
    "*.env.dist",
];

/// Credential filename patterns (gh #215 §1).
const PATTERNS: &[&str] = &[
    "*.env",
    "*.env.*",
    ".envrc",
    "*.pem",
    "*.key",
    "*.pkcs12",
    "*.pfx",
    "*.p12",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    "id_dsa*",
    "credentials.json",
    "service-account*.json",
    ".npmrc",
    ".pypirc",
    ".netrc",
];

/// Whether reading this path exposes credentials: the file name
/// matches a sensitive pattern and no exemption.
pub fn is_sensitive(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if EXEMPTIONS
        .iter()
        .any(|pattern| wildcard_match(pattern, name))
    {
        return false;
    }
    PATTERNS.iter().any(|pattern| wildcard_match(pattern, name))
}
