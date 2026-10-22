//! The `/persona` bundle as the interface sees it (gh #412).
//!
//! Split from `state.rs` for the workspace's 1,200-line ceiling (gate 11);
//! behaviour unchanged by the split.
//!
//! The host owns config and trust (`lca-config`); the interface owns the screen
//! and the transcript. This module is the seam between them: a neutral bundle of
//! four fields, and the three closures the host answers `/persona` with. Keeping
//! the shape here rather than leaking a config type across the seam is also what
//! lets a test build one without a config file at all.

use std::sync::Arc;

/// Host answer to "can this persona be applied **at all**?" (gh #412).
///
/// `Ok(())` means every field is reachable *in this session*: the model exists in
/// the offered set, the thinking level is one that model accepts, and every tool in
/// the loadout is registered. `Err(reason)` names the first field that is not -
/// and because this runs **before** anything is applied, a refusal leaves the
/// session exactly as it was. That ordering is the whole atomic guarantee: it is
/// structural, not a discipline someone has to remember to follow.
pub type PersonaCheck = Arc<dyn Fn(&PersonaBundle) -> Result<(), String> + Send + Sync>;

/// Host answer to "apply this persona" (gh #412). Called only after
/// [`PersonaCheck`] passed, and returns the notice naming what it set.
///
/// It writes through the same seams `/model`, `/thinking` and the tool gate use, so
/// the footer, the thinking border ramp, and the per-model overrides are already in
/// sync - there is no second state cell to keep in step (the gh #369 lesson).
pub type PersonaApply = Arc<dyn Fn(&PersonaBundle) -> String + Send + Sync>;

/// `/persona`'s listing: the personas this session can apply, in name order.
pub type PersonaList = Arc<dyn Fn() -> Vec<PersonaBundle> + Send + Sync>;

/// One persona as the interface sees it (gh #412): the four fields, and nothing
/// about where they came from.
///
/// A neutral shape on purpose. `[personas]` is parsed in `lca-config` (which owns
/// the settings ladder and the trust rule), and `lca-ui` does not depend on that
/// crate - the interface owns the *screen and the transcript*, and the host owns
/// the config. Keeping the bundle here rather than leaking a config type across the
/// seam is also what lets a test build one without a config file at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PersonaBundle {
    /// The name, for the notice and the listing.
    pub name: String,
    /// The model id, or empty to leave the model alone.
    pub model: String,
    /// The thinking level: `None` leaves it alone, `Some(None)` is the provider
    /// default, `Some(Some(level))` names one.
    pub thinking: Option<Option<String>>,
    /// The tool loadout: `None` leaves it alone, `Some(vec![])` means no tools.
    pub tools: Option<Vec<String>>,
    /// A conversational prologue, if the persona carries one.
    pub instructions: Option<String>,
}
