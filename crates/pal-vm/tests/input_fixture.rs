use pal_vm::event::MouseButton;
use pal_vm::{PalInputState, PalKey, PalMouseButton};

// ---- Keyboard three-state ----

#[test]
fn key_push_on_first_press() {
    let mut input = PalInputState::new();
    input.handle_keyboard_event("Return", true);
    assert!(
        input.key_push(PalKey::Return),
        "push should be set on initial press"
    );
    assert!(input.key_on(PalKey::Return), "on should be set while held");
    assert!(
        !input.key_pull(PalKey::Return),
        "pull should not be set on press"
    );
}

#[test]
fn key_on_persists_while_held() {
    let mut input = PalInputState::new();
    input.handle_keyboard_event("Return", true);

    input.begin_frame();
    input.handle_keyboard_event("Return", true); // still held
    assert!(!input.key_push(PalKey::Return), "no push on sustained hold");
    assert!(input.key_on(PalKey::Return));
    assert!(!input.key_pull(PalKey::Return));
}

#[test]
fn key_pull_on_release() {
    let mut input = PalInputState::new();
    input.handle_keyboard_event("Return", true);
    input.begin_frame();
    input.handle_keyboard_event("Return", false);
    assert!(!input.key_push(PalKey::Return));
    assert!(!input.key_on(PalKey::Return));
    assert!(
        input.key_pull(PalKey::Return),
        "pull should be set on release"
    );
}

#[test]
fn begin_frame_clears_push_and_pull() {
    let mut input = PalInputState::new();
    input.handle_keyboard_event("Return", true);
    assert!(input.key_push(PalKey::Return));
    input.begin_frame();
    assert!(
        !input.key_push(PalKey::Return),
        "push cleared by begin_frame"
    );

    input.handle_keyboard_event("Return", false);
    assert!(input.key_pull(PalKey::Return));
    input.begin_frame();
    assert!(
        !input.key_pull(PalKey::Return),
        "pull cleared by begin_frame"
    );
}

#[test]
fn key_on_not_cleared_by_begin_frame() {
    let mut input = PalInputState::new();
    input.handle_keyboard_event("Space", true);
    input.begin_frame();
    // key_on persists (key is still physically held)
    assert!(
        input.key_on(PalKey::Space),
        "on should survive begin_frame if key not released"
    );
}

// ---- Mouse button three-state ----

#[test]
fn mouse_push_on_first_press() {
    let mut input = PalInputState::new();
    input.handle_mouse_button_event(MouseButton::Left, true);
    assert!(input.mouse_push(PalMouseButton::Left));
    assert!(input.mouse_on(PalMouseButton::Left));
    assert!(!input.mouse_pull(PalMouseButton::Left));
}

#[test]
fn mouse_on_persists_while_held() {
    let mut input = PalInputState::new();
    input.handle_mouse_button_event(MouseButton::Left, true);
    input.begin_frame();
    input.handle_mouse_button_event(MouseButton::Left, true);
    assert!(!input.mouse_push(PalMouseButton::Left));
    assert!(input.mouse_on(PalMouseButton::Left));
}

#[test]
fn mouse_pull_on_release() {
    let mut input = PalInputState::new();
    input.handle_mouse_button_event(MouseButton::Right, true);
    input.begin_frame();
    input.handle_mouse_button_event(MouseButton::Right, false);
    assert!(input.mouse_pull(PalMouseButton::Right));
    assert!(!input.mouse_on(PalMouseButton::Right));
}

#[test]
fn mouse_push_cleared_by_begin_frame() {
    let mut input = PalInputState::new();
    input.handle_mouse_button_event(MouseButton::Left, true);
    assert!(input.mouse_push(PalMouseButton::Left));
    input.begin_frame();
    assert!(!input.mouse_push(PalMouseButton::Left));
}

// ---- Mouse position and delta ----

#[test]
fn mouse_position_tracks_cursor_moved() {
    let mut input = PalInputState::new();
    input.handle_cursor_moved(100.0, 200.0);
    assert_eq!(input.mouse_position(), (100, 200));
}

#[test]
fn mouse_delta_accumulates_within_frame() {
    let mut input = PalInputState::new();
    // First event: no delta yet (cursor not initialized)
    input.handle_cursor_moved(10.0, 20.0);
    // Second event same frame
    input.handle_cursor_moved(15.0, 25.0);
    let (dx, dy) = input.mouse_delta();
    assert_eq!(dx, 5);
    assert_eq!(dy, 5);
}

#[test]
fn mouse_delta_cleared_by_begin_frame() {
    let mut input = PalInputState::new();
    input.handle_cursor_moved(0.0, 0.0);
    input.handle_cursor_moved(50.0, 30.0);
    assert_ne!(input.mouse_delta(), (0, 0));
    input.begin_frame();
    assert_eq!(
        input.mouse_delta(),
        (0, 0),
        "delta must be cleared by begin_frame"
    );
}

// ---- Wheel delta ----

#[test]
fn wheel_delta_accumulates_within_frame() {
    let mut input = PalInputState::new();
    input.handle_mouse_wheel(0.0, 1.0);
    input.handle_mouse_wheel(0.0, 1.5);
    assert!((input.wheel_delta() - 2.5).abs() < 1e-4);
}

#[test]
fn wheel_delta_cleared_by_begin_frame() {
    let mut input = PalInputState::new();
    input.handle_mouse_wheel(0.0, 3.0);
    input.begin_frame();
    assert_eq!(input.wheel_delta(), 0.0);
}

#[test]
fn pal_input_clear_and_raw_key_setters_match_pal_masks() {
    let mut input = PalInputState::new();
    input.set_key_on_bits(0x10);
    input.set_key_push_bits(0x20);
    input.set_key_pull_bits(0x40);
    assert_eq!(input.raw_key_on(), 0x10);
    assert_eq!(input.raw_key_push(), 0x20);
    assert_eq!(input.raw_key_pull(), 0x40);

    input.clear();
    assert_eq!(input.raw_key_on(), 0);
    assert_eq!(input.raw_key_push(), 0);
    assert_eq!(input.raw_key_pull(), 0);
    assert_eq!(input.raw_mouse_on(), 0);
    assert_eq!(input.raw_mouse_push(), 0);
    assert_eq!(input.raw_mouse_pull(), 0);
    assert_eq!(input.mouse_delta(), (0, 0));
    assert_eq!(input.wheel_delta(), 0.0);
}

// ---- any_push helper ----

#[test]
fn any_push_true_when_key_pressed() {
    let mut input = PalInputState::new();
    assert!(!input.any_push());
    input.handle_keyboard_event("Return", true);
    assert!(input.any_push());
}

#[test]
fn any_push_true_when_mouse_pressed() {
    let mut input = PalInputState::new();
    input.handle_mouse_button_event(MouseButton::Left, true);
    assert!(input.any_push());
}

#[test]
fn any_push_false_after_begin_frame_without_new_events() {
    let mut input = PalInputState::new();
    input.handle_keyboard_event("Space", true);
    assert!(input.any_push());
    input.begin_frame();
    assert!(
        !input.any_push(),
        "any_push must be false after begin_frame clears transient state"
    );
}
