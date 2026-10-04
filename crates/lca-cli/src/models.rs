//! The enabled-model scope and the cycle through it (gh #8, EFG-003:
//! pi's `enabledModels` and `cycleForward`/`cycleBackward`).
//!
//! One mechanism serves both halves the issue names: the scope cuts the
//! `/model` listing *and* the cycle, so what a user can see is exactly
//! what a keypress can reach. An empty scope is no restriction -
//! everything the provider offers across its profiles (gh #31).

use lca_protocol::ModelInfo;

/// Whether one model is inside the enabled scope.
///
/// A pattern containing `*` is a glob; anything else is a
/// case-insensitive substring. Both are matched against the model's id,
/// its `profile/id` canonical form (so `zen/*` scopes to a profile), and
/// its display label. Empty scope answers `true`: no restriction.
pub fn in_scope(
    id: &str,
    label: &str,
    extras: &std::collections::BTreeMap<String, String>,
    patterns: &[String],
) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let profile = extras.get("profile").map(String::as_str).unwrap_or("");
    let canonical = if profile.is_empty() {
        id.to_string()
    } else {
        format!("{profile}/{id}")
    };
    let needles = [id, canonical.as_str(), label];
    patterns.iter().any(|pattern| {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return false;
        }
        if pattern.contains('*') {
            needles
                .iter()
                .any(|needle| lca_permissions::wildcard_match(pattern, needle))
        } else {
            let needle = pattern.to_lowercase();
            needles
                .iter()
                .any(|text| text.to_lowercase().contains(&needle))
        }
    })
}

/// Cut a model list to the enabled scope, keeping the provider's order
/// (the cycle's order is the list's order - pi cycles over the same
/// snapshot the picker shows).
pub fn filter_enabled(models: Vec<ModelInfo>, patterns: &[String]) -> Vec<ModelInfo> {
    if patterns.is_empty() {
        return models;
    }
    models
        .into_iter()
        .filter(|model| in_scope(&model.id, &model.name, &model.extras, patterns))
        .collect()
}

/// The index a cycle step lands on (pi's `cycleForward`/`cycleBackward`
/// arithmetic, including pi's rule for a current model the list does not
/// contain: start from the head and advance, so the first step *enters*
/// the scope). `None` when there is nothing to cycle to.
pub fn cycle_index(current: Option<usize>, len: usize, forward: bool) -> Option<usize> {
    if len <= 1 {
        return None;
    }
    let index = current.unwrap_or(0);
    Some(if forward {
        (index + 1) % len
    } else {
        (index + len - 1) % len
    })
}

/// The `model-change` record one switch appends (gh #8, EFG-013): the
/// model that left, the model that arrived, and the provider/profile that
/// will answer for it - the log's witness of a switch, and the payload a
/// reader needs to reconstruct which endpoint was billed (gh #31: routing
/// follows the model). `from` is absent when the session had no model yet.
pub fn model_change_record(
    from: Option<&str>,
    to: &str,
    provider: &str,
    profile: Option<&str>,
) -> lca_protocol::Record {
    lca_protocol::Record::ModelChange {
        v: lca_protocol::FORMAT_VERSION,
        ts: lca_session::now_ms(),
        id: lca_session::new_record_id(),
        from: from
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string),
        to: to.to_string(),
        provider: provider.to_string(),
        profile: profile.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, profile: Option<&str>) -> ModelInfo {
        let mut extras = std::collections::BTreeMap::new();
        if let Some(profile) = profile {
            extras.insert("profile".to_string(), profile.to_string());
            extras.insert("label".to_string(), profile.to_string());
        }
        let label = extras
            .get("label")
            .map(String::as_str)
            .unwrap_or("openai-compatible");
        ModelInfo {
            id: id.to_string(),
            name: format!("{id} ({label})"),
            context_window: 0,
            max_tokens: 0,
            extras,
        }
    }

    // Verifies: gh #8 - an empty scope is no restriction: every offered
    // model, across profiles, is in the cycle and in the picker.
    #[test]
    fn an_empty_scope_keeps_every_offered_model() {
        let all = vec![
            model("zen-free", Some("zen")),
            model("bunny-free", Some("opencode-go")),
            model("plain", None),
        ];
        assert_eq!(filter_enabled(all.clone(), &[]), all, "empty = all");
    }

    // Verifies: gh #8 - `models.enabled` / `--models` restrict the set by
    // exact id, substring, glob, and profile (`zen/*` and bare `zen` both
    // mean "the zen profile's models").
    #[test]
    fn the_scope_matches_ids_profiles_and_globs() {
        let all = vec![
            model("mimo-v2.6-flash-free", Some("zen")),
            model("mimo-v2.5-free", Some("zen")),
            model("space-bunny-free", Some("opencode-go")),
            model("longcat-2.5-preview-free", Some("opencode-go")),
        ];

        let exact = filter_enabled(all.clone(), &["mimo-v2.5-free".to_string()]);
        assert_eq!(exact.len(), 1, "an exact id: {exact:?}");

        let substring = filter_enabled(all.clone(), &["MIMO-V2.6".to_string()]);
        assert_eq!(substring.len(), 1, "case-insensitive substring");

        let glob = filter_enabled(all.clone(), &["zen/*".to_string()]);
        assert_eq!(glob.len(), 2, "the profile glob: {glob:?}");

        let profile = filter_enabled(all.clone(), &["opencode-go".to_string()]);
        assert_eq!(profile.len(), 2, "the bare profile name");

        let two = filter_enabled(
            all.clone(),
            &["*bunny*".to_string(), "mimo-v2.5".to_string()],
        );
        assert_eq!(two.len(), 2, "several patterns union");

        let miss = filter_enabled(all.clone(), &["nothing-matches-this".to_string()]);
        assert!(
            miss.is_empty(),
            "a scope that matches nothing offers nothing"
        );
    }

    // Verifies: gh #8 (pi's cycle arithmetic) - forward wraps to the head,
    // backward wraps to the tail, a model outside the scope enters the
    // scope instead of sticking, and one model is not a cycle.
    #[test]
    fn the_cycle_wraps_and_refuses_a_singleton() {
        assert_eq!(cycle_index(Some(0), 3, true), Some(1));
        assert_eq!(cycle_index(Some(2), 3, true), Some(0), "wrap forward");
        assert_eq!(cycle_index(Some(0), 3, false), Some(2), "wrap backward");
        assert_eq!(
            cycle_index(None, 3, true),
            Some(1),
            "pi: head, then advance"
        );
        assert_eq!(cycle_index(None, 3, false), Some(2));
        // A current model the scope does not contain is `None` to this
        // function (the caller's find failed), and pi's rule applies:
        // head, then advance - the first step *enters* the scope.
        assert_eq!(cycle_index(None, 3, false), Some(2));
        assert_eq!(cycle_index(Some(0), 1, true), None, "one model, no cycle");
        assert_eq!(cycle_index(Some(0), 0, true), None, "no models, no cycle");
        assert_eq!(cycle_index(Some(1), 2, false), Some(0));
        assert_eq!(
            cycle_index(Some(0), 2, false),
            Some(1),
            "two models ping-pong"
        );
    }
}
