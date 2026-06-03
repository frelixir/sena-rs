use pal_vm::{
    PalGesture, PalRandomState, PalRectI, PalSystemPathKind, PalSystemState, PalWindowRequest,
};

#[test]
fn pal_random_seed_ring_matches_set_get_and_wrap_semantics() {
    let mut random = PalRandomState::new(3);
    assert_eq!(random.set_seed(&[10, 20, 30, 40]), 2);
    let mut out = [0; 4];
    assert_eq!(random.get_seed(&mut out), 3);
    assert_eq!(out, [10, 20, 30, 0]);
    assert_eq!(random.random_ex(), 10);
    assert_eq!(random.random_ex(), 20);
    assert_eq!(random.random_ex(), 30);
    assert_eq!(random.random_ex(), 10);
    assert_eq!(random.cursor(), 1);
}

#[test]
fn pal_system_language_window_touch_and_gesture_state_are_field_like() {
    let mut state = PalSystemState::new();
    assert_eq!(state.language(), 1);
    assert_eq!(state.set_language(2), 1);
    assert_eq!(state.language(), 2);

    assert_eq!(state.set_window_change_enabled(0), 1);
    assert_eq!(state.window_change_enabled(), 0);
    assert_eq!(state.change_window_mode(1), 1);
    assert_eq!(state.window_mode(), 1);
    assert_eq!(state.change_aspect_mode(3), 1);
    assert_eq!(state.aspect_mode(), 3);

    state.set_touch_enabled(true);
    assert!(state.touch_enabled());
    assert!(state.is_touch());
    assert_eq!(state.set_touch_mode(1), 1);
    assert_eq!(state.touch_mode(), 1);

    assert!(!state.gesture_check());
    assert!(state.set_gesture(
        2,
        PalGesture {
            kind: 5,
            x: 12,
            y: 34
        }
    ));
    assert!(state.gesture_check());
    assert_eq!(state.gestures()[2].kind, 5);
}

#[test]
fn pal_system_window_size_position_and_cursor_conversion_follow_pal_modes() {
    let mut state = PalSystemState::new();
    state.set_logical_size(1280, 720);
    state.set_window_pos(100, 50);
    state.set_window_size(640, 360);
    assert_eq!(state.window_pos(), (100, 50));
    assert_eq!(state.window_size(), (640, 360));
    assert_eq!(state.cursor_device_pos(640, 360), (420, 230));

    state.set_fullscreen_content_rect(PalRectI::new(10, 20, 1290, 740));
    state.change_window_mode(1);
    assert_eq!(state.window_pos(), (10, 20));
    assert_eq!(state.window_size(), (1280, 720));
    assert_eq!(state.cursor_device_pos(640, 360), (650, 380));
}

#[test]
fn pal_system_paths_are_cross_platform_path_lists() {
    let mut state = PalSystemState::new();
    assert!(state.push_system_path(PalSystemPathKind::Normal, "/game", "sys"));
    assert!(state.push_system_path(
        PalSystemPathKind::ChineseSimplified,
        "/game",
        "/override/cn"
    ));
    assert_eq!(
        state.system_paths(PalSystemPathKind::Normal)[0],
        std::path::PathBuf::from("/game/sys")
    );
    assert_eq!(
        state.system_paths(PalSystemPathKind::ChineseSimplified)[0],
        std::path::PathBuf::from("/override/cn")
    );
    assert!(state.clear_system_paths());
    assert!(state.system_paths(PalSystemPathKind::Normal).is_empty());
}

#[test]
fn pal_window_ops_record_platform_neutral_requests() {
    let mut state = PalSystemState::new();
    state.set_window_pos(12, 34);
    state.set_window_size(800, 600);
    state.change_window_mode(1);
    state.change_aspect_mode(3);
    assert_eq!(
        state.take_window_requests(),
        vec![
            PalWindowRequest::SetPos { x: 12, y: 34 },
            PalWindowRequest::ChangeSize {
                width: 800,
                height: 600
            },
            PalWindowRequest::ChangeMode { mode: 1 },
            PalWindowRequest::ChangeAspect { mode: 3 },
        ]
    );
    assert!(state.take_window_requests().is_empty());
}
