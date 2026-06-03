use pal_vm::task::TaskSystem;
use pal_vm::{
    PalAnimationAxis, PalAnimationFlags, PalAnimationFrameRecord, PalInputState, PalPoint3,
    PalRect, PalSequenceAnimationDesc, PalSheetAnimationDesc, SceneTexture, SceneTextureId,
    SpriteDesc, SpriteSystem,
};

fn make_sprite_system() -> (SpriteSystem, pal_vm::SpriteHandle) {
    let mut sprites = SpriteSystem::new();
    sprites.insert_texture(SceneTexture::rgba8(
        SceneTextureId(7),
        1,
        96,
        24,
        vec![255; 96 * 24 * 4],
    ));
    let sprite = sprites.create(SpriteDesc {
        texture_id: SceneTextureId(7),
        texture_width: 96,
        texture_height: 24,
        cell_width: 24,
        cell_height: 24,
        position: PalPoint3::new(0, 0, 0),
        ..SpriteDesc::new(SceneTextureId(7), 96, 24)
    });
    (sprites, sprite)
}

fn no_input() -> PalInputState {
    PalInputState::new()
}

// ---- Sheet animation via TaskSystem ----

#[test]
fn sheet_animation_via_task_advances_frame_and_reports_loop() {
    let (mut sprites, sprite) = make_sprite_system();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    tasks.set_pal_time(0);
    let handle = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc {
                sprite,
                flags: PalAnimationFlags::from(PalAnimationAxis::Horizontal),
                frame_delay_ms: 10,
                running: true,
            },
            None,
        )
        .unwrap();

    // At t=5 not enough elapsed
    tasks.set_pal_time(5);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 0);

    // At t=10 advance one frame
    tasks.set_pal_time(10);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 1);
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(24, 0, 48, 24)
    );
    assert!(!tasks.animation_take_looped(handle));

    // PAL: one advance per task update regardless of elapsed
    tasks.set_pal_time(40);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 2);
    assert!(!tasks.animation_take_looped(handle));

    // Advance through remaining frames (4 total: 96/24)
    tasks.set_pal_time(50);
    tasks.process(&mut sprites, &input);
    tasks.set_pal_time(60);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 0);
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(0, 0, 24, 24)
    );
    assert!(tasks.animation_take_looped(handle));
    assert!(
        !tasks.animation_take_looped(handle),
        "loop flag is one-shot"
    );
}

#[test]
fn sheet_animation_create_does_not_reset_existing_rect() {
    let (mut sprites, sprite) = make_sprite_system();
    assert!(sprites.set_rect(sprite, Some(PalRect::new(48, 0, 72, 24))));

    let mut tasks = TaskSystem::new();
    tasks.set_pal_time(0);
    let _ = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc::horizontal(sprite, 10),
            None,
        )
        .unwrap();

    // Rect must not have been touched by create
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(48, 0, 72, 24)
    );
}

#[test]
fn sheet_zero_delay_advances_once_per_task_process() {
    let (mut sprites, sprite) = make_sprite_system();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    tasks.set_pal_time(0);
    let handle = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc::horizontal(sprite, 0),
            None,
        )
        .unwrap();

    // delay == 0: every process() call advances exactly one frame
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 1);

    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 2);
}

#[test]
fn get_animation_point_returns_flags_raw_for_sheet() {
    let (mut sprites, sprite) = make_sprite_system();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    tasks.set_pal_time(0);
    let handle = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc {
                sprite,
                flags: PalAnimationFlags::VERTICAL,
                frame_delay_ms: 10,
                running: true,
            },
            None,
        )
        .unwrap();

    tasks.set_pal_time(10);
    tasks.process(&mut sprites, &input);

    // task_data+0x10 is the flags byte for sheet animations
    assert_eq!(
        tasks.animation_get_point(handle),
        Some(PalAnimationFlags::VERTICAL.raw() as i32)
    );
}

// ---- Sequence animation via TaskSystem ----

#[test]
fn sequence_animation_applies_24_byte_records_via_task() {
    let (mut sprites, sprite) = make_sprite_system();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    tasks.set_pal_time(0);
    let handle = tasks
        .create_animation_sequence(
            &mut sprites,
            PalSequenceAnimationDesc {
                sprite,
                frames: vec![
                    PalAnimationFrameRecord::new(PalRect::new(0, 0, 16, 16), 5, 1),
                    PalAnimationFrameRecord::new(PalRect::new(16, 0, 32, 16), 5, -1),
                ],
                running: true,
            },
            None,
        )
        .unwrap();

    // Frame 0 applied on creation
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(0, 0, 16, 16)
    );

    tasks.set_pal_time(5);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.animation_get_point(handle), Some(1));
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(16, 0, 32, 16)
    );

    // Terminal: current_frame -> -1, rect stays
    tasks.set_pal_time(10);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.animation_get_point(handle), Some(-1));
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(16, 0, 32, 16)
    );
    // get_time returns None for sequence handles (PAL task_data+0x04 would be record table ptr)
    assert_eq!(tasks.animation_get_time(handle), None);
}

// ---- Animation only changes sprite rect, not draw commands ----

#[test]
fn animation_only_changes_sprite_rect_never_draw_commands() {
    let (mut sprites, sprite) = make_sprite_system();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    // Sprite is invisible (default) → no draw commands
    tasks.set_pal_time(0);
    let _ = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc::horizontal(sprite, 0),
            None,
        )
        .unwrap();

    tasks.process(&mut sprites, &input);
    tasks.process(&mut sprites, &input);
    tasks.process(&mut sprites, &input);

    let cmds = sprites.commands();
    assert!(
        cmds.is_empty(),
        "animation task must not produce draw commands; only sprite state does"
    );

    // Make sprite visible: now draw commands appear (driven by SpriteSystem, not animation)
    sprites.view_ctrl(sprite, true);
    let cmds = sprites.commands();
    assert!(
        !cmds.is_empty(),
        "visible sprite should produce draw commands"
    );
}

// ---- Animation control API ----

#[test]
fn animation_start_stop_changes_running_state() {
    let (mut sprites, sprite) = make_sprite_system();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    tasks.set_pal_time(0);
    let handle = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc::horizontal(sprite, 10),
            None,
        )
        .unwrap();

    // Stop: no advance even if time elapsed
    tasks.animation_stop(handle);
    tasks.set_pal_time(20);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 0);

    // Start: resumes from current position
    tasks.animation_start(handle);
    tasks.set_pal_time(30);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 1);
}

#[test]
fn animation_reset_goes_to_frame_zero() {
    let (mut sprites, sprite) = make_sprite_system();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    tasks.set_pal_time(0);
    let handle = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc::horizontal(sprite, 10),
            None,
        )
        .unwrap();

    tasks.set_pal_time(10);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 1);

    tasks.animation_reset(handle, &mut sprites);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 0);
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(0, 0, 24, 24)
    );
}
