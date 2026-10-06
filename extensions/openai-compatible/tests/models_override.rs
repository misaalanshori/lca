//! User model metadata overrides, `~/.lca/models.toml` (gh #64):
//! user entries beat curated tables and discovered values, `$VAR`
//! interpolates from the environment, `!command` never runs, and an
//! absent file changes nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::collections::BTreeMap;

use openai_compatible::{apply_model_overrides, parse_model_overrides};

fn curated() -> BTreeMap<String, u32> {
    BTreeMap::from([("m1".to_string(), 1000)])
}

fn models() -> Vec<lca_protocol::ModelInfo> {
    // Rows arrive with curated-or-discovered windows already folded in;
    // the override beats whatever the row carries.
    curated()
        .into_iter()
        .map(|(id, context_window)| lca_protocol::ModelInfo {
            name: id.clone(),
            id,
            context_window,
            max_tokens: 0,
            extras: BTreeMap::new(),
        })
        .collect()
}

const ONE_MODEL: &str = "[[model]]\nid = \"m1\"\ncontext_window = 2000\n";

// Verifies: gh #64 (user beats curated): the override's window replaces
// the curated table's.
#[test]
fn an_override_beats_the_curated_window() {
    let overrides = parse_model_overrides(ONE_MODEL);
    assert_eq!(overrides.len(), 1);
    let out = apply_model_overrides(models(), &overrides, "openai-compatible");
    assert_eq!(out[0].context_window, 2000);
}

// Verifies: gh #64 (user beats discovered): the override replaces a
// live-reported window too.
#[test]
fn an_override_beats_a_discovered_window() {
    let overrides = parse_model_overrides(ONE_MODEL);
    let mut live = models();
    live[0].context_window = 4000;
    let out = apply_model_overrides(live, &overrides, "openai-compatible");
    assert_eq!(out[0].context_window, 2000);
}

// Verifies: gh #64 (the full precedence chain): explicit env beats
// the user override fed as the endpoint value, which beats curated,
// which beats unknown. The caller feeds the override in the endpoint
// slot of `context_window_for`; this pins that composition.
#[test]
fn the_window_chain_runs_env_then_override_then_curated() {
    use openai_compatible::context_window_for;
    let curated = BTreeMap::from([("m1".to_string(), 1000)]);
    let overrides = parse_model_overrides(ONE_MODEL);
    let user = openai_compatible::override_for(&overrides, "openai-compatible", "m1")
        .and_then(|item| item.context_window);
    assert_eq!(user, Some(2000));
    assert_eq!(context_window_for("m1", 5000, user, &curated), 5000);
    assert_eq!(context_window_for("m1", 0, user, &curated), 2000);
    assert_eq!(context_window_for("m1", 0, None, &curated), 1000);
    assert_eq!(context_window_for("mx", 0, None, &curated), 0);
}

// Verifies: gh #64 (unknown ids are ignored, pi's rule): an entry for a
// model nobody offers changes nothing and breaks nothing.
#[test]
fn unknown_override_ids_are_ignored() {
    let overrides = parse_model_overrides("[[model]]\nid = \"ghost\"\ncontext_window = 9\n");
    let out = apply_model_overrides(models(), &overrides, "openai-compatible");
    assert_eq!(out[0].context_window, 1000);
}

// Verifies: gh #64 (absent file is current behavior): empty text parses
// to no overrides, and models pass through untouched.
#[test]
fn an_absent_file_changes_nothing() {
    assert!(parse_model_overrides("").is_empty());
    assert!(parse_model_overrides("not toml [[[").is_empty());
    let out = apply_model_overrides(models(), &[], "openai-compatible");
    assert_eq!(out, models());
}
// Verifies: gh #64 (`$VAR` interpolation): values read the environment,
// which is already trusted input. An unset var leaves the field absent,
// never zero.
#[test]
fn dollar_var_values_interpolate_from_the_environment() {
    unsafe { std::env::remove_var("LCA_TEST_WINDOW_64") };
    let toml_text = "[[model]]\nid = \"m1\"\ncontext_window = \"$LCA_TEST_WINDOW_64\"\n";
    let overrides = parse_model_overrides(toml_text);
    assert_eq!(overrides.len(), 1);
    assert_eq!(
        overrides[0].context_window, None,
        "unset vars leave the field absent, never zero"
    );
    unsafe { std::env::set_var("LCA_TEST_WINDOW_64", "3000") };
    let toml_text = "[[model]]\nid = \"m1\"\ncontext_window = \"${LCA_TEST_WINDOW_64}\"\n";
    let overrides = parse_model_overrides(toml_text);
    assert_eq!(overrides[0].context_window, Some(3000));
    unsafe { std::env::remove_var("LCA_TEST_WINDOW_64") };
}

// Verifies: gh #64 (no `!command`, documented divergence): a leading
// `!` never executes - the value is not a number and stays out.
#[test]
fn bang_command_values_never_execute() {
    let probe = std::env::temp_dir().join("lca-must-not-exist-47");
    let _ = std::fs::remove_file(&probe);
    // Forward slashes: a Windows temp path carries backslashes, which
    // are not valid TOML escapes and would fail parsing before the
    // refusal is even reached. The value stays hostile either way.
    let hostile = probe.display().to_string().replace('\\', "/");
    let toml_text = format!("[[model]]\nid = \"m1\"\ncontext_window = \"!touch {hostile}\"\n");
    let overrides = parse_model_overrides(&toml_text);
    assert_eq!(overrides.len(), 1);
    assert_eq!(
        overrides[0].context_window, None,
        "a command is not a number and never runs"
    );
    assert!(!probe.exists(), "parsing runs nothing");
}

// Verifies: gh #64 (limits ride along): resize profiles and prompt
// cache lifetimes land in the model's extras for their consumers.
#[test]
fn resize_and_cache_lifetimes_ride_the_extras() {
    let text = "[[model]]\nid = \"m1\"\ninput = [\"text\", \"image\"]\n\
        [model.input_limits.images.resize]\nmax_width = 1568\nmax_height = 1568\n\
        max_bytes = 524288\njpeg_quality = 75\n\
        [model.prompt_cache]\nshort = 300\nlong = 3600\n";
    let overrides = parse_model_overrides(text);
    assert_eq!(overrides.len(), 1);
    let out = apply_model_overrides(models(), &overrides, "openai-compatible");
    assert_eq!(
        out[0].extras.get("image.vision").map(String::as_str),
        Some("true"),
        "the input list derives vision"
    );
    assert_eq!(
        out[0].extras.get("image.resize").map(String::as_str),
        Some("1568x1568:524288"),
        "the resize profile rides the #39 shape"
    );
    assert_eq!(
        out[0].extras.get("prompt_cache").map(String::as_str),
        Some("short=300,long=3600"),
        "lifetimes are carried for the warming epic"
    );
}

// Verifies: gh #64 (effective values surface in rows): a `list_models`
// row for an overridden model carries the override window.
#[test]
fn list_rows_carry_the_override_window() {
    use lca_ext_abi::ExtensionDispatch;
    let root = lca_testkit::scratch_path("lca-models-override-rows");
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = lca_permissions::ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    struct Always;
    impl lca_permissions::PermissionPrompt for Always {
        fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
            lca_permissions::Decision::Always
        }
        fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
            false
        }
    }
    let mut engine = lca_tools::Capabilities::new(
        "openai-compatible",
        openai_compatible::manifest_grants(),
        roots,
        std::sync::Arc::new(std::sync::Mutex::new(Always)),
        std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        )),
        root.join("project"),
        None,
    );
    engine.set_resources(openai_compatible::resources());
    let cap = std::sync::Arc::new(engine);
    cap.credentials_set("models", "mimo-v2.6-flash")
        .expect("models");
    let settings = openai_compatible::Settings {
        model_overrides: "[[model]]\nid = \"mimo-v2.6-flash\"\ncontext_window = 999001\n"
            .to_string(),
        ..Default::default()
    };
    let handle = openai_compatible::OpenAiCompat::with_settings(cap, settings);
    let row = handle
        .provider_models(&[])
        .expect("models")
        .into_iter()
        .find(|model| model.id == "mimo-v2.6-flash")
        .expect("the curated model is listed");
    assert_eq!(row.context_window, 999001);
}
