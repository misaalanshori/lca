//! The overlay layer stack (gh #373 gap 5): **one** ordered list of what is on
//! screen, above the transcript, and the single answer to two questions that used
//! to be answered by three independent predicates.
//!
//! # Why a stack, and not the predicates
//!
//! Keyboard ownership was a set of independent booleans — `picker_open()`,
//! `world.modal_active()`, `search.is_some()` — plus separate paint, mouse and focus
//! code that each consulted them in **its own order**. A set cannot answer "who is
//! next", and the orders disagreed. The measured consequences, before this module:
//!
//! * a permission prompt raised over an open `/model` picker **owned every key and
//!   painted nothing**, because `handle_modal_key` runs first in the key path while
//!   `compose_pickers` returns early in the paint path. A user answered a question
//!   they could not see;
//! * the same prompt over a picker sent **mouse** clicks to the picker
//!   (`handle_mouse_event` asks `picker_open()` first) while keys went to the prompt.
//!
//! Neither was a missing branch. Both were the same structural fault: three
//! predicates, four consumers, and no shared order.
//!
//! # The model
//!
//! [`Layer`] is what can sit above the transcript. [`LayerStack`] holds the
//! currently-raised ones in **z-order, bottom first**, so the last entry is what the
//! user is looking at and what takes the next keystroke. Every consumer asks the
//! stack:
//!
//! | Question | Answer |
//! |---|---|
//! | what to paint, and what covers what | [`LayerStack::top`] and [`LayerStack::contains`] |
//! | who takes this key | [`LayerStack::top`] |
//! | who owns the caret | [`LayerStack::top`] |
//! | whose box this click is inside | the same [`LayerStack::top`], shared by the pointer path |
//! | does the editor have focus | [`LayerStack::top`] answering `None` |
//!
//! # Focus order is not insertion order
//!
//! Raising a layer that is already below brings it to the front ([`LayerStack::raise`]
//! is a move, not a push). That is pi's `focusOrder`: focus brings a layer forward.
//! It is what makes "the wizard paints above the list it was opened from, and the list
//! is back when the wizard closes" true *by construction* rather than by which branch
//! happens to run first — and it is why closing a layer can never leave the one
//! beneath it unreachable.
//!
//! # What this module deliberately does not have
//!
//! pi's three focus-restore states (`inactive`/`eligible`/`blocked`) exist for a
//! focused overlay that **yields** the keyboard to an interloper and reclaims it
//! later. LCA has no such surface: every layer that takes the keyboard is modal in
//! the sense that closing it returns to whatever is beneath, which the z-order gives
//! for free. So two states are implemented — present, or absent — and a third would
//! be machinery no row could exercise. See the report for what would need it.

/// One surface that can sit above the transcript and take input.
///
/// Coarse-grained on purpose: the stack answers "which surface", and each surface's
/// own module decides what a key means inside it. Splitting a picker into one layer
/// per picker would make the stack a registry of fields, which is the shape this
/// module exists to replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    /// A searchable list: `/model`, `/theme`, `/settings`, `/tree`, the hub, the
    /// extension manager, the MCP manager, the install consent.
    Picker,
    /// A question the user must answer: a permission prompt, a login step, a
    /// `[y/n]` confirm, a host dialog, an extension modal.
    Modal,
    /// The side panel (gh #207). Owns no keyboard, but it is composited above the
    /// transcript and below the modal layer, so it occupies a rung.
    Panel,
    /// The transcript search readout (FR-UI-12): dock chrome, beneath both.
    Search,
}

impl Layer {
    /// Whether this layer takes the keyboard while it is on top.
    ///
    /// The panel does not: it is a display surface with its own toggle key, and
    /// saying so here is what keeps `handle_panel` a *fallback* rather than a rival
    /// owner.
    pub fn owns_keyboard(self) -> bool {
        !matches!(self, Layer::Panel)
    }
}

/// The ordered set of raised layers, bottom first.
///
/// Small by nature — at most one picker, one modal, one wizard, the panel and the
/// search readout — so a `Vec` with linear `contains` is the right container; a map
/// would cost more than it saves and would lose the order the whole module is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerStack {
    layers: Vec<Layer>,
}

impl LayerStack {
    /// An empty stack: the editor has the keyboard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Raise a layer, bringing it to the front if it is already in the stack.
    ///
    /// A move rather than a push, which is the whole "focus order is not insertion
    /// order" rule: a wizard raised above the list it came from moves, and closing it
    /// leaves the list in place.
    pub fn raise(&mut self, layer: Layer) {
        self.layers.retain(|held| *held != layer);
        self.layers.push(layer);
    }

    /// Whether the layer is raised at all.
    pub fn contains(&self, layer: Layer) -> bool {
        self.layers.contains(&layer)
    }

    /// The layer the user is looking at: the last raised that owns the keyboard.
    ///
    /// Skips keyboard-less layers (the panel), so "who takes this key" is answered by
    /// the topmost surface that can take one rather than by whatever happens to be
    /// composited highest. When only the panel is up this answers `None`, and the
    /// editor keeps the keyboard — which is correct: the panel never took it.
    pub fn top(&self) -> Option<Layer> {
        self.layers
            .iter()
            .rev()
            .copied()
            .find(|l| l.owns_keyboard())
    }

    /// Whether `layer` is covered by something above it.
    ///
    /// A covered layer is not the one taking input, so a surface that paints below
    /// another must not also answer for it.
    pub fn is_covered(&self, layer: Layer) -> bool {
        match self.layers.iter().position(|held| *held == layer) {
            Some(at) => self.layers[at + 1..].iter().any(|l| l.owns_keyboard()),
            None => false,
        }
    }
}

#[cfg(test)]
impl LayerStack {
    /// Lower a layer, wherever it is in the stack. Test-only:
    ///
    /// **Not `pop`.** A middle close — a picker dismissed while a modal sits above it —
    /// has to remove the picker and leave the modal on top, and a `pop` would remove the
    /// modal instead. `retain` is the whole repair: because the stack is ordered by
    /// z-order rather than by insertion, removing any member leaves the rest coherent.
    /// Production never needs it because the stack is *derived* from the surface fields
    /// every time it is asked, so a closed layer is simply absent from the next
    /// derivation - which is a stronger guarantee than a correctly-placed `retain`.
    fn lower(&mut self, layer: Layer) {
        self.layers.retain(|held| *held != layer);
    }

    /// Whether nothing is raised: **the editor has the keyboard.**
    fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// The topmost raised layer of any kind, for the paint order.
    fn topmost(&self) -> Option<Layer> {
        self.layers.last().copied()
    }

    /// The layer stacking order, bottom first.
    fn order(&self) -> &[Layer] {
        &self.layers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_stack_has_no_owner() {
        let stack = LayerStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.top(), None);
        assert_eq!(stack.topmost(), None);
    }

    // Verifies: gh #373 gap 5 - raising is a **move**, so re-raising the lower layer
    // brings it to the front and the order is focus order, not insertion order.
    #[test]
    fn re_raising_a_layer_brings_it_to_the_front() {
        let mut stack = LayerStack::new();
        stack.raise(Layer::Picker);
        stack.raise(Layer::Modal);
        assert_eq!(stack.top(), Some(Layer::Modal));
        stack.raise(Layer::Picker);
        assert_eq!(
            stack.top(),
            Some(Layer::Picker),
            "focus moved the picker up"
        );
        assert_eq!(stack.order(), &[Layer::Modal, Layer::Picker]);
    }

    // Verifies: gh #373 gap 5 - **a middle close repairs rather than popping the
    // wrong end.** Removing the picker leaves the modal on top.
    #[test]
    fn lowering_the_middle_of_three_leaves_the_rest_coherent() {
        let mut stack = LayerStack::new();
        stack.raise(Layer::Search);
        stack.raise(Layer::Picker);
        stack.raise(Layer::Modal);
        stack.lower(Layer::Picker);
        assert_eq!(stack.order(), &[Layer::Search, Layer::Modal]);
        assert_eq!(stack.top(), Some(Layer::Modal));
        assert!(stack.contains(Layer::Search), "the bottom survives too");
    }

    // Verifies: gh #373 gap 5 - a keyboard-less layer never answers "who takes this
    // key", however high it is composited.
    #[test]
    fn the_panel_is_not_a_keyboard_owner() {
        let mut stack = LayerStack::new();
        stack.raise(Layer::Panel);
        assert_eq!(stack.top(), None, "the editor keeps the keyboard");
        assert_eq!(stack.topmost(), Some(Layer::Panel), "but it is composited");
        assert!(!stack.is_empty());
    }

    // Verifies: gh #373 gap 5 - a covered layer is not the one answering.
    #[test]
    fn a_covered_layer_is_not_the_owner() {
        let mut stack = LayerStack::new();
        stack.raise(Layer::Picker);
        assert!(!stack.is_covered(Layer::Picker), "nothing above it yet");
        stack.raise(Layer::Modal);
        assert!(stack.is_covered(Layer::Picker));
        assert!(!stack.is_covered(Layer::Modal), "the top covers nothing");
        assert!(
            !stack.is_covered(Layer::Search),
            "an absent layer covers nothing"
        );
    }
}
