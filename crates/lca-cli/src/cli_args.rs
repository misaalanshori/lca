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
    // #109 (pi's `-p`/`--print`): a bare `-p` is print mode; `-p <text>`
    // carries the first message. The empty missing-value is the
    // documented edge: it contributes no message, so plain English
    // never collides with a sentinel (`-p ""` behaves like bare `-p`).
    /// Run the supplied prompts and print the replies, without the
    /// interface. Bare `-p` is print mode; `-p <text>` prepends a message.
    #[arg(
        short = 'p',
        long = "print",
        num_args = 0..=1,
        default_missing_value = "",
        value_name = "MESSAGE"
    )]
    pub print: Option<String>,
    // #109: the legacy spelling stays. Long-only now: `-p` belongs to
    // `--print`, and removing `--prompt` would break scripts for zero gain.
    /// Run a single prompt and print the reply, without the interface
    /// (legacy spelling; prefer `-p <text>` or positional messages).
    #[arg(long = "prompt", value_name = "PROMPT")]
    pub prompt: Option<String>,
    // #109 (pi's `messages` positional): with `-p`/`--print` these run
    // as headless turns in order; without it the TUI opens with the
    // first submitted. `@file` expansion is out of scope (it is #71's).
    /// Messages: headless turns in order with `-p`, else the TUI's first
    /// submission.
    #[arg(value_name = "MESSAGE")]
    pub messages: Vec<String>,
    // ADR-0042: `permissions.mode = "yolo"`, said out loud.
    /// Approve every permission prompt automatically, recording each one
    /// like a human "always" answer. Explicit deny rules still deny.
    #[arg(long)]
    pub yolo: bool,
    // gh #80 (pi's `-a`/`--approve`): trust the project folder for this
    // process only (ADR-0039 session semantics). Refused with `-na`.
    /// Trust this project for this run (session-scoped, never stored).
    #[arg(short = 'a', long = "approve")]
    pub approve: bool,
    // gh #80 (pi's `-na`/`--no-approve`): ignore trust-gated project
    // files for this process. Long-only with a `--na` alias: `-n`
    // already skips context files, and clap would read `-na` as the
    // `-n -a` cluster (the day `-na` meant approve-all plus skip-files
    // is a day nobody wants). Refused with `-a`.
    /// Treat this project as untrusted for this run, ignoring stored
    /// trust and project-local files.
    #[arg(long = "no-approve", visible_alias = "na")]
    pub no_approve: bool,
    // `docs/headless.md`: the JSON-lines envelope.
    /// Print one JSON object per line, for scripts (deprecated alias
    /// for `--mode json`; flags are stable within a major).
    #[arg(long)]
    pub json: bool,
    // gh #56 (pi's `--mode`): text prints the reply, json prints the
    // event stream, rpc starts the JSONL command loop on stdin/stdout.
    // Short is `-m`: `-M` reads as the models flag elsewhere... which
    // does not exist; `-m` is free and mirrors pi's own short.
    /// Output protocol: `text` prints the reply, `json` prints the
    /// event stream, `rpc` starts the stdin/stdout command loop.
    #[arg(long = "mode", value_name = "MODE", value_parser = clap::builder::PossibleValuesParser::new(["text", "json", "rpc"]))]
    pub mode: Option<String>,
    // #152: pi's `--verbose` debug shape. Diagnostics always land in
    // `~/.lca/logs/lca.log`; this flag also routes them to stderr.
    /// Print diagnostic logs to stderr as well as the log file.
    #[arg(long)]
    pub verbose: bool,
    // ADR-0029: attach an image to the one-shot turn.
    /// Attach an image file to the turn (repeat for several).
    #[arg(long = "attach", value_name = "PATH")]
    pub attach: Vec<std::path::PathBuf>,
    // RC-G (issue #6): continue previous session.
    /// Continue previous session.
    #[arg(short = 'c', long = "continue")]
    pub r#continue: bool,
    // RC-G (issue #6): alias to resume session by id; gh #110 adopts
    // pi's shape (bare `-r` browses, `-r <id>` resumes): `None` is
    // absent, `Some(None)` is the bare picker, `Some(Some(_))` an id.
    /// Browse sessions in a picker, or resume one by ID.
    #[arg(short = 'r', long = "resume", value_name = "ID", num_args = 0..=1)]
    pub resume_id: Option<Option<String>>,
    // gh #110: pi's `--session <path|id>` for a direct resume.
    /// Resume a session directly, by ID or by session-directory path.
    #[arg(long = "session", value_name = "ID|PATH")]
    pub session: Option<String>,
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
    // gh #68 (pi's `--system-prompt`): a file whose content replaces
    // the built-in prompt for this run only.
    /// Replace the system prompt with a file's content, for this run.
    #[arg(long = "system-prompt", value_name = "PATH")]
    pub system_prompt: Option<std::path::PathBuf>,
    // gh #68 (pi's `--append-system-prompt`): instructions appended for
    // this run only.
    /// Append a file's content to the system prompt, for this run.
    #[arg(long = "append-system-prompt", value_name = "PATH")]
    pub append_system_prompt: Option<std::path::PathBuf>,
    // gh #74 (pi's `-nc`): skip AGENTS.md/CLAUDE.md discovery.
    /// Skip AGENTS.md and CLAUDE.md discovery for this run.
    // Short is `-n` only: `-c` already means `--continue`, so pi's
    // `-nc` cluster would resume a session as a side effect. Spell it
    // `-n` or `--no-context-files`.
    #[arg(short = 'n', long = "no-context-files")]
    pub no_context_files: bool,
    // gh #71 (pi's `--offline`): no automatic network activity for this
    // run - update checks, catalog refreshes, remote installs. Model
    // requests still go out (that is the run); `LCA_OFFLINE=1` agrees.
    /// Disable automatic network activity for this run.
    #[arg(long)]
    pub offline: bool,
    // gh #67 (pi's `-t`: allowlist; plain names replace, `+`/`-`
    // deltas modify, `*` globs).
    /// Run with exactly these tools (built-in, extension, or custom).
    #[arg(short = 't', long = "tools", value_name = "LIST")]
    pub tools: Option<String>,
    // gh #67 (pi's `-xt`): patterns disabled after everything else.
    /// Disable these tools, after all other selection.
    #[arg(long = "exclude-tools", value_name = "LIST", visible_alias = "xt")]
    pub exclude_tools: Option<String>,
    // gh #67 (pi's `-nbt`): default built-ins off, extensions stay.
    /// Disable the built-in tools, keeping extension tools.
    #[arg(long = "no-builtin-tools", visible_alias = "nbt")]
    pub no_builtin_tools: bool,
    // gh #67 (pi's `-nt`): every tool starts disabled.
    /// Start with every tool disabled.
    #[arg(long = "no-tools", visible_alias = "nt")]
    pub no_tools: bool,
    // gh #69 (pi's `--session-id`): the exact project session id, or a
    // fresh session created under it when absent.
    /// Open the exact session id, creating it when absent.
    #[arg(long = "session-id", value_name = "ID")]
    pub session_id: Option<String>,
    // gh #69 (pi's `--no-session`): a volatile run that persists
    // nothing - sessions live in a temp dir dropped at exit. Grant
    // decisions still persist (trust is not session state).
    /// Run without persisting any session.
    #[arg(long = "no-session")]
    pub no_session: bool,
    // gh #69 (pi's `-n/--name`): `-n` already skips context files, so
    // the short stays there and the name is long-only.
    /// Name the run's session (fresh sessions start titled; resumed
    /// ones rename).
    #[arg(long = "name", value_name = "NAME")]
    pub name: Option<String>,
    // gh #69 (pi's `--fork`): fork an existing session at its tip into
    // this project and run it. Exclusive with the other selectors,
    // combinable with `--session-id` (the fork's id) and `--name`.
    /// Fork a session at its tip and run the fork.
    #[arg(long = "fork", value_name = "ID")]
    pub fork: Option<String>,
    // gh #70 (pi's `-e`): repeatable one-run extension paths (WASM
    // components; directories contribute their skills, like data-only
    // packages). Installed extensions still load unless
    // `--no-extensions`.
    /// Load an extension file for this run (repeatable).
    #[arg(short = 'e', long = "extension", value_name = "PATH")]
    pub extension: Vec<std::path::PathBuf>,
    // gh #70 (pi's `--no-extensions`): installed, configured, and
    // built-in extensions stay unloaded - but the providers must still
    // resolve, or the run cannot start at all (LCA providers ARE
    // extensions, documented divergence). Explicit `-e` still loads.
    /// Skip installed and built-in extensions for this run.
    #[arg(long = "no-extensions")]
    pub no_extensions: bool,
    // gh #70 (pi's `--skill`): repeatable skill files or directories.
    /// Load a skill file or directory for this run (repeatable).
    #[arg(long = "skill", value_name = "PATH")]
    pub skill: Vec<std::path::PathBuf>,
    // gh #70 (pi's `--no-skills`, long-only: `-n` already skips
    // context files, so pi's `-ns` cluster has no single-char home).
    /// Skip discovered and configured skills for this run.
    #[arg(long = "no-skills")]
    pub no_skills: bool,
    // gh #70 (pi's `--theme`): repeatable theme files or directories -
    // they join the picker's pool for this run.
    /// Load a theme file or directory for this run (repeatable).
    #[arg(long = "theme", value_name = "PATH")]
    pub theme: Vec<std::path::PathBuf>,
    // gh #70 (pi's `--no-themes`): the pool is built-ins plus explicit
    // `--theme` paths.
    /// Skip discovered and configured themes for this run.
    #[arg(long = "no-themes")]
    pub no_themes: bool,
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
    // gh #205.
    /// Duplicate a session at its tip into a new session.
    Clone {
        /// The session to clone from.
        session: String,
        /// Title for the clone (defaults to `Clone of <parent-title>`).
        title: Option<String>,
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
    // gh #72 (pi's credential commands): probe and manage provider
    // credentials without opening the interface.
    /// Check, log in, and log out without opening the interface.
    Auth {
        /// What to do with the credentials.
        #[command(subcommand)]
        cmd: AuthCmd,
    },
    // The SRDD's command-line section, FR-DIST-*.
    /// Install, update, remove, and inspect extensions.
    Ext {
        /// What to do with extensions.
        #[command(subcommand)]
        cmd: ext::ExtCmd,
    },
    // gh #81 (pi's diagnostics row).
    /// Print version, paths, extensions, config, sessions, crashes.
    Doctor,
}

/// `lca auth ...`: pi's credential commands, minus the printers (gh
/// #72). `lca` never prints credentials: tokens in scrollback or
/// history is the secrets law, and no parity flag overrides it.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum AuthCmd {
    /// Probe credential state: print `ready`, `not_ready`, or
    /// `invalid`; exit 0, 1, or 2, respectively. Requires
    /// `--provider` or `--model`, like pi.
    Check {
        /// Resolve credentials for a provider.
        #[arg(long, value_name = "PROVIDER")]
        provider: Option<String>,
        /// Resolve credentials from a model id or pattern.
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
        /// Write the structured result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Sign in non-interactively where the provider allows it: an
    /// API key resolves from the environment, an OAuth flow prints
    /// its authorization URL and reads the pasted callback from stdin.
    Login {
        /// The provider to sign in to.
        #[arg(long, value_name = "PROVIDER")]
        provider: String,
    },
    /// Sign out: revoke where the endpoint supports it, then clear
    /// the stored credential.
    Logout {
        /// The provider to sign out of.
        #[arg(long, value_name = "PROVIDER")]
        provider: String,
    },
    /// Refused: `lca` never prints credentials (see above).
    #[command(hide = true)]
    PrintApiKey {
        /// Accepted and ignored: the refusal names the provider.
        #[arg(long, value_name = "PROVIDER")]
        provider: Option<String>,
    },
    /// Refused: `lca` never prints credentials (see above).
    #[command(hide = true)]
    PrintBearerToken {
        /// Accepted and ignored: the refusal names the provider.
        #[arg(long, value_name = "PROVIDER")]
        provider: Option<String>,
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
    /// `--system-prompt`: replace the prompt for this run (gh #68).
    pub system_prompt: Option<std::path::PathBuf>,
    /// `--append-system-prompt`: append for this run (gh #68).
    pub append_system_prompt: Option<std::path::PathBuf>,
    /// `-n`/`--no-context-files`: skip context-file discovery (gh #74).
    pub no_context_files: bool,
    /// `--offline` (gh #71): no automatic network activity. Not a config
    /// key: it joins the flag, the `LCA_OFFLINE` env var, or nothing -
    /// never the files (an air gap is per-invocation, not stored).
    pub offline: bool,
    /// `-a`/`--approve` (gh #80): trust this project for the run. Not a
    /// config key: session trust is per-invocation, never the files.
    pub approve: bool,
    /// `--no-approve` (gh #80): treat this project as untrusted for the
    /// run. Not a config key, same reason.
    pub no_approve: bool,
    /// `--tools` (gh #67): the run's tool allowlist. Run-scoped like
    /// `--provider`, never the files.
    pub tools: Option<String>,
    /// `--exclude-tools` (gh #67): patterns disabled last. Run-scoped.
    pub exclude_tools: Option<String>,
    /// `--no-builtin-tools` (gh #67): executor tools off, extensions
    /// stay. Run-scoped.
    pub no_builtin_tools: bool,
    /// `--no-tools` (gh #67): every tool starts disabled. Run-scoped.
    pub no_tools: bool,
    /// `--name` (gh #69): the run's display name. Run-scoped cosmetic,
    /// never the files.
    pub name: Option<String>,
    /// `--no-session` (gh #69): volatile sessions. Run-scoped.
    pub no_session: bool,
    /// `--extension` (gh #70): one-run extension paths. Run-scoped.
    pub extension: Vec<std::path::PathBuf>,
    /// `--no-extensions` (gh #70): skip installed extensions. Run-scoped.
    pub no_extensions: bool,
    /// `--skill` (gh #70): one-run skill paths. Run-scoped.
    pub skill: Vec<std::path::PathBuf>,
    /// `--no-skills` (gh #70): skip discovered skills. Run-scoped.
    pub no_skills: bool,
    /// `--theme` (gh #70): one-run theme paths. Run-scoped.
    pub theme: Vec<std::path::PathBuf>,
    /// `--no-themes` (gh #70): skip discovered themes. Run-scoped.
    pub no_themes: bool,
}

impl CliFlags {
    /// The values this command line carries into the flag layer.
    pub fn from_cli(cli: &Cli) -> CliFlags {
        CliFlags {
            models: cli.models.clone(),
            thinking: cli.thinking.clone(),
            provider: cli.provider.clone(),
            system_prompt: cli.system_prompt.clone(),
            append_system_prompt: cli.append_system_prompt.clone(),
            no_context_files: cli.no_context_files,
            offline: cli.offline || crate::invoke::offline_env(),
            approve: cli.approve,
            no_approve: cli.no_approve,
            tools: cli.tools.clone(),
            exclude_tools: cli.exclude_tools.clone(),
            no_builtin_tools: cli.no_builtin_tools,
            no_tools: cli.no_tools,
            name: cli.name.clone(),
            no_session: cli.no_session,
            extension: cli.extension.clone(),
            no_extensions: cli.no_extensions,
            skill: cli.skill.clone(),
            no_skills: cli.no_skills,
            theme: cli.theme.clone(),
            no_themes: cli.no_themes,
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
