//! Gh #351 finding 3: a **bundled** extension built for the component target
//! (`wasm32-wasip2`) runs through `lca-ext-host` and answers like its
//! native-linked twin.
//!
//! # What was actually missing, measured
//!
//! The issue's premise was "no bundled extension compiles to `wasm32-wasip2`
//! and runs through `lca-ext-host`". That is only **half** true, and the half
//! that is true is the interesting one:
//!
//! * `tests/provider/shipped.rs` **already** loads the shipped
//!   `openai-compatible` component through `ExtHost::load` and diffs one export
//!   (`provider_models`) against the native handle
//!   (`context_windows_are_identical_across_modes`). So "a bundled extension
//!   runs through the host" was covered, for that one export, from the
//!   **committed** fixture.
//! * What nothing did was connect the two artifacts: `scripts/wasm-check.sh`
//!   builds every component for the target and compares the **committed** file
//!   against a fresh build by *interface* only - and its own comment names the
//!   hole ("a guest change that alters behaviour without altering the interface
//!   passes here"). Nothing ran the fresh build, and nothing compared ais two
//!   artifacts' answers.
//! * And no bundled extension had **more than one export** diffed across the
//!   delivery modes. The world's surface (`identity_usage`, `login_options`,
//!   `identity_logout`) is where a mapping slip would hide, because the native
//!   handle answers those directly while the guest crosses `lca:host/*` imports.
//!
//! This file is that suite. It uses `openai-compatible` because it is the
//! bundled extension that already has both halves in the tree (it is a
//! dev-dependency of this crate and ships a committed component), so the diff
//! costs no dependency and no new build step - the brief's "no `Cargo.toml`
//! change" holds.
//!
//! # The skip, and why it is loud
//!
//! The fresh artifact is produced by `scripts/wasm-check.sh` (or
//! `cargo build --release --target wasm32-wasip2 -p openai-compatible`). A
//! test binary does not build it: a nested `cargo` inside a test is a
//! dependency on the build system from inside the suite, which is the kind of
//! thing that turns a green run into a flake (the gh #429 family). So when the
//! artifact is absent the rows **skip with a named reason** - which artifact,
//! which command produces it, and what therefore went unverified - and
//! `LCA_REQUIRE_WASM=1` turns that skip into a failure, exactly as
//! `LCA_REQUIRE_TMUX` does for the terminal suite (`docs/testing-plan.md`
//! section 14's house rule, applied here). The flag is set in the one CI job
//! where the artifact is guaranteed to exist (`extension components build`,
//! immediately after `wasm-check.sh`), so a cold checkout there fails; in the
//! `engines` test-split group the rows skip and their log says which artifact
//! and which command would have verified them.
//!
//! Verifies: gh #351 finding 3, NFR-25's substance for a *bundled* component,
//! `docs/testing-plan.md` section 13.
//!
//! Gh #351 finding 6 (the split): this lives in its own test target rather than
//! inside `provider.rs`, which is at gate 11's ceiling already - a new row there
//! would be the second reason to split it, and this subject (the built artifact,
//! not the provider world) is a different thing to look up.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch;
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};

/// The bundled extension under test. It is the one that satisfies all three
/// conditions at once: bundled (the CLI's `bundled-openai-compat` feature),
/// shipped as a committed component, and a dev-dependency that exposes its
/// native handle.
const FIXTURE: &[u8] =
    include_bytes!("../../../extensions/openai-compatible/fixtures/component.wasm");
const MANIFEST: &str = include_str!("../../../extensions/openai-compatible/extension.toml");
const NAME: &str = "openai-compatible";

struct Always;
impl lca_permissions::PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

/// One scratch root, shared by BOTH halves: the host serves the extension's
/// resource bag from `<state_dir>/extensions/<name>/resources`, and the native
/// twin reads the crate's embedded bag - so the diff is over identical bytes.
///
/// This is load-bearing and was measured: with the bag not laid out on disk, a
/// probe of this same fixture had the guest answer `context_window = 0` for
/// every model (the bag read is `NotFound`) while the native twin answered the
/// curated windows. That is a **fixture** divergence wearing a product
/// divergence's clothes, and the row would have reported it as one.
struct Fixture {
    root: PathBuf,
    prompt: Arc<Mutex<Always>>,
    store: Arc<Mutex<lca_permissions::GrantStore>>,
    roots: lca_permissions::ScopeRoots,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let root = lca_testkit::scratch_path(&format!("lca-bundled-wasm-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["project", "private", "config", "data", "tmp"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        // The bag at the install layout the host derives (ADR-0030/0032).
        let bag = root.join("data/extensions").join(NAME).join("resources");
        std::fs::create_dir_all(&bag).expect("bag dir");
        for (name, text) in [
            (
                "provider-presets.toml",
                include_str!(
                    "../../../extensions/openai-compatible/resources/provider-presets.toml"
                ),
            ),
            (
                "context-windows.toml",
                include_str!(
                    "../../../extensions/openai-compatible/resources/context-windows.toml"
                ),
            ),
            (
                "image-limits.toml",
                include_str!("../../../extensions/openai-compatible/resources/image-limits.toml"),
            ),
        ] {
            std::fs::write(bag.join(name), text).expect("write bag");
        }
        let roots = lca_permissions::ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        };
        Fixture {
            store: Arc::new(Mutex::new(
                lca_permissions::GrantStore::open(&root.join("grants.json")).expect("store"),
            )),
            prompt: Arc::new(Mutex::new(Always)),
            roots,
            root,
        }
    }

    fn env(&self) -> Arc<HostEnvironment> {
        Arc::new(HostEnvironment {
            roots: self.roots.clone(),
            prompt: self.prompt.clone(),
            grant_store: self.store.clone(),
            project: self.root.join("project"),
            proposals: None,
            dialogs: lca_permissions::SharedDialogs::default(),
        })
    }

    /// The WASM half: the bundled component through the real host.
    fn wasm(&self, bytes: &[u8]) -> Arc<dyn ExtensionDispatch> {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            self.env(),
        );
        Arc::new(
            host.load(bytes, MANIFEST)
                .expect("the bundled component links"),
        )
    }

    /// The native half: the same crate's dispatch trait, in process.
    fn native(&self) -> Arc<dyn ExtensionDispatch> {
        let mut cap = lca_tools::Capabilities::new(
            NAME,
            openai_compatible::manifest_grants(),
            self.roots.clone(),
            self.prompt.clone(),
            self.store.clone(),
            self.root.join("project"),
            None,
        );
        cap.set_resources(openai_compatible::resources());
        Arc::new(openai_compatible::OpenAiCompat::with_settings(
            Arc::new(cap),
            openai_compatible::Settings {
                model: String::new(),
                context_window: 0,
                ..Default::default()
            },
        ))
    }
}

/// The settings pair a `/model` listing carries (ADR-0035): one curated id and
/// one the catalog cannot know, so the answer exercises both the found and the
/// unknown path. Fixed, so the two halves are asked the same question.
fn asked() -> Vec<(String, String)> {
    vec![(
        "models".to_string(),
        "mimo-v2.6-flash,no-such-model".to_string(),
    )]
}

/// Where `wasm-check.sh` leaves the fresh build: `target/wasm32-wasip2/release/`.
/// `CARGO_TARGET_DIR` is honoured because a worker or CI may move it.
fn fresh_artifact() -> Option<PathBuf> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let path = target
        .join("wasm32-wasip2/release")
        .join(format!("{}.wasm", NAME.replace('-', "_")));
    path.is_file().then_some(path)
}

/// The house rule for a harness that is not present: **skip by name**, never
/// pass silently, and let an env flag turn it into a failure.
///
/// `LCA_REQUIRE_TMUX` is the precedent (`crates/lca-cli/tests/common/mod.rs`): a
/// row whose harness is absent prints what went unverified and returns, and CI
/// sets the flag on the leg that owns the harness. nextest has no runtime skip -
/// `#[ignore]` is a compile-time decision and this artifact's presence is not -
/// so the printed line **is** the skip, and what keeps it honest is that CI's
/// `extension components build` job runs this suite with `LCA_REQUIRE_WASM=1`
/// right after it has built the artifact (so a cold checkout there is a failure,
/// not a green row that exercised nothing).
fn artifact_or_skip(row: &str) -> Option<PathBuf> {
    let artifact = fresh_artifact();
    if artifact.is_none() {
        let reason = format!(
            "{row}: the fresh `{NAME}` component build is absent, so the bundled-WASM \
             path went UNVERIFIED here. `scripts/wasm-check.sh` (or `cargo build --release \
             --target wasm32-wasip2 -p {NAME}`) produces \
             `target/wasm32-wasip2/release/{}.wasm`, which this row reads rather than \
             building (a nested cargo call inside a test is a build-system dependency \
             this suite deliberately does not have).",
            NAME.replace('-', "_")
        );
        if std::env::var("LCA_REQUIRE_WASM").is_ok() {
            panic!("LCA_REQUIRE_WASM is set and {reason}");
        }
        eprintln!("skip: {reason}");
    }
    artifact
}

// Verifies: gh #351 finding 3 - **the headline.** A bundled extension compiled
// for `wasm32-wasip2` and instantiated by `lca-ext-host` answers its exports
// exactly as the native-linked handle of the same crate does.
//
// The exports are the whole read-only surface of the `provider` world, not one
// of them: a mapping slip in the ABI crossing (`ProviderOutcome` variants, the
// login-option shape, the `NotSupported` usage answer) shows up as a difference
// on one of these and nowhere else. The failure message names the side and both
// answers, because this class is a **divergence**, never a crash.
#[tokio::test]
async fn a_bundled_extension_built_for_the_component_target_answers_like_its_native_twin() {
    let Some(artifact) = artifact_or_skip(
        "a_bundled_extension_built_for_the_component_target_answers_like_its_native_twin",
    ) else {
        return;
    };
    let bytes = std::fs::read(&artifact)
        .unwrap_or_else(|err| panic!("read the fresh build {}: {err}", artifact.display()));

    let fixture = Fixture::new("fresh-twice");
    let wasm = fixture.wasm(&bytes);
    let native = fixture.native();

    // Identity first: a component that linked but is a different extension
    // would fail every comparison below for the wrong reason.
    assert_eq!(
        (wasm.name(), wasm.delivery().label()),
        (NAME, "sandboxed"),
        "the fresh build is the named extension, and it arrived through the sandbox"
    );
    assert_eq!(
        native.delivery().label(),
        "in-process (unsandboxed)",
        "and the twin is the in-process one (FR-EXT-7: the labels differ by design)"
    );

    let passed = asked();
    let wasm_models = wasm.provider_models(&passed).expect("wasm model listing");
    let native_models = native
        .provider_models(&passed)
        .expect("native model listing");
    assert_eq!(
        wasm_models.len(),
        native_models.len(),
        "the two delivery modes offer a different number of models.\n  \
         sandboxed build ({}): {wasm_models:?}\n  in-process twin ({}): {native_models:?}",
        artifact.display(),
        NAME
    );
    for (index, (sandboxed, in_process)) in wasm_models.iter().zip(native_models.iter()).enumerate()
    {
        assert_eq!(
            sandboxed,
            in_process,
            "model {index} diverges between the delivery modes.\n  \
             sandboxed build ({}): {sandboxed:?}\n  in-process twin: {in_process:?}",
            artifact.display()
        );
    }
    // The window is read from the resource bag, which the two halves reach
    // differently (the host serves a Dir, the twin reads the crate's embedded
    // copy). A build that stopped serving the bag would answer `0` here while
    // the twin still answered the curated value - the fixture divergence this
    // suite's own setup exists to keep out of the comparison.
    let window = |models: &[lca_protocol::ModelInfo]| {
        models
            .iter()
            .find(|model| model.id == "mimo-v2.6-flash")
            .unwrap_or_else(|| panic!("the curated id is in the list: {models:?}"))
            .context_window
    };
    assert_eq!(
        window(&wasm_models),
        1_048_576,
        "the guest reads the curated window from its bag, not `0`: {wasm_models:?}"
    );

    // The rest of the world's read-only surface. `identity_usage` is the
    // `NotSupported` answer no OpenAI-compatible server can promise (ADR-0012's
    // optional export), `login_options` rides the presets bag, and
    // `identity_logout` is the no-op - three shapes, three crossings.
    let wasm_usage = wasm.identity_usage().await.expect("wasm usage");
    let native_usage = native.identity_usage().await.expect("native usage");
    assert_eq!(
        format!("{wasm_usage:?}"),
        format!("{native_usage:?}"),
        "the usage answer diverges.\n  sandboxed build ({}): {wasm_usage:?}\n  in-process twin: {native_usage:?}",
        artifact.display()
    );

    let wasm_options = wasm.login_options().await.expect("wasm login options");
    let native_options = native.login_options().await.expect("native login options");
    assert_eq!(
        wasm_options.len(),
        native_options.len(),
        "the two modes offer a different number of login options.\n  \
         sandboxed build ({}): {wasm_options:?}\n  in-process twin: {native_options:?}",
        artifact.display()
    );
    for (index, (sandboxed, in_process)) in
        wasm_options.iter().zip(native_options.iter()).enumerate()
    {
        assert_eq!(
            sandboxed,
            in_process,
            "login option {index} diverges between the delivery modes.\n  \
             sandboxed build ({}): {sandboxed:?}\n  in-process twin: {in_process:?}",
            artifact.display()
        );
    }

    let wasm_logout = wasm.identity_logout().await.expect("wasm logout");
    let native_logout = native.identity_logout().await.expect("native logout");
    assert_eq!(
        format!("{wasm_logout:?}"),
        format!("{native_logout:?}"),
        "the logout answer diverges.\n  sandboxed build ({}): {wasm_logout:?}\n  in-process twin: {native_logout:?}",
        artifact.display()
    );
}

// Verifies: gh #351 finding 3 - **the half the interface gate cannot see.**
//
// `scripts/wasm-check.sh` compares the committed component against a fresh
// build by *interface*, and says in its own comment what that leaves open: a
// guest change that alters behaviour without altering the interface passes the
// gate. So the committed file - the one a user actually installs - and the
// build of today's source are asked the same question here, and the answers
// must agree. A behaviour drift between them is what this row exists for, and
// it is a **release** defect rather than a test one: the fixture users get
// would answer differently from the tree they read.
#[tokio::test]
async fn the_committed_component_and_a_fresh_build_answer_identically() {
    let Some(artifact) =
        artifact_or_skip("the_committed_component_and_a_fresh_build_answer_identically")
    else {
        return;
    };
    let fresh = std::fs::read(&artifact)
        .unwrap_or_else(|err| panic!("read the fresh build {}: {err}", artifact.display()));

    let committed_fixture = Fixture::new("committed");
    let committed = committed_fixture.wasm(FIXTURE);
    let fresh_fixture = Fixture::new("fresh");
    let rebuilt = fresh_fixture.wasm(&fresh);

    let passed = asked();
    let committed_models = committed
        .provider_models(&passed)
        .expect("committed model listing");
    let rebuilt_models = rebuilt
        .provider_models(&passed)
        .expect("fresh model listing");
    assert_eq!(
        committed_models,
        rebuilt_models,
        "the committed component and a build of today's source disagree, so the \
         shipped file is stale in *behaviour* (the interface gate is green by \
         construction and cannot see this).\n  committed \
         extensions/{NAME}/fixtures/component.wasm: {committed_models:?}\n  \
         fresh build ({}): {rebuilt_models:?}",
        artifact.display()
    );

    // And the shapes that are not a model list, because a stale guest can agree
    // on one export and differ on the next.
    for (label, a, b) in [
        (
            "usage",
            format!("{:?}", committed.identity_usage().await.expect("usage")),
            format!("{:?}", rebuilt.identity_usage().await.expect("usage")),
        ),
        (
            "logout",
            format!("{:?}", committed.identity_logout().await.expect("logout")),
            format!("{:?}", rebuilt.identity_logout().await.expect("logout")),
        ),
        (
            "login options",
            format!(
                "{:?}",
                committed.login_options().await.expect("login options")
            ),
            format!(
                "{:?}",
                rebuilt.login_options().await.expect("login options")
            ),
        ),
    ] {
        assert_eq!(
            a,
            b,
            "the committed component and a fresh build disagree on `{label}`: the \
             shipped file is stale in behaviour.\n  committed: {a}\n  fresh build \
             ({}): {b}",
            artifact.display()
        );
    }
}

// Verifies: gh #351 finding 3 - **the skip is visible, and the flag is strict.**
//
// The suite's absence path is load-bearing: a row that reported success while
// the artifact was missing would be exactly the "passes by exercising nothing"
// shape this issue is about, and the brief's rule is "skip-with-a-reason never
// reports success". This row cannot exercise the real skip (the artifact is
// present here or it is not), so it asserts the *properties* of the code path
// from the source: the reason is printed, it names the artifact and the command
// that builds it, and `LCA_REQUIRE_WASM` panics rather than skipping.
#[test]
fn the_absence_path_skips_by_name_and_the_require_flag_makes_it_fail() {
    let source = include_str!("bundled_wasm.rs");
    // The reason must name the three things a reader needs: what is missing,
    // what produces it, and that the path went unverified.
    for needle in [
        "went UNVERIFIED",
        "scripts/wasm-check.sh",
        "wasm32-wasip2/release",
        "LCA_REQUIRE_WASM",
        "eprintln!(\"skip:",
    ] {
        assert!(
            source.contains(needle),
            "the absence path must say {needle:?} - a silent skip is the defect \
             this row guards"
        );
    }
    // The flag has to *fail*, not merely annotate.
    let flag = source
        .find("if std::env::var(\"LCA_REQUIRE_WASM\").is_ok()")
        .expect("the require flag is consulted");
    assert!(
        source[flag..].contains("panic!"),
        "LCA_REQUIRE_WASM turns the skip into a named failure (as LCA_REQUIRE_TMUX does)"
    );
    // And no row may return quietly on a missing artifact: both artifact rows
    // consult the helper before their first assertion, so a row that lost its
    // call would be one that runs on an absent artifact.
    //
    // Counted over the source *above this row* - the scan itself writes the
    // name, so including this point would count its own text (that trap bit the
    // first version once already).
    let rows_only = source
        .split("fn the_absence_path_skips_by_name")
        .next()
        .unwrap_or_default();
    let call_sites = rows_only.matches("artifact_or_skip(").count()
        - rows_only.matches("fn artifact_or_skip(").count();
    assert_eq!(
        call_sites, 2,
        "exactly two rows consult the skip path: a third would be a row whose \
         skip nobody reviewed, and one would leave a row that never skips"
    );
}
