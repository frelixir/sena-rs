use pal_vm::PalEffectSystem;

#[test]
fn pal_effect_ex_tracks_enable_active_and_overlay_alpha() {
    let mut effects = PalEffectSystem::new();
    assert_eq!(effects.effect_ex(0, 100, 0, false, 0), 1);
    assert!(!effects.active());
    assert_eq!(effects.effect_ex(0x32, 100, 0, false, 0), 0);

    assert_eq!(effects.effect_ex(1, 100, 0, false, 10), 1);
    assert!(effects.active());
    assert_eq!(effects.effect_ex(2, 100, 0, false, 10), 0);
    let quad = effects.overlay_quad(1280, 720, 10).unwrap();
    assert_eq!(quad.dst.w, 1280.0);
    assert_eq!(quad.dst.h, 720.0);
    assert!(quad.color[3] > 0.9);
    effects.tick(110);
    assert!(!effects.active());
}

#[test]
fn pal_effect_disable_makes_effect_ex_a_successful_noop() {
    let mut effects = PalEffectSystem::new();
    effects.set_enabled(false);
    assert_eq!(effects.effect_ex(1, 100, 0, true, 0), 1);
    assert!(!effects.active());
}
