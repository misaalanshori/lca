//! User model-metadata overrides, `~/.lca/models.toml` (gh #64):
//! per-model fixes without a release. Parsing, `$VAR` interpolation,
//! and application to resolved rows; unknown ids are ignored (pi's
//! rule) and a malformed file yields no entries.

/// One user model-metadata override from `~/.lca/models.toml` (gh #64):
/// a metadata fix without a release. Unknown ids are ignored (pi's
/// rule); a malformed file yields no entries (the override-presets
/// precedent: an optional file must not break sign-in).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelOverride {
    /// The `ModelInfo.id` this fixes.
    pub id: String,
    /// Optional provider scope: applies only on this provider.
    pub provider: Option<String>,
    /// Fixed window; resolved `$VAR` at parse, absent when unset.
    pub context_window: Option<u32>,
    /// Fixed input modalities; derives `vision`.
    pub input: Option<Vec<String>>,
    /// Fixed resize profile (missing fields take pi's conservative
    /// defaults, `jpeg_quality` aside: the extras shape has no slot).
    pub resize: Option<lca_protocol::ImageResize>,
    /// Fixed cache lifetimes in seconds, per tier.
    pub prompt_cache: Option<PromptCache>,
}

/// Cache lifetimes in seconds, pi's `promptCache` tiers. Carried, not
/// yet acted on: no warming consumer exists (EFG-008 owns it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromptCache {
    /// The short-retention tier, when published.
    pub short: Option<u32>,
    /// The long-retention tier, when published.
    pub long: Option<u32>,
}

/// Resolve one integer-ish TOML value: a plain integer, or a `$VAR` /
/// `${VAR}` string read from the process environment (already trusted
/// input). Anything else - including a leading `!command`, which never
/// executes - resolves absent.
fn resolve_uint(value: &toml::Value) -> Option<u32> {
    match value {
        toml::Value::Integer(n) => u32::try_from(*n).ok(),
        toml::Value::String(text) => {
            let name = text
                .strip_prefix('$')
                .map(|name| {
                    name.strip_prefix('{')
                        .and_then(|name| name.strip_suffix('}'))
                        .unwrap_or(name)
                })
                .unwrap_or(text.as_str());
            if name != text.as_str() {
                return std::env::var(name)
                    .ok()
                    .and_then(|value| value.parse().ok());
            }
            text.parse().ok()
        }
        _ => None,
    }
}

/// Parse the user override file. Entries need an `id`; unknown ids
/// survive parsing (they are ignored at apply time, pi's rule).
/// `$VAR` resolves now, against the process environment.
pub fn parse_model_overrides(text: &str) -> Vec<ModelOverride> {
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    value
        .get("model")
        .and_then(|models| models.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let table = item.as_table()?;
                    let id = table.get("id")?.as_str()?.to_string();
                    let provider = table
                        .get("provider")
                        .and_then(|value| value.as_str())
                        .map(str::to_string);
                    let context_window = table.get("context_window").and_then(resolve_uint);
                    let input = table.get("input").and_then(|value| {
                        value.as_array().map(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.as_str().map(str::to_string))
                                .collect()
                        })
                    });
                    let resize = table
                        .get("input_limits")
                        .and_then(|value| value.as_table())
                        .and_then(|limits| limits.get("images"))
                        .and_then(|value| value.as_table())
                        .and_then(|images| images.get("resize"))
                        .and_then(|value| value.as_table())
                        .and_then(|resize| {
                            let width = resize.get("max_width").and_then(resolve_uint)?;
                            let height = resize.get("max_height").and_then(resolve_uint)?;
                            let bytes = resize
                                .get("max_bytes")
                                .and_then(resolve_uint)
                                .unwrap_or(4_500_000);
                            Some(lca_protocol::ImageResize {
                                max_width: width,
                                max_height: height,
                                max_bytes: bytes as usize,
                            })
                        });
                    let prompt_cache = table
                        .get("prompt_cache")
                        .and_then(|value| value.as_table())
                        .and_then(|cache| {
                            let short = cache.get("short").and_then(resolve_uint);
                            let long = cache.get("long").and_then(resolve_uint);
                            (short.is_some() || long.is_some())
                                .then_some(PromptCache { short, long })
                        });
                    Some(ModelOverride {
                        id,
                        provider,
                        context_window,
                        input,
                        resize,
                        prompt_cache,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The override for one model on one provider, if any.
pub fn override_for<'a>(
    overrides: &'a [ModelOverride],
    provider: &str,
    id: &str,
) -> Option<&'a ModelOverride> {
    overrides.iter().find(|item| {
        item.id == id
            && item
                .provider
                .as_deref()
                .is_none_or(|scope| scope == provider)
    })
}

/// An override's extras for the model's row (gh #64): vision derived
/// from the `input` list, the resize profile in the #39 shape, cache
/// lifetimes carried for the warming epic. Mirrors `image_extras`.
pub fn override_extras(item: &ModelOverride) -> Vec<(String, String)> {
    let mut extras = Vec::new();
    if let Some(input) = &item.input {
        extras.push((
            lca_protocol::IMAGE_VISION_EXTRA.to_string(),
            input.iter().any(|kind| kind == "image").to_string(),
        ));
    }
    if let Some(resize) = &item.resize {
        extras.push((
            lca_protocol::IMAGE_RESIZE_EXTRA.to_string(),
            format!(
                "{}x{}:{}",
                resize.max_width, resize.max_height, resize.max_bytes
            ),
        ));
    }
    if let Some(cache) = &item.prompt_cache {
        let mut tiers = Vec::new();
        if let Some(short) = cache.short {
            tiers.push(format!("short={short}"));
        }
        if let Some(long) = cache.long {
            tiers.push(format!("long={long}"));
        }
        if !tiers.is_empty() {
            extras.push((
                lca_protocol::PROMPT_CACHE_EXTRA.to_string(),
                tiers.join(","),
            ));
        }
    }
    extras
}

/// Apply user overrides to resolved models (gh #64): user beats curated
/// and discovered (both already folded into the rows), explicit env
/// still wins the window. Unknown ids match nothing and change nothing.
pub fn apply_model_overrides(
    models: Vec<lca_protocol::ModelInfo>,
    overrides: &[ModelOverride],
    provider: &str,
) -> Vec<lca_protocol::ModelInfo> {
    models
        .into_iter()
        .map(|mut model| {
            let Some(item) = override_for(overrides, provider, &model.id) else {
                return model;
            };
            if let Some(window) = item.context_window {
                model.context_window = window;
            }
            for (key, value) in override_extras(item) {
                model.extras.insert(key, value);
            }
            model
        })
        .collect()
}
