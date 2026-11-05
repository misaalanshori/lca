use super::*;

// Verifies: gh #164 - SGR press/release/motion/wheel normalize to
// the keystone event, coordinates zero-based, modifiers decoded.
#[test]
fn sgr_press_becomes_a_left_down() {
    assert_eq!(
        MouseEvent::from_sgr(0, 10, 5, true),
        MouseEvent::Down {
            col: 9,
            row: 4,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        }
    );
}

// Verifies: gh #164 - a release normalizes to Up.
#[test]
fn sgr_release_becomes_an_up() {
    assert!(matches!(
        MouseEvent::from_sgr(0, 10, 5, false),
        MouseEvent::Up { col: 9, row: 4, .. }
    ));
}

// Verifies: gh #164 - motion without a button is a hover; motion
// with one names it (pi's 1003 any-motion coordinates).
#[test]
fn motion_with_and_without_buttons() {
    // A hover reports motion with button bits 3 (xterm's no-button).
    assert!(matches!(
        MouseEvent::from_sgr(32 | 3, 3, 3, true),
        MouseEvent::Move { button: None, .. }
    ));
    assert!(matches!(
        MouseEvent::from_sgr(32 | 1, 3, 3, true),
        MouseEvent::Move {
            button: Some(MouseButton::Middle),
            ..
        }
    ));
}

// Verifies: gh #164 - wheel direction maps to a signed delta, with
// shift/alt/ctrl riding along.
#[test]
fn wheel_maps_direction_and_modifiers() {
    assert!(matches!(
        MouseEvent::from_sgr(64, 3, 3, true),
        MouseEvent::Wheel { delta: -1, .. }
    ));
    assert!(matches!(
        MouseEvent::from_sgr(64 | 1 | 4 | 8 | 16, 3, 3, true),
        MouseEvent::Wheel {
            delta: 1,
            modifiers: Modifiers {
                shift: true,
                alt: true,
                ctrl: true,
            },
            ..
        }
    ));
}

// Verifies: gh #164 - the registry resolves the topmost region,
// and misses cleanly.
#[test]
fn regions_hit_topmost_first() {
    let mut registry = MouseRegistry::new();
    registry.register(
        Rect {
            row: 0,
            col: 0,
            width: 10,
            height: 10,
            visible: true,
        },
        "bottom",
    );
    registry.register(
        Rect {
            row: 2,
            col: 2,
            width: 4,
            height: 4,
            visible: true,
        },
        "top",
    );
    assert_eq!(registry.hit(3, 3), Some("top"));
    assert_eq!(registry.hit(0, 0), Some("bottom"));
    assert_eq!(registry.hit(20, 20), None);
}
