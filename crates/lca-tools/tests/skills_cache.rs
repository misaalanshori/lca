//! The skill-scan cache (gh #153): the turn's per-round assembly
//! reuses scans until the skill trees change, so `assemble_request`
//! never blocks on directory walks.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_tools::skills::{SkillCache, SkillsRoots};
use std::sync::Mutex;

fn roots(dir: &std::path::Path) -> SkillsRoots {
    SkillsRoots {
        project: dir.join("project"),
        user: dir.join("user"),
        extensions: dir.join("ext"),
        ..Default::default()
    }
}

fn write_skill(dir: &std::path::Path, name: &str) {
    let skill = dir.join(name);
    std::fs::create_dir_all(&skill).expect("mkdir");
    std::fs::write(skill.join("SKILL.md"), "# hi\n---\nbody\n").expect("write");
}

// Verifies: gh #153 - the cache collects what `collect` finds, serves
// it unchanged while the trees are still, and notices a new skill
// (invalidation by content change, not by timer).
#[tokio::test]
async fn the_skill_cache_collects_and_notices_change() {
    let base = lca_testkit::scratch_path("skill-cache");
    std::fs::create_dir_all(base.join("project").join(".lca/skills")).expect("mkdir");
    std::fs::create_dir_all(base.join("user")).expect("mkdir");
    std::fs::create_dir_all(base.join("ext")).expect("mkdir");
    write_skill(&base.join("project").join(".lca/skills"), "alpha");
    let roots = roots(&base);

    let cache = Mutex::new(SkillCache::new());
    let first = SkillCache::collect_cached(&cache, &roots).await;
    assert_eq!(first.len(), 1, "the cache collects");
    assert!(cache.lock().expect("lock").is_fresh(&roots));

    // A new skill lands: the next collect sees it.
    write_skill(&base.join("user"), "beta");
    assert!(
        !cache.lock().expect("lock").is_fresh(&roots),
        "a new skill dirties the cache"
    );
    // Mtime granularity can hide a same-tick write; the content check
    // below is what the turn relies on either way.
    let second = SkillCache::collect_cached(&cache, &roots).await;
    assert_eq!(second.len(), 2, "invalidation re-scans");
    assert!(second.iter().any(|skill| skill.name == "beta"));
}

// Verifies: gh #59 - a per-package selection narrows which skill
// dirs load: `+` keeps, `-`/`!` drops, unlisted packages load whole.
#[test]
fn package_selection_narrows_which_skills_load() {
    use lca_tools::skills::{ResourceSelection, SkillsRoots, collect, parse_resource_selection};
    let base = lca_testkit::scratch_path("skill-selection");
    let ext = base
        .join("ext")
        .join("pack")
        .join("resources")
        .join("skills");
    for name in ["keep", "drop", "tmp-1"] {
        let dir = ext.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("SKILL.md"), "# hi\n---\nbody\n").expect("write");
    }
    let rules = |list: &[&str]| {
        parse_resource_selection(&list.iter().map(|rule| rule.to_string()).collect::<Vec<_>>())
            .expect("valid rules")
    };
    let mut roots = SkillsRoots {
        project: base.join("project"),
        user: base.join("user"),
        extensions: base.join("ext"),
        disabled: Vec::new(),
        extra: Vec::new(),
        resource_selection: [(
            "pack".to_string(),
            rules(&["+skills", "-skills/drop", "!skills/tmp*/**"]),
        )]
        .into_iter()
        .collect(),
    };
    let names = |skills: Vec<lca_tools::skills::Skill>| {
        let mut names: Vec<String> = skills.into_iter().map(|skill| skill.name).collect();
        names.sort();
        names
    };
    assert_eq!(names(collect(&roots)), vec!["keep".to_string()]);
    roots.resource_selection = std::collections::BTreeMap::new();
    let _ = ResourceSelection::default();
    assert_eq!(
        names(collect(&roots)),
        vec!["drop".to_string(), "keep".to_string(), "tmp-1".to_string()],
        "no selection loads everything"
    );
}
