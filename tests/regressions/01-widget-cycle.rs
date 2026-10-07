//! Finding 1 (critical, `../lca-issues.md`): a cyclic widget arena aborted
//! the process with a stack overflow. The renderer must visit each node at
//! most once and terminate.
//!
//! Verifies: ADR-0003's rendering boundary (defect 1).

use lca_protocol::Widget;
use lca_ui::ext_widgets::WidgetCtx;

fn ctx<'a>(
    theme: &'a lca_ui::theme::Theme,
    offsets: &'a std::collections::HashMap<String, usize>,
) -> WidgetCtx<'a> {
    WidgetCtx {
        theme,
        width: 80,
        region: "panel",
        offsets,
    }
}

fn theme() -> lca_ui::theme::Theme {
    lca_ui::theme::Theme::colored()
}

#[test]
fn a_cyclic_widget_arena_renders_once_and_terminates() {
    // node0 boxes node1; node1 lists node0 and itself again.
    let nodes = vec![
        Widget::Boxed {
            title: Some("box".to_string()),
            border: None,
            background: None,
            child: 1,
        },
        Widget::Column(vec![0, 1, 1]),
    ];
    assert_eq!(
        lca_ui::ext_widgets::widget_lines(
            &nodes,
            &ctx(&theme(), &std::collections::HashMap::new()),
        ),
        vec!["[box]".to_string()],
        "each node renders once"
    );

    // A node that points at itself must also terminate.
    let selfish = vec![Widget::Boxed {
        title: Some("self".to_string()),
        border: None,
        background: None,
        child: 0,
    }];
    assert_eq!(
        lca_ui::ext_widgets::widget_lines(
            &selfish,
            &ctx(&theme(), &std::collections::HashMap::new()),
        ),
        vec!["[self]".to_string()]
    );
}
