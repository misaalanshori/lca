//! gh #271: the system prompt advertises the tools the run actually exposes.
//!
//! The prompt used to hardcode `lca_core::BUILTIN_TOOLS`, so
//! `lca -p "…" --no-tools` told the model about seven tools it could not call —
//! actively contradicting the tool declarations sent to the provider in the same
//! request. The contract is that **the prompt and the declaration agree**, which
//! is what these rows assert: the prompt's tool section comes from the same
//! resolution the request's tool list uses.
//!
//! Rows drive `lca -p --mode json` and read the system prompt the provider
//! actually received (the loopback mock records every request body), so this is
//! the wire, not a rendered string we hope matches it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

use common::{Reply, sandbox, sse_text, start_mock};

/// The comma-separated tool line under `## Tools`, or empty when the section
/// is absent or blank.
fn tool_line(prompt: &str) -> String {
    let Some(rest) = prompt.split("## Tools").nth(1) else {
        return String::new();
    };
    // The section's content ends at the next `##` header; a blank section (the
    // `--no-tools` case) yields an empty string rather than the next heading.
    let mut lines = rest.lines().skip(1).map(str::trim);
    let content = lines
        .by_ref()
        .take_while(|line| !line.starts_with("##"))
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    content.to_string()
}

/// The system prompt one `lca -p` run sent to the provider.
fn system_prompt_for(args: &[&str], tag: &str) -> (String, String) {
    let runtime = common::rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("done"))]));
    let sandbox = sandbox(tag);
    let output = sandbox.run(Some(&mock), args);
    assert_eq!(
        output.status.code(),
        Some(0),
        "the run succeeds: {}",
        common::stderr(&output)
    );
    let bodies = mock.bodies().join("\n");
    let parsed: serde_json::Value =
        serde_json::from_str(mock.bodies().first().expect("one request").as_str())
            .expect("the request body is JSON");
    let mut prompt = String::new();
    // The system message may be a plain string or an array of blocks.
    for message in parsed["messages"].as_array().cloned().unwrap_or_default() {
        if message["role"] == "system" {
            match &message["content"] {
                serde_json::Value::String(text) => prompt.push_str(text),
                serde_json::Value::Array(blocks) => {
                    for block in blocks {
                        if let Some(text) = block["text"].as_str() {
                            prompt.push_str(text);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    prompt.push('\n');
    prompt.push_str(&bodies);
    (prompt, mock.bodies().join("\n"))
}

// Verifies: gh #271 acceptance 1 - `--no-tools` advertises no built-in tool in
// the system prompt. Before the fix the prompt listed all of them.
#[test]
fn no_tools_advertises_no_builtin_tool_in_the_prompt() {
    let (prompt, _) = system_prompt_for(&["-p", "hi", "--no-tools"], "prompt-no-tools");
    // The tools section lists the names comma-separated; a phrase unique to it
    // is safer than a bare word that also appears in prose.
    assert_eq!(
        tool_line(&prompt),
        "",
        "the tool section is empty under --no-tools:\n{prompt}"
    );
    // The declaration must agree: no tool is offered at all.
    assert!(
        !prompt.contains("\"tools\":[{"),
        "the request declares no tools under --no-tools:\n{prompt}"
    );
}

// Verifies: gh #271 acceptance 2 - `-t read,grep` advertises exactly those two,
// and the request declares exactly those two.
#[test]
fn an_allowlist_advertises_exactly_its_tools() {
    let (prompt, _) = system_prompt_for(&["-p", "hi", "-t", "read,grep"], "prompt-allowlist");
    let line = tool_line(&prompt);
    let names: Vec<&str> = line.split(',').map(str::trim).collect();
    assert_eq!(
        names,
        vec!["read", "grep"],
        "the tool line is exactly the allowlist:\n{prompt}"
    );
}

// Verifies: gh #271 - `--exclude-tools shell` never advertises `shell`, and the
// prompt matches the declaration.
//
// Named limitation: `--exclude-tools` **on its own** currently drops every
// built-in, not just the excluded one — `select_tools` starts the selection
// from `registry.active_tools()`, which holds only *registered extension*
// tools, so no built-in survives to be kept. That is a separate defect in the
// selection (not the prompt), it reproduces on unmodified `main`, and it is
// outside this brief's file fence; the assertion below is therefore about the
// contract this issue owns (no `shell`, prompt == declaration), not about how
// many tools survive.
#[test]
fn an_exclusion_never_advertises_the_excluded_tool() {
    let (prompt, bodies) =
        system_prompt_for(&["-p", "hi", "--exclude-tools", "shell"], "prompt-exclude");
    let line = tool_line(&prompt);
    let names: Vec<&str> = line
        .split(',')
        .map(|part| part.trim())
        .filter(|p| !p.is_empty())
        .collect();
    assert!(
        !names.contains(&"shell"),
        "the excluded tool is not advertised: {line}"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(bodies.lines().next().expect("body")).expect("JSON");
    let declared: Vec<String> = parsed["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !declared.iter().any(|tool| tool == "shell"),
        "the declaration agrees: {declared:?}"
    );
}

// Verifies: gh #271 - the prompt and the declaration never disagree. This is
// the real contract the issue is about, and it holds for every flag class, so
// it is asserted directly rather than inferred from one rendering.
#[test]
fn the_prompt_and_the_declaration_agree() {
    for (tag, args) in [
        ("agree-none", vec!["-p", "hi", "--no-tools"]),
        ("agree-allow", vec!["-p", "hi", "-t", "read,grep"]),
        (
            "agree-exclude",
            vec!["-p", "hi", "--exclude-tools", "shell"],
        ),
        ("agree-default", vec!["-p", "hi"]),
    ] {
        let (prompt, bodies) = system_prompt_for(&args, tag);
        let parsed: serde_json::Value =
            serde_json::from_str(bodies.lines().next().expect("one request line")).expect("JSON");
        let declared: Vec<String> = parsed["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let line = tool_line(&prompt);
        let advertised_names: Vec<&str> = line.split(',').map(str::trim).collect();
        for name in ["shell", "write", "edit"] {
            let advertised = advertised_names.contains(&name);
            let offered = declared.iter().any(|tool| tool == name);
            assert_eq!(
                advertised, offered,
                "`{name}`: the prompt says {advertised} but the request declares {offered}\n{prompt}"
            );
        }
    }
}

// The declaration and the prompt are both absent for `--no-builtin-tools` when
// no extension supplies tools: nothing is advertised, nothing is declared.
#[test]
fn no_builtin_tools_advertises_no_builtins() {
    let (prompt, _) = system_prompt_for(&["-p", "hi", "--no-builtin-tools"], "prompt-no-builtin");
    assert_eq!(
        tool_line(&prompt),
        "",
        "no built-in tool line under --no-builtin-tools:\n{prompt}"
    );
}
