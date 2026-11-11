//! Mermaid diagrams as Unicode art: pi renders top-level
//! ```` ```mermaid ```` blocks through `grok-mermaid` (RE
//! `agent-components/chrome.md` §mermaid).
//!
//! `grok-mermaid` is an npm package (0.2.3), so its JavaScript cannot be
//! taken; this is the subset the review names - flowcharts and sequence
//! diagrams - behind the same fence, emitting the same span classes
//! (`border`, `text`, `edge`, `edgeLabel`, `title`) that the theme
//! colors, and the same contract: `None` for anything it cannot parse
//! (markdown falls back to the fenced source), a width guard in the
//! caller, and warnings that make markdown prefer the raw source while a
//! message is still streaming.
//!
//! The module is a **barrel over the parser and the renderer** (gh #572):
//! `syntax.rs` decides what there is to draw (the model, the statement scan's
//! vocabulary, arrow tables, the canvas) and `render.rs` paints it. Neither
//! measures text itself - a renderer that needs a cell answer asks
//! `engine::text` - so the width rules stay in one place and the dependency
//! between the two is that single edge.

/// grok-mermaid's span classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Box-drawing borders.
    Border,
    /// Node and note text.
    Text,
    /// Arrows and connectors.
    Edge,
    /// Labels sitting on or beside an edge.
    EdgeLabel,
    /// The diagram title.
    Title,
    /// Untouched text.
    None,
}

/// One styled run of a diagram row.
#[derive(Debug, Clone)]
pub struct Span {
    /// The literal text.
    pub text: String,
    /// Which class it carries.
    pub class: Class,
}

impl Span {
    fn new(text: impl Into<String>, class: Class) -> Self {
        Self {
            text: text.into(),
            class,
        }
    }
}

/// A rendered diagram: rows of spans, its width, and any warnings.
#[derive(Debug, Clone)]
pub struct Art {
    /// The rows (grok-mermaid's `art.styled`).
    pub lines: Vec<Vec<Span>>,
    /// Width in cells (pi's `art.width` guard).
    pub width: usize,
    /// Parse-time warnings (pi's `art.warnings`).
    pub warnings: Vec<String>,
}

/// How many parsed diagrams the cache holds before it evicts.
///
/// **The bound is the decision, not an afterthought** (gh #325): an unbounded
/// cache would be the leak with a longer lifetime. Twenty is chosen against the
/// shape of the thing being cached - a transcript shows a handful of diagrams at
/// once, and the scroll-back that re-parses is a screenful - so a session with
/// one diagram in it holds one entry, and a session with thirty holds the last
/// twenty.
pub const PARSE_CACHE_SIZE: usize = 20;

thread_local! {
    /// Parsed diagrams by source text (gh #325), on the engine's one cache policy
    /// (gh #579).
    ///
    /// The parse is **width-independent** — `render` emits spans and a `width`,
    /// and the caller decides whether it fits - so one entry serves every resize
    /// and every repaint at any width. Without this cache a repaint re-parsed,
    /// which meant the leak fired on each one; with it, the fix for the leak is
    /// also the fix for the per-frame cost. See [`render`].
    ///
    /// **Least-recently-used, not insertion order** (gh #579). This cache copied
    /// `cache.keys().next()` from the width cache, which is `HashMap` hash order -
    /// the victim was arbitrary, so a diagram on screen could be evicted by an
    /// unrelated insert and re-parsed on the next frame. Measured before the fix:
    /// **37 wasted re-parses across 60 repaints** of one visible diagram while 60
    /// others scrolled past it.
    ///
    /// The defensible case for insertion order here - a transcript shows a bounded
    /// handful, so recency and insertion order rarely differ - does not survive
    /// that measurement, and even if it had, a second policy in one crate is the
    /// thing #323 existed to prevent. LRU is the answer for the same reason it was
    /// for widths: the diagram a frame draws is the one it will draw again.
    static PARSE_CACHE: std::cell::RefCell<crate::engine::lru::LruCache<String, Option<Art>>> =
        std::cell::RefCell::new(crate::engine::lru::LruCache::new(PARSE_CACHE_SIZE));
}

/// Render mermaid source, or `None` when this subset cannot parse it
/// (markdown keeps the fenced source, pi's fail-soft contract).
///
/// **Cached** (gh #325): the result is keyed by the exact source text and reused
/// until `PARSE_CACHE_SIZE` less-recently-used diagrams push it out. A `None` is
/// cached too, because "this is not a diagram this subset can draw" is the answer
/// that a streaming message asks for on every frame - and re-deciding it per frame
/// is the same wasted work as re-parsing.
///
/// A hit marks the diagram most-recently-used (gh #579), which is what keeps the
/// one on screen from being evicted by a diagram that merely scrolled past.
pub fn render(source: &str) -> Option<Art> {
    let key = source.to_string();
    if let Some(hit) = PARSE_CACHE.with(|cache| cache.borrow_mut().get(&key)) {
        return hit;
    }
    let parsed = parse(source);
    PARSE_CACHE.with(|cache| cache.borrow_mut().insert(key, parsed.clone()));
    parsed
}

/// How many times the parser has run, for tests (gh #325).
///
/// `pub(crate)` and behind a `#[cfg(test)]` read: the count exists so a row can
/// assert that a repaint is served from the cache rather than re-parsed, which no
/// allocation count can show once the leak is gone (a second parse of the same
/// text allocates and frees the same bytes, so live-byte totals cannot see it).
/// A plain `AtomicUsize` rather than a feature flag: it is one relaxed increment
/// on a path that already builds a canvas of rows.
static PARSE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many real parses have run in this process.
///
/// Public because the row that asserts "a repaint is served from the cache" lives
/// in an integration test, which cannot see `pub(crate)` items. Nothing in the
/// product calls it; it reads one atomic and cannot affect rendering.
#[must_use]
pub fn parse_count() -> usize {
    PARSE_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Parse without consulting the cache. [`render`] is the public entry.
fn parse(source: &str) -> Option<Art> {
    PARSE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut kind = None;
    let mut title: Option<String> = None;
    let mut warnings: Vec<String> = Vec::new();
    let mut statements: Vec<&str> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("%%") {
            continue;
        }
        if kind.is_none() {
            if let Some(rest) = trimmed.strip_prefix("sequenceDiagram") {
                kind = Some(Kind::Sequence);
                let rest = rest.trim();
                if !rest.is_empty() {
                    statements.push(rest);
                }
                continue;
            }
            if trimmed.starts_with("flowchart") || trimmed.starts_with("graph") {
                kind = Some(Kind::Flow {
                    direction: direction_of(trimmed),
                });
                let mut rest = trimmed.split_whitespace().nth(1).unwrap_or("");
                if trimmed.starts_with("graph") {
                    rest = trimmed.split_whitespace().nth(1).unwrap_or("");
                }
                if !rest.is_empty() && !matches!(rest, "TD" | "TB" | "LR" | "RL" | "BT") {
                    warnings.push(format!("unknown flow direction `{rest}`"));
                }
                continue;
            }
            // Anything else (class/state/pie/gantt...) is outside the subset.
            return None;
        }
        if let Some(value) = trimmed.strip_prefix("title ") {
            title = Some(value.trim().to_string());
            continue;
        }
        statements.push(trimmed);
    }
    match kind? {
        Kind::Sequence => render_sequence(&statements, title, warnings),
        Kind::Flow { direction } => render_flow(&statements, direction, title, warnings),
    }
}

mod render;
mod syntax;

use render::{render_flow, render_sequence};
use syntax::{Kind, direction_of};

#[cfg(test)]
mod tests;
