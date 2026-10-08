//! Prompt template tests (gh #58): collection from user/project dirs
//! and pi's substitution shapes.

use std::path::{Path, PathBuf};

use lca_tools::prompts::{PromptRoots, collect, expand};

fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

fn roots(user: &Path, project: &Path) -> PromptRoots {
    PromptRoots {
        user: user.to_path_buf(),
        project: project.to_path_buf(),
    }
}

// Verifies: gh #58 - `~/.lca/prompts/review.md` with frontmatter appears
// as `/review` with its description and hint; the project wins ties.
#[test]
fn a_prompt_template_collects_with_description_and_hint() {
    let base = scratch("prompts-collect");
    let user = base.join("user");
    let project = base.join("proj");
    std::fs::create_dir_all(user.join("x")).expect("mkdir");
    std::fs::create_dir_all(project.join(".lca/prompts")).expect("mkdir");
    std::fs::create_dir_all(user.join("prompts")).expect("mkdir");
    std::fs::write(
        user.join("prompts/review.md"),
        "---\ndescription: Review staged git changes\nargument-hint: \"[focus]\"\n---\nReview. Focus on $1.\n",
    )
    .expect("write");
    std::fs::write(
        project.join(".lca/prompts/review.md"),
        "---\ndescription: Project review\nargument-hint: \"[focus]\"\n---\nProject review of $1.\n",
    )
    .expect("write");
    let found = collect(&roots(&user.join("prompts"), &project));
    assert_eq!(found.len(), 1, "first name wins: {found:?}");
    let template = &found[0];
    assert_eq!(template.name, "review");
    assert_eq!(template.description, "Project review", "project wins");
    assert_eq!(template.argument_hint, "[focus]", "hint parses");
}

// Verifies: gh #58 - every pi substitution shape expands, with
// shell-like quoting for the arguments.
#[test]
fn a_prompt_template_expands_with_arguments() {
    let template = lca_tools::prompts::PromptTemplate {
        name: "review".to_string(),
        description: String::new(),
        argument_hint: String::new(),
        body: "one=$1 two=$2 all=$@ args=$ARGUMENTS d1=${1:-correctness} dall=${@:-none} from2=${@:2} slice=${@:2:1}".to_string(),
    };
    assert_eq!(
        expand(&template, r#"concurrency "API design""#),
        "one=concurrency two=API design all=concurrency API design args=concurrency API design d1=concurrency dall=concurrency API design from2=API design slice=API design"
    );
}

// Verifies: gh #58 - a missing argument without a default expands to
// nothing (never the literal `$1`), and unknown `$` sequences pass
// through untouched.
#[test]
fn missing_arguments_expand_empty_and_stray_dollars_survive() {
    let template = lca_tools::prompts::PromptTemplate {
        name: "r".to_string(),
        description: String::new(),
        argument_hint: String::new(),
        body: "a=$1 b=$2 d=${1:-fallback} price=$5 or $$".to_string(),
    };
    assert_eq!(expand(&template, ""), "a= b= d=fallback price= or $$");
}
