//! The host-side skills merge (FR-CTX-2, ADR-0030): three sources, project
//! precedence, attribution, and the extension lifecycle (removing a package
//! removes its skills).
//!
//! Verifies: FR-CTX-2.

use lca_core::{SkillSource, SkillsRoots, skills};

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
        },
    )
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
    let out = skills::transform(messages, &collected);
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
