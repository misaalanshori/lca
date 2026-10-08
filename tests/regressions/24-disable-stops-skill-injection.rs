//! Cycle-7 driving defect: `lca ext disable <pack>` did not stop a data-only
//! package's skill from injecting. The host-side skills merge reads the
//! install tree directly (ADR-0030/0034), so it has to apply the same
//! per-project enablement filter the extension registry does - otherwise the
//! threat model's promise that "disable removes its skill pack" is false.
//!
//! Verifies: FR-PROV-9, ADR-0030 (the threat-model resources row).

use lca_core::SkillsRoots;
use lca_tools::skills;

fn write_skill(dir: &std::path::Path, name: &str, body: &str) {
    let skill = dir.join(name);
    std::fs::create_dir_all(&skill).expect("mkdir");
    std::fs::write(skill.join("SKILL.md"), format!("name: {name}\n---\n{body}")).expect("write");
}

#[test]
fn a_disabled_package_contributes_no_skills() {
    let root = lca_testkit::scratch_path("regression-24-disabled-skills");
    let _ = std::fs::remove_dir_all(&root);
    let extensions = root.join("data/extensions");
    write_skill(
        &extensions.join("pack/resources/skills"),
        "packaged",
        "extension only",
    );

    let mut roots = SkillsRoots {
        project: root.join("project"),
        user: root.join("config/skills"),
        extensions,
        disabled: Vec::new(),
        extra: Vec::new(),
    };
    std::fs::create_dir_all(&roots.project).expect("mkdir");
    assert!(
        skills::collect(&roots).iter().any(|s| s.name == "packaged"),
        "enabled: collected"
    );

    roots.disabled = vec!["pack".to_string()];
    assert!(
        !skills::collect(&roots).iter().any(|s| s.name == "packaged"),
        "disabled: the skill pack falls out with the extension"
    );

    // Disabling one package never hides another's skills.
    write_skill(
        &roots.extensions.join("other/resources/skills"),
        "other-skill",
        "from the other package",
    );
    assert!(
        skills::collect(&roots)
            .iter()
            .any(|s| s.name == "other-skill"),
        "only the disabled package is filtered"
    );

    let _ = std::fs::remove_dir_all(&root);
}
