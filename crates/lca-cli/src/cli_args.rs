//! The parsed command line: the clap types and their documentation,
//! split out of `lib.rs` for the file ceiling (gate 11). Everything is
//! re-exported from the crate root, so no call site moved.

use clap::{Parser, Subcommand};

use crate::ext;
use crate::version_static;

/// The parsed command line.
#[derive(Parser, Debug)]
#[command(
    name = "lca",
    version = version_static(),
    about = "A lightweight, cross-platform, extensible coding agent for the terminal"
)]
pub struct Cli {
    // FR-CORE-3: one turn, no interface.
    /// Run a single prompt and print the reply, without the interface.
    #[arg(short = 'p', long = "prompt", value_name = "PROMPT")]
    pub prompt: Option<String>,
    // ADR-0042: `permissions.mode = "yolo"`, said out loud.
    /// Approve every permission prompt automatically, recording each one
    /// like a human "always" answer. Explicit deny rules still deny.
    #[arg(long)]
    pub yolo: bool,
    // `docs/headless.md`: the JSON-lines envelope.
    /// Print one JSON object per line, for scripts.
    #[arg(long)]
    pub json: bool,
    // ADR-0029: attach an image to the one-shot turn.
    /// Attach an image file to the turn (repeat for several).
    #[arg(long = "attach", value_name = "PATH")]
    pub attach: Vec<std::path::PathBuf>,
    // RC-G (issue #6): continue previous session.
    /// Continue previous session.
    #[arg(short = 'c', long = "continue")]
    pub r#continue: bool,
    // RC-G (issue #6): alias to resume session by id.
    /// Resume session by ID.
    #[arg(short = 'r', long = "resume", value_name = "ID")]
    pub resume_id: Option<String>,
    // RC-G (issue #6) + gh #8 (EFG-041): pi's `--model <pattern>[:thinking]`.
    /// Model id or fuzzy pattern (`profile/id` also works), with an
    /// optional `:thinking` suffix such as `sonnet:high`.
    #[arg(long = "model", value_name = "PATTERN")]
    pub model: Option<String>,
    // gh #8 (EFG-003): pi's `--thinking`, a session default.
    /// The session's reasoning level; clamped to what the model offers
    /// (the `models.thinking_levels` map, phase 4).
    #[arg(
        long = "thinking",
        value_name = "LEVEL",
        value_parser = clap::builder::PossibleValuesParser::new(lca_config::THINKING_LEVELS)
    )]
    pub thinking: Option<String>,
    // Deliberate divergence from pi (gh #8's DNA box): pi also has
    // `--api-key <key>`, and LCA will not add one. argv is world-readable
    // in the process list (`ps`), and the standing rule is that secrets
    // never touch argv, scrollback, or history - `OPENAI_API_KEY` is the
    // documented path for a key a script needs to supply.
    // gh #8 (EFG-041, pi 1.0.0): `--provider` exists to scope `--model`.
    /// Restrict `--model` resolution to one profile (or the provider
    /// itself). Requires `--model`.
    #[arg(long = "provider", value_name = "NAME")]
    pub provider: Option<String>,
    // gh #8 (EFG-003): pi's `--list-models [search]`, the CI building block.
    /// Print the offered models as `id  provider  context` lines and exit
    /// (an optional pattern filters the list).
    #[arg(
        long = "list-models",
        value_name = "SEARCH",
        num_args = 0..=1,
        default_missing_value = ""
    )]
    pub list_models: Option<String>,
    // gh #8 (EFG-003): pi's `--models` - the enabled-model scope that the
    // picker's listing and the model cycle are both cut to.
    /// Restrict the model list and the model cycle to a comma-separated
    /// list of id patterns (a pattern may be an id, a substring, or a
    /// `*` glob; unset = every offered model).
    #[arg(long = "models", value_name = "PATTERNS")]
    pub models: Option<String>,
    // gh #29 (QA-004): the friction path for a scripted run.
    /// Allow an endpoint host for this run only, never persisted (repeatable).
    #[arg(long = "allow-host", value_name = "HOST", value_parser = crate::net_consent::parse_allow_host)]
    pub allow_host: Vec<String>,
    #[command(subcommand)]
    /// A subcommand, when one is present.
    pub command: Option<Command>,
}

/// The session and configuration subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    // FR-SESS-2: list or reopen.
    /// List this project's sessions, or reopen one by id.
    Resume {
        /// The session id to reopen; omit it to list.
        id: Option<String>,
    },
    // FR-SESS-3.
    /// Copy a session up to a message into a new session.
    Fork {
        /// The session to fork from.
        session: String,
        /// The record id to fork at.
        message: String,
    },
    /// Give a session a new title.
    Rename {
        /// The session to rename.
        session: String,
        /// The new title.
        title: String,
    },
    // FR-SESS-7, `docs/session-log-format.md`.
    /// Write a session out for sharing or inspection.
    Export {
        /// The session to export.
        session: String,
        /// Include permission and extension-event records.
        #[arg(long)]
        audit: bool,
    },
    // The session-maintenance subcommands (D5's attachment GC).
    /// Session maintenance.
    Session {
        /// What to do with the session.
        #[command(subcommand)]
        cmd: SessionCmd,
    },
    // FR-CFG-2.
    /// Show the merged configuration and where each value came from.
    Config,
    // The SRDD's command-line section, FR-DIST-*.
    /// Install, update, remove, and inspect extensions.
    Ext {
        /// What to do with extensions.
        #[command(subcommand)]
        cmd: ext::ExtCmd,
    },
}

/// `lca session ...`: maintenance that does not open the interface.
#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// Delete attachments that no resolved record list references.
    Gc {
        /// The session whose fork tree to sweep.
        session: String,
    },
}

/// The command line's configuration-backed values: the flags that ride
/// the flag layer of the configuration merge (FR-CFG-1) instead of being
/// read straight off the parsed command line, so `/settings` reports the
/// winning source for them like it does for every other key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliFlags {
    /// `--models`: the enabled-model scope (`models.enabled`).
    pub models: Option<String>,
    /// `--thinking`: the session's reasoning level (`thinking`).
    pub thinking: Option<String>,
    /// `--provider`: the resolution scope for `--model`. Not a config
    /// key: `provider` names the provider *extension*, while this names
    /// a profile inside one - two different questions.
    pub provider: Option<String>,
}

impl CliFlags {
    /// The values this command line carries into the flag layer.
    pub fn from_cli(cli: &Cli) -> CliFlags {
        CliFlags {
            models: cli.models.clone(),
            thinking: cli.thinking.clone(),
            provider: cli.provider.clone(),
        }
    }

    /// The flag layer as `lca_config::LoadInput` wants it.
    pub fn layer(&self) -> std::collections::BTreeMap<String, String> {
        let mut flags = std::collections::BTreeMap::new();
        if let Some(models) = &self.models {
            flags.insert("models.enabled".to_string(), models.clone());
        }
        if let Some(thinking) = &self.thinking {
            flags.insert("thinking".to_string(), thinking.clone());
        }
        flags
    }
}
