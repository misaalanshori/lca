//! gh #427: every committed fixture under `extensions/conformance/fixtures/`
//! must link against the current ABI 0.6 host and the manifest that names it.
//!
//! The committed fixture exists so tests run offline without a wasm toolchain
//! (docs/phase-log.md, Phase 2) — but a committed binary has no way to notice
//! that the source moved on. On 2026-10-08 commit `2ccf187` added the eight
//! opt-in hook worlds to `wit/` and the conformance source, without rebuilding
//! the fixtures; the commit before it (`d24192b`, 2026-10-07) was the last to
//! touch `fixtures/component.wasm`. The result: installing that stale file
//! failed with
//!
//! ```text
//! skipping `conformance`: cannot link extension: no exported instance named `lca:ext/hook-message-end@0.6.0`
//! ```
//!
//! — a link error that looks like a host bug to anyone following the
//! authoring guide.
//!
//! This row is the guard against the same drift reappearing. It walks the
//! fixtures directory at **run time** (not `include_bytes!`, so a newly added
//! stale file cannot hide) and loads each one through `ExtHost::load`. That is
//! precisely the journey that failed in #427, because `load` builds a typed
//! pre for **every world the manifest declares** (`hooks_pre` in
//! `lib.rs`): a fixture missing any declared world's exports fails here with
//! `LoadError::Link`, before any call. A fixture that loads therefore
//! satisfies every declared world — the acceptance condition, by construction.
//!
//! Verifies: NFR-24 (gh #427), gh #427 acceptance 1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch;
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};

const MANIFEST: &str = include_str!("../../../extensions/conformance/extension.toml");

struct AllowPrompt;
impl lca_permissions::PermissionPrompt for AllowPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

/// The scratch root a probe's `tag` gets. `env` and a probe that has to seed
/// files under it (credentials, a resource bag, a workspace skill) must agree on
/// the path, and `scratch_path` is deterministic per name.
fn root_of(tag: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(&format!("lca-fixture-{tag}"))
}

fn env(tag: &str) -> Arc<HostEnvironment> {
    let root = root_of(tag);
    env_with_net(&root, false)
}

/// [`env`], optionally with a session grant for loopback `net` - what the
/// provider probe needs to reach its mock endpoint (and nothing else does).
fn env_with_net(root: &std::path::Path, loopback: bool) -> Arc<HostEnvironment> {
    let _ = std::fs::remove_dir_all(root);
    for dir in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = lca_permissions::ScopeRoots {
        workspace: root.join("workspace"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    let mut store = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("store");
    if loopback {
        store
            .attach_session_net_pattern(&roots.workspace, "127.0.0.1")
            .expect("loopback grant");
    }
    Arc::new(HostEnvironment {
        roots,
        prompt: Arc::new(Mutex::new(AllowPrompt)),
        grant_store: Arc::new(Mutex::new(store)),
        project: root.join("workspace"),
        proposals: None,
        dialogs: lca_permissions::SharedDialogs::default(),
    })
}

fn limits() -> ExtensionLimits {
    ExtensionLimits {
        memory_bytes: 64 * 1024 * 1024,
        fuel_per_call: 10_000_000,
        log_limit_bytes: 4096,
    }
}

/// The committed fixture directory (runtime, so the listing tracks the tree).
fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extensions/conformance/fixtures")
}

// Verifies: gh #427 - every committed conformance fixture links on the current
// host and satisfies every world the manifest declares. The pre-fix state
// fails here with the issue's exact error (`no exported instance named
// 'lca:ext/hook-message-end@0.6.0'`).
#[test]
fn every_committed_conformance_fixture_links_against_the_declared_worlds() {
    let dir = fixtures_dir();
    let mut fixtures: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("the fixtures directory must exist ({dir:?}): {err}"))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "wasm"))
        .collect();
    fixtures.sort();
    assert!(
        !fixtures.is_empty(),
        "no fixtures left in {dir:?}: if the offline-fixture value is being retired on purpose, this row and the phase-log note go with it"
    );

    for path in fixtures {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let bytes =
            std::fs::read(&path).unwrap_or_else(|err| panic!("read the fixture {name}: {err}"));
        let mut host = ExtHost::new(limits(), env(&name.replace('.', "-")));
        let loaded = host.load(&bytes, MANIFEST).unwrap_or_else(|err| {
            panic!(
                "committed fixture {name} no longer links against the current ABI 0.6 host \
                 and the manifest that declares its worlds (gh #427): {err}"
            )
        });
        assert_eq!(
            loaded.name(),
            "conformance",
            "the fixture links as the manifest names it: {name}"
        );
        assert!(loaded.is_enabled(), "and it comes up enabled: {name}");
    }
}

// ---------------------------------------------------------------------------
// gh #616: one behavioural probe per committed component
// ---------------------------------------------------------------------------
//
// `scripts/wasm-check.sh` compares a committed component to a fresh build **by
// interface**, and it must: wasm builds in this tree are not byte-reproducible
// (gh #614 measured one source, two rebuilds: `8910ec49…` then `358afde5…`), so a
// byte comparison reddens on codegen noise. An interface comparison therefore
// cannot see *behaviour* - gh #591's named hole - and twice a committed fixture
// went stale in a way that mattered: `antigravity` sat across four commits, and
// gh #236's measured half-budget fuel margin silently became 2.9x false.
//
// So: after the load, **one canonical call per component** - offline, no clock,
// no randomness, no environment - and two pins, the sha256 of the rendered
// result and the fuel the call consumed (`WasmExtension::last_call_fuel`, gh
// #614). The numbers below were measured on this 4-core x86_64 box on
// 2026-11-24; the band is ±25%, which is wide enough to absorb an engine patch's
// fuel drift and narrow enough that a 2.7x cost change (the #241 shape) cannot
// hide. `PROBES` is asserted to cover every committed component with a world, so
// a new fixture cannot be added without one.

/// One committed component's canonical call and its two pins.
struct Probe {
    /// The directory under `extensions/`.
    extension: &'static str,
    /// The committed file under its `fixtures/`.
    fixture: &'static str,
    /// The world the call belongs to (the manifest must declare it).
    world: &'static str,
    /// The call, in words, for a failure message.
    call: &'static str,
    /// The settings pairs the host passes a provider's `list-models` (ADR-0035);
    /// empty for every other world.
    settings: &'static [(&'static str, &'static str)],
    /// sha256 of the rendered result, measured 2026-11-24.
    digest: &'static str,
    /// Fuel the call consumed, measured 2026-11-24 on a 4-core x86_64 box.
    fuel: u64,
}

/// Every committed component, one probe each.
///
/// **Measured 2026-11-24 on this 4-core x86_64 box**, against the fixtures as of
/// `101adba4`. The digests are exactly reproducible across runs; fuel is not
/// quite - two runs of the antigravity probe read 3,413,755 and 3,413,662
/// (0.003%), so the band is ±25%: wide enough for that jitter and an engine
/// patch, narrow enough that the 2.7x cost move gh #614 measured cannot hide. A
/// deliberate change to a call re-measures and rewrites its two numbers.
///
/// The providers with no committed fixture (`anthropic`, `github-copilot`,
/// `kimi-coding`, `llama`, `meta`, `openrouter`) ship inside the binary, as do
/// the first-party tools and transforms without a `fixtures/` directory, so the
/// walk has nothing to probe for them; the coverage row asserts exactly that
/// split rather than trusting it.
const PROBES: &[Probe] = &[
    Probe {
        extension: "antigravity",
        fixture: "component.wasm",
        world: "provider",
        call: "list-models over a real-shaped loopback catalog (60 runtime ids, 12 families)",
        settings: &[],
        digest: "44720d06e22abc62575f0004e959d6a3544cef1669f4d734c8a4b686b0bcaf48",
        fuel: 3413755,
    },
    Probe {
        extension: "codex",
        fixture: "component.wasm",
        world: "provider",
        call: "list-models with no credentials (the static table)",
        settings: &[],
        digest: "048d897fdd2ff9cb820023c7a36340fd1c4c569074275439cd87b66a7871dfc2",
        fuel: 2262,
    },
    Probe {
        extension: "compaction-default",
        fixture: "component.wasm",
        world: "compaction",
        call: "compact over two fixed records (the completion capability is refused offline, so this is the degraded path)",
        settings: &[],
        digest: "bcaec3b8b494be99bf7fbc1f18c14ee5d72cc90c2a4e06d410bb53b6e9538f8b",
        fuel: 83662,
    },
    Probe {
        extension: "conformance",
        fixture: "tool-world.wasm",
        world: "tool",
        call: "execute mode=ok",
        settings: &[],
        digest: "f67953d3cb0f9dc2c45435d23b2c9359bbc51dd36ef41bcfb0f2b7013f555ebb",
        fuel: 6129,
    },
    Probe {
        extension: "grok",
        fixture: "component.wasm",
        world: "provider",
        call: "list-models with no credentials (the static table)",
        settings: &[],
        digest: "d208db9ed76d49e350710d00e600211508008b0ca8b65f3e3e90cc7bf3286b07",
        fuel: 2258,
    },
    Probe {
        extension: "openai-compatible",
        fixture: "component.wasm",
        world: "provider",
        call: "list-models over a fixed discovered list, resolving windows from the resource bag",
        settings: &[("models", "gpt-5.2,o3-mini")],
        digest: "73f7174e0bf8ecba0e7cbe04e002db5f12d49c6abcffd8dc5b996f333e807092",
        fuel: 1966638,
    },
    Probe {
        extension: "skills",
        fixture: "component.wasm",
        world: "context-transform",
        call: "transform a fixed conversation that matches one workspace skill",
        settings: &[],
        digest: "2cb4ba5bbb8c79b2e2b73f0da9061573b2d8896885db584880d2dbd92f7f7cd0",
        fuel: 44275,
    },
    Probe {
        extension: "tool-legacy",
        fixture: "component.wasm",
        world: "tool",
        call: "execute {text: \"probe\"}",
        settings: &[],
        digest: "85fec61d987288508d1bf826c7034b2a8ac1ba7e1fe83908b4bc78b7e6b45c71",
        fuel: 3184,
    },
];

/// The digest of a rendered result.
fn digest_of(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// A rendered `ModelInfo` list: one line per model, in listing order - the
/// `ninety_six_extra_models_render_the_golden_rows` row's shape (gh #614).
fn render_models(models: &[lca_protocol::ModelInfo]) -> String {
    models
        .iter()
        .map(|model| {
            let extras: Vec<String> = model
                .extras
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            format!(
                "{}|{}|{}|{}|{}",
                model.id,
                model.name,
                model.context_window,
                model.max_tokens,
                extras.join(",")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The loopback Google the provider probe reads: `fetchAvailableModels` answers
/// a real-shaped catalog (each family advertises its thinking levels as runtime
/// ids, so grouping is exercised), everything else `{}`.
fn catalog_mock(catalog: String) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = vec![0u8; 65536];
            let mut head = Vec::new();
            while let Ok(n) = std::io::Read::read(&mut stream, &mut buf) {
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&buf[..n]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&head).into_owned();
            let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
            let payload = if path.contains("fetchAvailableModels") {
                catalog.clone()
            } else {
                "{}".to_string()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
        }
    });
    format!("http://{addr}")
}

/// A real-shaped `fetchAvailableModels` catalog: twelve families, five runtime
/// ids each (the thinking-level suffixes and one `-tiered`), so the guest's
/// grouping has real work to do and a pre-#241 component (no grouping) answers
/// sixty rows instead of twelve.
fn real_shaped_catalog() -> String {
    let families = [
        ("gemini-3.8-flash", "Gemini 3.8 Flash"),
        ("gemini-3.7-flash", "Gemini 3.7 Flash"),
        ("gemini-3.6-flash", "Gemini 3.6 Flash"),
        ("gemini-3.5-flash", "Gemini 3.5 Flash"),
        ("gemini-3.1-pro", "Gemini 3.1 Pro"),
        ("gemini-2.5-pro", "Gemini 2.5 Pro"),
        ("gemini-2.5-flash", "Gemini 2.5 Flash"),
        ("gemini-2.0-flash", "Gemini 2.0 Flash"),
        ("claude-opus-4-6", "Claude Opus 4.6"),
        ("claude-sonnet-4-6", "Claude Sonnet 4.6"),
        ("claude-haiku-4-5", "Claude Haiku 4.5"),
        ("gpt-oss-120b", "GPT OSS 120B"),
    ];
    let mut models = serde_json::Map::new();
    for (base, display) in families {
        for level in ["-low", "-medium", "-high", "-tiered", ""] {
            let id = format!("{base}{level}");
            models.insert(
                id.clone(),
                serde_json::json!({
                    "isInternal": false,
                    "displayName": format!("{display} (Antigravity)"),
                    "label": display,
                    "modelName": id,
                    "supportsThinking": true,
                    "supportsImages": true,
                    "inputTokenLimit": 1_000_000,
                    "outputTokenLimit": 65_536,
                    "quotaInfo": {"remainingFraction": 0.87, "resetTime": "2026-10-17T00:00:00Z"},
                    "experiments": [format!("exp-{id}-a"), format!("exp-{id}-b")],
                }),
            );
        }
    }
    serde_json::json!({ "models": models, "defaultAgentModelId": "gemini-3.8-flash" }).to_string()
}

/// The two records the compaction probe summarizes.
fn compaction_records() -> Vec<lca_protocol::Record> {
    let ts = 1_700_000_000_000u64;
    vec![
        lca_protocol::Record::User {
            v: lca_protocol::FORMAT_VERSION,
            ts,
            id: "01".to_string(),
            parent: None,
            content: "first request".to_string(),
            attachments: Vec::new(),
            queue: None,
        },
        lca_protocol::Record::Assistant {
            v: lca_protocol::FORMAT_VERSION,
            ts: ts + 1,
            id: "02".to_string(),
            parent: None,
            content: vec![lca_protocol::ContentBlock::Text {
                text: "reply".to_string(),
            }],
            reasoning: None,
            reasoning_signature: None,
            provider_thinking_level: None,
            usage: None,
            provider: Some("fake".to_string()),
            model: Some("faux-1".to_string()),
        },
    ]
}

/// The conversation the skills probe transforms: the last user message matches
/// the skill seeded in the workspace.
fn skills_conversation() -> Vec<lca_protocol::ChatMessage> {
    use lca_protocol::{ChatMessage, MessageRole};
    vec![
        ChatMessage::text(MessageRole::System, "sys"),
        ChatMessage::text(MessageRole::User, "read the file"),
        ChatMessage::text(MessageRole::Assistant, "reply"),
        ChatMessage::text(MessageRole::User, "please commit this"),
    ]
}

/// Seed what a probe's call reads besides the component: credentials and the
/// loopback endpoint for the provider, the resource bag for a package that ships
/// one, and a workspace skill for `skills`.
fn seed(probe: &Probe, root: &std::path::Path, api_base: Option<&str>) {
    if let Some(base) = api_base {
        let creds = root.join("data/credentials/antigravity.json");
        std::fs::create_dir_all(creds.parent().expect("dir")).expect("mkdir");
        std::fs::write(
            &creds,
            serde_json::json!({
                "access": "probe-access",
                "expires": "9999999999",
                "project": "probe-project",
                "api_base": base,
            })
            .to_string(),
        )
        .expect("credentials");
    }
    // The host serves an installed package's bag from the state dir (ADR-0030),
    // so a probe reads what a user's install would.
    let bag = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../extensions")
        .join(probe.extension)
        .join("resources");
    if bag.is_dir() {
        let dest = root
            .join("data/extensions")
            .join(probe.extension)
            .join("resources");
        std::fs::create_dir_all(&dest).expect("bag dir");
        for entry in std::fs::read_dir(&bag).expect("bag listing").flatten() {
            if entry.path().is_file() {
                std::fs::copy(entry.path(), dest.join(entry.file_name())).expect("bag copy");
            }
        }
    }
    if probe.extension == "skills" {
        let skill = root.join("workspace/.lca/skills/commit/SKILL.md");
        std::fs::create_dir_all(skill.parent().expect("dir")).expect("mkdir");
        std::fs::write(
            &skill,
            "---\nname: commit\nmatch: commit, changelog\n---\nWrite commit messages in the imperative.\n",
        )
        .expect("skill");
    }
}

/// Run one probe: load the committed component with its own manifest, make the
/// canonical call, and hand back the rendered result. `None` skips with a reason
/// when the component cannot load in this environment (never a failure: an
/// environment limitation is not drift).
fn run_probe(probe: &Probe) -> Option<(String, u64)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../extensions")
        .join(probe.extension);
    let manifest = std::fs::read_to_string(dir.join("extension.toml"))
        .unwrap_or_else(|err| panic!("{}: unreadable manifest: {err}", probe.extension));
    let bytes = std::fs::read(dir.join("fixtures").join(probe.fixture))
        .unwrap_or_else(|err| panic!("{}: unreadable fixture: {err}", probe.extension));

    let base = (probe.extension == "antigravity").then(|| catalog_mock(real_shaped_catalog()));
    let root = root_of(&format!("probe-{}", probe.extension));
    let env = env_with_net(&root, base.is_some());
    seed(probe, &root, base.as_deref());

    let mut host = ExtHost::new(limits(), env);
    let handle = match host.load(&bytes, &manifest) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!(
                "skip: {} ({}) cannot load in this environment: {err}",
                probe.extension, probe.fixture
            );
            return None;
        }
    };
    let world = lca_ext_abi::parse_world(probe.world)
        .unwrap_or_else(|| panic!("`{}` is not a world name", probe.world));
    assert!(
        handle.worlds().contains(&world),
        "{} must declare the world its probe calls (`{}`)",
        probe.extension,
        probe.world
    );

    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    };
    let rendered = match probe.world {
        "provider" => {
            let settings: Vec<(String, String)> = probe
                .settings
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            render_models(&handle.provider_models(&settings).expect(probe.call))
        }
        "compaction" => {
            let records = compaction_records();
            runtime()
                .block_on(handle.compact(&records))
                .unwrap_or_else(|err| panic!("{}: {}: {err}", probe.extension, probe.call))
        }
        "context-transform" => {
            let transformed = runtime()
                .block_on(handle.transform_messages(skills_conversation()))
                .unwrap_or_else(|err| panic!("{}: {}: {err}", probe.extension, probe.call))
                .unwrap_or_else(|rejected| panic!("{}: rejected: {rejected}", probe.extension));
            transformed
                .iter()
                .map(|message| {
                    let text: Vec<&str> = message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    format!("{:?}|{}", message.role, text.join(" "))
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "tool" => {
            let arguments = if probe.extension == "conformance" {
                r#"{"mode":"ok"}"#.to_string()
            } else {
                r#"{"text":"probe"}"#.to_string()
            };
            let call = lca_protocol::ToolCall {
                call_id: "probe".to_string(),
                name: handle.name().to_string(),
                arguments,
                parent_call_id: None,
            };
            let result = runtime()
                .block_on(handle.execute_tool(&call))
                .unwrap_or_else(|err| panic!("{}: {}: {err}", probe.extension, probe.call));
            format!(
                "{:?}|{}|truncated={}",
                result.status, result.content, result.truncated
            )
        }
        other => panic!("{}: no render for world `{other}`", probe.extension),
    };
    let fuel = handle
        .last_call_fuel()
        .unwrap_or_else(|| panic!("{}: the call recorded no fuel", probe.extension));
    Some((rendered, fuel))
}

/// Every committed component answers its canonical call the way it did when the
/// pins were measured. A stale fixture reddens here on the digest, the fuel, or
/// both, and the message names the component and both numbers (#616).
#[test]
fn every_committed_component_answers_its_canonical_call_the_same_way() {
    for probe in PROBES {
        let Some((rendered, fuel)) = run_probe(probe) else {
            continue;
        };
        let digest = digest_of(&rendered);
        let band = probe.fuel / 4;
        assert!(
            digest == probe.digest && fuel.abs_diff(probe.fuel) <= band,
            "{}/{}: `{}` no longer answers the way it did when the pins were measured \
             (#616).\n  digest {} (pinned {}{})\n  fuel   {fuel} (pinned {} ±25%, band {band})\n\
             A stale fixture or a cost regression shows here; re-measure and update the \
             pin if the change is deliberate.\nresult:\n{rendered}",
            probe.extension,
            probe.fixture,
            probe.call,
            digest,
            probe.digest,
            if digest == probe.digest {
                ""
            } else {
                " - CHANGED"
            },
            probe.fuel,
        );
    }
}

/// The pins cover the walk: every committed component with a fixture has exactly
/// one probe, every probe has its fixture, and the components that declare
/// worlds but commit no fixture are named rather than forgotten. This is the
/// guard against the guard rotting - a new fixture with no probe fails here.
#[test]
fn the_probe_table_covers_every_committed_component() {
    let extensions = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extensions");
    let mut committed: Vec<(String, String)> = Vec::new();
    let mut no_fixture_with_worlds: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&extensions)
        .expect("extensions dir")
        .flatten()
    {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !dir.join("extension.toml").is_file() {
            continue;
        }
        let text = std::fs::read_to_string(dir.join("extension.toml")).expect("manifest");
        let declares_worlds = text
            .lines()
            .any(|line| line.trim_start().starts_with("worlds"));
        let mut fixtures: Vec<String> = std::fs::read_dir(dir.join("fixtures"))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "wasm"))
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        fixtures.sort();
        if fixtures.is_empty() {
            if declares_worlds {
                no_fixture_with_worlds.push(name.to_string());
            }
            continue;
        }
        for fixture in fixtures {
            committed.push((name.to_string(), fixture));
        }
    }
    committed.sort();

    let mut probed: Vec<(String, String)> = PROBES
        .iter()
        .map(|probe| (probe.extension.to_string(), probe.fixture.to_string()))
        .collect();
    probed.sort();
    assert_eq!(
        committed, probed,
        "every committed component must have exactly one probe (#616); a new fixture \
         needs a `PROBES` entry with a measured digest and fuel, and a component whose \
         fixture was removed needs its entry deleted"
    );
    for probe in PROBES {
        assert!(
            !probe.digest.is_empty() && probe.fuel > 0,
            "{}: the pin is measured, never left blank",
            probe.extension
        );
    }

    // The documented split: a component that declares worlds and commits no
    // fixture ships inside the binary, so this walk has nothing to probe. It is
    // listed in `PROBES`'s doc comment; the assertion keeps the two in step.
    let expected_without_fixture = [
        "anthropic",
        "formatter",
        "github-copilot",
        "handoff",
        "hash-edit",
        "hooks-example",
        "kimi-coding",
        "llama",
        "meta",
        "openrouter",
        "project-memory",
        "question",
        "rules-compat",
        "stream-guard",
        "ui-example",
        "ultrathink",
        "webfetch",
    ];
    no_fixture_with_worlds.sort();
    assert_eq!(
        no_fixture_with_worlds,
        expected_without_fixture
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>(),
        "a component gained or lost a fixture without this row being revisited"
    );
}
