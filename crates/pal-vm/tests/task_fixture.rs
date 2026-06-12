use pal_vm::task::{TaskSystem, TASK_POOL_CAPACITY};
use pal_vm::{
    PalAnimationAxis, PalAnimationFlags, PalAnimationFrameRecord, PalInputState, PalPoint3,
    PalRect, PalSequenceAnimationDesc, PalSheetAnimationDesc, SceneTexture, SceneTextureId,
    SpriteDesc, SpriteSystem,
};

fn make_sprites() -> (SpriteSystem, pal_vm::SpriteHandle) {
    let mut sprites = SpriteSystem::new();
    sprites.insert_texture(SceneTexture::rgba8(
        SceneTextureId(10),
        1,
        96,
        24,
        vec![255; 96 * 24 * 4],
    ));
    let sprite = sprites.create(SpriteDesc {
        texture_id: SceneTextureId(10),
        texture_width: 96,
        texture_height: 24,
        cell_width: 24,
        cell_height: 24,
        position: PalPoint3::new(0, 0, 0),
        visible: true,
        ..SpriteDesc::new(SceneTextureId(10), 96, 24)
    });
    (sprites, sprite)
}

fn no_input() -> PalInputState {
    PalInputState::new()
}

// ---- Pool allocation ----

#[test]
fn task_pool_fills_to_capacity() {
    let mut tasks = TaskSystem::new();
    let mut handles = Vec::new();
    for _ in 0..TASK_POOL_CAPACITY {
        if let Some(h) = tasks.create_wait_frame(100) {
            handles.push(h);
        }
    }
    assert_eq!(handles.len(), TASK_POOL_CAPACITY);
    assert!(tasks.create_wait_frame(1).is_none(), "pool should be full");
}

#[test]
fn freed_slot_is_reusable_after_process() {
    let mut tasks = TaskSystem::new();
    let mut sprites = SpriteSystem::new();
    let input = no_input();

    // Fill pool
    let mut handles: Vec<_> = (0..TASK_POOL_CAPACITY)
        .map(|_| tasks.create_wait_frame(100).unwrap())
        .collect();
    assert!(tasks.create_wait_frame(1).is_none());

    // Free first task and process so it goes from PendingFree -> Free
    tasks.free(handles[0]);
    tasks.process(&mut sprites, &input);
    // One slot freed; can allocate again
    let new_handle = tasks.create_wait_frame(1);
    assert!(new_handle.is_some());
    handles[0] = new_handle.unwrap();
}

// ---- Pending-free lifecycle ----

#[test]
fn free_does_not_release_immediately() {
    let mut tasks = TaskSystem::new();
    let mut sprites = SpriteSystem::new();
    let input = no_input();

    let h = tasks.create_wait_frame(50).unwrap();
    assert!(tasks.is_alive(h));

    tasks.free(h);
    // Task is pending-free but still alive until process() runs
    assert!(
        tasks.is_alive(h),
        "pending-free task should still be alive before process"
    );

    tasks.process(&mut sprites, &input);
    assert!(!tasks.is_alive(h), "task must be released after process");
}

#[test]
fn wait_frame_self_frees_after_countdown() {
    let mut tasks = TaskSystem::new();
    let mut sprites = SpriteSystem::new();
    let input = no_input();

    let h = tasks.create_wait_frame(2).unwrap();

    // Frame 1: remaining 2 -> 1
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(h));

    // Frame 2: remaining 1 -> 0, FreeSelf -> PendingFree
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(h), "PendingFree is still alive");

    // Frame 3: PendingFree -> Free
    tasks.process(&mut sprites, &input);
    assert!(!tasks.is_alive(h));
}

#[test]
fn wait_frame_forever_never_self_frees() {
    let mut tasks = TaskSystem::new();
    let mut sprites = SpriteSystem::new();
    let input = no_input();

    let h = tasks.create_wait_frame(-1).unwrap();
    for _ in 0..20 {
        tasks.process(&mut sprites, &input);
    }
    assert!(tasks.is_alive(h));
}

#[test]
fn wait_time_self_frees_after_cached_pal_time_elapses() {
    let mut tasks = TaskSystem::new();
    let mut sprites = SpriteSystem::new();
    let input = no_input();

    tasks.set_pal_time(100);
    let h = tasks.create_wait_time(250).unwrap();

    tasks.set_pal_time(349);
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(h));

    tasks.set_pal_time(350);
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(h), "PendingFree is still alive");

    tasks.process(&mut sprites, &input);
    assert!(!tasks.is_alive(h));
}

#[test]
fn raw_pal_task_preserves_state_message_data_and_next_process() {
    let mut tasks = TaskSystem::new();
    let handle = tasks
        .create_raw_task(0x1000, 0x2000, 8, 0, None, "RawPalTask")
        .unwrap();

    assert_eq!(
        tasks.task_state(handle),
        Some(pal_vm::task::TaskState::Active)
    );
    assert_eq!(tasks.task_state_raw(handle), 1);
    assert_eq!(tasks.task_data(handle).unwrap(), &[0; 8]);
    assert!(tasks.set_task_data(handle, vec![1, 2, 3, 4]));
    assert_eq!(tasks.task_data(handle).unwrap(), &[1, 2, 3, 4]);

    assert!(tasks.set_message(handle, 1234));
    assert_eq!(tasks.take_message(handle), 1234);
    assert_eq!(tasks.take_message(handle), 0);

    assert!(tasks.change_next(handle, 0x2000));
    assert_eq!(
        tasks.task_state(handle),
        Some(pal_vm::task::TaskState::PendingFree)
    );
}

// ---- Child depth-first order ----

#[test]
fn child_task_processes_depth_first_under_blocking_parent() {
    // PAL depth-first semantics: children are processed before the parent task itself.
    // Even under a blocking parent (WaitClick), the child animation should still advance.
    let (mut sprites, sprite) = make_sprites();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    // Blocking parent: WaitClick never frees without input.
    let parent = tasks.create_wait_click().unwrap();

    // Sibling of parent (inserted in root list after parent): should be blocked.
    // We use another WaitClick to represent "never fires without input".
    let sibling = tasks.create_wait_click().unwrap();

    // Child animation under the blocking parent (delay 0: advances every process call).
    tasks.set_pal_time(0);
    let child_anim = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc {
                sprite,
                flags: PalAnimationFlags::from(PalAnimationAxis::Horizontal),
                frame_delay_ms: 0,
                running: true,
            },
            Some(parent),
        )
        .unwrap();

    // Frame 1: parent is processed; children (child_anim) are processed first (depth-first).
    tasks.process(&mut sprites, &input);

    // Child animation advanced one frame (depth-first processing confirmed).
    assert_eq!(
        tasks.get_animation_sheet(child_anim).unwrap().current_frame,
        1,
        "child animation should have advanced via depth-first processing"
    );

    // Sibling after the blocking parent must NOT have been processed.
    // Both WaitClick tasks are alive (no input given, so neither freed itself).
    assert!(tasks.is_alive(parent));
    assert!(
        tasks.is_alive(sibling),
        "sibling must not have run; it was blocked by parent"
    );
}

#[test]
fn child_animation_processes_under_blocking_parent() {
    let (mut sprites, sprite) = make_sprites();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    // Blocking parent (WaitClick, never self-frees without input)
    let parent = tasks.create_wait_click().unwrap();

    // Child animation under the blocking parent
    let _child_anim = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc {
                sprite,
                flags: PalAnimationFlags::from(PalAnimationAxis::Horizontal),
                frame_delay_ms: 0,
                running: true,
            },
            Some(parent),
        )
        .unwrap();

    // With delay 0, every process() call advances one frame.
    tasks.set_pal_time(0);
    tasks.process(&mut sprites, &input);

    // Child was processed (frame advanced from 0 to 1).
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        pal_vm::PalRect::new(24, 0, 48, 24)
    );
}

// ---- Blocking task stops later siblings ----

#[test]
fn blocking_task_prevents_later_sibling_update() {
    let mut tasks = TaskSystem::new();
    let mut sprites = SpriteSystem::new();
    let input = no_input();

    // Blocking WaitFrame(2): will block for 2 process calls before going PendingFree
    let _blocker = tasks.create_wait_frame(2).unwrap();
    // Sibling after blocker: WaitFrame(1) — should NOT decrement while blocker is active
    let sibling = tasks.create_wait_frame(1).unwrap();

    // Frame 1: blocker processes (remaining 2->1), sibling is blocked
    tasks.process(&mut sprites, &input);
    assert!(
        tasks.is_alive(sibling),
        "sibling should still be alive (blocked)"
    );

    // Frame 2: blocker (remaining 1->0 -> PendingFree), sibling still blocked
    tasks.process(&mut sprites, &input);
    assert!(
        tasks.is_alive(sibling),
        "sibling still blocked while blocker is pending-free"
    );

    // Frame 3: blocker freed (PendingFree -> Free), sibling now unblocked and processes (1->0 -> PendingFree)
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(sibling), "sibling is now PendingFree");

    // Frame 4: sibling freed
    tasks.process(&mut sprites, &input);
    assert!(!tasks.is_alive(sibling));
}

#[test]
fn nonblocking_animation_siblings_both_advance() {
    // Animation tasks have no blocking flag; two siblings both process every frame.
    let mut sprites = SpriteSystem::new();
    sprites.insert_texture(SceneTexture::rgba8(
        SceneTextureId(10),
        1,
        96,
        24,
        vec![255; 96 * 24 * 4],
    ));
    let sprite_a = sprites.create(SpriteDesc {
        texture_id: SceneTextureId(10),
        texture_width: 96,
        texture_height: 24,
        cell_width: 24,
        cell_height: 24,
        position: PalPoint3::new(0, 0, 0),
        ..SpriteDesc::new(SceneTextureId(10), 96, 24)
    });
    let sprite_b = sprites.create(SpriteDesc {
        texture_id: SceneTextureId(10),
        texture_width: 96,
        texture_height: 24,
        cell_width: 24,
        cell_height: 24,
        position: PalPoint3::new(0, 0, 0),
        ..SpriteDesc::new(SceneTextureId(10), 96, 24)
    });

    let mut tasks = TaskSystem::new();
    let input = no_input();

    tasks.set_pal_time(0);
    let anim_a = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc::horizontal(sprite_a, 0),
            None,
        )
        .unwrap();
    let anim_b = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc::horizontal(sprite_b, 0),
            None,
        )
        .unwrap();

    // Both advance each process call (delay 0, non-blocking).
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(anim_a).unwrap().current_frame, 1);
    assert_eq!(tasks.get_animation_sheet(anim_b).unwrap().current_frame, 1);

    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(anim_a).unwrap().current_frame, 2);
    assert_eq!(tasks.get_animation_sheet(anim_b).unwrap().current_frame, 2);
}

// ---- WaitClick responds to input ----

#[test]
fn wait_click_frees_on_any_push() {
    let mut tasks = TaskSystem::new();
    let mut sprites = SpriteSystem::new();

    let h = tasks.create_wait_click().unwrap();

    let mut input = PalInputState::new();
    // No push yet
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(h));

    // Simulate a mouse push
    input.begin_frame();
    input.handle_mouse_button_event(pal_vm::event::MouseButton::Left, true);
    tasks.process(&mut sprites, &input);
    // Now PendingFree
    assert!(
        tasks.is_alive(h),
        "WaitClick should be PendingFree after push"
    );

    input.begin_frame();
    tasks.process(&mut sprites, &input);
    assert!(
        !tasks.is_alive(h),
        "WaitClick must be freed after pending-free process"
    );
}

#[test]
fn wait_click_or_time_frees_on_input_before_timeout() {
    let mut tasks = TaskSystem::new();
    let mut sprites = SpriteSystem::new();
    let mut input = PalInputState::new();

    tasks.set_pal_time(100);
    let h = tasks.create_wait_click_or_time(250).unwrap();

    tasks.set_pal_time(200);
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(h));

    input.begin_frame();
    input.handle_mouse_button_event(pal_vm::event::MouseButton::Left, true);
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(h));

    input.begin_frame();
    tasks.process(&mut sprites, &input);
    assert!(!tasks.is_alive(h));
}

#[test]
fn wait_click_or_time_frees_on_timeout_without_input() {
    let mut tasks = TaskSystem::new();
    let mut sprites = SpriteSystem::new();
    let input = PalInputState::new();

    tasks.set_pal_time(100);
    let h = tasks.create_wait_click_or_time(250).unwrap();

    tasks.set_pal_time(349);
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(h));

    tasks.set_pal_time(350);
    tasks.process(&mut sprites, &input);
    assert!(tasks.is_alive(h));

    tasks.set_pal_time(351);
    tasks.process(&mut sprites, &input);
    assert!(!tasks.is_alive(h));
}

// ---- Animation via TaskSystem ----

#[test]
fn sheet_animation_driven_by_task_system() {
    let (mut sprites, sprite) = make_sprites();
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

    // Not enough time elapsed
    tasks.set_pal_time(5);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 0);

    // Enough time elapsed: advance one frame
    tasks.set_pal_time(10);
    tasks.process(&mut sprites, &input);
    assert_eq!(tasks.get_animation_sheet(handle).unwrap().current_frame, 1);
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(24, 0, 48, 24)
    );
}

#[test]
fn sheet_animation_does_not_generate_draw_command() {
    // Animation tasks only modify sprite source_rect; they never produce draw commands.
    // Draw commands are produced exclusively by SpriteSystem based on sprite visibility.
    let mut sprites = SpriteSystem::new();
    sprites.insert_texture(SceneTexture::rgba8(
        SceneTextureId(10),
        1,
        96,
        24,
        vec![255; 96 * 24 * 4],
    ));
    // Create sprite with visible: false (default from SpriteDesc::new).
    let sprite = sprites.create(SpriteDesc::new(SceneTextureId(10), 96, 24));
    // Confirm: invisible sprite, no commands yet.
    assert!(sprites.commands().is_empty());

    let mut tasks = TaskSystem::new();
    let input = no_input();
    tasks.set_pal_time(0);
    let _h = tasks
        .create_animation_sheet(
            &mut sprites,
            PalSheetAnimationDesc {
                sprite,
                flags: PalAnimationFlags::from(PalAnimationAxis::Horizontal),
                frame_delay_ms: 0,
                running: true,
            },
            None,
        )
        .unwrap();

    tasks.process(&mut sprites, &input);
    tasks.process(&mut sprites, &input);
    tasks.process(&mut sprites, &input);

    // Animation ran, but sprite is still invisible — no draw commands.
    let cmds = sprites.commands();
    assert!(
        cmds.is_empty(),
        "animation task must not produce draw commands; only visible sprites do"
    );

    // Now make visible: SpriteSystem produces draw commands, animation still doesn't add extras.
    sprites.view_ctrl(sprite, true);
    let cmds = sprites.commands();
    assert_eq!(
        cmds.len(),
        1,
        "one visible sprite should produce exactly one draw command"
    );
}

#[test]
fn sequence_animation_driven_by_task_system() {
    let (mut sprites, sprite) = make_sprites();
    let mut tasks = TaskSystem::new();
    let input = no_input();

    tasks.set_pal_time(0);
    let handle = tasks
        .create_animation_sequence(
            &mut sprites,
            PalSequenceAnimationDesc {
                sprite,
                frames: vec![
                    PalAnimationFrameRecord::new(PalRect::new(0, 0, 24, 24), 5, 1),
                    PalAnimationFrameRecord::new(PalRect::new(24, 0, 48, 24), 5, -1),
                ],
                running: true,
            },
            None,
        )
        .unwrap();

    // Frame 0 is applied immediately on create
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(0, 0, 24, 24)
    );

    // Not enough time
    tasks.set_pal_time(3);
    tasks.process(&mut sprites, &input);
    assert_eq!(
        tasks.get_animation_sequence(handle).unwrap().current_frame,
        0
    );

    // Advance to frame 1
    tasks.set_pal_time(5);
    tasks.process(&mut sprites, &input);
    assert_eq!(
        tasks.get_animation_sequence(handle).unwrap().current_frame,
        1
    );
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(24, 0, 48, 24)
    );

    // Terminal frame: current_frame becomes -1
    tasks.set_pal_time(10);
    tasks.process(&mut sprites, &input);
    assert_eq!(
        tasks.get_animation_sequence(handle).unwrap().current_frame,
        -1
    );
    // Rect stays at last frame
    assert_eq!(
        sprites.get(sprite).unwrap().source_rect,
        PalRect::new(24, 0, 48, 24)
    );
}
