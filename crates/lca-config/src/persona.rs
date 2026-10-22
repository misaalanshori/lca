//! `[personas]` - named bundles of model, thinking level, tool loadout, and
//! instructions (gh #412).
//!
//! A persona is **a bundle of existing settings, not a new setting**. It names a
//! model, a thinking level, a tool loadout, and optional instructions; applying it
//! writes those through the same paths `/model`, `/thinking`, and the tool gate
//! already use, so the footer, the thinking border ramp (gh #387 gap 4), and the
//! per-model thinking overrides stay in sync because there is nothing to keep in
//! sync - one owner per fact (the gh #369 lesson).
//!
//! # The word
//!
//! **Preset** is reserved for provider endpoint entries (`docs/glossary.md`): an
//! extension's `resources/provider-presets.toml`. This feature is `/persona` and
//! `[personas]` everywhere, because a persona is not an endpoint.
//!
//! # Why a table key and not three new scalars
//!
//! A persona is one thing with four fields; splitting it into `personas.plan.model`
//! and friends would let a file describe half a persona, and the whole contract is
//! that half a persona is never applied. One table, parsed as one value, validated
//! as one bundle, is what makes "atomic or nothing" structural rather than a
//! discipline someone has to remember.
//!
//! # What is NOT validated here
//!
//! Whether the model exists, whether the thinking level is one that model accepts,
//! and whether the tools are registered. Those are session facts the config layer
//! cannot see - a config file is read before any provider or extension is loaded.
//! This module validates **shape** (every field present, of the right type, a level
//! the vocabulary knows); the session validates **reachability** before applying
//! anything, which is where the atomic guarantee actually lives
//! (`crates/lca-ui/src/chat_persona.rs`).

use std::collections::BTreeMap;

/// One persona: a named bundle.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Persona {
    /// The model id this persona selects. Empty means "leave the model alone" -
    /// a persona may be thinking-only or loadout-only.
    pub model: String,
    /// The thinking level, or `None` to leave the current one alone. `""` in the
    /// file means the provider default, which is `Some(None)` here after parsing.
    pub thinking: Option<Option<String>>,
    /// Tool names to enable. `None` leaves the loadout alone; `Some(vec![])` means
    /// "no tools", which is a real and useful persona (a pure-chat persona).
    pub tools: Option<Vec<String>>,
    /// A short instruction prologue injected as a **conversational** message, never
    /// a `System` message (gh #299: a trailing System message mutates the root
    /// prompt and busts the cache).
    pub instructions: Option<String>,
}

impl Persona {
    /// One line naming everything this persona carries, for `/persona`'s listing.
    ///
    /// A bundle whose contents you cannot see is a bundle you cannot trust, so the
    /// listing and the applied notice both use this.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        parts.push(if self.model.is_empty() {
            "model unchanged".to_string()
        } else {
            self.model.clone()
        });
        match &self.thinking {
            None => parts.push("thinking unchanged".to_string()),
            Some(None) => parts.push("thinking provider default".to_string()),
            Some(Some(level)) => parts.push(level.clone()),
        }
        match &self.tools {
            None => parts.push("tools unchanged".to_string()),
            Some(tools) if tools.is_empty() => parts.push("no tools".to_string()),
            Some(tools) => parts.push(format!("tools {}", tools.join("+"))),
        }
        if self.instructions.is_some() {
            parts.push("instructions".to_string());
        }
        parts.join(" + ")
    }

    /// Whether this persona asks for a tool loadout at all.
    pub fn sets_tools(&self) -> bool {
        self.tools.is_some()
    }
}

/// Every persona the config files define, and where each came from.
///
/// Provenance matters here for one reason above all: a persona from an
/// **untrusted** project file must not apply (FR-PERM-9). Keeping the layer with
/// the persona is what lets the session decide that without re-reading anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Personas {
    /// persona name -> (its bundle, the source that defined it).
    entries: BTreeMap<String, (Persona, PersonaSource)>,
}

/// Which layer defined a persona, **in precedence order**: the later variant wins.
///
/// Declared low-to-high so `Ord` answers "which layer is stronger" directly, which
/// is what [`Personas::insert_layer`] needs. Getting this backwards is silent - the
/// project persona would simply never replace the user one - so the order is
/// asserted by `a_project_persona_replaces_a_user_persona_wholesale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PersonaSource {
    /// The user file in the config directory.
    UserFile,
    /// The project file, `.lca/config.toml` - applies only while trusted, and
    /// beats the user file (the documented ladder, FR-CFG-1).
    ProjectFile,
}

impl PersonaSource {
    /// Whether this layer's personas may apply in the current session.
    ///
    /// The project layer is a **config-driven behaviour switch** - it names a
    /// model, a tool loadout, and instructions - which is exactly what an
    /// untrusted file must not be able to set (FR-PERM-9). The user layer always
    /// applies: it is the user's own file.
    pub fn applies(self, project_trusted: bool) -> bool {
        match self {
            PersonaSource::UserFile => true,
            PersonaSource::ProjectFile => project_trusted,
        }
    }
}

impl Personas {
    /// Register one layer's personas.
    ///
    /// A **higher-precedence** layer replaces a same-named persona wholesale. It
    /// does not merge fields: a persona is a bundle, and a merged bundle would be a
    /// persona nobody wrote. Precedence follows the settings ladder - the project
    /// file beats the user file.
    pub fn insert_layer(&mut self, source: PersonaSource, layer: BTreeMap<String, Persona>) {
        for (name, persona) in layer {
            match self.entries.get(&name) {
                // Only a strictly higher layer may replace.
                Some((_, existing)) if *existing > source => {}
                _ => {
                    self.entries.insert(name, (persona, source));
                }
            }
        }
    }

    /// The persona named `name`, if a layer that applies in this session defined it.
    ///
    /// `None` covers both "no such persona" and "defined only in an untrusted
    /// project file"; [`Self::blocked`] tells the caller which, because the message
    /// the user gets differs.
    pub fn get(&self, name: &str, project_trusted: bool) -> Option<&Persona> {
        self.entries
            .get(name)
            .filter(|(_, source)| source.applies(project_trusted))
            .map(|(persona, _)| persona)
    }

    /// Whether `name` exists but is blocked because its layer is untrusted.
    pub fn blocked(&self, name: &str, project_trusted: bool) -> bool {
        self.entries
            .get(name)
            .is_some_and(|(_, source)| !source.applies(project_trusted))
    }

    /// Every persona that applies in this session, in name order, with its source.
    ///
    /// `/persona` lists these. A blocked persona is **omitted** rather than listed
    /// as unusable: it is not part of this session, and showing a name that cannot
    /// be applied invites the user to try it.
    pub fn available(&self, project_trusted: bool) -> Vec<(&str, &Persona, PersonaSource)> {
        self.entries
            .iter()
            .filter(|(_, (_, source))| source.applies(project_trusted))
            .map(|(name, (persona, source))| (name.as_str(), persona, *source))
            .collect()
    }

    /// How many personas exist in all layers, for a diagnostic that has to say
    /// "you defined N but none can apply".
    pub fn total(&self) -> usize {
        self.entries.len()
    }

    /// Whether any persona exists at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Parse the `[personas]` table from one config layer.
///
/// Shape only: each entry must be a table with a known field set, and a
/// `thinking` value must be in the documented vocabulary (or `""` for the
/// provider default). A misspelled field is an **error**, not a silent ignore -
/// `[personas.plan] modle = "x"` would otherwise apply a persona with no model and
/// look like the model switch failed.
pub fn parse_personas(value: &toml::Value) -> Result<BTreeMap<String, Persona>, String> {
    let table = value
        .as_table()
        .ok_or_else(|| format!("expected a table of personas, got {}", type_name(value)))?;
    let mut persona_by_name = BTreeMap::new();
    for (name, entry) in table {
        if name.trim().is_empty() {
            return Err("a persona needs a name".to_string());
        }
        let fields = entry.as_table().ok_or_else(|| {
            format!(
                "persona `{name}` must be a table of fields, got {}",
                type_name(entry)
            )
        })?;
        let mut persona = Persona::default();
        for (field, value) in fields {
            match field.as_str() {
                "model" => {
                    persona.model = value
                        .as_str()
                        .ok_or_else(|| format!("persona `{name}`: `model` must be a string"))?
                        .trim()
                        .to_string();
                }
                "thinking" => {
                    let level = value
                        .as_str()
                        .ok_or_else(|| format!("persona `{name}`: `thinking` must be a string"))?;
                    let level = level.trim();
                    if level.is_empty() {
                        // `""` is the provider default, which is a real choice and
                        // distinct from "leave it alone" (`None`).
                        persona.thinking = Some(None);
                    } else {
                        let canonical = crate::THINKING_LEVELS
                            .iter()
                            .find(|known| known.eq_ignore_ascii_case(level))
                            .ok_or_else(|| {
                                format!(
                                    "persona `{name}`: `thinking` = `{level}` is not a level; \
                                     accepted: {} (or \"\" for the provider default)",
                                    crate::THINKING_LEVELS.join(", ")
                                )
                            })?;
                        persona.thinking = Some(Some((*canonical).to_string()));
                    }
                }
                "tools" => {
                    let list = value.as_array().ok_or_else(|| {
                        format!("persona `{name}`: `tools` must be a list of tool names")
                    })?;
                    let mut tools = Vec::new();
                    for tool in list {
                        let tool = tool.as_str().ok_or_else(|| {
                            format!("persona `{name}`: every `tools` entry must be a string")
                        })?;
                        if tool.trim().is_empty() {
                            return Err(format!("persona `{name}`: an empty tool name"));
                        }
                        tools.push(tool.trim().to_string());
                    }
                    persona.tools = Some(tools);
                }
                "instructions" => {
                    persona.instructions = Some(
                        value
                            .as_str()
                            .ok_or_else(|| {
                                format!("persona `{name}`: `instructions` must be a string")
                            })?
                            .to_string(),
                    );
                }
                other => {
                    return Err(format!(
                        "persona `{name}`: unknown field `{other}`; accepted: model, thinking, \
                         tools, instructions"
                    ));
                }
            }
        }
        if persona == Persona::default() {
            return Err(format!(
                "persona `{name}` sets nothing; it needs at least one of model, thinking, \
                 tools, instructions"
            ));
        }
        persona_by_name.insert(name.clone(), persona);
    }
    Ok(persona_by_name)
}

fn type_name(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "a string",
        toml::Value::Integer(_) => "an integer",
        toml::Value::Float(_) => "a float",
        toml::Value::Boolean(_) => "a boolean",
        toml::Value::Datetime(_) => "a datetime",
        toml::Value::Array(_) => "a list",
        toml::Value::Table(_) => "a table",
    }
}
