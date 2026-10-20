//! gh #418 Phase F (gh #73): `lca mcp add|remove|list|login|logout`.
//!
//! These rows drive the **real binary** (`CARGO_BIN_EXE_lca`) because the
//! surface is the CLI: a unit row on the manager would pass while the parser,
//! the trust gate, or the exit code was wrong. The fixture is the spawn-probe
//! server, which answers `initialize`/`tools/list` the way a real one does.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

use std::path::{Path, PathBuf};

use common::sandbox;

fn fixture() -> String {
    // Forward slashes everywhere: a Windows backslash path would read as a
    // JSON escape inside `mcp.json`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lca-mcp/fixtures/spawn-probe-mcp-server.py")
        .to_string_lossy()
        .replace('\\', "/")
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Run one `lca mcp ...` verb in the sandbox's project directory.
fn mcp(sandbox: &common::Sandbox, args: &[&str]) -> std::process::Output {
    let mut full = vec!["mcp"];
    full.extend_from_slice(args);
    sandbox.run(None, &full)
}

/// The user-level `mcp.json`, parsed.
fn user_config(home: &Path) -> serde_json::Value {
    let path = home.join(".lca/mcp.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    serde_json::from_str(&text).expect("json")
}

// Verifies: gh #418 Phase F (gh #73) - `add` writes the user file, the entry
// parses back through the same loader a session uses, and the command exits 0.
#[test]
fn add_writes_a_stdio_entry_to_the_user_file() {
    let sandbox = sandbox("mcp-add-stdio");
    let output = mcp(&sandbox, &["add", "echo", "--", "python3", &fixture()]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "add succeeds: {}",
        common::stderr(&output)
    );
    let config = user_config(&sandbox.home);
    let entry = &config["mcpServers"]["echo"];
    assert_eq!(entry["command"], "python3", "{config}");
    assert_eq!(entry["args"][0], fixture(), "{config}");
    // The reply names the file it wrote, so a script can find it.
    let text = common::stdout(&output);
    assert!(
        text.contains("Added user MCP server \"echo\""),
        "the confirmation names the scope and the server: {text}"
    );
}

// Verifies: gh #418 Phase F - `add --local` writes the project file, and an
// untrusted project gets the FR-PERM-9 note (the file is inert until trust).
#[test]
fn add_local_writes_the_project_file_and_notes_untrusted_projects() {
    let sandbox = sandbox("mcp-add-local");
    let output = mcp(&sandbox, &["add", "local", "--local", "--", "true"]);
    assert_eq!(output.status.code(), Some(0), "{}", common::stderr(&output));
    let path = sandbox.project().join(".lca/mcp.json");
    assert!(path.is_file(), "the project file exists");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(config["mcpServers"]["local"]["command"], "true", "{config}");
    let text = common::stdout(&output);
    assert!(
        text.contains("not trusted") && text.contains("ignored until"),
        "an untrusted project's file is inert, and the note says so: {text}"
    );
}

// Verifies: gh #418 Phase F - `add --url` writes an HTTP entry with the
// headers, the bearer-token env indirection, and the OAuth settings.
#[test]
fn add_writes_an_http_entry_with_headers_and_oauth() {
    let sandbox = sandbox("mcp-add-http");
    let output = mcp(
        &sandbox,
        &[
            "add",
            "remote",
            "--url",
            "https://example.invalid/mcp",
            "--header",
            "X-Tenant=acme",
            "--bearer-token-env-var",
            "SOME_TOKEN",
            "--oauth-client-id",
            "cid",
            "--oauth-client-name",
            "my-lca",
            "--exposure",
            "deferred",
            "--description",
            "Docs server.",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", common::stderr(&output));
    let config = user_config(&sandbox.home);
    let entry = &config["mcpServers"]["remote"];
    assert_eq!(entry["url"], "https://example.invalid/mcp", "{config}");
    assert_eq!(entry["headers"]["X-Tenant"], "acme", "{config}");
    assert_eq!(
        entry["headers"]["Authorization"], "Bearer ${SOME_TOKEN}",
        "the bearer env var becomes an indirection, not a value: {config}"
    );
    assert_eq!(entry["oauth"]["clientId"], "cid", "{config}");
    assert_eq!(entry["oauth"]["clientName"], "my-lca", "{config}");
    assert_eq!(entry["exposure"], "deferred", "{config}");
    assert_eq!(entry["description"], "Docs server.", "{config}");
}

// Verifies: gh #418 Phase F - an option that belongs to the other transport is
// a usage error naming the transport, not a silently ignored flag (pi's rule).
#[test]
fn add_refuses_an_option_that_belongs_to_the_other_transport() {
    let sandbox = sandbox("mcp-add-misplaced");
    let output = mcp(
        &sandbox,
        &["add", "x", "--url", "https://e.invalid/mcp", "--env", "K=V"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", common::stdout(&output));
    let text = common::stderr(&output);
    assert!(
        text.contains("--env") && text.contains("stdio"),
        "the error names the flag and where it belongs: {text}"
    );
    // Nothing was written: a refused command mutates nothing.
    assert!(
        !sandbox.home.join(".lca/mcp.json").is_file(),
        "a refused add writes no file"
    );
}

// Verifies: gh #418 Phase F - `add` needs exactly one transport.
#[test]
fn add_refuses_both_or_neither_transport() {
    let sandbox = sandbox("mcp-add-transport");
    for args in [
        vec!["add", "x"],
        vec!["add", "x", "--url", "https://e.invalid/mcp", "--", "true"],
    ] {
        let output = mcp(&sandbox, &args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "refused: {args:?}: {}",
            common::stderr(&output)
        );
        let text = common::stderr(&output);
        assert!(
            text.contains("--url") && text.contains("-- <command>"),
            "the usage names both shapes: {text}"
        );
    }
}

// Verifies: gh #418 Phase F - `remove` exits 0 and the entry is gone; asking
// the wrong scope exits 1 with the flag that would reach the right one (pi's
// hint).
#[test]
fn remove_reports_the_other_scope_and_exits_one() {
    let sandbox = sandbox("mcp-remove-scope");
    let added = mcp(&sandbox, &["add", "echo", "--", "true"]);
    assert_eq!(added.status.code(), Some(0), "{}", common::stderr(&added));

    let wrong = mcp(&sandbox, &["remove", "echo", "--local"]);
    assert_eq!(
        wrong.status.code(),
        Some(1),
        "the wrong scope fails: {}",
        common::stdout(&wrong)
    );
    let text = common::stderr(&wrong);
    assert!(
        text.contains("omit --local"),
        "the hint names the flag that would work: {text}"
    );

    let right = mcp(&sandbox, &["remove", "echo"]);
    assert_eq!(right.status.code(), Some(0), "{}", common::stderr(&right));
    let config = user_config(&sandbox.home);
    assert!(
        config["mcpServers"].get("echo").is_none(),
        "the entry is gone: {config}"
    );
}

// Verifies: gh #418 Phase F - `list` connects and reports a tool, exiting 0.
#[test]
fn list_reports_a_connected_server_and_exits_zero() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let sandbox = sandbox("mcp-list-ok");
    let added = mcp(&sandbox, &["add", "echo", "--", "python3", &fixture()]);
    assert_eq!(added.status.code(), Some(0), "{}", common::stderr(&added));
    let output = mcp(&sandbox, &["list"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "a connected server exits 0: {}",
        common::stderr(&output)
    );
    let text = common::stdout(&output);
    assert!(
        text.contains("echo: connected, 1 tool"),
        "the row reports the state and count: {text}"
    );
    // The qualified name is `mcp__<server>__<tool>`: the server is named
    // `echo` here and the fixture's tool is `probe`.
    assert!(
        text.contains("mcp__echo__probe"),
        "the tool name is listed: {text}"
    );
}

// Verifies: gh #418 Phase F - `list` exits 1 when an enabled server cannot
// connect, and the failure names the cause.
#[test]
fn list_exits_one_when_a_server_cannot_connect() {
    let sandbox = sandbox("mcp-list-fail");
    let added = mcp(
        &sandbox,
        &["add", "broken", "--", "definitely-not-a-real-program-f"],
    );
    assert_eq!(added.status.code(), Some(0), "{}", common::stderr(&added));
    let output = mcp(&sandbox, &["list"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a failing server exits 1: {}",
        common::stdout(&output)
    );
    let text = common::stdout(&output);
    assert!(text.contains("broken: failed"), "{text}");
}

// Verifies: gh #418 Phase F - `list --json` is machine-readable and carries
// the same facts, including the error, so a script needs no screen-scraping.
#[test]
fn list_json_carries_state_tools_and_errors() {
    let sandbox = sandbox("mcp-list-json");
    mcp(
        &sandbox,
        &["add", "broken", "--", "definitely-not-a-real-program-f"],
    );
    let output = mcp(&sandbox, &["list", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "the JSON report keeps the exit code: {}",
        common::stdout(&output)
    );
    let report: serde_json::Value =
        serde_json::from_str(&common::stdout(&output)).expect("the output is JSON");
    assert_eq!(report["servers"][0]["name"], "broken", "{report}");
    assert_eq!(report["servers"][0]["state"], "failed", "{report}");
    assert!(
        report["servers"][0]["error"].is_string(),
        "the failure carries its cause: {report}"
    );
    assert_eq!(report["servers"][0]["scope"], "user", "{report}");
}

// Verifies: gh #418 Phase F, design decision 4 - the client secret never
// appears in the command's output, and it is not written to `mcp.json`
// either: it goes to the extension's credential namespace.
#[test]
fn add_never_echoes_or_files_the_oauth_client_secret() {
    let sandbox = sandbox("mcp-add-secret");
    let secret = "SUPERSECRET-do-not-print-9f3a";
    let output = mcp(
        &sandbox,
        &[
            "add",
            "remote",
            "--url",
            "https://example.invalid/mcp",
            "--oauth-client-id",
            "cid",
            "--oauth-client-secret",
            secret,
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", common::stderr(&output));
    let combined = common::stdout(&output) + &common::stderr(&output);
    assert!(
        !combined.contains(secret),
        "the secret never echoes back: {combined}"
    );
    let config = std::fs::read_to_string(sandbox.home.join(".lca/mcp.json")).expect("mcp.json");
    assert!(
        !config.contains(secret),
        "the secret is not in mcp.json (a config file gets copied and committed): {config}"
    );
    // It did go somewhere: the extension's credential namespace.
    let credentials =
        std::fs::read_to_string(sandbox.home.join(".lca/credentials/mcp.json")).unwrap_or_default();
    assert!(
        credentials.contains(secret),
        "the secret lands in the credential namespace: {credentials}"
    );
}

// Verifies: gh #418 Phase F - `logout` clears the stored credentials and exits
// 0; a server that never signed in says so and still exits 0 (pi's rule).
#[test]
fn logout_clears_credentials_and_exits_zero() {
    let sandbox = sandbox("mcp-logout");
    let added = mcp(
        &sandbox,
        &["add", "remote", "--url", "https://example.invalid/mcp"],
    );
    assert_eq!(added.status.code(), Some(0), "{}", common::stderr(&added));

    let never = mcp(&sandbox, &["logout", "remote"]);
    assert_eq!(
        never.status.code(),
        Some(0),
        "no stored credentials is not a failure: {}",
        common::stderr(&never)
    );
    assert!(
        common::stdout(&never).contains("No stored credentials"),
        "{}",
        common::stdout(&never)
    );
}

// Verifies: gh #418 Phase F - the verbs work when the session's MCP is
// disabled. Management is how a user *fixes* a disabled setup, so `--no-mcp`
// and the enablement record must not gate it (design decision 2).
#[test]
fn the_verbs_ignore_the_enablement_record() {
    let sandbox = sandbox("mcp-disabled-verbs");
    sandbox.approve_loopback_net(serde_json::json!({
        "extensions": { "mcp": false },
    }));
    // `true` is not a portable executable (macOS has no such binary, and the
    // failure there was a silently empty report). The row is about enablement,
    // not spawning, so it uses the fixture and asserts only that the
    // *configured* server is listed.
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let added = mcp(&sandbox, &["add", "echo", "--", "python3", &fixture()]);
    assert_eq!(
        added.status.code(),
        Some(0),
        "add works with MCP disabled: {}",
        common::stderr(&added)
    );
    let listed = mcp(&sandbox, &["list"]);
    let text = common::stdout(&listed) + &common::stderr(&listed);
    assert!(
        text.contains("echo"),
        "list shows the configured server with MCP disabled: {text}"
    );
}

// Verifies: gh #418 Phase F - `logout` on a stdio server is a clear error, not
// a crash or a silent success.
#[test]
fn logout_refuses_a_stdio_server_with_a_named_reason() {
    let sandbox = sandbox("mcp-logout-stdio");
    let added = mcp(&sandbox, &["add", "echo", "--", "true"]);
    assert_eq!(added.status.code(), Some(0), "{}", common::stderr(&added));
    let output = mcp(&sandbox, &["logout", "echo"]);
    assert_eq!(output.status.code(), Some(1), "{}", common::stdout(&output));
    assert!(
        common::stderr(&output).contains("stdio"),
        "the reason names the transport: {}",
        common::stderr(&output)
    );
}

// Verifies: gh #418 Phase F - the `login` journey against a two-host mock IdP
// (the Phase D harness shape): an MCP server whose 401 names a
// `resource_metadata` document on an *identity-provider* host signs in, and the
// command reports the tool count afterwards.
// Unix-only: the row stands in for a desktop browser with an `xdg-open`
// shim and a loopback callback. Windows launches through a different path
// (no `xdg-open`), so the row would wait out its own timeout there rather
// than test anything.
#[cfg(unix)]
#[test]
fn login_signs_in_against_an_identity_provider_on_another_host() {
    // The MCP host 401s with a challenge pointing at the IdP, and the IdP
    // serves RFC 9728 + RFC 8414 + registration + token on its own host.
    let idp = common::oauth_idp_host();
    let metadata_url = format!("{}/.well-known/oauth-protected-resource", idp.base);
    // The 401's challenge is the hint discovery follows (the RFC 9728
    // pointer to a host that is *not* the MCP server's).
    let challenge = format!(r#"Bearer resource_metadata="{metadata_url}", scope="resources:read""#);
    let _ = common::CHALLENGE.set(challenge);
    let mcp = common::scripted_host("127.0.0.1", move |_path| {
        (
            401,
            r#"{"jsonrpc":"2.0","id":0,"error":{"code":-32001,"message":"unauthorized"}}"#
                .to_string(),
        )
    });
    let sandbox = sandbox("mcp-login-two-host");
    common::write_oauth_mcp_config(&sandbox.home, &format!("{}/mcp", mcp.base), &idp.base);
    sandbox.approve_loopback_net(serde_json::json!({}));

    // A browser that records the authorize URL and answers the callback the
    // way a desktop would: the CLI publishes the URL and waits on the
    // loopback, so the test has to be the browser.
    let browser = sandbox.root.join("bin");
    std::fs::create_dir_all(&browser).expect("mkdir");
    // The launcher name is platform-specific: `xdg-open` on Linux, `open` on
    // macOS (`lca_tools::url_launchers`). Both are written, so this row is a
    // real browser on either.
    let shim = "#!/bin/bash\npython3 - \"$1\" <<'PY'\nimport sys, urllib.parse, urllib.request\nurl = sys.argv[1]\nq = urllib.parse.parse_qs(urllib.parse.urlparse(url).query)\nredirect = q[\"redirect_uri\"][0]\nstate = q[\"state\"][0]\nurllib.request.urlopen(redirect + \"?\" + urllib.parse.urlencode({\"code\": \"cli-code\", \"state\": state}), timeout=20).read()\nPY\n";
    for name in ["xdg-open", "open"] {
        let script = browser.join(name);
        std::fs::write(&script, shim).expect("write browser shim");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
    }

    let path_env = format!(
        "{}:{}",
        browser.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = sandbox.run_env(
        None,
        &["mcp", "login", "remote", "--timeout", "30"],
        &[("PATH", path_env.as_str())],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "the cross-host sign-in completes: stdout={} stderr={}",
        common::stdout(&output),
        common::stderr(&output)
    );
    // The IdP host was actually reached, in order.
    let seen = idp.paths.lock().expect("paths").clone();
    for needle in [
        "oauth-protected-resource",
        "oauth-authorization-server",
        "/register",
        "/token",
    ] {
        assert!(
            seen.iter().any(|path| path.contains(needle)),
            "{needle} ran on the IdP host: {seen:?}"
        );
    }
    // The token is stored, so a later session picks it up.
    let credentials =
        std::fs::read_to_string(sandbox.home.join(".lca/credentials/mcp.json")).unwrap_or_default();
    assert!(
        credentials.contains("cli-access"),
        "the access token is stored: {credentials}"
    );
}

// Verifies: gh #418 Phase F - a file-only or credential-only verb never
// spawns the server it is editing. The first version of `remove` opened a manager (which connects),
// so deleting a server *started* it: the spawn-probe marker appeared, and on a
// piped stdout the orphaned child could take the process down with SIGPIPE.
#[test]
fn removing_a_server_never_spawns_it() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let sandbox = sandbox("mcp-remove-no-spawn");
    let marker = sandbox.root.join("spawns.txt");
    let marker_env = marker.to_str().expect("utf8").to_string();
    // The fixture writes the marker as its first statement, so its presence
    // proves a spawn happened.
    let added = sandbox.run_env(
        None,
        &["mcp", "add", "echo", "--", "python3", &fixture()],
        &[("LCA_SPAWN_MARKER", marker_env.as_str())],
    );
    assert_eq!(added.status.code(), Some(0), "{}", common::stderr(&added));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default(),
        "",
        "`add` writes config and starts nothing"
    );
    let removed = sandbox.run_env(
        None,
        &["mcp", "remove", "echo"],
        &[("LCA_SPAWN_MARKER", marker_env.as_str())],
    );
    assert_eq!(
        removed.status.code(),
        Some(0),
        "{}",
        common::stderr(&removed)
    );
    // A background connect would land the marker *after* `remove` exits, so
    // give a stray worker a moment before asserting: a guard that can only
    // fail on a slow machine is not a guard.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default(),
        "",
        "`remove` never spawns the server it is deleting"
    );

    // `logout` is the same class: it clears a credential, so it needs the
    // store and nothing else (gh #418 Phase F).
    let added = sandbox.run_env(
        None,
        &["mcp", "add", "echo", "--", "python3", &fixture()],
        &[("LCA_SPAWN_MARKER", marker_env.as_str())],
    );
    assert_eq!(added.status.code(), Some(0), "{}", common::stderr(&added));
    let logged_out = sandbox.run_env(
        None,
        &["mcp", "logout", "echo"],
        &[("LCA_SPAWN_MARKER", marker_env.as_str())],
    );
    // A stdio server has no credentials, so the verb refuses (exit 1) — but
    // it must refuse *without* spawning it.
    assert_eq!(
        logged_out.status.code(),
        Some(1),
        "{}",
        common::stdout(&logged_out)
    );
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default(),
        "",
        "`logout` never spawns the server whose credentials it clears"
    );
}

// Verifies: gh #418 Phase F - an empty credential file means "nothing stored
// yet", not corruption. macOS CI produced exactly this during a `login` (a
// store created but not yet written), and the old reader turned it into a
// permanent "credential store corrupt" error that made every later sign-in
// fail until the file was deleted by hand.
#[test]
fn an_empty_credential_file_reads_as_no_credentials() {
    let sandbox = sandbox("mcp-empty-credentials");
    let credentials = sandbox.home.join(".lca/credentials");
    std::fs::create_dir_all(&credentials).expect("mkdir");
    std::fs::write(credentials.join("mcp.json"), "").expect("write empty store");
    let added = mcp(
        &sandbox,
        &["add", "remote", "--url", "https://example.invalid/mcp"],
    );
    assert_eq!(added.status.code(), Some(0), "{}", common::stderr(&added));
    // `logout` reads the store before clearing it: with the old reader this
    // failed with "credential store corrupt".
    let output = mcp(&sandbox, &["logout", "remote"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "an empty store is nothing stored, not corruption: {}",
        common::stderr(&output)
    );
    assert!(
        common::stdout(&output).contains("No stored credentials"),
        "{}",
        common::stdout(&output)
    );
}
