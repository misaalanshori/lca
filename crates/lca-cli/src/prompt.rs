//! Sectioned system-prompt assembly (gh #43, #68, #74; RM-015's core):
//! preamble, tool declarations, project context with attribution,
//! additional instructions, the skills catalog, and workspace facts.
//!
//! Pure string assembly over caller-gathered inputs, so the golden test
//! below pins the exact section order and headers. File discovery (with
//! the trust rule) and flag handling live beside it; `-nc` suppresses
//! the project-context section only.

/// One discovered context file with its attribution header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    /// Header naming the source, e.g. `~/.lca/AGENTS.md`.
    pub source: String,
    /// The file's text.
    pub body: String,
}

/// One append instruction with its attribution header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendInstruction {
    /// Header naming the source, e.g. `user APPEND_SYSTEM.md`.
    pub source: String,
    /// The instruction text.
    pub body: String,
}

/// Gathered prompt files: the optional preamble replacement, appends,
/// and context files.
#[derive(Debug, Default)]
pub struct PromptFiles {
    /// A `SYSTEM.md` (or `--system-prompt`) replacement for the preamble.
    pub preamble_override: Option<String>,
    /// Append instructions, user before project before flag.
    pub appends: Vec<AppendInstruction>,
    /// Context files with attribution.
    pub context_files: Vec<ContextFile>,
}

fn read_if_present(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
}

/// Read the prompt files (gh #68, #74): user `SYSTEM.md`/`APPEND_SYSTEM.md`
/// always apply; the trusted project's `.lca` pair wins by replacement
/// (never combined, pi's rule); context files come from the user dir and
/// the trusted project dir only - never untrusted parent traversal (the
/// documented trust divergence: pi reads parents trust-free, which is a
/// prompt-injection surface this folder-trust model exists to close).
/// `-nc` drops context discovery; the flag files override for one run.
/// A flag naming a missing file is an error (like `--attach`).
pub fn load_prompt_files(
    data_dir: &std::path::Path,
    cwd: &std::path::Path,
    trusted: bool,
    no_context_files: bool,
    system_flag: Option<&std::path::Path>,
    append_flag: Option<&std::path::Path>,
) -> Result<PromptFiles, String> {
    let mut files = PromptFiles::default();
    if let Some(path) = system_flag {
        files.preamble_override = Some(
            std::fs::read_to_string(path)
                .map(|text| text.trim().to_string())
                .map_err(|err| format!("cannot read --system-prompt {}: {err}", path.display()))?,
        );
    } else {
        let user = read_if_present(&data_dir.join("SYSTEM.md"));
        let project = trusted
            .then(|| read_if_present(&cwd.join(".lca/SYSTEM.md")))
            .flatten()
            .filter(|text| !text.is_empty());
        files.preamble_override = project.or(user).filter(|text| !text.is_empty());
    }
    let user_append = read_if_present(&data_dir.join("APPEND_SYSTEM.md"));
    let project_append = trusted
        .then(|| read_if_present(&cwd.join(".lca/APPEND_SYSTEM.md")))
        .flatten()
        .filter(|text| !text.is_empty());
    // Same-name precedence, not combination (pi's rule): the trusted
    // project's append wins over the user's.
    let file_append = project_append
        .map(|body| ("project APPEND_SYSTEM.md", body))
        .or_else(|| {
            user_append
                .filter(|text| !text.is_empty())
                .map(|body| ("user APPEND_SYSTEM.md", body))
        });
    if let Some((source, body)) = file_append {
        files.appends.push(AppendInstruction {
            source: source.to_string(),
            body,
        });
    }
    if let Some(path) = append_flag {
        let body = std::fs::read_to_string(path).map_err(|err| {
            format!(
                "cannot read --append-system-prompt {}: {err}",
                path.display()
            )
        })?;
        if !body.trim().is_empty() {
            files.appends.push(AppendInstruction {
                source: "--append-system-prompt".to_string(),
                body: body.trim().to_string(),
            });
        }
    }
    if !no_context_files {
        files.context_files = read_context_dir(data_dir);
        if trusted {
            files.context_files.extend(read_context_dir(cwd));
        }
    }
    Ok(files)
}

/// Context files in one directory, pi's set and override rule: the
/// same-directory `AGENTS.override.md` replaces its siblings, otherwise
/// every present name loads. Attribution is the full path. Entries
/// dedupe by canonical path: on a case-insensitive filesystem
/// `AGENTS.md` and `AGENTS.MD` are one file, and loading it twice
/// would steer the prompt twice.
/// The context file paths one directory contributes (gh #131): the
/// same walk [`read_context_dir`] performs, sources only, so the
/// startup header lists exactly what the prompt loads.
pub(crate) fn context_sources(
    data_dir: &std::path::Path,
    cwd: &std::path::Path,
    trusted: bool,
    no_context_files: bool,
) -> Vec<std::path::PathBuf> {
    if no_context_files {
        return Vec::new();
    }
    let mut out: Vec<std::path::PathBuf> = read_context_dir(data_dir)
        .into_iter()
        .map(|file| std::path::PathBuf::from(file.source))
        .collect();
    if trusted {
        out.extend(
            read_context_dir(cwd)
                .into_iter()
                .map(|file| std::path::PathBuf::from(file.source)),
        );
    }
    out
}

fn read_context_dir(dir: &std::path::Path) -> Vec<ContextFile> {
    let present = |name: &str| {
        let path = dir.join(name);
        read_if_present(&path)
            .filter(|text| !text.is_empty())
            .map(|body| (path, body))
    };
    let mut found: Vec<(std::path::PathBuf, String)> = Vec::new();
    if let Some(over) = present("AGENTS.override.md") {
        found.push(over);
    } else {
        for name in ["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"] {
            if let Some(file) = present(name) {
                found.push(file);
            }
        }
    }
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    found
        .into_iter()
        .filter(|(path, _)| {
            let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            if seen.contains(&key) {
                return false;
            }
            seen.push(key);
            true
        })
        .map(|(path, body)| ContextFile {
            source: path.to_string_lossy().into_owned(),
            body,
        })
        .collect()
}

/// The agent's system prompt for one process (gh #43, #68, #74): the
/// sectioned assembly over everything the run gathered - files, flags,
/// trust, skills. Both front ends call this once at startup. A flag
/// naming a missing file fails the startup (the `Err` carries why).
pub fn agent_system_prompt(
    cwd: &std::path::Path,
    model_id: &str,
    flags: &crate::CliFlags,
    trusted: bool,
) -> Result<String, String> {
    let data = crate::data_dir();
    let files = load_prompt_files(
        &data,
        cwd,
        trusted,
        flags.no_context_files,
        flags.system_prompt.as_deref(),
        flags.append_system_prompt.as_deref(),
    )?;
    let roots = crate::skills_roots(cwd);
    let collected = lca_tools::skills::collect(&roots);
    let advertised = collected.iter().any(|skill| skill.model_invocable);
    let catalog = advertised.then(|| lca_tools::skills::catalog(&collected));
    let identity;
    let preamble = match files.preamble_override.as_deref() {
        Some(text) => text,
        None => {
            identity = lca_core::identity_prompt(model_id, std::env::consts::OS);
            identity.as_str()
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let cwd_text = cwd.to_string_lossy();
    let date_text = civil_date(now);
    Ok(compose_system_prompt(&PromptInputs {
        preamble,
        tool_names: lca_core::BUILTIN_TOOLS,
        context_files: files.context_files,
        appends: files.appends,
        skills_catalog: catalog.as_deref(),
        cwd: &cwd_text,
        date: &date_text,
    }))
}

/// Inputs to [`compose_system_prompt`]; the caller reads the files.
pub struct PromptInputs<'a> {
    /// The preamble: the built-in identity, a `SYSTEM.md` replacement,
    /// or `--system-prompt`.
    pub preamble: &'a str,
    /// Built-in tool names for the declarations section.
    pub tool_names: &'a [&'a str],
    /// Project-context files (empty with `-nc`).
    pub context_files: Vec<ContextFile>,
    /// Append instructions (user/project files plus the flag).
    pub appends: Vec<AppendInstruction>,
    /// The skills catalog text (omitted when no skill is advertised).
    pub skills_catalog: Option<&'a str>,
    /// The working directory.
    pub cwd: &'a str,
    /// The calendar date (`YYYY-MM-DD`).
    pub date: &'a str,
}

/// Assemble the system prompt: fixed section order, attribution headers
/// on every gathered block, empty optional sections dropped.
pub fn compose_system_prompt(inputs: &PromptInputs<'_>) -> String {
    let mut out = inputs.preamble.trim_end().to_string();
    out.push_str("\n\n## Tools\n");
    out.push_str(&inputs.tool_names.join(", "));
    if !inputs.context_files.is_empty() {
        out.push_str("\n\n## Project context");
        for file in &inputs.context_files {
            out.push_str("\n### From ");
            out.push_str(&file.source);
            out.push('\n');
            out.push_str(file.body.trim_end());
            out.push('\n');
        }
        out.pop();
    }
    if !inputs.appends.is_empty() {
        out.push_str("\n\n## Additional instructions");
        for append in &inputs.appends {
            out.push_str("\n### From ");
            out.push_str(&append.source);
            out.push('\n');
            out.push_str(append.body.trim_end());
            out.push('\n');
        }
        out.pop();
    }
    if let Some(catalog) = inputs.skills_catalog {
        out.push_str("\n\n## Skills\n");
        out.push_str(catalog.trim_end());
    }
    out.push_str("\n\n## Workspace\ncwd: ");
    out.push_str(inputs.cwd);
    out.push_str("\ndate: ");
    out.push_str(inputs.date);
    out.push('\n');
    out
}

/// Civil date from epoch seconds (Howard Hinnant's days-to-civil
/// algorithm, public domain): the `YYYY-MM-DD` workspace fact without
/// a calendar crate.
pub fn civil_date(epoch_secs: u64) -> String {
    let shifted = epoch_secs.div_euclid(86_400) as i64 + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era.div_euclid(1_460) + day_of_era.div_euclid(36_524)
        - day_of_era.div_euclid(146_096))
    .div_euclid(365);
    let mut year = year_of_era + era * 400;
    let day_of_year =
        day_of_era - (365 * year_of_era + year_of_era.div_euclid(4) - year_of_era.div_euclid(100));
    let month_part = (5 * day_of_year + 2).div_euclid(153);
    let day = day_of_year - (153 * month_part + 2).div_euclid(5) + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod golden_tests {
    use super::*;

    // Verifies: RM-015 (the sectioned assembly): exact section order and
    // attribution headers, empty sections dropped.
    #[test]
    fn the_assembly_orders_sections_with_attribution() {
        let tools = [
            "read", "write", "edit", "list", "glob", "grep", "shell", "skill",
        ];
        let out = compose_system_prompt(&PromptInputs {
            preamble: "You are LCA.",
            tool_names: &tools,
            context_files: vec![
                ContextFile {
                    source: "~/.lca/AGENTS.md".to_string(),
                    body: "Always run tests.".to_string(),
                },
                ContextFile {
                    source: "/repo/.lca/AGENTS.override.md".to_string(),
                    body: "Never run tests here.".to_string(),
                },
            ],
            appends: vec![AppendInstruction {
                source: "--append-system-prompt".to_string(),
                body: "Answer in English.".to_string(),
            }],
            skills_catalog: Some(
                "Available skills:\n- commits — Write commit messages. (from user)\n",
            ),
            cwd: "/repo",
            date: "2026-10-06",
        });
        assert_eq!(
            out,
            "You are LCA.\n\
             \n\
             ## Tools\n\
             read, write, edit, list, glob, grep, shell, skill\n\
             \n\
             ## Project context\n\
             ### From ~/.lca/AGENTS.md\n\
             Always run tests.\n\
             \n\
             ### From /repo/.lca/AGENTS.override.md\n\
             Never run tests here.\n\
             \n\
             ## Additional instructions\n\
             ### From --append-system-prompt\n\
             Answer in English.\n\
             \n\
             ## Skills\n\
             Available skills:\n\
             - commits — Write commit messages. (from user)\n\
             \n\
             ## Workspace\n\
             cwd: /repo\n\
             date: 2026-10-06\n"
        );
    }

    // Verifies: `-nc` (and empty sources generally): optional sections
    // drop out instead of printing lonely headers.
    #[test]
    fn empty_sections_drop_out() {
        let tools = ["read"];
        let out = compose_system_prompt(&PromptInputs {
            preamble: "You are LCA.",
            tool_names: &tools,
            context_files: vec![],
            appends: vec![],
            skills_catalog: None,
            cwd: "/repo",
            date: "2026-10-06",
        });
        assert_eq!(
            out,
            "You are LCA.\n\n## Tools\nread\n\n## Workspace\ncwd: /repo\ndate: 2026-10-06\n"
        );
    }

    // The date math carries its own row: three known epochs.
    #[test]
    fn civil_dates_match_the_calendar() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(1767225600), "2026-01-01");
        assert_eq!(civil_date(1791247449), "2026-10-06");
    }
}

#[cfg(test)]
mod loader_tests {
    use super::*;

    fn tree(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = lca_testkit::scratch_path(name);
        let data = root.join("data");
        let cwd = root.join("project");
        std::fs::create_dir_all(&cwd).expect("mkdir");
        (data, cwd)
    }

    fn load(data: &std::path::Path, cwd: &std::path::Path, trusted: bool, nc: bool) -> PromptFiles {
        load_prompt_files(data, cwd, trusted, nc, None, None).expect("load")
    }

    // Verifies: gh #68 (user SYSTEM.md replaces the preamble).
    #[test]
    fn user_system_md_replaces_the_preamble() {
        let (data, cwd) = tree("prompt-user-system");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(data.join("SYSTEM.md"), "Custom identity.\n").expect("write");
        let files = load(&data, &cwd, false, false);
        assert_eq!(files.preamble_override.as_deref(), Some("Custom identity."));
    }

    // Verifies: gh #68 (trusted project wins; untrusted project is
    // ignored - the trust posture holds for prompt files too).
    #[test]
    fn trusted_project_system_md_wins_untrusted_is_ignored() {
        let (data, cwd) = tree("prompt-project-system");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(data.join("SYSTEM.md"), "User identity.\n").expect("write");
        std::fs::create_dir_all(cwd.join(".lca")).expect("mkdir");
        std::fs::write(cwd.join(".lca/SYSTEM.md"), "Project identity.\n").expect("write");
        assert_eq!(
            load(&data, &cwd, true, false).preamble_override.as_deref(),
            Some("Project identity.")
        );
        assert_eq!(
            load(&data, &cwd, false, false).preamble_override.as_deref(),
            Some("User identity."),
            "untrusted project files never load"
        );
    }

    // Verifies: gh #68 (appends): user and project appends, same-name
    // precedence without combining.
    #[test]
    fn appends_prefer_the_trusted_project_without_combining() {
        let (data, cwd) = tree("prompt-append");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(data.join("APPEND_SYSTEM.md"), "User note.\n").expect("write");
        std::fs::create_dir_all(cwd.join(".lca")).expect("mkdir");
        std::fs::write(cwd.join(".lca/APPEND_SYSTEM.md"), "Project note.\n").expect("write");
        let trusted = load(&data, &cwd, true, false);
        assert_eq!(trusted.appends.len(), 1, "same name is not combined");
        assert_eq!(trusted.appends[0].body, "Project note.");
        let untrusted = load(&data, &cwd, false, false);
        assert_eq!(untrusted.appends.len(), 1);
        assert_eq!(untrusted.appends[0].body, "User note.");
    }

    // Verifies: gh #68 (flags): `--system-prompt` wins for one run, and
    // a missing flag file is an error, not a silent default.
    #[test]
    fn flag_files_override_for_one_run_and_missing_is_an_error() {
        let (data, cwd) = tree("prompt-flags");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(data.join("SYSTEM.md"), "User identity.\n").expect("write");
        let flag = data.join("flag.md");
        std::fs::write(&flag, "Flag identity.\n").expect("write");
        let files = load_prompt_files(&data, &cwd, false, false, Some(&flag), None).expect("load");
        assert_eq!(files.preamble_override.as_deref(), Some("Flag identity."));
        assert!(
            load_prompt_files(
                &data,
                &cwd,
                false,
                false,
                Some(&data.join("missing.md")),
                None
            )
            .is_err(),
            "a missing flag file fails loudly"
        );
    }

    // Verifies: gh #74 (context discovery under the trust rule): user
    // dir plus trusted project, never an untrusted project, and `-nc`
    // drops it all.
    #[test]
    fn context_files_load_from_user_and_trusted_project_only() {
        let (data, cwd) = tree("prompt-context");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(data.join("AGENTS.md"), "User instructions.\n").expect("write");
        std::fs::write(cwd.join("CLAUDE.md"), "Project instructions.\n").expect("write");
        let trusted = load(&data, &cwd, true, false);
        assert_eq!(trusted.context_files.len(), 2);
        assert!(trusted.context_files[0].source.ends_with("AGENTS.md"));
        assert!(trusted.context_files[1].source.ends_with("CLAUDE.md"));
        let untrusted = load(&data, &cwd, false, false);
        assert_eq!(
            untrusted.context_files.len(),
            1,
            "untrusted project skipped"
        );
        let nc = load(&data, &cwd, true, true);
        assert!(nc.context_files.is_empty(), "-nc suppresses discovery");
    }

    // Verifies: gh #74 (the override rule): `AGENTS.override.md`
    // replaces its same-directory siblings, nothing else.
    #[test]
    fn an_override_replaces_only_its_same_directory_siblings() {
        let (data, cwd) = tree("prompt-override");
        std::fs::create_dir_all(&data).expect("mkdir");
        std::fs::write(data.join("AGENTS.md"), "User instructions.\n").expect("write");
        std::fs::write(cwd.join("AGENTS.md"), "Project instructions.\n").expect("write");
        std::fs::write(cwd.join("AGENTS.override.md"), "Override wins.\n").expect("write");
        let files = load(&data, &cwd, true, false);
        assert_eq!(files.context_files.len(), 2);
        assert!(files.context_files[0].source.ends_with("AGENTS.md"));
        assert_eq!(files.context_files[1].body, "Override wins.");
    }

    // Verifies: gh #130 acceptance - an edited skill description appears
    // the next time the system prompt builds (what `/reload` triggers):
    // collection reads disk every time, nothing caches it.
    #[test]
    fn an_edited_skill_description_appears_on_rebuild() {
        let (data, cwd) = tree("prompt-reload-skill");
        std::fs::create_dir_all(&data).expect("mkdir");
        let dir = cwd.join(".lca/skills/demo");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let skill = dir.join("SKILL.md");
        std::fs::write(&skill, "description: first words\n---\nBody.\n").expect("write");
        let flags = crate::CliFlags::default();
        let before = agent_system_prompt(&cwd, "m", &flags, true).expect("build");
        assert!(before.contains("first words"), "v1 catalogued:\n{before}");
        std::fs::write(&skill, "description: second thoughts\n---\nBody.\n").expect("edit");
        let after = agent_system_prompt(&cwd, "m", &flags, true).expect("rebuild");
        assert!(
            after.contains("second thoughts") && !after.contains("first words"),
            "v2 replaces v1:\n{after}"
        );
    }
}

#[cfg(test)]
mod acceptance_tests {
    use super::*;

    // Verifies: gh #43 (`a_skill_loads_only_when_named`): a session with
    // three skills sends a system prompt listing three one-line entries
    // and no skill bodies. (The `/skill:<name> args` half appends the
    // body plus args as one user block through the host command, covered
    // by the live receipt: no Ui exists in unit scope to drive it.)
    #[test]
    fn a_skill_loads_only_when_named() {
        let tools = ["read", "skill"];
        let catalog = "Available skills (load one with the `skill` tool or `/skill:name`):\n\
             - commits — Write commit messages. (from project)\n\
             - review — Review code. (from user)\n\
             - deploy — Ship it. (from extension `pack`)\n";
        let out = compose_system_prompt(&PromptInputs {
            preamble: "You are LCA.",
            tool_names: &tools,
            context_files: vec![],
            appends: vec![],
            skills_catalog: Some(catalog),
            cwd: "/repo",
            date: "2026-10-06",
        });
        for name in ["commits", "review", "deploy"] {
            assert!(out.contains(name), "the catalog names {name}");
        }
        assert!(!out.contains("intent-bearing"), "no bodies ride along");
        assert!(out.contains("## Skills"), "the catalog has its section");
    }
}

#[cfg(test)]
#[cfg(unix)]
mod collision_tests {
    use super::read_context_dir;

    // Verifies: gh #74 review (case-insensitive filesystems): `AGENTS.md`
    // and `AGENTS.MD` spell one file there - it loads once, not twice.
    // A symlink reproduces the shared-canonical-path shape on Linux.
    #[test]
    fn case_variants_of_one_file_load_once() {
        let root = lca_testkit::scratch_path("prompt-case-dedupe");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join("AGENTS.md"), "Steer.\n").expect("write");
        if std::os::unix::fs::symlink(root.join("AGENTS.md"), root.join("AGENTS.MD")).is_err() {
            eprintln!("skip: symlinks need privileges on this host");
            return;
        }
        let files = read_context_dir(&root);
        assert_eq!(files.len(), 1, "one file, one section: {files:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
