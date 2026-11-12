//! Refusal rows: what config load rejects, and what it must name while
//! rejecting.
//!
//! Split out of `tests/merge.rs` under gate 11's 1,200-line ceiling (gh #545).
//! An unknown key, a hostile catalog id, a bad typed value, an untrusted
//! project catalog: each is a *refusal*, and the user-facing claim is which
//! source to look at - so these read together.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_config::Config;

use super::support::{scratch, write};

// Verifies: FR-CFG-1 (typed keys reject bad values at load, with the source named)
#[test]
fn bad_values_report_the_source_that_set_them() {
    let dir = scratch("bad");
    write(&dir.join("user.toml"), "tool.timeout_seconds = \"soon\"\n");
    let err = Config::load(&lca_config::LoadInput {
        flags: Default::default(),
        env: Default::default(),
        project_file: None,
        trusted: true,
        user_file: Some(dir.join("user.toml")),
        headless: false,
    })
    .expect_err("must fail");
    let message = err.to_string();
    assert!(message.contains("tool.timeout_seconds"), "{message}");
    assert!(message.contains("user file"), "{message}");
}

// Verifies: gh #273 (SANCTIONED CHANGE above) - the proxy decision the
// previous row's tail documented: the proxy is environment-only by design,
// so `net.proxy_url` is not a key. Under the new contract that is an error
// naming the key, which tells the user where to go (`HTTP_PROXY`) instead
// of accepting a setting nothing reads.
#[test]
fn an_unknown_key_is_refused_rather_than_ignored() {
    let dir = scratch("proxy-key-refused");
    write(
        &dir.join("user.toml"),
        "net.proxy_url = \"http://proxy:8080\"\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("`net.proxy_url` is not a key: the proxy is environment-only");
    let text = err.to_string();
    assert!(
        text.contains("net.proxy_url"),
        "the refusal names the key: {text}"
    );
}

// Verifies: gh #348 - a hostile catalog id is refused at CONFIG LOAD, in
// both the user file and (decisively) a trusted project file. The id names
// a cache file, so refusing here is what stops a repository's config from
// ever reaching a refresh that would write outside the cache directory.
//
// The trusted-project half is the attack shape: FR-PERM-9 keeps an
// *untrusted* project file out of the merge entirely, so the hole only
// opens once a user trusts the repo (which is the normal state for a repo
// they cloned and agreed to work in).
#[test]
fn a_hostile_catalog_id_is_refused_at_load() {
    let dir = scratch("catalog-id-hostile");
    for id in [
        "../../cron.d/malicious",
        "/absolute",
        "sub/dir",
        "back\\slash",
        "",
        "dots.in.name",
    ] {
        let body = format!(
            "[[extensions.catalogs]]\nid = {id:?}\nname = \"Evil\"\nurl = \"https://attacker.example/c.toml\"\n"
        );
        write(&dir.join("user.toml"), &body);
        let err = Config::load(&lca_config::LoadInput {
            user_file: Some(dir.join("user.toml")),
            ..Default::default()
        })
        .expect_err("a hostile id is refused in the user file");
        assert!(
            err.to_string().contains("catalog id"),
            "the refusal names the key for `{id}`: {err}"
        );

        write(&dir.join("project.toml"), &body);
        let err = Config::load(&lca_config::LoadInput {
            project_file: Some(dir.join("project.toml")),
            trusted: true,
            ..Default::default()
        })
        .expect_err("a hostile id is refused in a trusted project file");
        assert!(
            err.to_string().contains("catalog id"),
            "the refusal names the key for `{id}`: {err}"
        );
    }
}

// Verifies: gh #348 - an untrusted project file never contributes a
// catalog at all (FR-PERM-9), so the refusal above is defence in depth
// rather than the only gate. Pinned because it is the answer to "is the
// untrusted project config the real hole?" - it is not; the trusted one is.
#[test]
fn an_untrusted_project_catalog_never_reaches_the_merge() {
    let dir = scratch("catalog-id-untrusted");
    write(
        &dir.join("project.toml"),
        "[[extensions.catalogs]]\nid = \"acme\"\nname = \"Acme\"\nurl = \"https://example.invalid/c.toml\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        project_file: Some(dir.join("project.toml")),
        trusted: false,
        ..Default::default()
    })
    .expect("load");
    assert!(
        config.extensions_catalogs().is_empty(),
        "an untrusted project file contributes no catalog"
    );
}

// Verifies: #227 - custom extension catalogs parse from the user file
// as id/name/url tables; a missing field or a non-table entry is
// refused at load. Empty by default.
#[test]
fn custom_extension_catalogs_parse_and_refuse_junk() {
    assert!(
        Config::defaults().extensions_catalogs().is_empty(),
        "no custom catalogs by default"
    );
    let dir = scratch("extension-catalogs");
    write(
        &dir.join("user.toml"),
        "[[extensions.catalogs]]\nid = \"acme\"\nname = \"Acme Corp\"\nurl = \"https://example.invalid/catalog.toml\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(
        config.extensions_catalogs(),
        &[lca_config::CustomCatalog {
            id: "acme".to_string(),
            name: "Acme Corp".to_string(),
            url: "https://example.invalid/catalog.toml".to_string(),
        }],
        "the catalog reads back with all three fields"
    );
    write(
        &dir.join("user.toml"),
        "[[extensions.catalogs]]\nid = \"acme\"\nname = \"Acme Corp\"\n",
    );
    assert!(
        Config::load(&lca_config::LoadInput {
            user_file: Some(dir.join("user.toml")),
            ..Default::default()
        })
        .is_err(),
        "a catalog without a url is refused"
    );
    write(
        &dir.join("user.toml"),
        "[extensions]\ncatalogs = \"nope\"\n",
    );
    assert!(
        Config::load(&lca_config::LoadInput {
            user_file: Some(dir.join("user.toml")),
            ..Default::default()
        })
        .is_err(),
        "a non-list catalogs value is refused"
    );
}

// Verifies: FR-CFG-7 (`trust.default_project` defaults to `ask` and
// rides the user file, never the project file)
#[test]
fn default_project_trust_defaults_to_ask_and_refuses_project_files() {
    let dir = scratch("trust-default");
    let config = Config::load(&lca_config::LoadInput {
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.trust_default_project(), "ask");

    write(
        &dir.join("user.toml"),
        "[trust]\ndefault_project = \"always\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.trust_default_project(), "always");

    // A project file that grants its own trust defeats FR-PERM-9: loud.
    write(
        &dir.join("project.toml"),
        "[trust]\ndefault_project = \"always\"\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        project_file: Some(dir.join("project.toml")),
        trusted: true,
        ..Default::default()
    })
    .expect_err("project-file trust fallback is refused");
    assert!(err.to_string().contains("user file"), "{err}");
}
