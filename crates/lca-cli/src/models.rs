//! The enabled-model scope and the cycle through it (gh #8, EFG-003:
//! pi's `enabledModels` and `cycleForward`/`cycleBackward`).
//!
//! One mechanism serves both halves the issue names: the scope cuts the
//! `/model` listing *and* the cycle, so what a user can see is exactly
//! what a keypress can reach. An empty scope is no restriction -
//! everything the provider offers across its profiles (gh #31).

use lca_protocol::ModelInfo;

/// The read tool's image behavior for the resolved model (#39): the
/// provider's per-model extras decide, and a model the list does not
/// name behaves exactly as today.
pub fn image_policy_for(models: &[ModelInfo], model_id: &str) -> lca_tools::ImagePolicy {
    models
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| lca_tools::ImagePolicy::for_extras(&model.extras))
        .unwrap_or_default()
}

/// Whether one model is inside the enabled scope - the same matcher
/// `--list-models` filters its search with.
///
/// A pattern containing `*` is a glob; anything else is a
/// case-insensitive substring. Both are matched against the model's id,
/// its `profile/id` canonical form (so `zen/*` scopes to a profile), the
/// profile's own label, and the display name. Empty patterns answer
/// `true`: no restriction.
pub fn in_scope(model: &ModelInfo, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let profile = model
        .extras
        .get("profile")
        .map(String::as_str)
        .unwrap_or("");
    let canonical = if profile.is_empty() {
        model.id.clone()
    } else {
        format!("{profile}/{}", model.id)
    };
    let label = model.extras.get("label").map(String::as_str).unwrap_or("");
    let needles = [
        model.id.as_str(),
        canonical.as_str(),
        label,
        model.name.as_str(),
    ];
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
                .any(|text| !text.is_empty() && text.to_lowercase().contains(&needle))
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
        .filter(|model| in_scope(model, patterns))
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

/// The `thinking-level-change` record one switch appends (gh #47
/// review): the log's witness of a level switch, mirroring
/// `model_change_record` - including its only-on-change rule, so
/// reopening the picker or re-picking the active level writes nothing.
/// `None` when the level did not move; a move to or from unset records
/// the `default` sentinel (no level in the picker's set is named that).
pub fn thinking_level_change_record(
    previous: Option<&str>,
    effective: Option<&str>,
) -> Option<lca_protocol::Record> {
    if previous == effective {
        return None;
    }
    Some(lca_protocol::Record::ThinkingLevelChange {
        v: lca_protocol::FORMAT_VERSION,
        ts: lca_session::now_ms(),
        id: lca_session::new_record_id(),
        level: effective.unwrap_or("default").to_string(),
    })
}

// Verifies: EFG-041 (the resolver rules pi's `model-resolver.ts` states,
// mirrored) - exact id, `profile/id`, an ambiguous id that names its
// candidates instead of guessing, alias-over-dated fuzzy matching, and a
// `:thinking` suffix that only splits when the pattern as a whole is not
// itself a model (an id containing colons keeps them).
/// What a model pattern resolved to: the id to run, the `:thinking`
/// suffix it carried (when any), and the profile that owns it - gh #31's
/// rule that routing follows the model makes the profile part of the
/// answer, not a detail the caller has to look up again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModel {
    /// The model id to use (never a label, G2).
    pub id: String,
    /// The `:thinking` suffix, when the pattern carried a valid one.
    pub thinking: Option<String>,
    /// The owning profile, when the model belongs to a named one.
    pub profile: Option<String>,
}

/// Resolve a model pattern the way pi's `model-resolver.ts` states it
/// (EFG-041: port the rules), in one vocabulary for every surface that
/// takes a pattern - `--model <pattern>[:thinking]` and `/model <arg>`:
///
/// 1. the pattern as a whole is a model (`profile/id`, or an id) - one
///    match wins, several matches are an error that lists them, because
///    guessing between two profiles means guessing which endpoint bills;
/// 2. otherwise a trailing `:<level>` splits off (a pattern that matched
///    in step 1 keeps its colons: `openrouter:weird` is an id);
/// 3. the base repeats steps 1-2, then falls back to a case-insensitive
///    substring of the id and name, preferring an alias over dated
///    versions (pi's tie-break);
/// 4. nothing matches: the pattern is the id itself - the endpoint may
///    know a model the list does not carry, which is the flag's
///    historical meaning and the row the suite pins.
///
/// An unknown suffix is not a level: once the pattern as a whole is not
/// a model it is dropped (pi's scope-mode fallback), never guessed at.
pub fn resolve_pattern(pattern: &str, available: &[ModelInfo]) -> Result<ResolvedModel, String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("no model named `` - pass an id, a prefix, or `profile/id`".to_string());
    }
    if let Some(model) = exact_match(pattern, available)? {
        return Ok(resolved(model, None));
    }
    let (base, thinking) = split_thinking(pattern);
    if let Some(model) = exact_match(base, available)? {
        return Ok(resolved(model, thinking));
    }
    if let Some(model) = fuzzy_match(base, available) {
        return Ok(resolved(model, thinking));
    }
    Ok(ResolvedModel {
        id: base.to_string(),
        thinking: thinking.map(str::to_string),
        profile: None,
    })
}

/// Whether a model answers to `--provider <name>`: its profile id, its
/// label, or the provider extension itself (case-insensitive) - one flag
/// covering both readings of "provider" in this product: the extension,
/// and a gh #31 profile inside it. An unknown name matches nothing, and
/// the caller says so with pi's message.
pub fn in_provider(model: &ModelInfo, name: &str, provider_name: &str) -> bool {
    let name = name.to_lowercase();
    if provider_name.eq_ignore_ascii_case(&name) {
        return true;
    }
    ["profile", "label"].iter().any(|field| {
        model
            .extras
            .get(*field)
            .is_some_and(|value| value.to_lowercase() == name)
    })
}

/// The canonical reference a model answers to: `profile/id` for a named
/// profile, the id otherwise - the same form pi matches first.
fn canonical(model: &ModelInfo) -> String {
    match model.extras.get("profile") {
        Some(profile) => format!("{profile}/{}", model.id),
        None => model.id.clone(),
    }
}

/// One exact match: a `provider/id` qualification first (gh #177),
/// then the canonical form, then the bare id. Zero matches is `None`;
/// several is an error naming every candidate.
fn exact_match<'a>(
    pattern: &str,
    available: &'a [ModelInfo],
) -> Result<Option<&'a ModelInfo>, String> {
    // A qualification names the cross-provider model directly; the
    // provider tag is the catalog's, never the profile's, so a
    // `profile/id` pattern still falls to the canonical reading below.
    if let Some((scope, id)) = pattern.split_once('/') {
        let qualified: Vec<&ModelInfo> = available
            .iter()
            .filter(|model| {
                model.id.eq_ignore_ascii_case(id)
                    && model
                        .extras
                        .get("provider")
                        .is_some_and(|provider| provider.eq_ignore_ascii_case(scope))
            })
            .collect();
        if qualified.len() == 1 {
            return Ok(Some(qualified[0]));
        }
    }
    let by_canonical: Vec<&ModelInfo> = available
        .iter()
        .filter(|model| canonical(model).eq_ignore_ascii_case(pattern))
        .collect();
    let matches = if by_canonical.is_empty() {
        available
            .iter()
            .filter(|model| model.id.eq_ignore_ascii_case(pattern))
            .collect()
    } else {
        by_canonical
    };
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0])),
        _ => Err(format!(
            "`{pattern}` is ambiguous across providers and profiles: {}. Say which one as `profile/id` or `provider/id`.",
            matches
                .iter()
                .map(|model| qualified_label(model))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The candidate label an ambiguity error lists: the profile reading
/// first (most specific within one provider), else the provider
/// reading, else the bare id.
fn qualified_label(model: &ModelInfo) -> String {
    match (model.extras.get("profile"), model.extras.get("provider")) {
        (Some(profile), _) => format!("{profile}/{}", model.id),
        (None, Some(provider)) => format!("{provider}/{}", model.id),
        _ => model.id.clone(),
    }
}

/// Split a trailing `:<level>` off a pattern that did not match whole.
/// An unknown suffix is dropped with it (pi's scope-mode fallback).
pub(crate) fn split_thinking(pattern: &str) -> (&str, Option<&str>) {
    match pattern.rsplit_once(':') {
        Some((base, suffix)) if !base.is_empty() => {
            if lca_config::THINKING_LEVELS.contains(&suffix) {
                (base, Some(suffix))
            } else {
                (base, None)
            }
        }
        _ => (pattern, None),
    }
}

/// The substring match: the id or the display name contains the pattern,
/// case-insensitively. Several matches are decided pi's way - an alias
/// (an id with no `-YYYYMMDD` version tail, or a `-latest`) beats dated
/// versions, and within a group the highest sort wins.
fn fuzzy_match<'a>(pattern: &str, available: &'a [ModelInfo]) -> Option<&'a ModelInfo> {
    let needle = pattern.to_lowercase();
    let mut matches: Vec<&ModelInfo> = available
        .iter()
        .filter(|model| {
            model.id.to_lowercase().contains(&needle) || model.name.to_lowercase().contains(&needle)
        })
        .collect();
    if matches.is_empty() {
        return None;
    }
    matches.sort_by(|a, b| b.id.cmp(&a.id));
    matches
        .iter()
        .find(|model| is_alias(&model.id))
        .copied()
        .or(Some(matches[0]))
}

/// pi's `isAlias`: an id without a `-YYYYMMDD` tail (or with `-latest`) is
/// the name people type; the dated id is the version it stands for.
fn is_alias(id: &str) -> bool {
    if id.ends_with("-latest") {
        return true;
    }
    let dated = id
        .rsplit('-')
        .next()
        .is_some_and(|tail| tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()));
    !dated
}

fn resolved(model: &ModelInfo, thinking: Option<&str>) -> ResolvedModel {
    ResolvedModel {
        id: model.id.clone(),
        thinking: thinking.map(str::to_string),
        profile: model.extras.get("profile").cloned(),
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    fn model(id: &str, profile: Option<&str>) -> ModelInfo {
        let mut extras = std::collections::BTreeMap::new();
        if let Some(profile) = profile {
            extras.insert("profile".to_string(), profile.to_string());
            extras.insert("label".to_string(), profile.to_string());
        }
        ModelInfo {
            id: id.to_string(),
            name: id.to_string(),
            context_window: 0,
            max_tokens: 0,
            extras,
        }
    }

    fn catalog() -> Vec<ModelInfo> {
        vec![
            model("claude-sonnet-4-5", None),
            model("claude-sonnet-4-5-20250929", None),
            model("mimo-v2.6-flash-free", Some("zen")),
            model("mimo-v2.6-flash-free", Some("opencode-go")),
            model(lca_testkit::SMOKE_MODEL, Some("opencode-go")),
            model("openrouter:weird", None),
        ]
    }

    // Exact id wins, and the id a user types is the id they get.
    #[test]
    fn an_exact_id_resolves_to_itself() {
        let resolved = resolve_pattern(lca_testkit::SMOKE_MODEL, &catalog()).expect("resolve");
        assert_eq!(resolved.id, lca_testkit::SMOKE_MODEL);
        assert_eq!(resolved.thinking, None, "no suffix, no thinking level");
    }

    // Verifies: gh #177 - a `provider/id` qualification picks the
    // cross-provider model directly, while a bare shared id stays
    // ambiguous and lists the provider readings.
    #[test]
    fn a_provider_qualified_id_resolves_across_providers() {
        let catalog = vec![
            model("shared", None),
            lca_protocol::ModelInfo {
                id: "shared".to_string(),
                name: "shared (codex)".to_string(),
                context_window: 0,
                max_tokens: 0,
                extras: [("provider".to_string(), "codex".to_string())].into(),
            },
        ];
        let resolved = resolve_pattern("codex/shared", &catalog).expect("resolve");
        assert_eq!(resolved.id, "shared");
        let err = resolve_pattern("shared", &catalog).expect_err("still shared");
        assert!(
            err.contains("codex/shared"),
            "lists the provider reading: {err}"
        );
        assert!(err.contains("provider/id"), "says how to fix it: {err}");
    }

    // `profile/id` picks the profile's model when the bare id is shared.
    #[test]
    fn a_profile_qualified_id_picks_that_profile() {
        let resolved = resolve_pattern("zen/mimo-v2.6-flash-free", &catalog()).expect("resolve");
        assert_eq!(resolved.id, "mimo-v2.6-flash-free");
        assert_eq!(
            resolved.profile.as_deref(),
            Some("zen"),
            "the winner carries its profile, so routing follows it (gh #31)"
        );
    }

    // The same id under two profiles is ambiguous and says so, listing the
    // candidates - pi's `resolveCliModel` error, in our vocabulary.
    #[test]
    fn a_shared_id_is_ambiguous_and_lists_the_profiles() {
        let err = resolve_pattern("mimo-v2.6-flash-free", &catalog())
            .expect_err("two profiles own this id");
        assert!(err.contains("ambiguous"), "names the problem: {err}");
        assert!(
            err.contains("zen/mimo-v2.6-flash-free")
                && err.contains("opencode-go/mimo-v2.6-flash-free"),
            "lists both candidates: {err}"
        );
        assert!(err.contains("profile/id"), "says how to fix it: {err}");
    }

    // Fuzzy matching prefers the alias over dated versions (pi's rule),
    // and matches the id as a substring.
    #[test]
    fn a_fuzzy_pattern_prefers_the_alias_over_a_dated_version() {
        let resolved = resolve_pattern("claude-sonnet-4-5-", &catalog()).expect("resolve");
        assert_eq!(
            resolved.id, "claude-sonnet-4-5-20250929",
            "no alias matches that hyphen, so the dated one is the match"
        );
        let resolved = resolve_pattern("sonnet", &catalog()).expect("resolve");
        assert_eq!(
            resolved.id, "claude-sonnet-4-5",
            "the alias wins when both match"
        );
        let resolved =
            resolve_pattern(&lca_testkit::SMOKE_MODEL.to_uppercase(), &catalog()).expect("resolve");
        assert_eq!(resolved.id, lca_testkit::SMOKE_MODEL, "case-insensitive");
    }

    // A `:thinking` suffix splits off when it names a level; a pattern
    // that matches as a whole never splits (ids containing colons keep
    // them), and an unknown suffix is part of the id.
    #[test]
    fn a_thinking_suffix_splits_only_when_the_pattern_is_not_itself_a_model() {
        let resolved = resolve_pattern("sonnet:high", &catalog()).expect("resolve");
        assert_eq!(resolved.id, "claude-sonnet-4-5");
        assert_eq!(resolved.thinking.as_deref(), Some("high"));

        let resolved = resolve_pattern("openrouter:weird", &catalog()).expect("resolve");
        assert_eq!(
            resolved.id, "openrouter:weird",
            "an id that matches as a whole keeps its colon"
        );
        assert_eq!(resolved.thinking, None);

        let resolved = resolve_pattern("openrouter:weird:medium", &catalog()).expect("resolve");
        assert_eq!(resolved.id, "openrouter:weird");
        assert_eq!(resolved.thinking.as_deref(), Some("medium"));

        let resolved = resolve_pattern(
            &format!("{}:not-a-level", lca_testkit::SMOKE_MODEL),
            &catalog(),
        )
        .expect("resolve");
        assert_eq!(
            resolved.id,
            lca_testkit::SMOKE_MODEL,
            "the id itself, no split"
        );
        assert_eq!(resolved.thinking, None, "an unknown suffix is not a level");
    }

    // A pattern no model matches is used as given: the endpoint may still
    // know an id the list does not carry (the flag's historical meaning,
    // and the row `the_model_flag_beats_env_and_config_at_startup` pins).
    #[test]
    fn a_pattern_nothing_matches_is_taken_as_the_id_itself() {
        let resolved = resolve_pattern("flag-model", &catalog()).expect("resolve");
        assert_eq!(resolved.id, "flag-model");
        assert_eq!(resolved.thinking, None);
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
            model(lca_testkit::SMOKE_MODEL, Some("opencode-go")),
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
            // Arbitrary filter-fixture data, not the live model: any
            // stable permitted id keeps these counts.
            model("muse-spark-1.3-contributor", Some("opencode-go")),
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
            &["*spark*".to_string(), "mimo-v2.5".to_string()],
        );
        assert_eq!(two.len(), 2, "several patterns union");

        let miss = filter_enabled(all.clone(), &["nothing-matches-this".to_string()]);
        assert!(
            miss.is_empty(),
            "a scope that matches nothing offers nothing"
        );
    }

    // Verifies: gh #8 (EFG-041) - `--provider` scopes by profile id, by
    // the row's label, or by the provider extension itself, so both
    // readings of "provider" in this product answer to one flag, and a
    // name nothing owns matches nothing.
    #[test]
    fn provider_scope_matches_a_profile_the_label_or_the_extension() {
        let zen = model("mimo-v2.6-flash-free", Some("zen"));
        let plain = model("plain", None);
        assert!(in_provider(&zen, "zen", "openai-compatible"), "profile id");
        assert!(in_provider(&zen, "ZEN", "openai-compatible"), "case-blind");
        assert!(
            in_provider(&plain, "openai-compatible", "openai-compatible"),
            "the extension"
        );
        assert!(
            !in_provider(&zen, "opencode-go", "openai-compatible"),
            "another profile"
        );
        assert!(
            !in_provider(&plain, "zen", "openai-compatible"),
            "no profile, no match"
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

    // Verifies: gh #47 review (a level switch appends a record, like a
    // model switch): a move lands with the level, a re-pick appends
    // nothing, and a move to the provider default records `default`.
    #[test]
    fn a_thinking_switch_appends_a_record_and_a_repick_does_not() {
        let moved = thinking_level_change_record(None, Some("high"));
        let Some(lca_protocol::Record::ThinkingLevelChange { level, .. }) = moved else {
            panic!("a switch must produce a record");
        };
        assert_eq!(level, "high");
        assert!(
            thinking_level_change_record(Some("high"), Some("high")).is_none(),
            "re-picking the active level is no change"
        );
        let Some(lca_protocol::Record::ThinkingLevelChange { level, .. }) =
            thinking_level_change_record(Some("high"), None)
        else {
            panic!("a move to unset must produce a record");
        };
        assert_eq!(level, "default");
    }

    // Verifies: gh #47 review (the record lands in the log): the
    // helper's record appends through the store and reads back.
    #[test]
    fn a_thinking_switch_record_lands_in_the_log() {
        let root = lca_testkit::scratch_path("lca-thinking-record");
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = lca_session::SessionStore::new(root.clone());
        let session = store.create_session(&project, "test").expect("create");
        let record =
            thinking_level_change_record(None, Some("high")).expect("a switch produces a record");
        store.append(&session, record).expect("append");
        let outcome = store.read(&session).expect("read");
        assert!(!outcome.truncated);
        assert!(
            outcome.records.iter().any(|r| matches!(
                r,
                lca_protocol::Record::ThinkingLevelChange { level, .. }
                if level == "high"
            )),
            "the thinking-level-change record is in the log"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
