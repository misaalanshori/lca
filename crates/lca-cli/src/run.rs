//! Split from `lib.rs` for the workspace file ceiling (gate 11,
//! #158). Behaviour unchanged.

use super::session_cmds::{list_models_command, route_with};
use super::session_maint::{gc_command, migrate_command};
use super::{
    auth, diagnostics, doctor, ext, invoke,
    session_cmds::{
        clone_command, config_command, delete_command, export_command, fork_command,
        rename_command, resume_list,
    },
};
use crate::{Cli, CliFlags, OutputMode, Route, check_flag_contradictions, exit, headless, rpc};

/// Dispatch a parsed command line; returns the process exit code.
pub async fn run(cli: Cli) -> i32 {
    diagnostics::init_crash_context(); // gh #81: before anything that can panic.
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::INTERNAL;
        }
    };
    // pi 1.0.0 parity: `--provider` exists to scope the `--model`
    // lookup, so it refuses to run without one instead of quietly doing
    // nothing - exit 2 (a usage error), naming the rule.
    if cli.provider.is_some() && cli.model.is_none() {
        eprintln!("error: --provider scopes the --model lookup; pass --model <pattern> with it");
        return exit::USAGE;
    }
    // #109/#111: contradictory flags are usage errors before routing.
    if let Some(err) = check_flag_contradictions(&cli) {
        eprintln!("error: {err}");
        return exit::USAGE;
    }
    let mut flags = CliFlags::from_cli(&cli);
    // gh #71: a probe routes first so subcommand stdin (consent,
    // callbacks) never feeds the draft; turns read when consumed.
    let (invocation, attach) = match invoke::prepare_turn_input(&cli, &cwd) {
        Ok(prepared) => prepared,
        Err(code) => return code,
    };
    if let Err(code) = invoke::prepare_resources(&cli, &cwd, &mut flags) {
        return code; // gh #70: skill packs merge, bad paths refuse.
    }
    // `--list-models` lists and exits (pi's "lists, then exits").
    if let Some(search) = cli.list_models.as_deref() {
        return list_models_command(&cwd, search, &flags, &cli.allow_host);
    }
    match route_with(&cli, &invocation) {
        Route::Headless {
            messages,
            model,
            session,
            mode,
        } => {
            // `--json` stays working as the deprecated alias for
            // `--mode json` (gh #56): flags are stable within a major.
            // Stderr, never stdout: scripts parse stdout as JSONL.
            if cli.json && cli.mode.is_none() {
                eprintln!("warning: --json is deprecated, use --mode json");
            }
            match mode {
                OutputMode::Rpc => {
                    rpc(
                        model.as_deref(),
                        &session,
                        &cwd,
                        cli.yolo,
                        &cli.allow_host,
                        &flags,
                    )
                    .await
                }
                OutputMode::Text | OutputMode::Json => {
                    headless(
                        &messages,
                        model.as_deref(),
                        &session,
                        mode == OutputMode::Json,
                        &cwd,
                        &attach,
                        cli.yolo,
                        &cli.allow_host,
                        &flags,
                    )
                    .await
                }
            }
        }
        Route::Interactive {
            resume,
            resume_picker,
            model,
            initial,
            fork,
            session_id,
        } => invoke::interactive(
            &cwd,
            resume.as_deref(),
            resume_picker,
            cli.yolo,
            model.as_deref(),
            &initial,
            &invocation.file_images,
            fork.as_deref(),
            session_id.as_deref(),
            &cli.allow_host,
            &flags,
        ),
        Route::Config {
            approve,
            no_approve,
        } => config_command(&cwd, approve, no_approve),
        Route::ResumeList => resume_list(&cwd),
        Route::Fork { session, message } => fork_command(&cwd, &session, &message),
        Route::Clone { session, title } => clone_command(&cwd, &session, title.as_deref()),
        Route::Rename { session, title } => rename_command(&cwd, &session, &title),
        Route::Delete { session } => delete_command(&cwd, &session),
        Route::Export {
            session,
            audit,
            html,
            output,
        } => export_command(&cwd, &session, audit, html, output.as_deref()),
        Route::Gc { session } => gc_command(&cwd, &session),
        Route::Migrate { session } => migrate_command(&cwd, &session),
        Route::Ext(cmd) => ext::run(cmd, flags.offline).await,
        Route::Auth(cmd) => auth::run(&cmd, &cli.allow_host),
        Route::Doctor => doctor::run(&crate::data_dir(), &cwd),
        Route::Update {
            check,
            extensions,
            models,
            all,
            force,
            changelog,
        } => {
            crate::update::run(crate::update::plan(
                check, extensions, models, all, force, changelog,
            ))
            .await
        }
    }
}
