//! Prompt templates (gh #58): Markdown files become `/` commands.
//! Pi's `prompt-templates.md` shape on our skill-root pattern: direct
//! `.md` children of the user (`<data>/prompts`) and project
//! (`.lca/prompts`) directories, frontmatter `description` and
//! `argument-hint`, pi's `$1`/`$@`/`${1:-default}`/`${@:N:L}`
//! substitutions with shell-like quoting. Packages and explicit paths
//! stay out: user/project covers the workflow; the rest is a later
//! issue, not a silent half.

use std::collections::HashSet;
use std::path::PathBuf;

/// Where templates load from, highest precedence first.
#[derive(Debug, Clone, Default)]
pub struct PromptRoots {
    /// The user's template directory (`<data>/prompts`).
    pub user: PathBuf,
    /// The workspace root; `.lca/prompts` inside it is the project source.
    pub project: PathBuf,
}

/// One template: the command name is the filename stem.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    /// The `/` command name.
    pub name: String,
    /// The completion description (frontmatter, else the first
    /// non-empty body line, pi's fallback).
    pub description: String,
    /// The completion hint (`[optional]`, `<required>`).
    pub argument_hint: String,
    /// The expandable body.
    pub body: String,
}

/// Collect both sources with precedence (project > user), first name
/// wins, sorted by name for determinism.
pub fn collect(roots: &PromptRoots) -> Vec<PromptTemplate> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    // Empty roots stay out: joining onto one reads the process cwd.
    if !roots.project.as_os_str().is_empty() {
        for template in templates_in(&roots.project.join(".lca/prompts")) {
            if seen.insert(template.name.clone()) {
                found.push(template);
            }
        }
    }
    if !roots.user.as_os_str().is_empty() {
        for template in templates_in(&roots.user) {
            if seen.insert(template.name.clone()) {
                found.push(template);
            }
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// The direct `.md` children of one directory, sorted by name.
fn templates_in(dir: &std::path::Path) -> Vec<PromptTemplate> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "md"))
        .filter_map(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .filter(|stem| !stem.is_empty())
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let text = std::fs::read_to_string(dir.join(format!("{name}.md"))).ok()?;
            Some(parse_template(&name, &text))
        })
        .collect()
}

/// Split frontmatter (`description`, `argument-hint`) from the body.
fn parse_template(name: &str, text: &str) -> PromptTemplate {
    let (frontmatter, body) = match text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    {
        Some(rest) => match rest
            .split_once("\n---\n")
            .or_else(|| rest.split_once("\r\n---\r\n"))
        {
            Some((front, body)) => (front, body.to_string()),
            None => ("", text.to_string()),
        },
        None => ("", text.to_string()),
    };
    let mut description = String::new();
    let mut argument_hint = String::new();
    for line in frontmatter.lines() {
        if let Some((key, value)) = line.split_once(':') {
            match key.trim() {
                "description" => description = dequote(value.trim()),
                "argument-hint" => argument_hint = dequote(value.trim()),
                _ => {}
            }
        }
    }
    if description.is_empty() {
        // Pi's fallback: the first non-empty line speaks for the file.
        description = body
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_string();
    }
    PromptTemplate {
        name: name.to_string(),
        description,
        argument_hint,
        body,
    }
}

/// Strip one layer of matching single or double quotes (YAML scalars).
fn dequote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

/// Expand a template over shell-like arguments (pi's substitution
/// table): `$1..$9`, `$@`/`$ARGUMENTS`, `${1:-default}`,
/// `${@:-default}`, `${@:N}`, `${@:N:L}`. A missing argument without a
/// default expands to nothing; anything else after `$` passes through
/// untouched.
pub fn expand(template: &PromptTemplate, arguments: &str) -> String {
    let args = split_args(arguments);
    let all = args.join(" ");
    let mut out = String::new();
    let mut chars = template.body.chars().peekable();
    while let Some(char) = chars.next() {
        if char != '$' {
            out.push(char);
            continue;
        }
        match chars.peek() {
            Some('{') => {
                chars.next();
                let mut inner = String::new();
                for char in chars.by_ref() {
                    if char == '}' {
                        break;
                    }
                    inner.push(char);
                }
                out.push_str(&braced(&inner, &args, &all));
            }
            Some(digit) if digit.is_ascii_digit() => {
                let index = (*digit as u8 - b'0') as usize;
                chars.next();
                out.push_str(
                    args.get(index.wrapping_sub(1))
                        .map(String::as_str)
                        .unwrap_or(""),
                );
            }
            Some('@') => {
                chars.next();
                out.push_str(&all);
            }
            Some('A') => {
                // `$ARGUMENTS` is pi's long spelling of `$@`.
                let rest: String = chars.clone().take("ARGUMENTS".len()).collect();
                if rest == "ARGUMENTS" {
                    for _ in 0.."ARGUMENTS".len() {
                        chars.next();
                    }
                    out.push_str(&all);
                } else {
                    out.push('$');
                }
            }
            _ => out.push('$'),
        }
    }
    out
}

/// One `${...}` substitution; unparseable shapes stay literal.
fn braced(inner: &str, args: &[String], all: &str) -> String {
    if inner == "@" || inner == "ARGUMENTS" {
        return all.to_string();
    }
    if let Some(rest) = inner.strip_prefix("@:-") {
        return if all.is_empty() {
            rest.to_string()
        } else {
            all.to_string()
        };
    }
    if let Some(rest) = inner.strip_prefix("@:") {
        let mut parts = rest.splitn(2, ':');
        let start: usize = parts.next().and_then(|n| n.parse().ok()).unwrap_or(1);
        let args = &args_at(args, start);
        return match parts.next().and_then(|n| n.parse::<usize>().ok()) {
            Some(len) => args.iter().take(len).cloned().collect::<Vec<_>>().join(" "),
            None => args.join(" "),
        };
    }
    if let Some((index, default)) = inner.split_once(":-") {
        return match index.parse::<usize>() {
            Ok(n) => args
                .get(n.wrapping_sub(1))
                .cloned()
                .unwrap_or_else(|| default.to_string()),
            Err(_) => format!("${{{inner}}}"),
        };
    }
    match inner.parse::<usize>() {
        Ok(n) => args.get(n.wrapping_sub(1)).cloned().unwrap_or_default(),
        Err(_) => format!("${{{inner}}}"),
    }
}

/// Arguments from 1-based position `start` onward (clamped).
fn args_at(args: &[String], start: usize) -> &[String] {
    args.get(start.saturating_sub(1)..).unwrap_or_default()
}

/// Shell-like argument splitting; an unbalanced line is one argument
/// rather than an error.
fn split_args(arguments: &str) -> Vec<String> {
    if arguments.trim().is_empty() {
        return Vec::new();
    }
    shell_words::split(arguments).unwrap_or_else(|_| vec![arguments.to_string()])
}
