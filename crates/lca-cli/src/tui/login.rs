//! The host login flow's wiring (S1): gather each provider's options,
//! apply a finished step, and expose the four `/login` seams. Moved
//! verbatim from the closure that used to capture `run`'s locals.

use std::sync::Arc;

use lca_ui::LoginNext;

use super::Ui;

/// What the waiting modal shows while a background login step runs (R4).
/// Gh #200: the waiting screen names the copy key, because over SSH
/// there is no browser to open and no URL to click - Ctrl+X through
/// the copy ladder (native clipboard, else OSC 52) is the way out.
const WAIT_LABEL: &str = "waiting for browser sign-in… (esc cancels, ctrl+x copies the link)";

/// How long the interface waits before offering the manual
/// "paste the callback URL" fallback (R4(c)).
const MANUAL_AFTER: std::time::Duration = std::time::Duration::from_secs(8);

/// Where `/login <target>` goes once its options are gathered (gh #25,
/// ADR-0033): a provider whose login is its own flow offers no picker
/// presets, so it gets its identity `login` export - the same handler
/// `/<provider>.login` reaches - rather than a picker that would come up
/// empty but for the host's universal `Custom endpoint…` row, which is
/// attributed to `openai-compatible` and is not the target the user named.
///
/// `owned_by_target` counts the gathered options the named provider owns:
/// [`Ui::gather`] also carries the user's `openai-compatible` preset
/// overrides, and those are never that provider's.
fn identity_instead_of_picker(argument: &str, names: &[String], owned_by_target: usize) -> bool {
    !argument.is_empty() && names.iter().any(|name| name == argument) && owned_by_target == 0
}

impl Ui {
    /// Every picker choice: each enabled provider extension's own options
    /// (custom endpoints included, gh #188), plus the user's named
    /// custom endpoints (D1's override layer). The host appends
    /// nothing: an empty set answers with the way out.
    fn gather(&self, scope: &[String]) -> Vec<(String, lca_protocol::LoginOption)> {
        let mut out: Vec<(String, lca_protocol::LoginOption)> = Vec::new();
        for name in scope {
            let Some(handle) = self.registry().provider(name).cloned() else {
                continue;
            };
            let name = name.clone();
            let result = lca_core::drive_blocking(async move { handle.login_options().await });
            match result {
                Ok(options) => out.extend(options.into_iter().map(|o| (name.clone(), o))),
                Err(err) => tracing::warn!("{name} login options: {err}"),
            }
        }
        // D1's override layer keeps the openai-compatible attribution:
        // the file's own grammar (base_url/auth/models) is that
        // extension's preset shape. The default-host filter below it is
        // manifest-driven, never a literal.
        let needs = crate::provider_needs::provider_needs(
            &self.registry(),
            &self.data,
            "openai-compatible",
        );
        out.extend(crate::login::override_presets(
            &self.preset_overrides,
            "openai-compatible",
            needs.as_ref(),
        ));
        out
    }

    /// Apply one finished login step (R4). `Next` opens the next field or
    /// message directly. `Submit` carries the blocking half: the
    /// extension's `login_submit` runs its identity flow, which for an
    /// OAuth provider waits out a browser callback on a 300-second loop.
    /// That runs on a background thread and the interface shows a
    /// cancellable `Waiting` state; the result arrives through
    /// [`Ui::poll_login`].
    fn login_apply(self: &Arc<Self>, step: crate::login::Step) -> LoginNext {
        use crate::login::Step;
        match step {
            Step::Next(next) => next,
            Step::Submit {
                provider,
                choice,
                kind,
                values,
            } => {
                let target = if provider.is_empty() {
                    self.live_name()
                } else {
                    provider
                };
                self.begin_wait(&target);
                let ui = self.clone();
                std::thread::spawn(move || {
                    let next = ui.finish_login(target, choice, kind, values);
                    *ui.login_pending.lock().unwrap_or_else(|p| p.into_inner()) = Some(next);
                });
                LoginNext::Waiting {
                    label: WAIT_LABEL.to_string(),
                }
            }
        }
    }

    /// Arm a background wait against `target`'s handle (R4): reset the
    /// manual-offer and URL state, stamp the start, and keep the handle
    /// for cancellation and for delivering a pasted callback.
    fn begin_wait(&self, target: &str) {
        *self
            .login_manual_offered
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = false;
        *self
            .login_url_shown
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        *self
            .login_cancelled
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = false;
        *self
            .login_wait_since
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
        let handle = self.registry().provider(target).cloned();
        *self.login_handle.lock().unwrap_or_else(|p| p.into_inner()) = handle;
    }

    /// The blocking half of a `Submit`, on the background thread: store
    /// the secret through the same atomic, owner-only credential writer
    /// extensions use, persist the opaque settings, light the session up,
    /// and offer the ad hoc grant.
    fn finish_login(
        &self,
        target: String,
        choice: String,
        kind: String,
        values: std::collections::BTreeMap<String, String>,
    ) -> LoginNext {
        // Choosing a login option for a provider that was disabled is an
        // explicit request to use it: re-enable it for this project, or the
        // settings land on something that will not run.
        let re_enabled = {
            let mut store = self.grants.lock().unwrap_or_else(|p| p.into_inner());
            if store.extension_enabled(&self.cwd, &target) == Some(false) {
                let _ = store.set_extension_enabled(&self.cwd, &target, true);
                true
            } else {
                false
            }
        };
        // Every option delegates to its extension (gh #188): the host
        // renders the declared fields and submits the answers, and the
        // extension stores the key and returns its own settings - the
        // custom endpoint included, through its declared preset.
        let Some(handle) = self.registry().provider(&target).cloned() else {
            return LoginNext::Message(format!("`{target}` cannot log in"));
        };
        let answer = lca_protocol::LoginAnswer {
            choice: choice.clone(),
            values: values.clone(),
        };
        let mut settings =
            match lca_core::drive_blocking(async move { handle.login_submit(answer).await }) {
                Ok(settings) => settings,
                Err(err) => return LoginNext::Message(format!("could not sign in: {err}")),
            };
        // E5: the footer names the preset (`opencode-go`), not the extension.
        // A `custom`-kind option has no preset, so the extension name
        // stands in - the kind is the extension's own declaration (WIT
        // `login-option.kind`), never host knowledge of an id.
        let identity = if kind == "custom" {
            target.clone()
        } else {
            settings.push(("preset".to_string(), choice.clone()));
            choice.clone()
        };
        // #4: remember the answer so the endpoint grant's approval can re-run
        // model discovery; the submit's own discovery ran before the grant
        // existed and could not reach the endpoint.
        *self.login_answer.lock().unwrap_or_else(|p| p.into_inner()) =
            Some((target.clone(), choice.clone(), values.clone()));
        for (key, value) in &settings {
            if let Err(err) = crate::store_provider_secret(
                &self.data,
                &self.cwd,
                &target,
                key,
                value,
                &self.grants,
            ) {
                return LoginNext::Message(format!("could not store {key} for {target}: {err}"));
            }
        }
        // `list-models` reads the same pairs `complete` gets in its `extras`
        // (ADR-0035): one source, no second store.
        {
            let mut cell = self.settings_cell.lock().unwrap_or_else(|p| p.into_inner());
            cell.clear();
            cell.extend(settings.iter().cloned());
        }
        // Light the session up now that the provider can answer (G3: one
        // seam for both login paths, see `adopt_login_success`). A
        // foreign target asks the switch confirmation first (gh #177) -
        // the request path still consents its endpoint later (gh #29), so
        // skipping the login-time grant offer here loses nothing.
        if target != self.live_name() {
            return LoginNext::ConfirmSwitch {
                provider: target.clone(),
                prompt: format!(
                    "signed in to `{target}` - switch this session from `{}` to it?",
                    self.live_name(),
                ),
            };
        }
        self.adopt_login_success(&target, &identity);
        // A non-default endpoint needs its ad hoc `net` grant, offered now
        // that the user is signed in (FR-PERM-16). gh #21: this login's
        // own endpoint comes first - a preset login stores its base URL
        // under its profile, which the default-profile store read below
        // never sees, so a local preset never got its offer and every
        // later turn failed with no recourse.
        // Gh #157: the target's manifest declares the defaults; the
        // submit's own pairs come first (a preset login stores its base
        // URL under its profile, which the default-profile read below
        // never sees - gh #21), then the configured default endpoint.
        let needs = crate::provider_needs::provider_needs(&self.registry(), &self.data, &target);
        if let Some(host) = login_endpoint_hosts(&settings, needs.as_ref())
            .into_iter()
            .find(|host| {
                crate::ungranted_host(&self.grants, &self.cwd, Some(host.clone())).is_some()
            })
            .or_else(|| {
                crate::ungranted_host(
                    &self.grants,
                    &self.cwd,
                    crate::provider_needs::provider_ad_hoc_host(
                        &self.data,
                        &target,
                        needs.as_ref(),
                    ),
                )
            })
        {
            return LoginNext::Grant {
                provider: target.clone(),
                host: host.clone(),
                prompt: format!(
                    "{target}'s endpoint is {host}, which its manifest does not cover; \
                     add it as an ad hoc grant?"
                ),
            };
        }
        let suffix = if re_enabled {
            format!("; re-enabled `{target}` for this project")
        } else {
            String::new()
        };
        LoginNext::Message(format!(
            "signed in {target}; the settings are stored under its namespace{suffix}"
        ))
    }

    /// Poll the background login/identity step (R4): the interface calls
    /// this every loop tick. `Some` applies a step, `None` keeps waiting.
    pub(super) fn poll_login(&self) -> Option<LoginNext> {
        // A finished background step wins; clearing the handle ends the
        // wait (its manual offer and URL state go with it).
        if let Some(next) = self
            .login_pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            *self.login_handle.lock().unwrap_or_else(|p| p.into_inner()) = None;
            *self
                .login_wait_since
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = None;
            // A cancel the user asked for reports as a cancel; the thread's
            // own error (`call cancelled`) would read as a failure.
            let cancelled = {
                let mut flag = self
                    .login_cancelled
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                let was = *flag;
                *flag = false;
                was
            };
            if cancelled {
                return Some(LoginNext::Message("login cancelled".to_string()));
            }
            return Some(next);
        }
        let handle = self
            .login_handle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        // R3: fold the auth URL into the waiting label as soon as the
        // extension asks the host to open it, so a failed auto-open still
        // leaves a copyable URL on screen.
        if let Some(url) = handle.oauth_last_url()
            && self
                .login_url_shown
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_deref()
                != Some(url.as_str())
        {
            *self
                .login_url_shown
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = Some(url.clone());
            // gh #178 (pi's login-dialog): the URL rides in an explicit
            // OSC 8 sequence, so a wrapped modal row still clicks whole.
            return Some(LoginNext::Waiting {
                label: format!("{WAIT_LABEL}\n\n\x1b]8;;{url}\x07{url}\x1b]8;;\x07"),
            });
        }
        // R4(c): after a quiet period, offer the manual fallback — paste
        // the callback URL (pi's `acquireAuthCode`) and the flow completes
        // even where the loopback listener never got the browser's visit.
        let offer = if *self
            .login_manual_offered
            .lock()
            .unwrap_or_else(|p| p.into_inner())
        {
            false
        } else {
            let since = *self
                .login_wait_since
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            since.is_some_and(|t| t.elapsed() >= MANUAL_AFTER)
        };
        if offer {
            *self
                .login_manual_offered
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = true;
            return Some(LoginNext::Secret {
                provider: handle.name().to_string(),
                label: "Nothing in the browser? Paste the callback URL:".to_string(),
                masked: false,
            });
        }
        None
    }

    /// Cancel the background step (R4): interrupt the extension, which
    /// releases a blocked `oauth_await` through the capability engine's
    /// cancel flag (NFR-21).
    pub(super) fn cancel_login(&self) {
        *self
            .login_cancelled
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = true;
        let handle = self
            .login_handle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(handle) = handle {
            handle.interrupt();
        }
    }

    /// Light the session up after a successful sign-in: the identity cell
    /// (E5's footer regression) and the same model refresh the `/model`
    /// picker does - the first model the provider now lists, not the empty
    /// startup state (G3, issue #2: the list is refreshed at login success,
    /// so a sign-in resolves the session's model with no restart). One seam
    /// for both login paths: the host's submit flow and a provider's own
    /// `login` export.
    pub(super) fn adopt_login_success(&self, target: &str, identity: &str) {
        // A foreign target's move belongs to the switch confirmation
        // (gh #177): adopting its identity now would relabel a session
        // still running on the old provider.
        if target != self.live_name() {
            return;
        }
        super::adopt_login_identity(&self.identity_cell, identity);
        // First *non-empty* id: a provider with nothing listed yet leaves
        // the session as it was rather than resolving to a blank model.
        if let Some(model) = self
            .live_provider()
            .list_models()
            .into_iter()
            .find(|model| !model.id.is_empty())
        {
            *self.model_cell.lock().unwrap_or_else(|p| p.into_inner()) = super::ModelChoice {
                id: model.id.clone(),
                window: model.context_window,
            };
            *self
                .context_window_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = u64::from(model.context_window);
            *self.label_cell.lock().unwrap_or_else(|p| p.into_inner()) =
                format!("{identity}/{}", model.id);
        }
    }

    /// Run one provider's `login` identity command on a background thread
    /// (R4): the namespaced `/antigravity.login` path is the owner's
    /// freeze. Answer with a waiting step; the result arrives through
    /// [`Ui::poll_login`].
    pub(super) fn spawn_identity_login(self: &Arc<Self>, handle: lca_ext_native::NativeHandle) {
        *self.login_pending.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *self
            .login_manual_offered
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = false;
        *self
            .login_url_shown
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        *self
            .login_cancelled
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = false;
        *self
            .login_wait_since
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
        *self.login_handle.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle.clone());
        let pending = self.login_pending.clone();
        let ui = self.clone();
        std::thread::spawn(move || {
            use lca_core::drive_blocking;
            let name = handle.name().to_string();
            let outcome = drive_blocking(async move { handle.identity_login().await });
            let next = match outcome {
                Ok(lca_protocol::IdentityOutcome::Ok) => {
                    // G3: the same refresh the submit flow runs - the
                    // model list is read now, not at the next startup.
                    let identity = crate::stored_provider_preset(&ui.data, &name)
                        .unwrap_or_else(|| name.clone());
                    ui.adopt_login_success(&name, &identity);
                    confirm_switch_next(&name, &ui.live_name())
                }
                Ok(lca_protocol::IdentityOutcome::NotSupported) => {
                    LoginNext::Message(format!("login is not supported by `{name}`"))
                }
                Ok(lca_protocol::IdentityOutcome::Failed(reason)) => {
                    LoginNext::Message(format!("login failed: {reason}"))
                }
                Err(err) => LoginNext::Message(format!("login failed: {err}")),
            };
            *pending.lock().unwrap_or_else(|p| p.into_inner()) = Some(next);
        });
    }

    /// The `/login` entry seam.
    pub(super) fn login_options(self: &Arc<Self>) -> lca_ui::LoginRequest {
        let ui = self.clone();
        Arc::new(move |argument: &str| -> LoginNext {
            // Zero *enabled* providers is a valid state (FR-PROV-9, D1),
            // and `/login` is how the interface leaves it: the picker still
            // opens, with the host's universal entry attributed to the
            // configured provider. Refusing here would lock the interface
            // out of its own settings surface.
            let names = ui.registry().provider_names();
            // `/login <provider>` scopes the picker to that provider. A
            // bare `/login`, or an argument naming an option id, needs the
            // full list (an id may belong to any enabled provider).
            let scope: Vec<String> = if names.iter().any(|name| name == argument) {
                vec![argument.to_string()]
            } else {
                names.clone()
            };
            let options = ui.gather(&scope);
            // `/login <option>` skips the picker: the same journey, drivable
            // from a script.
            if !argument.is_empty()
                && let Some((owner, _)) = options
                    .iter()
                    .find(|(_, option)| option.id == argument)
                    .map(|(owner, option)| (owner.clone(), option.clone()))
            {
                let mut flow = ui.flow.lock().unwrap_or_else(|p| p.into_inner());
                let _ = flow.offer(options);
                return match flow.pick(&owner, argument) {
                    crate::login::Step::Next(next) => next,
                    step => ui.login_apply(step),
                };
            }
            if !argument.is_empty()
                && !names.iter().any(|name| name == argument)
                && !options.iter().any(|(_, option)| option.id == argument)
            {
                return LoginNext::Message(format!(
                    "no provider or login option named `{argument}`; installed: {}",
                    names.join(", ")
                ));
            }
            // gh #25 / ADR-0033: the zero-options route, closed at the
            // target-resolution site so no provider can reach an empty
            // picker. `Message` is what `/<provider>.login` answers with,
            // so both routes read the same line and `poll_login` opens the
            // waiting modal from here.
            if identity_instead_of_picker(
                argument,
                &names,
                options
                    .iter()
                    .filter(|(owner, _)| owner.as_str() == argument)
                    .count(),
            ) && let Some(handle) = ui.registry().provider(argument).cloned()
            {
                ui.spawn_identity_login(handle);
                return LoginNext::Message(WAIT_LABEL.to_string());
            }
            let mut flow = ui.flow.lock().unwrap_or_else(|p| p.into_inner());
            flow.offer(options)
        })
    }

    /// The user chose a `/login` picker entry.
    pub(super) fn login_pick(self: &Arc<Self>) -> lca_ui::LoginPick {
        let ui = self.clone();
        Arc::new(move |provider: &str, choice: &str| -> LoginNext {
            let step = ui
                .flow
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pick(provider, choice);
            ui.login_apply(step)
        })
    }

    /// A secret the user typed for `/login`.
    pub(super) fn login_complete(self: &Arc<Self>) -> lca_ui::LoginComplete {
        let ui = self.clone();
        Arc::new(move |provider: &str, value: &str| -> LoginNext {
            // R4(c): while the manual fallback is open the field collects a
            // pasted callback URL, not the next flow field.
            if *ui
                .login_manual_offered
                .lock()
                .unwrap_or_else(|p| p.into_inner())
            {
                return ui.deliver_manual_callback(value);
            }
            let step = ui
                .flow
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(provider, value);
            ui.login_apply(step)
        })
    }

    /// Hand a pasted OAuth callback URL to the waiting flow (R4(c), pi's
    /// `acquireAuthCode`): parse the redirect URL's query, deliver it on
    /// the flow's manual channel, and go back to waiting for the exchange.
    ///
    /// The paste window's state table (issue #8): every path that means the
    /// wait is over - delivered, exchange failed, no handle left - disarms
    /// `login_manual_offered`, so later input reaches the normal flow again
    /// instead of being judged as a URL. The parse-failure path stays armed
    /// on purpose (nothing was delivered; the flow still wants the real
    /// redirect, so a bad paste must stay retryable) and its message now
    /// says so - which is what issue #8 was missing when the repeat
    /// complaint read as a stuck dialog.
    pub(super) fn deliver_manual_callback(&self, value: &str) -> LoginNext {
        let Some(params) = crate::login::parse_callback(value) else {
            return LoginNext::Message(
                "that is not a callback URL — paste the whole redirect (…/callback?code=…). \
                 The sign-in is still waiting for it; Esc cancels the sign-in."
                    .to_string(),
            );
        };
        let handle = self
            .login_handle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let Some(handle) = handle else {
            // Nothing waits any more: settle the window, so the next input
            // is not judged as a callback either (issue #8's class).
            *self
                .login_manual_offered
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = false;
            return LoginNext::Message("no login is waiting".to_string());
        };
        match handle.oauth_manual_callback(params) {
            Ok(()) => {
                *self
                    .login_manual_offered
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = false;
                *self
                    .login_wait_since
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
                LoginNext::Waiting {
                    label: "callback delivered — finishing sign-in… (esc cancels)".to_string(),
                }
            }
            Err(err) => {
                *self
                    .login_manual_offered
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = false;
                LoginNext::Message(format!("could not use that callback: {err}"))
            }
        }
    }

    /// Persists an ad hoc `net` grant the user approved at login.
    pub(super) fn login_confirm(self: &Arc<Self>) -> lca_ui::LoginConfirm {
        let ui = self.clone();
        Arc::new(move |provider: &str, host: &str| -> String {
            match crate::store_ad_hoc_grant(&ui.grants, &ui.cwd, host) {
                Ok(()) => {
                    // The endpoint is reachable now; discover the model list
                    // the submit's pre-grant attempt could not fetch (#4).
                    ui.refresh_models_after_grant(provider);
                    format!("{provider} may now reach {host}")
                }
                Err(err) => format!("could not store the ad hoc grant: {err}"),
            }
        })
    }

    /// Re-run the login's model discovery now that the endpoint's ad-hoc
    /// grant is stored, persist the discovered list, and hand it to the live
    /// settings cell so `/model` sees it without a restart. Best-effort: a
    /// failure leaves the configured model as the only choice, as before.
    fn refresh_models_after_grant(&self, provider: &str) {
        let answer = self
            .login_answer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let Some((target, choice, values)) = answer else {
            return;
        };
        if target != provider {
            return;
        }
        let Some(handle) = self.registry().provider(provider).cloned() else {
            return;
        };
        let answer = lca_protocol::LoginAnswer { choice, values };
        let Ok(settings) =
            lca_core::drive_blocking(async move { handle.login_submit(answer).await })
        else {
            return;
        };
        for (key, value) in &settings {
            let _ = crate::store_provider_secret(
                &self.data,
                &self.cwd,
                &target,
                key,
                value,
                &self.grants,
            );
        }
        let mut cell = self.settings_cell.lock().unwrap_or_else(|p| p.into_inner());
        cell.clear();
        cell.extend(settings);
    }
}

/// What a successful login to `name` says next (gh #177): the same
/// provider reports the sign-in; a foreign one asks the switch
/// confirmation - the surviving half of the old stranded message.
fn confirm_switch_next(name: &str, current: &str) -> LoginNext {
    if name == current {
        LoginNext::Message(format!("logged in via `{name}`"))
    } else {
        LoginNext::ConfirmSwitch {
            provider: name.to_string(),
            prompt: format!("signed in to `{name}` - switch this session from `{current}` to it?"),
        }
    }
}

/// Endpoint hosts this login configured (gh #21): the submit's own
/// `*.base_url` pairs in order, deduplicated. A manifest-declared
/// default host never needs a grant, so it never appears here.
fn login_endpoint_hosts(
    settings: &[(String, String)],
    needs: Option<&crate::provider_needs::ProviderNeeds>,
) -> Vec<String> {
    let mut hosts = Vec::new();
    for (key, value) in settings {
        if !key.ends_with("base_url") {
            continue;
        }
        let authority = value.split("://").nth(1).unwrap_or(value);
        if let Some(host) = crate::ad_hoc_host_from_authority(authority, needs)
            && !hosts.contains(&host)
        {
            hosts.push(host);
        }
    }
    hosts
}

#[cfg(test)]
mod tests {
    use super::{confirm_switch_next, identity_instead_of_picker};

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    // Verifies: gh #177 - a login to the session's own provider
    // reports the sign-in; a login anywhere else asks the switch
    // confirmation naming both sides (the stranded message is gone).
    #[test]
    fn a_login_reports_home_and_asks_away() {
        let home = confirm_switch_next("openai-compatible", "openai-compatible");
        assert!(
            matches!(home, lca_ui::LoginNext::Message(_)),
            "home reports the sign-in"
        );
        let away = confirm_switch_next("codex", "openai-compatible");
        match away {
            lca_ui::LoginNext::ConfirmSwitch { provider, prompt } => {
                assert_eq!(provider, "codex");
                assert!(
                    prompt.contains("openai-compatible") && prompt.contains("codex"),
                    "the question names both sides: {prompt}"
                );
            }
            other => panic!("away must ask, got a message: {other:?}"),
        }
    }

    // Verifies: gh #25 (the fallback-leak row) - `/login` scoped to a
    // provider with no picker options runs its identity `login` export.
    // Offering the picker instead opens it empty, and an empty set
    // answers with the way out (gh #188: no phantom custom entry).
    #[test]
    fn login_scoped_to_a_provider_with_no_options_asks_for_its_identity_flow() {
        assert!(identity_instead_of_picker(
            "antigravity",
            &names(&["antigravity", "openai-compatible"]),
            0
        ));
    }

    // Verifies: gh #25 (unchanged row) - a provider that does offer presets
    // keeps the scoped picker, `Custom endpoint…` row and all.
    #[test]
    fn login_scoped_to_a_provider_with_options_still_offers_the_picker() {
        assert!(!identity_instead_of_picker(
            "openai-compatible",
            &names(&["antigravity", "openai-compatible"]),
            19
        ));
    }

    // Verifies: gh #25 (unchanged row) - the bare `/login` always offers
    // the picker, so the interface always has a way into its settings
    // surface (FR-PROV-9's zero-provider escape hatch).
    #[test]
    fn a_bare_login_always_offers_the_picker() {
        assert!(!identity_instead_of_picker("", &names(&["antigravity"]), 0));
    }

    // Verifies: gh #25 (unchanged row) - a name that is not a provider is
    // left to the caller's "no provider or login option named" message,
    // never routed into an identity flow.
    #[test]
    fn a_name_that_is_not_a_provider_is_left_to_the_unknown_message() {
        assert!(!identity_instead_of_picker(
            "bogus",
            &names(&["antigravity"]),
            0
        ));
    }

    // Verifies: gh #25 - the preset overrides `gather` appends belong to
    // openai-compatible, so they never make another provider look like it
    // has options of its own.
    #[test]
    fn another_providers_preset_overrides_do_not_count_as_the_targets_options() {
        assert!(identity_instead_of_picker(
            "antigravity",
            &names(&["antigravity"]),
            0
        ));
        assert!(!identity_instead_of_picker(
            "openai-compatible",
            &names(&["antigravity"]),
            2
        ));
    }

    // Verifies: gh #21 - a preset login's endpoint comes from the
    // submit's own pairs (`profile.<id>.base_url`), which the
    // default-profile store read misses, so the FR-PERM-16 grant offer
    // names this login's host.
    #[test]
    fn a_preset_login_names_its_own_endpoint_host() {
        assert_eq!(
            super::login_endpoint_hosts(
                &[(
                    "profile.ollama.base_url".to_string(),
                    "http://localhost:11434/v1".to_string()
                )],
                None
            ),
            vec!["localhost".to_string()]
        );
    }

    // Verifies: gh #21 - a bare `base_url` pair (a custom endpoint)
    // names its host the same way, and the default host needs no grant.
    #[test]
    fn a_bare_base_url_names_its_host_and_the_default_needs_none() {
        // Gh #157: the default-host filter is a manifest row, not a
        // literal - the same declaration the host reads at runtime.
        let needs = crate::provider_needs::test_needs();
        assert_eq!(
            super::login_endpoint_hosts(
                &[(
                    "base_url".to_string(),
                    "https://llm.example.com/v1".to_string()
                )],
                Some(&needs)
            ),
            vec!["llm.example.com".to_string()]
        );
        assert!(
            super::login_endpoint_hosts(
                &[(
                    "base_url".to_string(),
                    "https://api.acme.test/v1".to_string()
                )],
                Some(&needs)
            )
            .is_empty()
        );
        assert!(
            super::login_endpoint_hosts(
                &[("api_key".to_string(), "sk-x".to_string())],
                Some(&needs)
            )
            .is_empty()
        );
    }
}
