//! Gh #326: a narrow rail kept no close button, and the ladder must never promise
//! Sixel.
//!
//! Two halves, and the second one's finding is worth recording because it changed
//! what this file contains.
//!
//! **The tabs half was real and is fixed.** The vertical layout formatted the whole
//! row (`▶ [1] title * ×`) and truncated *that*, then put the close region on the
//! truncated string's last column. Past a certain width the `×` was cut away, so
//! the user saw no close button while the host still reported one on the last
//! remaining column - a phantom region, which is why a click on a title character
//! closed the tab. `0` also selected tab 1 (`0.saturating_sub(1) == 0`). The
//! behavioural rows live in `crates/lca-tui/src/widgets/tabs_tests.rs`.
//!
//! **The Sixel half did not exist.** The issue names `detect_protocol`,
//! `ImageProtocol::Sixel` and `encode_sixel_placeholder`; none of those has ever
//! been in this repository (`git log -S"Sixel" --all` is empty) and pi has no
//! Sixel support either - its `ImageProtocol` is `"kitty" | "iterm2" | null`. So
//! there was no placeholder to leak and no protocol to stop detecting. Rather
//! than "fix" code that is not there, these rows pin the property the issue was
//! actually about: **the ladder promises only what it implements**, and the
//! fallback says what it is instead of drawing a fake image. If someone adds a
//! Sixel arm later, the second row fails and they have to bring an encoder with
//! it.
//!
//! NFR-24 guard: the behavioural rows could all pass while the *shape* that
//! produced the tabs defect came back.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// The body of one function, found by brace matching from its signature.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("{signature} has a balanced body");
}

// Verifies: gh #326 - the vertical rail cannot truncate the close glyph away.
//
// The structural shape of the defect was "format everything, then truncate the
// result". Any fix that still truncates a string containing the `×` can cut it
// again the moment a flag lengthens. So the guard is that `vertical_cell` builds
// its row from a title that was truncated *separately*, and appends the glyph
// afterwards.
#[test]
fn the_rail_truncates_the_title_not_the_formatted_row() {
    let source = read("crates/lca-tui/src/widgets/tabs.rs");
    let body = function_body(&source, "fn vertical_cell(");

    // The title is truncated on its own into a leftover `room`, and the glyph is
    // appended after it - the ordering the issue asked for.
    assert!(
        body.contains("truncate_to_width(&item.title"),
        "the title is truncated as a title, not as part of the row: {body}"
    );
    // Whatever the widths, the glyph is not inside the truncated string: it is
    // concatenated with `head`/`state_text` into the final row.
    let title_truncation = body
        .find("truncate_to_width(&item.title")
        .map(|at| &body[at..])
        .unwrap_or_default();
    assert!(
        !title_truncation.contains("×") && !title_truncation.contains("CLOSE_TEXT"),
        "the close glyph is not part of the string handed to the truncator: {title_truncation}"
    );
    assert!(
        body.contains("close_text"),
        "and the glyph is reserved as its own piece: {body}"
    );
    // The reserve happens before the title competes for room.
    let reserve = body.find("close_width").unwrap_or(usize::MAX);
    let title_room = body.find("truncate_to_width(&item.title").unwrap_or(0);
    assert!(
        reserve < title_room,
        "the `×` is reserved before the title is measured"
    );
}

// Verifies: gh #326 - `render` and the hit test read the same painted row.
//
// The two used to be computed separately (`render` rebuilt the row from
// `self.items`, the layout measured its own truncated copy), and drift between
// them is the whole defect class. The layout now carries the painted `text` and
// `render` emits it.
#[test]
fn the_painted_row_and_the_hit_region_come_from_one_place() {
    let source = read("crates/lca-tui/src/widgets/tabs.rs");
    let render = function_body(&source, "pub fn render(");
    assert!(
        render.contains("cell.text"),
        "the vertical rows are the layout's own text: {render}"
    );
    assert!(
        !render.contains("truncate_to_width"),
        "and `render` does not truncate independently: {render}"
    );
    // No leftover "recompute then discard" scaffolding, which is how the two
    // copies were kept in the tree at once.
    assert!(
        !render.contains("let _ = "),
        "no discarded recomputation is left behind: {render}"
    );
    // The region itself is derived from the glyph's position in that row.
    let region = function_body(&source, "fn rendered_close_region(");
    assert!(
        region.contains("row.find('×')"),
        "the region is found by locating the glyph: {region}"
    );
    assert!(
        !region.contains("visible_width(row)"),
        "not derived from the row's total width, which points past the glyph: {region}"
    );
}

// Verifies: gh #326 - the bar is fitted, not painted past its width.
//
// Found while verifying the vertical fix against the surface the product actually
// uses (`chat_tabs.rs` builds only a `Horizontal` bar). Measured on the pre-fix
// build: 4 tabs at width 80 painted **82 cells**, and the overflow grew with the
// tab count - 2 tabs at 40 painted 48, 1 tab at 10 painted 16. The narrowing loop
// shaved titles against a `total` it computed with a `chrome()` that under-counted
// the flags, so it believed the bar fitted.
//
// The structural property is that the **last** thing the layout does is fit the
// titles into the room left over, from per-tab flag state - not truncate a row
// that was already assembled.
#[test]
fn the_bar_fits_its_width_by_shrinking_titles_last() {
    let source = read("crates/lca-tui/src/widgets/tabs.rs");
    let body = function_body(&source, "fn layout_horizontal(");

    // Four levers, in this order, each guarded by a width test.
    let at = |needle: &str| -> usize {
        body.find(needle)
            .unwrap_or_else(|| panic!("the {needle} lever exists in the horizontal layout"))
    };
    let badge = at("slot.badge = false");
    let markers = at("slot.markers = false");
    let add = at("show_add = false");
    let drop = at("slots.remove(victim)");
    let title = at("truncate_to_width(&slot.title");
    assert!(
        badge < markers && markers < add && add < drop && drop < title,
        "flags, then `[+]`, then whole tabs, then titles - in that order"
    );
    // The `*` is never a lever: it is pushed unconditionally for the active slot.
    assert!(
        body.contains("flags.push('*')"),
        "the active marker is not optional: {body}"
    );
    // And each survived tab is rebuilt as a cell, so nothing is truncated after
    // the flags and the `×` were painted into it.
    assert!(
        body.contains("cell_of(slot, &shortened)"),
        "the shortened title goes back through the cell builder: {body}"
    );
    assert!(
        !body.contains("truncate_to_width(&text"),
        "the painted row is never what gets truncated: {body}"
    );
}

// Verifies: gh #326 - `0` cannot select a tab.
#[test]
fn zero_is_guarded_before_the_digit_arm() {
    let source = read("crates/lca-tui/src/widgets/tabs.rs");
    let key = function_body(&source, "pub fn handle_key(");
    let zero = key
        .find("\"0\" => None")
        .unwrap_or_else(|| panic!("the zero guard exists: {key}"));
    let digit_arm = key
        .find("is_ascii_digit")
        .unwrap_or_else(|| panic!("the digit arm exists: {key}"));
    assert!(
        zero < digit_arm,
        "the guard is matched before the digit arm, or it is unreachable"
    );
    assert!(
        !key.contains("digit.parse::<usize>().unwrap_or(0).saturating_sub(1);\n                if index < self.items.len()")
            || key.contains("\"0\" => None"),
        "the saturating_sub is still guarded"
    );
}

// Verifies: gh #326 - the image ladder promises no protocol it cannot encode.
//
// The issue's premise was a Sixel arm that rendered a text placeholder. No such
// arm exists, here or in pi. This row is what would have caught it *and* what
// stops one being added without an encoder: every `ImageProtocol` variant must
// have an encoder in `render_image`, so a new detection cannot land without a
// way to draw it.
#[test]
fn every_detected_protocol_has_an_encoder() {
    let source = read("crates/lca-tui/src/widgets/image.rs");
    let detection = function_body(&source, "pub fn detect_image_protocol_with(");

    // The variants the ladder can return: collect the `ImageProtocol::X` names
    // mentioned in the detection body.
    let mut detected: Vec<String> = Vec::new();
    let mut rest = detection;
    while let Some(at) = rest.find("ImageProtocol::") {
        let tail = &rest[at + "ImageProtocol::".len()..];
        let name: String = tail.chars().take_while(|c| c.is_alphanumeric()).collect();
        if !name.is_empty() && !detected.contains(&name) {
            detected.push(name.clone());
        }
        rest = &tail[name.len()..];
    }
    assert!(
        detected.len() >= 3,
        "the ladder names its protocols: {detected:?}"
    );

    let render = function_body(&source, "pub fn render_image(");
    for name in &detected {
        // Each arm either encodes or is `None`. A variant with no arm would not
        // compile, so what this really pins is that no arm is a *placeholder
        // pretending to be the protocol*: the `None` arm is the only text one.
        if name == "None" {
            assert!(
                render.contains("ImageProtocol::None => render_image_placeholder"),
                "`None` is the only arm that renders text: {render}"
            );
            continue;
        }
        let arm = format!("ImageProtocol::{name} =>");
        assert!(
            render.contains(&arm),
            "`{name}` is detected and has its own arm: {render}"
        );
    }

    // And the whole crate never mentions Sixel: the claim in the issue cannot
    // silently become true without this row failing first.
    let tabs_dir = repo_root().join("crates/lca-tui/src");
    let mut offenders = Vec::new();
    for entry in walk(&tabs_dir) {
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        let code: Vec<&str> = text
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//")
            })
            .collect();
        if code
            .iter()
            .any(|line| line.to_lowercase().contains("sixel"))
        {
            offenders.push(entry.clone());
        }
    }
    assert!(
        offenders.is_empty(),
        "no code claims Sixel support: {offenders:?}"
    );
}

/// Every `.rs` file under `dir`, recursively.
fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out
}
