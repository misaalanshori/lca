//! `/persona` - one command sets model, thinking level, tool loadout, and
//! instructions (gh #412).
//!
//! # Atomic, or nothing
//!
//! The order is the guarantee: the **whole** bundle is validated against this
//! session first (does the model exist? does that model accept that thinking level?
//! is every tool registered?), and only then is any of it written. A persona whose
//! model resolves but whose thinking level does not applies **nothing** - not the
//! model, not the loadout, not the instructions.
//!
//! That is worth the extra pass because the failure it prevents is silent: a
//! half-applied persona leaves the user believing they are in a persona they are
//! not in, and nothing on screen would tell them otherwise.
//!
//! # A bundle of existing settings
//!
//! Applying writes through the same seams `/model`, `/thinking`, and the tool gate
//! already use, so the footer, the thinking border ramp (gh #387 gap 4) and the
//! per-model thinking overrides are in sync by construction. There is deliberately
//! **no** "current persona" state cell: a cell that could disagree with the three
//! settings it describes is the gh #369 lesson (one owner per fact) unlearned.
//!
//! # The word
//!
//! **Preset** is reserved for provider endpoint entries (`docs/glossary.md`). This
//! is `/persona` and `[personas]` everywhere.

use super::chat::Chat;
use crate::state::PersonaBundle;

impl Chat {
    /// `/persona` or `/persona <name>`.
    pub(super) fn dispatch_persona(&mut self, argument: &str) {
        let name = argument.trim();
        if name.is_empty() {
            self.list_personas();
            return;
        }
        let Some(list) = self.world.options.hooks.persona_list.clone() else {
            self.say("this host cannot read personas".to_string());
            return;
        };
        let available = list();
        let Some(bundle) = available
            .iter()
            .find(|bundle| bundle.name.eq_ignore_ascii_case(name))
        else {
            self.say(crate::state::sanitize_text(&format!(
                "no persona named `{name}`{}",
                if available.is_empty() {
                    "; define one under [personas] in your config".to_string()
                } else {
                    format!(
                        "; available: {}",
                        available
                            .iter()
                            .map(|bundle| bundle.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            )));
            return;
        };
        let bundle = bundle.clone();

        // The whole bundle is checked before anything is written. A host without
        // the seam cannot know whether the fields are reachable, so it refuses
        // rather than applying blind - the atomic guarantee is worthless if it is
        // skipped exactly when it cannot be verified.
        let Some(check) = self.world.options.hooks.persona_check.clone() else {
            self.say("this host cannot validate a persona; nothing was changed".to_string());
            return;
        };
        if let Err(reason) = check(&bundle) {
            self.say(crate::state::sanitize_text(&format!(
                "persona `{}` was not applied: {reason}",
                bundle.name
            )));
            return;
        }
        let Some(apply) = self.world.options.hooks.persona_apply.clone() else {
            self.say("this host cannot apply a persona; nothing was changed".to_string());
            return;
        };
        let notice = apply(&bundle);
        // The instructions ride as a **conversational** message, never a `System`
        // one (gh #299: a trailing System message mutates the root prompt and busts
        // the cache). Attribution is part of the text, so the model can tell a
        // persona's framing from the user's own words.
        //
        // Emitted **before** the applied notice on purpose: found by this cycle's
        // own row, where the instructions' line took the notice's place and the user
        // never saw what had been applied. The last line a reader sees should be the
        // answer to the command they typed.
        if let Some(instructions) = bundle.instructions.clone() {
            self.say(format!("persona `{}`: {instructions}", bundle.name));
        }
        self.say(crate::state::sanitize_block(&notice));
    }

    /// The `/persona` listing: what each persona carries.
    ///
    /// A bundle whose contents you cannot see is a bundle you cannot trust, so the
    /// listing shows the model, the level, the loadout and whether instructions
    /// ride along - the same facts the applied notice names.
    fn list_personas(&mut self) {
        let Some(list) = self.world.options.hooks.persona_list.clone() else {
            self.say("this host cannot read personas".to_string());
            return;
        };
        let personas = list();
        if personas.is_empty() {
            self.say("no personas defined; add [personas.<name>] to your config".to_string());
            return;
        }
        let mut text = String::from("personas:\n");
        for bundle in personas {
            text.push_str("  ");
            text.push_str(&bundle.name);
            text.push_str(" - ");
            text.push_str(&summary(&bundle));
            text.push('\n');
        }
        text.push_str("run /persona <name> to apply one");
        self.push_output(text);
    }
}

/// One line naming everything a persona carries.
///
/// Mirrors `lca_config::Persona::summary`, deliberately: the interface cannot
/// depend on the config crate (it owns no config), and the two must say the same
/// thing. The regression guard pins that they do.
pub fn summary(bundle: &PersonaBundle) -> String {
    let mut parts = Vec::new();
    parts.push(if bundle.model.is_empty() {
        "model unchanged".to_string()
    } else {
        bundle.model.clone()
    });
    match &bundle.thinking {
        None => parts.push("thinking unchanged".to_string()),
        Some(None) => parts.push("thinking provider default".to_string()),
        Some(Some(level)) => parts.push(level.clone()),
    }
    match &bundle.tools {
        None => parts.push("tools unchanged".to_string()),
        Some(tools) if tools.is_empty() => parts.push("no tools".to_string()),
        Some(tools) => parts.push(format!("tools {}", tools.join("+"))),
    }
    if bundle.instructions.is_some() {
        parts.push("instructions".to_string());
    }
    parts.join(" + ")
}
