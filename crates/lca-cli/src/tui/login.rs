//! The host login flow's wiring (S1): gather each provider's options,
//! apply a finished step, and expose the four `/login` seams. Moved
//! verbatim from the closure that used to capture `run`'s locals.

use std::sync::Arc;

use lca_ui::LoginNext;

use super::Ui;

impl Ui {
    /// Every picker choice: each enabled provider extension's own options,
    /// plus the user's named custom endpoints (D1's override layer). The
    /// host's universal "Custom endpoint..." entry is appended by the flow.
    fn gather(&self, scope: &[String]) -> Vec<(String, lca_protocol::LoginOption)> {
        let mut out: Vec<(String, lca_protocol::LoginOption)> = Vec::new();
        for name in scope {
            let Some(handle) = self.registry.provider(name).cloned() else {
                continue;
            };
            let name = name.clone();
            let result = lca_core::drive_blocking(async move { handle.login_options().await });
            match result {
                Ok(options) => out.extend(options.into_iter().map(|o| (name.clone(), o))),
                Err(err) => tracing::warn!("{name} login options: {err}"),
            }
        }
        out.extend(crate::login::override_presets(
            &self.preset_overrides,
            "openai-compatible",
        ));
        out
    }

    /// Submit the finished login: store the secret through the same atomic,
    /// owner-only credential writer extensions use, persist the opaque
    /// settings, light the session up, and offer the ad hoc grant.
    fn login_apply(&self, step: crate::login::Step) -> LoginNext {
        use crate::login::Step;
        let (target, choice, values) = match step {
            Step::Next(next) => return next,
            Step::Submit {
                provider,
                choice,
                values,
            } => (provider, choice, values),
        };
        let target = if target.is_empty() {
            self.provider_name.clone()
        } else {
            target
        };
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
        // The host's universal entry has no extension behind it: the values
        // themselves are the settings. A preset delegates to its extension,
        // which stores the key and returns its own settings.
        let mut settings = if choice == lca_ui::CUSTOM_OPTION {
            values
                .iter()
                .filter(|(field, _)| *field != "api-key")
                .map(|(field, value)| (field.replace('-', "_"), value.clone()))
                .collect::<Vec<_>>()
        } else {
            let Some(handle) = self.registry.provider(&target).cloned() else {
                return LoginNext::Message(format!("`{target}` cannot log in"));
            };
            let answer = lca_protocol::LoginAnswer {
                choice: choice.clone(),
                values: values.clone(),
            };
            match lca_core::drive_blocking(async move { handle.login_submit(answer).await }) {
                Ok(settings) => settings,
                Err(err) => return LoginNext::Message(format!("could not sign in: {err}")),
            }
        };
        // E5: the footer names the preset (`opencode-go`), not the extension.
        // A custom endpoint has no preset, so the extension name stands in.
        let identity = if choice == lca_ui::CUSTOM_OPTION {
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
        if let Some(secret) = values.get("api-key")
            && let Err(err) =
                crate::store_provider_secret(&self.data, &self.cwd, &target, "api_key", secret)
        {
            return LoginNext::Message(format!("could not store the key for {target}: {err}"));
        }
        for (key, value) in &settings {
            if let Err(err) =
                crate::store_provider_secret(&self.data, &self.cwd, &target, key, value)
            {
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
        // Light the session up now that the provider can answer.
        if target == self.provider_name
            && let Some(model) = self.provider.list_models().first()
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
        // A non-default endpoint needs its ad hoc `net` grant, offered now
        // that the user is signed in (FR-PERM-16).
        if let Some(host) = crate::ungranted_host(
            &self.grants,
            &self.cwd,
            crate::openai_ad_hoc_host(&self.data),
        ) {
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

    /// The `/login` entry seam.
    pub(super) fn login_options(self: &Arc<Self>) -> lca_ui::LoginRequest {
        let ui = self.clone();
        Arc::new(move |argument: &str| -> LoginNext {
            // Zero *enabled* providers is a valid state (FR-PROV-9, D1),
            // and `/login` is how the interface leaves it: the picker still
            // opens, with the host's universal entry attributed to the
            // configured provider. Refusing here would lock the interface
            // out of its own settings surface.
            let names = ui.registry.provider_names();
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
                let _ = flow.offer(options, &ui.provider_name);
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
            let mut flow = ui.flow.lock().unwrap_or_else(|p| p.into_inner());
            flow.offer(options, &ui.provider_name)
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
            let step = ui
                .flow
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(provider, value);
            ui.login_apply(step)
        })
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
        if target != provider || choice == lca_ui::CUSTOM_OPTION {
            return;
        }
        let Some(handle) = self.registry.provider(provider).cloned() else {
            return;
        };
        let answer = lca_protocol::LoginAnswer { choice, values };
        let Ok(settings) =
            lca_core::drive_blocking(async move { handle.login_submit(answer).await })
        else {
            return;
        };
        for (key, value) in &settings {
            let _ = crate::store_provider_secret(&self.data, &self.cwd, &target, key, value);
        }
        let mut cell = self.settings_cell.lock().unwrap_or_else(|p| p.into_inner());
        cell.clear();
        cell.extend(settings);
    }
}
