//! The host-side skills merge (FR-CTX-2, ADR-0030): three sources, project
//! precedence, attribution, and the extension lifecycle (removing a package
//! removes its skills).
//!
//! Verifies: FR-CTX-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_core::{SkillSource, SkillsRoots};
use lca_tools::skills;

fn write_skill(dir: &std::path::Path, name: &str, header: &str, body: &str) {
    let skill = dir.join(name);
    std::fs::create_dir_all(&skill).expect("mkdir");
    std::fs::write(skill.join("SKILL.md"), format!("{header}\n---\n{body}")).expect("write");
}

fn roots(name: &str) -> (std::path::PathBuf, SkillsRoots) {
    let root = lca_testkit::scratch_path(&format!("skills-{name}"));
    let project = root.join("project");
    let user = root.join("config/skills");
    let extensions = root.join("data/extensions");
    std::fs::create_dir_all(&project).expect("mkdir");
    (
        root.clone(),
        SkillsRoots {
            project,
            user,
            extensions,
            disabled: Vec::new(),
            extra: Vec::new(),
        },
    )
}

// Cycle-7 driving defect: `ext disable <pack>` did not stop a data-only
// package's skill from injecting, because the host-side merge read the
// install tree and never consulted per-project enablement. The threat model
// promises "disable removes its skill pack".
//
// Verifies: ADR-0030, FR-PROV-9 (the threat-model resources row).
#[test]
fn a_disabled_extension_contributes_no_skills() {
    let (_root, mut roots) = roots("disabled");
    write_skill(
        &roots.extensions.join("pack/resources/skills"),
        "packaged",
        "name: packaged",
        "extension only",
    );
    assert!(
        skills::collect(&roots).iter().any(|s| s.name == "packaged"),
        "enabled by default: its skill is collected"
    );
    roots.disabled = vec!["pack".to_string()];
    assert!(
        !skills::collect(&roots).iter().any(|s| s.name == "packaged"),
        "a disabled package takes its skill pack with it"
    );
}

#[test]
fn the_merge_prefers_project_then_user_then_extension() {
    let (_root, roots) = roots("merge");
    // Same skill name in all three sources: project wins.
    write_skill(
        &roots.project.join(".lca/skills"),
        "style",
        "name: style",
        "from project",
    );
    write_skill(&roots.user, "style", "name: style", "from user");
    write_skill(
        &roots.extensions.join("pack/resources/skills"),
        "style",
        "name: style",
        "from extension",
    );
    // A user-only skill and an extension-only skill both survive.
    write_skill(&roots.user, "personal", "name: personal", "user only");
    write_skill(
        &roots.extensions.join("pack/resources/skills"),
        "packaged",
        "name: packaged",
        "extension only",
    );

    let collected = skills::collect(&roots);
    let by_name = |name: &str| {
        collected
            .iter()
            .find(|skill| skill.name == name)
            .unwrap_or_else(|| panic!("skill {name} collected"))
    };
    assert_eq!(by_name("style").source, SkillSource::Project);
    assert_eq!(by_name("style").body, "from project");
    assert_eq!(by_name("personal").source, SkillSource::User);
    assert_eq!(
        by_name("packaged").source,
        SkillSource::Extension("pack".to_string())
    );
    assert_eq!(collected.len(), 3, "the collision deduplicated");
}

#[test]
fn a_matched_skill_is_injected_with_attribution() {
    let (_root, roots) = roots("inject");
    write_skill(
        &roots.extensions.join("pack/resources/skills"),
        "commits",
        "name: commits\nmatch: commit, changelog",
        "Write one intent-bearing sentence.",
    );
    write_skill(
        &roots.user,
        "unmatched",
        "name: unmatched\nmatch: zzz",
        "never",
    );

    let collected = skills::collect(&roots);
    let messages = vec![lca_protocol::ChatMessage::text(
        lca_protocol::MessageRole::User,
        "please write a commit message",
    )];
    let out = skills::transform(messages, &collected, true);
    let injected = out.last().expect("an injection was appended");
    let text: String = injected
        .content
        .iter()
        .filter_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("[skill commits from extension `pack`]"),
        "attribution names the source: {text}"
    );
    assert!(text.contains("Write one intent-bearing sentence."));
    assert!(!text.contains("unmatched"), "an unmatched skill stays out");
}

#[test]
fn removing_the_extension_removes_its_skills() {
    let (_root, roots) = roots("lifecycle");
    let pack = roots.extensions.join("pack/resources/skills");
    write_skill(&pack, "packaged", "name: packaged", "packaged body");
    assert_eq!(skills::collect(&roots).len(), 1);

    std::fs::remove_dir_all(roots.extensions.join("pack")).expect("uninstall");
    assert!(
        skills::collect(&roots).is_empty(),
        "disabling/removing the extension removes its skills"
    );
}

#[test]
fn no_roots_collects_nothing() {
    assert!(skills::collect(&SkillsRoots::default()).is_empty());
}

// Verifies: gh #43 (the catalog): the prompt carries names, one-line
// descriptions, and source attribution - never bodies.
#[test]
fn the_catalog_advertises_without_bodies() {
    let (_root, roots) = roots("catalog");
    write_skill(
        &roots.user,
        "commits",
        "name: commits\ndescription: Write commit messages.",
        "Write one intent-bearing sentence. The full body stays out.",
    );
    let collected = skills::collect(&roots);
    assert_eq!(collected.len(), 1);
    assert_eq!(
        collected[0].description, "Write commit messages.",
        "the description parses"
    );
    let catalog = skills::catalog(&collected);
    assert!(catalog.contains("commits"), "the name is advertised");
    assert!(
        catalog.contains("Write commit messages."),
        "the description is advertised"
    );
    assert!(catalog.contains("user"), "the source is attributed");
    assert!(
        !catalog.contains("The full body stays out"),
        "bodies never ride the catalog"
    );
}

// Verifies: gh #43 (lazy bodies): a named skill loads its full text on
// demand through the same files the merge reads.
#[test]
fn a_named_skill_loads_its_full_body() {
    let (_root, roots) = roots("load");
    write_skill(
        &roots.user,
        "commits",
        "name: commits\ndescription: Write commit messages.",
        "Write one intent-bearing sentence.",
    );
    let body = skills::load_body(&roots, "commits").expect("the body loads");
    assert_eq!(body, "Write one intent-bearing sentence.");
    assert!(
        skills::load_body(&roots, "missing").is_none(),
        "an unknown name loads nothing"
    );
}

// Verifies: gh #43 (`disable-model-invocation`): a restricted skill
// loads for explicit invocation but never for the model's paths.
#[test]
fn a_restricted_skill_is_explicit_only() {
    let (_root, roots) = roots("restricted");
    write_skill(
        &roots.user,
        "deploy",
        "name: deploy\ndescription: Ship it.\ndisable-model-invocation: true",
        "The deploy runbook.",
    );
    let collected = skills::collect(&roots);
    assert_eq!(collected.len(), 1);
    assert!(!collected[0].model_invocable, "the flag parses");
    assert!(
        !skills::catalog(&collected).contains("deploy"),
        "restricted skills stay out of the model-visible catalog"
    );
    assert_eq!(
        skills::load_body(&roots, "deploy").as_deref(),
        Some("The deploy runbook."),
        "explicit invocation still loads it"
    );
    let messages = vec![lca_protocol::ChatMessage::text(
        lca_protocol::MessageRole::User,
        "please deploy it now",
    )];
    let out = skills::transform(messages, &collected, true);
    assert_eq!(
        out.len(),
        1,
        "even opt-in injection skips restricted skills"
    );
}

// Verifies: gh #43 (matched injection is opt-in, default OFF): the same
// match injects only when asked.
#[test]
fn matched_injection_only_runs_when_opted_in() {
    let (_root, roots) = roots("optin");
    write_skill(
        &roots.user,
        "commits",
        "name: commits\nmatch: commit\ndescription: Write commit messages.",
        "Write one intent-bearing sentence.",
    );
    let collected = skills::collect(&roots);
    let messages = || {
        vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "please write a commit message",
        )]
    };
    assert_eq!(
        skills::transform(messages(), &collected, false).len(),
        1,
        "default: no injection"
    );
    let injected = skills::transform(messages(), &collected, true);
    assert_eq!(injected.len(), 2, "opt-in: the match injects");
}
