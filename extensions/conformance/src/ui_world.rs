wit_bindgen::generate!({
    path: "../../wit",
    world: "ui",
    export_macro_name: "export_ui",
    with: {
        "lca:host/log@0.6.0": generate,
        "lca:host/ui@0.6.0": generate,
        "lca:host/ui-dialogs@0.6.0": generate,
        "lca:host/resources@0.6.0": generate,
        "lca:host/state@0.6.0": generate,
    },
});

use exports::lca::ext::interaction::{
    Effect as WasmEffect, Guest as InteractionGuest, Input as WasmInput,
};
use exports::lca::ext::render::{Guest as RenderGuest, Widget as WasmWidget};
use lca::ext::types::TextStyle as WasmTextStyle;

pub struct UiWasm;

/// Protocol style -> WIT style: every field crosses by value.
fn wit_text_style(style: &lca_protocol::TextStyle) -> WasmTextStyle {
    WasmTextStyle {
        fg: style.fg.clone(),
        bg: style.bg.clone(),
        bold: style.bold,
        dim: style.dim,
        italic: style.italic,
        underline: style.underline,
    }
}

fn to_wit(widget: lca_protocol::Widget) -> WasmWidget {
    use lca_protocol::Widget as W;
    match widget {
        W::Text { content, role } => WasmWidget::Text((content, role)),
        W::StyledText { content, style } => {
            WasmWidget::StyledText((content, wit_text_style(&style)))
        }
        W::Markdown { source } => WasmWidget::Markdown(source),
        W::Button { id, label } => WasmWidget::Button((id, label)),
        W::Table { headers, rows } => WasmWidget::Table((headers, rows)),
        W::ScrollContainer {
            max_height,
            children,
        } => WasmWidget::ScrollContainer((max_height, children)),
        W::Image { media_type, bytes } => WasmWidget::Image((media_type, bytes)),
        W::Boxed {
            title,
            border,
            background,
            child,
        } => WasmWidget::Boxed((title, border, background, child)),
        W::Row(children) => WasmWidget::Row(children),
        W::Column(children) => WasmWidget::Column(children),
        W::Spinner { frames } => WasmWidget::Spinner(frames),
        W::Progress { label, fill } => WasmWidget::Progress((label, fill)),
        W::KeyValue(pairs) => WasmWidget::Keyvalue(pairs),
        W::Vendor(kind) => WasmWidget::Vendor(kind),
    }
}

impl RenderGuest for UiWasm {
    fn render(region: String) -> Option<Vec<WasmWidget>> {
        crate::ui_script(&region).map(|nodes| nodes.into_iter().map(to_wit).collect())
    }
}

impl InteractionGuest for UiWasm {
    fn handle(region: String, input: WasmInput) -> WasmEffect {
        let input = match input {
            WasmInput::Key(key) => lca_protocol::UiInput::Key { key },
            WasmInput::Submit(text) => lca_protocol::UiInput::Submit { text },
            WasmInput::Cancel => lca_protocol::UiInput::Cancel,
            WasmInput::ClickWidget(id) => lca_protocol::UiInput::ClickWidget { id },
            WasmInput::Click((col, row)) => lca_protocol::UiInput::Click { col, row },
            WasmInput::Scroll(delta) => lca_protocol::UiInput::Scroll { delta },
        };
        match crate::ui_event_script(&region, &input) {
            lca_protocol::UiEffect::None => WasmEffect::None,
            lca_protocol::UiEffect::CloseModal => WasmEffect::CloseModal,
            lca_protocol::UiEffect::OpenModal => WasmEffect::OpenModal,
            lca_protocol::UiEffect::ShowNotice(text) => WasmEffect::ShowNotice(text),
            lca_protocol::UiEffect::InsertText(text) => WasmEffect::InsertText(text),
            lca_protocol::UiEffect::SubmitPrompt(text) => WasmEffect::SubmitPrompt(text),
        }
    }
}

export_ui!(UiWasm);
