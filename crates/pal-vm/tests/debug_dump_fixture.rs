use pal_vm::{
    collect_frame_dump, pal_debug_enabled, FrameEvent, FrameScene, RuntimeStatus, SceneTexture,
    SceneTextureId, SpriteDesc, SpriteSystem, TaskSystem, WaitRequest,
};

fn make_sprite_system() -> SpriteSystem {
    let mut sprites = SpriteSystem::new();
    sprites.insert_texture(SceneTexture::rgba8(
        SceneTextureId(10),
        1,
        32,
        32,
        vec![128u8; 32 * 32 * 4],
    ));
    sprites.create(SpriteDesc {
        texture_id: SceneTextureId(10),
        texture_width: 32,
        texture_height: 32,
        cell_width: 32,
        cell_height: 32,
        visible: true,
        ..SpriteDesc::new(SceneTextureId(10), 32, 32)
    });
    sprites
}

#[test]
fn pal_debug_enabled_reads_env() {
    // Don't set PAL_DEBUG in test env; just verify it doesn't panic.
    let _enabled = pal_debug_enabled();
}

#[test]
fn collect_dump_produces_expected_counts() {
    let sprites = make_sprite_system();
    let tasks = TaskSystem::new();
    let status = RuntimeStatus::Running { pc: 0x1234 };
    let events = vec![FrameEvent::ExtCallSkipped {
        pc: 0x100,
        category: 3,
        index: 2,
        name: Some("sp_set".to_owned()),
    }];
    let scene = FrameScene::boot();
    let dump = collect_frame_dump(1, 100, 16, &status, &events, &tasks, &sprites, &scene);

    assert_eq!(dump.frame_index, 1);
    assert_eq!(dump.pal_time_ms, 100);
    assert_eq!(dump.delta_ms, 16);
    assert!(dump.runtime_status.contains("0x00001234"));
    assert_eq!(dump.sprite_count, 1);
    assert_eq!(dump.surface_count, 1);
    assert_eq!(dump.render_node_count, 1);
    assert_eq!(dump.draw_command_count, 0);
    assert_eq!(dump.frame_events.len(), 1);
    assert!(dump.frame_events[0].contains("ext_skip"));
    assert!(dump.frame_events[0].contains("sp_set"));
}

#[test]
fn frame_event_wait_formatted_correctly() {
    let sprites = SpriteSystem::new();
    let tasks = TaskSystem::new();
    let status = RuntimeStatus::WaitFrame { pc: 0xABCD };
    let events = vec![FrameEvent::WaitEmitted {
        pc: 0xABCD,
        kind: WaitRequest::Frame(1),
    }];
    let scene = FrameScene::boot();
    let dump = collect_frame_dump(5, 200, 16, &status, &events, &tasks, &sprites, &scene);
    assert!(dump.runtime_status.contains("wait frame"));
    assert!(dump.frame_events[0].contains("Frame(1)"));
}

#[test]
fn dump_sprite_entry_fields_match() {
    let sprites = make_sprite_system();
    let tasks = TaskSystem::new();
    let dump = collect_frame_dump(
        0,
        0,
        0,
        &RuntimeStatus::NotBooted,
        &[],
        &tasks,
        &sprites,
        &FrameScene::boot(),
    );
    assert_eq!(dump.sprite_entries.len(), 1);
    let sp = &dump.sprite_entries[0];
    assert!(sp.visible);
    assert_eq!(sp.texture_size, (32, 32));
    assert_eq!(sp.cell_size, (32, 32));
}
