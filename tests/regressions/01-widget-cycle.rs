//! Finding 1 (critical, `../lca-issues.md`): a cyclic widget arena aborted
//! the process with a stack overflow. The renderer must visit each node at
//! most once and terminate.
//!
//! Verifies: ADR-0003's rendering boundary (defect 1).

use lca_protocol::Widget;

#[test]
fn a_cyclic_widget_arena_renders_once_and_terminates() {
    // node0 boxes node1; node1 lists node0 and itself again.
    let nodes = vec![
        Widget::Boxed {
            title: Some("box".to_string()),
            child: 1,
        },
        Widget::Column(vec![0, 1, 1]),
    ];
    assert_eq!(
        lca_tui::widget_lines(&nodes),
        vec!["[box]".to_string()],
        "each node renders once"
    );

    // A node that points at itself must also terminate.
    let selfish = vec![Widget::Boxed {
        title: Some("self".to_string()),
        child: 0,
    }];
    assert_eq!(lca_tui::widget_lines(&selfish), vec!["[self]".to_string()]);
}
