//! Antigravity model catalog virtualization (gh #241B): Google's
//! internal endpoint variants collapse into canonical parents
//! (pi-antigravity `src/models/grouping.ts`, proportionate port).
//! Split from `lib.rs` under the workspace's 1,200-line file ceiling.

/// Thinking suffixes, longest first (pi-antigravity
/// `THINKING_SUFFIXES`): the order decides, since `extra-low` also
/// ends in `-low`.
const THINKING_SUFFIXES: &[(&str, &str)] = &[
    ("extra-low", "low"),
    ("extra-high", "xhigh"),
    ("thinking", "high"),
    ("minimal", "minimal"),
    ("medium", "medium"),
    ("high", "high"),
    ("low", "low"),
];

/// Display-name levels, longest first (pi-antigravity
/// `DISPLAY_LEVELS`): explicit parentheticals beat suffix parsing.
const DISPLAY_LEVELS: &[(&str, &str)] = &[
    ("(extra low)", "low"),
    ("(extra high)", "xhigh"),
    ("(thinking)", "high"),
    ("(minimal)", "minimal"),
    ("(medium)", "medium"),
    ("(high)", "high"),
    ("(low)", "low"),
];

/// Backend aliases whose runtime ids share no suffix with their
/// family (pi-antigravity `RUNTIME_ALIASES`).
const RUNTIME_ALIASES: &[(&str, &str, &str)] = &[
    ("gemini-3-flash-agent", "gemini-3.5-flash", "high"),
    ("gemini-pro-agent", "gemini-3.1-pro", "high"),
];

/// Canonical level order for advertised sets (the picker's order).
const LEVEL_ORDER: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];

/// Whether a discovered id may list (pi-antigravity
/// `isSelectableRuntimeModelId`).
fn is_selectable(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    if !(lower.starts_with("gemini-")
        || lower.starts_with("claude-")
        || lower.starts_with("gpt-oss-"))
    {
        return false;
    }
    if id.contains(char::is_whitespace) || id.starts_with("MODEL_") {
        return false;
    }
    let lower = lower.as_str();
    if lower.starts_with("chat_") || lower.starts_with("tab_") {
        return false;
    }
    if lower.contains("image") {
        return false;
    }
    true
}

/// The display name a catalog entry advertises, if any.
fn display_name(entry: &serde_json::Value) -> Option<&str> {
    entry
        .get("displayName")
        .or_else(|| entry.get("label"))
        .or_else(|| entry.get("modelName"))
        .and_then(|value| value.as_str())
        .filter(|name| !name.is_empty())
}

/// The level a display name declares (`Gemini 3.8 Flash (High)`),
/// if any.
fn display_level(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    DISPLAY_LEVELS
        .iter()
        .find(|(pattern, _)| lower.contains(pattern))
        .map(|(_, level)| *level)
}

/// Fold a catalog `supportsThinking` flag into the group (later
/// `true` wins, matching pi-antigravity `absorbMetadata`).
fn absorb_thinking(group: &mut RuntimeGroup, entry: &serde_json::Value) {
    match entry.get("supportsThinking").and_then(|v| v.as_bool()) {
        Some(true) => group.supports_thinking = Some(true),
        Some(false) if group.supports_thinking != Some(true) => {
            group.supports_thinking = Some(false);
        }
        _ => {}
    }
}

/// Merge lone `-agent` groups into their display family (pi-antigravity
/// `mergeAgentSingletons`): an agent id nobody aliased still routes
/// as its family's high, instead of listing as its own model.
fn merge_agent_singletons(groups: &mut Vec<(String, RuntimeGroup)>) {
    // Decisions first (shared borrows), application after: merging
    // moves one runtime id plus metadata, then drops the husk.
    let mut moves: Vec<(usize, usize, &'static str)> = Vec::new();
    for (index, (public, group)) in groups.iter().enumerate() {
        if !public.ends_with("-agent") || !group.variants.is_empty() {
            continue;
        }
        let Some(first) = group.display_names.first() else {
            continue;
        };
        let family = display_family(first);
        if family.is_empty() {
            continue;
        }
        let Some(target) = groups.iter().position(|(other, candidate)| {
            other != public
                && candidate
                    .display_names
                    .iter()
                    .any(|name| display_family(name) == family)
        }) else {
            continue;
        };
        if groups[index].1.unsuffixed.is_none() {
            continue;
        }
        let level = display_level(first).unwrap_or("high");
        moves.push((target, index, level));
    }
    let mut husks: Vec<usize> = moves.iter().map(|(_, source, _)| *source).collect();
    for (target, source, level) in moves {
        if let Some(runtime) = groups[source].1.unsuffixed.clone() {
            groups[target].1.variants.push((level, runtime));
        }
        if groups[source].1.supports_thinking == Some(true) {
            groups[target].1.supports_thinking = Some(true);
        }
        let names = groups[source].1.display_names.clone();
        groups[target].1.display_names.extend(names);
    }
    husks.sort_unstable();
    husks.dedup();
    for index in husks.into_iter().rev() {
        groups.remove(index);
    }
}

/// The display family: the name minus a trailing level
/// parenthetical, lowercased (pi-antigravity `displayFamily`).
fn display_family(name: &str) -> String {
    let mut family = name.trim().to_string();
    for (pattern, _) in DISPLAY_LEVELS {
        if family.to_ascii_lowercase().ends_with(pattern) {
            family.truncate(family.len() - pattern.len());
            break;
        }
    }
    family.trim().to_ascii_lowercase()
}

/// Emit one canonical model for a group: the family name (or the
/// humanized id), the group's max windows, and the discovered
/// thinking levels in canonical order (absent when the group
/// neither varies nor thinks, so the picker offers everything).
fn emit_group(
    public: &str,
    group: &RuntimeGroup,
    models: &serde_json::Map<String, serde_json::Value>,
) -> lca_protocol::ModelInfo {
    let name = group
        .display_names
        .iter()
        .map(|name| display_family(name))
        .find(|family| !family.is_empty())
        .map(title_case)
        .map(|family| format!("{family} (Antigravity)"))
        .unwrap_or_else(|| format!("{} (Antigravity)", humanize_public_id(public)));
    let mut context_window = 0u32;
    let mut max_tokens = 0u32;
    let mut windows = group
        .variants
        .iter()
        .map(|(_, id)| id.as_str())
        .collect::<Vec<_>>();
    if let Some(unsuffixed) = group.unsuffixed.as_deref() {
        windows.push(unsuffixed);
    }
    for id in windows {
        if let Some(entry) = models.get(id) {
            context_window = context_window.max(
                entry
                    .get("inputTokenLimit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32,
            );
            max_tokens = max_tokens.max(
                entry
                    .get("outputTokenLimit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32,
            );
        }
    }
    let mut levels: Vec<&str> = group.variants.iter().map(|(level, _)| *level).collect();
    levels.sort();
    levels.dedup();
    if levels.is_empty()
        && group.supports_thinking != Some(false)
        && (group.supports_thinking == Some(true) || group.unsuffixed.is_some())
    {
        levels.push("high");
    }
    let mut extras = std::collections::BTreeMap::new();
    if !levels.is_empty() {
        let mut ordered: Vec<&str> = LEVEL_ORDER
            .iter()
            .filter(|level| levels.contains(level))
            .copied()
            .collect();
        ordered.sort_by_key(|level| LEVEL_ORDER.iter().position(|l| l == level));
        extras.insert("thinking_levels".to_string(), ordered.join(","));
    }
    lca_protocol::ModelInfo {
        id: public.to_string(),
        name,
        context_window,
        max_tokens,
        extras,
    }
}

/// Title-case a family name (`gemini 3.8 flash` to `Gemini 3.8 Flash`).
fn title_case(family: String) -> String {
    family
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            let head = chars.next().unwrap_or('?').to_ascii_uppercase();
            format!("{head}{}", chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rank canonical ids newest-flash-first (pi-antigravity
/// `comparePublicModels`): flash by descending version, then opus,
/// sonnet, claude, pro, gemini, gpt-oss, everything else.
fn compare_public_models(a: &str, b: &str) -> std::cmp::Ordering {
    fn version(id: &str) -> u64 {
        let rest = id.strip_prefix("gemini-").unwrap_or("");
        let mut parts = rest.split(['.', '-']);
        let major: u64 = parts.next().unwrap_or("").parse().unwrap_or(0);
        let minor: u64 = parts
            .next()
            .unwrap_or("")
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        major * 1000 + minor
    }
    fn rank(id: &str) -> (u64, i64) {
        let lower = id.to_ascii_lowercase();
        if lower.starts_with("gemini-") && lower.contains("flash") && !lower.contains("pro") {
            return (0, -(version(id) as i64));
        }
        if lower.starts_with("claude-opus") {
            return (1, 0);
        }
        if lower.starts_with("claude-sonnet") {
            return (2, 0);
        }
        if lower.starts_with("claude-") {
            return (3, 0);
        }
        if lower.starts_with("gemini-") && lower.contains("pro") {
            return (4, -(version(id) as i64));
        }
        if lower.starts_with("gemini-") {
            return (5, -(version(id) as i64));
        }
        if lower.starts_with("gpt-oss") {
            return (6, 0);
        }
        (7, 0)
    }
    rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
}
fn humanize_public_id(id: &str) -> String {
    let tokens: Vec<&str> = id.split('-').collect();
    let mut words = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if token.is_empty() {
            index += 1;
            continue;
        }
        let next = tokens.get(index + 1).copied().unwrap_or("");
        if token == "gpt" && next == "oss" {
            words.push("GPT-OSS".to_string());
            index += 2;
            continue;
        }
        if token.chars().all(|c| c.is_ascii_digit())
            && next.chars().all(|c| c.is_ascii_digit())
            && !next.is_empty()
        {
            words.push(format!("{token}.{next}"));
            index += 2;
            continue;
        }
        if token.starts_with(|c: char| c.is_ascii_digit()) {
            words.push(token.to_ascii_uppercase());
        } else {
            let mut chars = token.chars();
            let head = chars.next().unwrap_or('?').to_ascii_uppercase();
            words.push(format!("{head}{}", chars.as_str()));
        }
        index += 1;
    }
    words.join(" ")
}

/// Find a family's group, creating it first.
fn group_at(groups: &mut Vec<(String, RuntimeGroup)>, public: &str) -> usize {
    if let Some(at) = groups.iter().position(|(id, _)| id == public) {
        return at;
    }
    groups.push((public.to_string(), RuntimeGroup::default()));
    groups.len() - 1
}

/// One canonical family under construction.
#[derive(Default)]
struct RuntimeGroup {
    /// level -> runtime id.
    variants: Vec<(&'static str, String)>,
    /// A suffixed id with no level, or a `-tiered` id.
    unsuffixed: Option<String>,
    display_names: Vec<String>,
    supports_thinking: Option<bool>,
}

/// Collapse a discovered catalog into canonical parents
/// (pi-antigravity `buildAntigravityCatalog`, proportionate port:
/// grouping, aliases, agent-singleton merging, and rank — the
/// fallback synthesis stays the static table, which `list_models`
/// already uses on empty).
///
/// Each parent advertises its discovered thinking levels through
/// `thinking_levels` extras (gh #242's picker reads them); a group
/// with no levels and no thinking support advertises nothing, so
/// the picker offers everything as before.
pub fn virtualize_catalog(
    models: &serde_json::Map<String, serde_json::Value>,
) -> Vec<lca_protocol::ModelInfo> {
    let mut groups: Vec<(String, RuntimeGroup)> = Vec::new();
    for (runtime, entry) in models {
        if !is_selectable(runtime) {
            continue;
        }
        if entry.get("isInternal").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let name = display_name(entry);
        if let Some(base) = runtime.strip_suffix("-tiered") {
            let at = group_at(&mut groups, base);
            let group = &mut groups[at].1;
            if let Some(name) = name {
                group.display_names.push(name.to_string());
            }
            if group.unsuffixed.is_none() {
                group.unsuffixed = Some(runtime.clone());
            }
            absorb_thinking(group, entry);
            continue;
        }
        let mut public = runtime.as_str();
        let mut level: Option<&'static str>;
        if let Some((_, parent, alias_level)) =
            RUNTIME_ALIASES.iter().find(|(id, _, _)| *id == runtime)
        {
            public = parent;
            level = Some(alias_level);
        } else {
            // Ground-truth precedence: explicit display parentheticals
            // beat suffix parsing (a `-low` id advertised as "(High)"
            // serves high).
            level = name.and_then(display_level);
            if level.is_none() {
                let lower = runtime.to_ascii_lowercase();
                for (suffix, suffix_level) in THINKING_SUFFIXES {
                    if lower.ends_with(&format!("-{suffix}")) {
                        public = &runtime[..runtime.len() - suffix.len() - 1];
                        level = Some(suffix_level);
                        break;
                    }
                }
            } else {
                // A display-leveled id still groups under its base.
                let lower = runtime.to_ascii_lowercase();
                for (suffix, _) in THINKING_SUFFIXES {
                    if lower.ends_with(&format!("-{suffix}")) {
                        public = &runtime[..runtime.len() - suffix.len() - 1];
                        break;
                    }
                }
            }
        }
        // Slicing a borrowed map key: re-own the public id before
        // the mutable group borrow below.
        let public = public.to_string();
        let at = group_at(&mut groups, &public);
        let group = &mut groups[at].1;
        if let Some(name) = name {
            group.display_names.push(name.to_string());
        }
        absorb_thinking(group, entry);
        match level {
            Some(level) => group.variants.push((level, runtime.clone())),
            None => {
                if group.unsuffixed.is_none() {
                    group.unsuffixed = Some(runtime.clone());
                }
            }
        }
    }
    merge_agent_singletons(&mut groups);
    if groups.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (public, group) in &groups {
        out.push(emit_group(public, group, models));
    }
    out.sort_by(|a, b| compare_public_models(&a.id, &b.id));
    out
}
