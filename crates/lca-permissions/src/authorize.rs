//! The authorize flow (FR-TOOL-3's approval path): proposals,
//! the store, the sensitive escalation, then the prompt. Split from
//! `lib.rs` under the workspace's 1,200-line file ceiling.

use std::path::Path;

use crate::{
    Action, Decision, Error, GrantStore, Outcome, PermissionMode, PermissionPrompt, Proposals,
    RuleMatch, sensitive,
};

/// Run the authorize flow for one action: review a changed proposal set
/// first (FR-PERM-10), then consult the store, then prompt if needed
/// (FR-TOOL-3's approval path; a hook denial is upstream of this and never
/// reaches the prompt, FR-CORE-10).
pub fn authorize(
    store: &mut GrantStore,
    project_dir: &Path,
    action: &Action,
    proposals: Option<&Proposals>,
    prompt: &mut dyn PermissionPrompt,
    annotations: Option<&lca_protocol::ToolAnnotations>,
) -> Result<Outcome, Error> {
    let mut reviewed = false;
    if let Some(proposals) = proposals {
        let diff = store.proposal_diff(project_dir, proposals);
        if !diff.is_empty() {
            reviewed = true;
            if prompt.review_proposals(&diff) {
                store.apply_proposals(project_dir, proposals)?;
            }
        }
    }

    if store.is_allowed(project_dir, action) {
        // Gh #258: the pipeline path, cheapest first (rule, trust,
        // or stored pattern — the detail names the action shape).
        let via = if store.rule_decision(project_dir, action) == RuleMatch::Allow {
            "rule.allow"
        } else if store.is_trusted_here(project_dir) {
            "trust.allow"
        } else {
            "pattern.allow"
        };
        lca_debug::permissions::event(via, &action.display());
        return Ok(Outcome {
            allowed: true,
            prompted: false,
            reviewed,
            stored_pattern: None,
            denied_by_rule: false,
            yolo: false,
        });
    }
    // A deny rule refuses without prompting (permission-UX plan §3.2; and
    // in yolo mode too: the user's own words outrank a mode that answers
    // prompts, ADR-0042).
    if store.rule_denied(project_dir, action) {
        lca_debug::permissions::event("rule.deny", &action.display());
        return Ok(Outcome {
            allowed: false,
            prompted: false,
            reviewed,
            stored_pattern: None,
            denied_by_rule: true,
            yolo: false,
        });
    }

    // R3's fatigue cut: a read outside the workspace never prompts. The
    // model needs to find its bearings, and a deny rule has already had its
    // say above. Nothing is recorded: no user decision was made. Gh #215:
    // sensitive filenames skip the cut — credential reads prompt even
    // in-workspace, falling through to yolo or the modal below.
    if matches!(action, Action::ReadPath { path } if !sensitive::is_sensitive(path)) {
        lca_debug::permissions::event("read.cut", &action.display());
        return Ok(Outcome {
            allowed: true,
            prompted: false,
            reviewed,
            stored_pattern: None,
            denied_by_rule: false,
            yolo: false,
        });
    }
    // Gh #258: the escalation itself is the event (the prompt's
    // answer lands below as a prompt.* event).
    if matches!(action, Action::ReadPath { .. }) {
        lca_debug::permissions::event("sensitive.ask", &action.display());
    }

    // ADR-0042: yolo answers the remaining prompts as "always, for this
    // exact pattern" and persists the pattern the same way a human answer
    // does, so the grant store and the session log both read as if the user
    // had approved each action. `yolo: true` tells the caller to record it.
    if store.permission_mode() == PermissionMode::Yolo {
        let pattern = action.suggested_pattern();
        // Gh #215: yolo auto-approves sensitive reads like any other
        // prompt, but the approval is loud in the log — credentials
        // reaching a provider on a mode flag deserve a paper trail.
        if matches!(action, Action::ReadPath { path } if sensitive::is_sensitive(path)) {
            tracing::warn!(
                "yolo auto-approved a sensitive file read: {}",
                action.display()
            );
        }
        match action {
            Action::Net { .. } => store.approve_net_pattern(project_dir, &pattern)?,
            _ => store.approve_pattern(project_dir, pattern.clone())?,
        }
        lca_debug::permissions::event(
            "yolo.approve",
            &format!("{} pattern={pattern}", action.display()),
        );
        return Ok(Outcome {
            allowed: true,
            prompted: false,
            reviewed,
            stored_pattern: Some(pattern),
            denied_by_rule: false,
            yolo: true,
        });
    }

    // Gh #258: the prompt's answer, whatever it was — the one event
    // per evaluation that always fires when a human (or yolo) decides.
    let outcome = match prompt.ask_annotated(action, annotations) {
        Decision::Denied => Ok(Outcome {
            allowed: false,
            prompted: true,
            reviewed,
            stored_pattern: None,
            denied_by_rule: false,
            yolo: false,
        }),
        Decision::Once => {
            // A host consent's `once` is a *session* allowance (gh #31
            // review): host requests are per call, so a one-call answer
            // would re-prompt on every turn and leave the work it was
            // given for denied - and persisting would make `once`
            // identical to `always`. It joins the same session set
            // `--allow-host` uses: this run only, a fresh process asks
            // again. Every other action's `once` stays what it was -
            // this call only.
            if let Action::Net { host } = action {
                store.attach_session_net_pattern(project_dir, host)?;
            }
            Ok(Outcome {
                allowed: true,
                prompted: true,
                reviewed,
                stored_pattern: None,
                denied_by_rule: false,
                yolo: false,
            })
        }
        Decision::TrustFolder => {
            store.trust_for_session(project_dir);
            Ok(Outcome {
                allowed: true,
                prompted: true,
                reviewed,
                stored_pattern: None,
                denied_by_rule: false,
                yolo: false,
            })
        }
        Decision::Always => {
            let pattern = action.suggested_pattern();
            // A `net` approval goes into the host vocabulary (ADR-0022),
            // everything else into the shell/path patterns set.
            match action {
                Action::Net { .. } => store.approve_net_pattern(project_dir, &pattern)?,
                _ => store.approve_pattern(project_dir, pattern.clone())?,
            }
            Ok(Outcome {
                allowed: true,
                prompted: true,
                reviewed,
                stored_pattern: Some(pattern),
                denied_by_rule: false,
                yolo: false,
            })
        }
    };
    if let Ok(done) = &outcome {
        lca_debug::permissions::event(
            if done.allowed {
                "prompt.allow"
            } else {
                "prompt.deny"
            },
            &action.display(),
        );
    }
    outcome
}
