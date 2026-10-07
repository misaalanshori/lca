//! `lca auth ...`: pi's credential commands without the printers (gh
//! #72, #38).
//!
//! `check` probes credential state with pi's exit table (0/1/2 =
//! ready/not_ready/invalid) and pi's `--json` shape; `login` signs in
//! non-interactively where the provider allows it (an API key resolves
//! from the environment, an OAuth flow prints its authorization URL and
//! reads the pasted callback from stdin — pi's remote/headless shape);
//! `logout` runs the provider's own logout. The credential printers
//! (`print-api-key`, `print-bearer-token`) are refused: `lca` never
//! prints credentials — tokens in scrollback or history is the secrets
//! law, and no parity flag overrides it.
//!
//! Nothing here interprets provider-shaped data. The probe composes the
//! identity exports every provider already ships: `usage` answering
//! means ready; `usage` failing means the provider reports itself
//! unusable (invalid); `usage` unsupported means the provider cannot
//! self-probe, so an API-key login attempt decides (its success means a
//! key resolved, its failure means none is configured). An OAuth
//! provider that never logged in therefore reads `invalid` rather than
//! pi's `not_ready` — the surface cannot tell "never stored" from
//! "stored but broken" without matching on the provider's own message,
//! and the fix is the same either way: log in.

use std::sync::Arc;

use super::*;
use lca_ext_abi::ExtensionDispatch;
use lca_ext_abi::World;
use lca_protocol::{DispatchError, IdentityOutcome, LoginOption};
use lca_provider::Provider as _;

/// pi's `AuthCheckStatus` (`docs/cli.md#credential-commands`).
/// Shared with the `/model` catalog (gh #177): readiness there is this
/// probe, not mere installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Ready,
    NotReady,
    Invalid,
}

impl Status {
    /// The word on stdout, pi's spelling.
    fn word(self) -> &'static str {
        match self {
            Status::Ready => "ready",
            Status::NotReady => "not_ready",
            Status::Invalid => "invalid",
        }
    }

    /// pi's exit table: 0, 1, 2.
    fn exit(self) -> i32 {
        match self {
            Status::Ready => exit::OK,
            Status::NotReady => exit::INTERNAL,
            Status::Invalid => exit::USAGE,
        }
    }
}

/// What `check` probed, in pi's `--json` shape (`reason` and `authType`
/// ride only when they say something).
struct Report {
    status: Status,
    provider: String,
    reason: Option<&'static str>,
    auth_type: Option<&'static str>,
}

impl Report {
    fn json(&self) -> String {
        let mut out = serde_json::Map::new();
        out.insert(
            "status".to_string(),
            serde_json::Value::String(self.status.word().to_string()),
        );
        out.insert(
            "provider".to_string(),
            serde_json::Value::String(self.provider.clone()),
        );
        if let Some(reason) = self.reason {
            out.insert(
                "reason".to_string(),
                serde_json::Value::String(reason.to_string()),
            );
        }
        if let Some(auth_type) = self.auth_type {
            out.insert(
                "authType".to_string(),
                serde_json::Value::String(auth_type.to_string()),
            );
        }
        serde_json::Value::Object(out).to_string()
    }
}

/// pi's `AuthCheckReason`, the three this command can produce (the
/// fourth, `credential_not_available`, belongs to the refused
/// `--credentials` flag).
fn auth_type_of(options: &[LoginOption]) -> Option<&'static str> {
    if options.iter().any(|option| option.kind == "oauth") {
        Some("oauth")
    } else if options.iter().any(|option| option.kind == "api-key") {
        Some("api_key")
    } else {
        None
    }
}

/// The answers one `check` collected, in probe order.
struct Probe {
    /// `usage`'s answer: `Ok` is healthy, `Failed` is provider-reported
    /// unusable, `NotSupported` is no self-probe.
    usage: Result<(), IdentityOutcome>,
    /// The provider's login options (for the auth type, and to tell an
    /// API-key login attempt from an OAuth flow a check must not start).
    options: Vec<LoginOption>,
    /// The fallback login attempt, `Some` only when it ran: no OAuth
    /// option on offer and no self-probe, so resolving a key is
    /// side-effect free (nothing to launch, nothing to refresh).
    login: Option<Result<(), ()>>,
}

/// Classify one probe into pi's table. Pure over the answers, so the
/// exit table is pinned without a provider in the room.
fn classify(provider: &str, probe: Probe) -> Report {
    let auth_type = auth_type_of(&probe.options);
    match probe.usage {
        Ok(()) => Report {
            status: Status::Ready,
            provider: provider.to_string(),
            reason: None,
            auth_type,
        },
        Err(IdentityOutcome::Failed(_)) => Report {
            status: Status::Invalid,
            provider: provider.to_string(),
            reason: Some("invalid_state"),
            auth_type,
        },
        Err(_) => {
            let ready = probe.options.iter().any(|option| option.kind == "oauth")
                || probe.options.is_empty();
            if ready {
                // An OAuth flow (or nothing to attempt with): a check
                // launches no browser and no paste prompt, so without a
                // self-probe there is nothing to establish.
                Report {
                    status: Status::NotReady,
                    provider: provider.to_string(),
                    reason: Some("credentials_not_configured"),
                    auth_type,
                }
            } else {
                match probe.login {
                    Some(Ok(())) => Report {
                        status: Status::Ready,
                        provider: provider.to_string(),
                        reason: None,
                        auth_type,
                    },
                    _ => Report {
                        status: Status::NotReady,
                        provider: provider.to_string(),
                        reason: Some("credentials_not_configured"),
                        auth_type,
                    },
                }
            }
        }
    }
}

/// One line of the headless OAuth exchange with the user.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    /// Nothing typed yet; keep polling the login thread.
    Pending,
    /// A line was pasted.
    Text(String),
    /// stdin closed: the user walked away.
    Closed,
}

/// Run one `auth` subcommand; the process exit code.
pub(super) fn run(cmd: &AuthCmd, allow_host: &[String]) -> i32 {
    if matches!(
        cmd,
        AuthCmd::PrintApiKey { .. } | AuthCmd::PrintBearerToken { .. }
    ) {
        eprintln!(
            "error: lca never prints credentials; tokens in scrollback or history \
             would outlive the command that printed them"
        );
        return exit::INTERNAL;
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::INTERNAL;
        }
    };
    let data = data_dir();
    let grants = match lca_permissions::GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => Arc::new(std::sync::Mutex::new(grants)),
        Err(err) => {
            eprintln!("error: cannot open the grant store: {err}");
            return exit::INTERNAL;
        }
    };
    // Session-less like `--list-models`: the flag layer still applies,
    // `--allow-host` grants this run only, and nothing prompts.
    let flags = CliFlags::default();
    let config = match load_config_flags(&cwd, &lock(&grants), true, false, &flags) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    };
    for host in allow_host {
        if let Err(err) = lock(&grants).attach_session_net_pattern(&cwd, host) {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    }
    let prompt = HeadlessPrompt::default();
    let shared_prompt = lca_permissions::SharedPrompt::default();
    shared_prompt.set(Arc::new(std::sync::Mutex::new(prompt.clone())));
    let stats: lca_ext_native::StatsSource = Arc::new(String::new);
    let registry = crate::registry::assemble(&cwd, &config, shared_prompt, &grants, stats);
    match cmd {
        AuthCmd::Check {
            provider,
            model,
            json,
        } => check(
            &registry,
            &config,
            provider.as_deref(),
            model.as_deref(),
            *json,
        ),
        AuthCmd::Login { provider } => login(&registry, provider),
        AuthCmd::Logout { provider } => logout(&registry, provider),
        AuthCmd::PrintApiKey { .. } | AuthCmd::PrintBearerToken { .. } => exit::INTERNAL,
    }
}

/// Resolve the check's target: `--provider` names an extension, `--model`
/// resolves through the first provider listing it (pi's model runtime in
/// one vocabulary). Neither is pi's usage error; an unresolvable model
/// is pi's `invalid`, the bucket a throwing `checkAuth` lands in.
fn resolve_target(
    registry: &lca_core::ExtensionRegistry,
    config: &lca_config::Config,
    provider: Option<&str>,
    model: Option<&str>,
    json: bool,
) -> Result<(String, Arc<dyn ExtensionDispatch>), i32> {
    if let Some(name) = provider {
        return registry
            .provider(name)
            .cloned()
            .map(|handle| (name.to_string(), handle))
            .ok_or_else(|| {
                emit(
                    json,
                    &Report {
                        status: Status::NotReady,
                        provider: name.to_string(),
                        reason: Some("provider_not_found"),
                        auth_type: None,
                    },
                )
            });
    }
    if let Some(pattern) = model {
        for handle in registry.enabled() {
            if !handle.worlds().contains(&World::Provider) {
                continue;
            }
            let provider = lca_core::ExtensionProvider::new(handle.clone());
            let models =
                crate::models::filter_enabled(provider.list_models(), config.models_enabled());
            // Strict only: `resolve_pattern` passes unknown patterns
            // through (scope mode), so membership decides. A pattern
            // that names no listed model is not this provider's.
            if let Ok(resolved) = crate::models::resolve_pattern(pattern, &models)
                && models.iter().any(|model| model.id == resolved.id)
            {
                let name = handle.name().to_string();
                return Ok((name, handle.clone()));
            }
        }
        let report = Report {
            status: Status::Invalid,
            provider: pattern.to_string(),
            reason: Some("invalid_state"),
            auth_type: None,
        };
        return Err(emit(json, &report));
    }
    eprintln!("error: auth checks require --provider <provider> or --model <model>");
    Err(exit::USAGE)
}

/// Print one report on stdout (the word, or pi's JSON shape) and return
/// its exit code. Stdout carries the answer; usage errors stay on
/// stderr, so scripts can parse stdout either way.
fn emit(json: bool, report: &Report) -> i32 {
    if json {
        println!("{}", report.json());
    } else {
        println!("{}", report.status.word());
    }
    report.status.exit()
}

/// Run one provider's probe: `usage`, then its login options, then the
/// fallback login attempt exactly where `auth check` runs it (options
/// on offer, none of them OAuth - an API-key login resolving from the
/// environment, never a browser or a prompt). The attempt keeps
/// `auth check`'s side effect: a key it resolves is promoted into the
/// namespace, so a provider found ready stays ready.
fn probe(handle: &Arc<dyn ExtensionDispatch>) -> Result<Probe, ()> {
    let usage = lca_core::drive_blocking({
        let handle = handle.clone();
        async move { handle.identity_usage().await }
    });
    let usage = match usage {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(outcome)) => Err(outcome),
        Err(_) => return Err(()),
    };
    let options = lca_core::drive_blocking({
        let handle = handle.clone();
        async move { handle.login_options().await }
    })
    .unwrap_or_default();
    // The fallback attempt runs only where it cannot launch anything:
    // options on offer, none of them OAuth.
    let login = if matches!(usage, Err(IdentityOutcome::NotSupported))
        && !options.is_empty()
        && !options.iter().any(|option| option.kind == "oauth")
    {
        let outcome = lca_core::drive_blocking({
            let handle = handle.clone();
            async move { handle.identity_login().await }
        });
        Some(match outcome {
            Ok(IdentityOutcome::Ok) => Ok(()),
            _ => Err(()),
        })
    } else {
        None
    };
    Ok(Probe {
        usage,
        options,
        login,
    })
}

/// Probe one provider the way `auth check` does and return only its
/// status (gh #177): the `/model` catalog lists a provider's models
/// exactly when this answers `Ready`; anything else contributes
/// nothing, silently.
pub(crate) fn check_status(registry: &lca_core::ExtensionRegistry, provider: &str) -> Status {
    let Some(handle) = registry.provider(provider).cloned() else {
        return Status::NotReady;
    };
    match probe(&handle) {
        Ok(probe) => classify(provider, probe).status,
        Err(()) => Status::Invalid,
    }
}

/// Probe one provider and report pi's table.
fn check(
    registry: &lca_core::ExtensionRegistry,
    config: &lca_config::Config,
    provider: Option<&str>,
    model: Option<&str>,
    json: bool,
) -> i32 {
    let (name, handle) = match resolve_target(registry, config, provider, model, json) {
        Ok(target) => target,
        Err(code) => return code,
    };
    let probe = match probe(&handle) {
        Ok(probe) => probe,
        Err(()) => {
            return emit(
                json,
                &Report {
                    status: Status::Invalid,
                    provider: name.clone(),
                    reason: Some("invalid_state"),
                    auth_type: None,
                },
            );
        }
    };
    emit(json, &classify(&name, probe))
}

/// Sign in: one driver for both shapes. An API-key login resolves from
/// the environment inside the extension and returns; an OAuth login
/// publishes its authorization URL, which this loop prints and follows
/// with the pasted callback — pi's remote/headless shape, and the same
/// `oauth_await` wakeup the interface's manual fallback delivers to.
fn login(registry: &lca_core::ExtensionRegistry, provider: &str) -> i32 {
    let Some(handle) = registry.provider(provider).cloned() else {
        eprintln!(
            "error: no provider named {provider:?}; see `lca ext list` for what is installed"
        );
        return exit::USAGE;
    };
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = handle.clone();
    std::thread::spawn(move || {
        let outcome = lca_core::drive_blocking(async move { worker.identity_login().await });
        let _ = done_tx.send(outcome);
    });
    // stdin rides its own thread: a blocking read would pin this loop,
    // which must keep polling the login thread and the auth URL.
    let (line_tx, line_rx) = std::sync::mpsc::channel::<Line>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let lines = std::io::BufRead::lines(stdin.lock());
        for line in lines {
            match line {
                Ok(text) => {
                    if line_tx.send(Line::Text(text)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = line_tx.send(Line::Closed);
    });
    let mut read_line = move || {
        line_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .unwrap_or(Line::Pending)
    };
    let mut print = |text: &str| println!("{text}");
    oauth_headless_login(&handle, &done_rx, &mut read_line, &mut print)
}

/// The headless OAuth exchange over injected lines, so the paste flow is
/// pinned without a terminal: print each new authorization URL once,
/// parse each pasted line with the one callback parser, deliver it to
/// the waiting flow, and report the login thread's outcome. The done
/// channel is polled, never awaited: this loop also watches the URL and
/// stdin, and the login thread's own timeout bounds a walkaway.
fn oauth_headless_login(
    handle: &Arc<dyn ExtensionDispatch>,
    done_rx: &std::sync::mpsc::Receiver<Result<IdentityOutcome, DispatchError>>,
    read_line: &mut dyn FnMut() -> Line,
    print: &mut dyn FnMut(&str),
) -> i32 {
    let mut shown: Option<String> = None;
    loop {
        // R3's headless half: the URL the extension asked the host to
        // open is the whole flow when no browser can reach the loopback.
        if let Some(url) = handle.oauth_last_url()
            && shown.as_deref() != Some(url.as_str())
        {
            shown = Some(url.clone());
            print(&format!("Open this URL in your browser:\n\n{url}\n"));
            print("Then paste the callback URL or authorization code here:");
        }
        if let Ok(outcome) = done_rx.try_recv() {
            return report_login_outcome(handle, outcome, print);
        }
        match read_line() {
            Line::Pending => {}
            Line::Closed => {
                // Stdin died (a pipe, `/dev/null`, a walkaway): an
                // in-flight API-key login still lands — it never needs
                // a paste — so give it a moment before cancelling.
                match done_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    Ok(outcome) => return report_login_outcome(handle, outcome, print),
                    Err(_) => {
                        eprintln!("error: no callback pasted; login cancelled");
                        return exit::INTERNAL;
                    }
                }
            }
            Line::Text(value) => {
                // The one callback parser (R4(c)): the interface's
                // manual fallback delivers through this same seam.
                match crate::login::parse_callback(&value) {
                    None => print(
                        "that is not a callback URL — paste the whole redirect (…/callback?code=…).",
                    ),
                    Some(params) => match handle.oauth_manual_callback(params) {
                        Ok(()) => print("callback delivered — finishing sign-in…"),
                        Err(err) => print(&format!("could not use that callback: {err}")),
                    },
                }
            }
        }
    }
}

/// Report what the login thread did. One reporter for the polled
/// outcome and the stdin-closed grace wait, so the two agree.
fn report_login_outcome(
    handle: &Arc<dyn ExtensionDispatch>,
    outcome: Result<IdentityOutcome, DispatchError>,
    print: &mut dyn FnMut(&str),
) -> i32 {
    match outcome {
        Ok(IdentityOutcome::Ok) => {
            print(&format!("signed in {}", handle.name()));
            exit::OK
        }
        Ok(IdentityOutcome::NotSupported) => {
            eprintln!("error: {} runs no login flow", handle.name());
            exit::INTERNAL
        }
        Ok(IdentityOutcome::Failed(reason)) => {
            eprintln!("error: {reason}");
            exit::INTERNAL
        }
        Err(err) => {
            eprintln!("error: {err}");
            exit::INTERNAL
        }
    }
}

/// Sign out through the provider's own logout.
fn logout(registry: &lca_core::ExtensionRegistry, provider: &str) -> i32 {
    let Some(handle) = registry.provider(provider).cloned() else {
        eprintln!(
            "error: no provider named {provider:?}; see `lca ext list` for what is installed"
        );
        return exit::USAGE;
    };
    match lca_core::drive_blocking({
        let handle = handle.clone();
        async move { handle.identity_logout().await }
    }) {
        Ok(IdentityOutcome::Ok) => {
            println!("signed out {provider}");
            exit::OK
        }
        Ok(IdentityOutcome::NotSupported) => {
            eprintln!("error: {provider} has no logout to run");
            exit::INTERNAL
        }
        Ok(IdentityOutcome::Failed(reason)) => {
            eprintln!("error: {reason}");
            exit::INTERNAL
        }
        Err(err) => {
            eprintln!("error: {err}");
            exit::INTERNAL
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn option(kind: &str) -> LoginOption {
        LoginOption {
            id: kind.to_string(),
            name: kind.to_string(),
            kind: kind.to_string(),
            host: String::new(),
            fields: Vec::new(),
            extras: std::collections::BTreeMap::new(),
        }
    }

    // Verifies: gh #72, FR-PROV-3 - a healthy `usage` probe is `ready`,
    // with the auth type read off the option kinds, never the provider.
    #[test]
    fn a_healthy_usage_probe_is_ready() {
        let report = classify(
            "p",
            Probe {
                usage: Ok(()),
                options: vec![option("oauth")],
                login: None,
            },
        );
        assert_eq!(report.status, Status::Ready);
        assert_eq!(report.status.exit(), 0);
        assert_eq!(report.auth_type, Some("oauth"));
        assert!(report.reason.is_none());
    }

    // Verifies: gh #72 - a provider-reported failure is `invalid`
    // (exit 2), whether the token expired or was never stored: the
    // surface cannot tell those apart, and the fix is the same login.
    #[test]
    fn a_failed_usage_probe_is_invalid() {
        let report = classify(
            "p",
            Probe {
                usage: Err(IdentityOutcome::Failed("no login yet".to_string())),
                options: vec![],
                login: None,
            },
        );
        assert_eq!(report.status, Status::Invalid);
        assert_eq!(report.status.exit(), 2);
        assert_eq!(report.reason, Some("invalid_state"));
    }

    // Verifies: gh #72 - with no self-probe, an API-key login attempt
    // decides: a resolved key is `ready`, none configured is `not_ready`.
    #[test]
    fn without_a_self_probe_the_key_attempt_decides() {
        let ready = classify(
            "p",
            Probe {
                usage: Err(IdentityOutcome::NotSupported),
                options: vec![option("api-key")],
                login: Some(Ok(())),
            },
        );
        assert_eq!((ready.status, ready.status.exit()), (Status::Ready, 0));
        assert_eq!(ready.auth_type, Some("api_key"));

        let missing = classify(
            "p",
            Probe {
                usage: Err(IdentityOutcome::NotSupported),
                options: vec![option("api-key")],
                login: Some(Err(())),
            },
        );
        assert_eq!(
            (missing.status, missing.status.exit()),
            (Status::NotReady, 1)
        );
        assert_eq!(missing.reason, Some("credentials_not_configured"));
    }

    // Verifies: gh #72 - a check launches nothing: with no self-probe
    // and an OAuth-shaped (or empty) option set, the answer is
    // `not_ready` even though no login was attempted.
    #[test]
    fn a_check_never_launches_an_oauth_flow() {
        for options in [vec![option("oauth")], vec![]] {
            let report = classify(
                "p",
                Probe {
                    usage: Err(IdentityOutcome::NotSupported),
                    options,
                    login: None,
                },
            );
            assert_eq!(report.status, Status::NotReady);
            assert_eq!(report.reason, Some("credentials_not_configured"));
        }
    }

    /// The stub provider the driver tests log in against: `identity_login`
    /// blocks until a callback is delivered (like `oauth_await`), then
    /// completes with `outcome`. The URL it "asked to open" is fixed.
    /// A delivered callback: the query pairs `oauth_await` would have
    /// handed back.
    type Callback = Vec<(String, String)>;

    struct Stub {
        url: Option<String>,
        delivered: std::sync::mpsc::Sender<Callback>,
        receiver: Mutex<Option<std::sync::mpsc::Receiver<Callback>>>,
        outcome: IdentityOutcome,
    }

    impl Stub {
        fn new(url: Option<&str>, outcome: IdentityOutcome) -> Arc<Self> {
            let (tx, rx) = std::sync::mpsc::channel();
            Arc::new(Stub {
                url: url.map(str::to_string),
                delivered: tx,
                receiver: Mutex::new(Some(rx)),
                outcome,
            })
        }
    }

    impl lca_ext_abi::ExtensionDispatch for Stub {
        fn name(&self) -> &str {
            "stub"
        }

        fn delivery(&self) -> lca_ext_abi::DeliveryMode {
            lca_ext_abi::DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<lca_ext_abi::World> {
            vec![lca_ext_abi::World::Provider]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Ok(Vec::new())
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            Ok(Vec::new())
        }

        fn on_pre_turn(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_session_close(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>>
        {
            Box::pin(std::future::ready(Err(DispatchError::Failed(
                "no tools".to_string(),
            ))))
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }

        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>>
        {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }

        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn identity_login(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<IdentityOutcome, DispatchError>> {
            let rx = self
                .receiver
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
            let outcome = self.outcome.clone();
            Box::pin(async move {
                match rx {
                    // No receiver: completes without a callback, like a
                    // loopback flow the browser reached on its own.
                    None => Ok(outcome),
                    Some(rx) => rx
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .map(|_| outcome)
                        .map_err(|_| DispatchError::Failed("no callback".to_string())),
                }
            })
        }

        fn oauth_last_url(&self) -> Option<String> {
            self.url.clone()
        }

        fn oauth_manual_callback(
            &self,
            params: Vec<(String, String)>,
        ) -> Result<(), DispatchError> {
            self.delivered
                .send(params)
                .map_err(|_| DispatchError::Failed("nobody waiting".to_string()))
        }
    }

    /// Drive the loop like `login` does: the login thread plus a
    /// scripted line queue. Returns the exit code and everything
    /// printed.
    fn drive(stub: Arc<Stub>, mut script: Vec<Line>) -> (i32, Vec<String>) {
        script.reverse();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker: Arc<dyn ExtensionDispatch> = stub.clone();
        std::thread::spawn(move || {
            let outcome = lca_core::drive_blocking(async move { worker.identity_login().await });
            let _ = done_tx.send(outcome);
        });
        let mut read_line = move || script.pop().unwrap_or(Line::Pending);
        let mut printed = Vec::new();
        let mut print = |text: &str| printed.push(text.to_string());
        let handle: Arc<dyn ExtensionDispatch> = stub;
        let code = oauth_headless_login(&handle, &done_rx, &mut read_line, &mut print);
        // The login thread already sent (the loop only returns after
        // the outcome), so nothing leaks; a closed-early run's worker
        // is still parked, and dies with the test process.
        (code, printed)
    }

    // Verifies: gh #38, FR-PROV-3 - the headless paste flow: the auth URL
    // prints once, a pasted callback delivers to the waiting login, and
    // the outcome reports.
    #[test]
    fn a_pasted_callback_completes_the_headless_oauth_flow() {
        let stub = Stub::new(Some("https://idp.example/auth?x=1"), IdentityOutcome::Ok);
        let (code, printed) = drive(
            stub,
            vec![Line::Text(
                "https://idp.example/cb?code=abc&state=s".to_string(),
            )],
        );
        assert_eq!(code, 0);
        assert!(
            printed
                .iter()
                .any(|line| line.contains("https://idp.example/auth?x=1")),
            "the URL prints: {printed:?}"
        );
        assert!(
            printed.iter().any(|line| line.contains("signed in stub")),
            "the outcome reports: {printed:?}"
        );
    }

    // Verifies: gh #38 - a line that is not a callback re-prompts
    // instead of failing the flow; the next paste still completes it.
    #[test]
    fn a_non_callback_line_reprompts_and_the_flow_survives() {
        let stub = Stub::new(Some("https://idp.example/auth"), IdentityOutcome::Ok);
        let (code, printed) = drive(
            stub,
            vec![
                Line::Text("hello".to_string()),
                Line::Text("code=abc&state=s".to_string()),
            ],
        );
        assert_eq!(code, 0);
        assert!(
            printed
                .iter()
                .any(|line| line.contains("not a callback URL")),
            "the re-prompt shows: {printed:?}"
        );
    }

    // Verifies: gh #38 - a closed stdin cancels the login instead of
    // hanging on the flow's timeout.
    #[test]
    fn a_closed_stdin_cancels_the_login() {
        let stub = Stub::new(Some("https://idp.example/auth"), IdentityOutcome::Ok);
        let (code, _) = drive(stub, vec![Line::Closed]);
        assert_ne!(code, 0, "walking away is a failure, not a hang");
    }

    // Verifies: gh #38 - a provider failure after delivery still
    // reports: the paste worked, the exchange did not.
    #[test]
    fn a_failed_exchange_reports_after_delivery() {
        let stub = Stub::new(
            Some("https://idp.example/auth"),
            IdentityOutcome::Failed("bad_verification_code".to_string()),
        );
        let (code, printed) = drive(stub, vec![Line::Text("code=abc&state=s".to_string())]);
        assert_ne!(code, 0);
        assert!(
            printed
                .iter()
                .any(|line| line.contains("callback delivered")),
            "delivery happened first: {printed:?}"
        );
    }
}
