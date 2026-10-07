//! Split from `lib.rs` (cycle 7, P3). Behaviour unchanged.

use super::*;

// ---------------------------------------------------------------------------
// The ui world: widget trees out, interactions in (ADR-0003)
// ---------------------------------------------------------------------------

use lca_ext_abi::host::ui::exports::lca::ext as ui_exports;

/// The WIT case -> protocol widget.
fn from_wit_widget(widget: ui_exports::render::Widget) -> lca_protocol::Widget {
    use lca_protocol::Widget as W;
    use ui_exports::render::Widget as Wit;
    match widget {
        Wit::Text((content, role)) => W::Text { content, role },
        Wit::StyledText((content, style)) => W::StyledText {
            content,
            style: lca_protocol::TextStyle {
                fg: style.fg,
                bg: style.bg,
                bold: style.bold,
                dim: style.dim,
                italic: style.italic,
                underline: style.underline,
            },
        },
        Wit::Markdown(source) => W::Markdown { source },
        Wit::Button((id, label)) => W::Button { id, label },
        Wit::Table((headers, rows)) => W::Table { headers, rows },
        Wit::ScrollContainer((max_height, children)) => W::ScrollContainer {
            max_height,
            children,
        },
        Wit::Image((media_type, bytes)) => W::Image { media_type, bytes },
        Wit::Boxed((title, border, background, child)) => W::Boxed {
            title,
            border,
            background,
            child,
        },
        Wit::Row(children) => W::Row(children),
        Wit::Column(children) => W::Column(children),
        Wit::Spinner(frames) => W::Spinner { frames },
        Wit::Progress((label, fill)) => W::Progress { label, fill },
        Wit::Keyvalue(pairs) => W::KeyValue(pairs),
        Wit::Vendor(kind) => W::Vendor(kind),
    }
}

pub(super) fn render_work(
    inner: &Inner,
    region: &str,
) -> Result<Option<lca_protocol::WidgetTree>, CallError> {
    let pre = inner
        .ui
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no ui world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let tree = instance
        .lca_ext_render()
        .call_render(&mut store, region)
        .map_err(|err| inner.classify(err))?;
    Ok(tree.map(|nodes| lca_protocol::WidgetTree {
        nodes: nodes.into_iter().map(from_wit_widget).collect(),
    }))
}

pub(super) fn event_work(
    inner: &Inner,
    region: &str,
    input: &lca_protocol::UiInput,
) -> Result<lca_protocol::UiEffect, CallError> {
    use lca_protocol::UiEffect;
    use ui_exports::interaction::Input as WasmInput;
    let pre = inner
        .ui
        .as_ref()
        .ok_or_else(|| CallError::InvalidArguments("no ui world".into()))?;
    let mut store = inner.build_store()?;
    let instance = pre
        .instantiate(&mut store)
        .map_err(|err| inner.classify(err))?;
    let wasm_input = match input {
        lca_protocol::UiInput::Key { key } => WasmInput::Key(key.clone()),
        lca_protocol::UiInput::Submit { text } => WasmInput::Submit(text.clone()),
        lca_protocol::UiInput::Cancel => WasmInput::Cancel,
        lca_protocol::UiInput::ClickWidget { id } => WasmInput::ClickWidget(id.clone()),
        lca_protocol::UiInput::Click { col, row } => WasmInput::Click((*col, *row)),
        lca_protocol::UiInput::Scroll { delta } => WasmInput::Scroll(*delta),
    };
    let effect = instance
        .lca_ext_interaction()
        .call_handle(&mut store, region, &wasm_input)
        .map_err(|err| inner.classify(err))?;
    use ui_exports::interaction::Effect;
    let _ = region;
    Ok(match effect {
        Effect::None => UiEffect::None,
        Effect::CloseModal => UiEffect::CloseModal,
        Effect::OpenModal => UiEffect::OpenModal,
        Effect::ShowNotice(text) => UiEffect::ShowNotice(text),
        Effect::InsertText(text) => UiEffect::InsertText(text),
        Effect::SubmitPrompt(text) => UiEffect::SubmitPrompt(text),
    })
}
