use pal_vm::{
    pal_device_rect_to_clip_quad, sprite_transform_debug, DrawCommand, PalColor, PalPoint2,
    PalPoint3, PalRect, RectF, RenderTargetMetrics, SceneTexture, SceneTextureId, SolidQuad,
    SpriteDesc, SpriteSystem,
};

fn textured_sprite_system() -> SpriteSystem {
    let mut sprites = SpriteSystem::new();
    sprites.insert_texture(SceneTexture::rgba8(
        SceneTextureId(42),
        1,
        64,
        64,
        vec![200u8; 64 * 64 * 4],
    ));
    sprites.create(SpriteDesc {
        texture_id: SceneTextureId(42),
        texture_width: 64,
        texture_height: 64,
        cell_width: 64,
        cell_height: 64,
        visible: true,
        color: PalColor::WHITE,
        source_name: "test_bg".to_owned(),
        ..SpriteDesc::new(SceneTextureId(42), 64, 64)
    });
    sprites
}

#[test]
fn invisible_sprite_produces_no_draw_command() {
    let mut sprites = SpriteSystem::new();
    sprites.insert_texture(SceneTexture::rgba8(
        SceneTextureId(1),
        1,
        32,
        32,
        vec![0u8; 32 * 32 * 4],
    ));
    sprites.create(SpriteDesc {
        texture_id: SceneTextureId(1),
        texture_width: 32,
        texture_height: 32,
        visible: false, // hidden
        ..SpriteDesc::new(SceneTextureId(1), 32, 32)
    });
    let commands: Vec<DrawCommand> = sprites.commands();
    assert!(
        commands.is_empty(),
        "invisible sprite must not produce a draw command"
    );
}

#[test]
fn visible_sprite_produces_sprite_draw_command() {
    let sprites = textured_sprite_system();
    let commands: Vec<DrawCommand> = sprites.commands();
    assert_eq!(commands.len(), 1, "one visible sprite → one draw command");
    match &commands[0] {
        DrawCommand::Sprite(s) => {
            assert_eq!(s.texture_id, SceneTextureId(42));
            assert!(s.dst.w > 0.0, "dst width must be positive");
            assert!(s.dst.h > 0.0, "dst height must be positive");
        }
        DrawCommand::SolidQuad(_) => panic!("expected Sprite, got SolidQuad"),
    }
}

#[test]
fn solid_quad_draw_command_fields() {
    let cmd = DrawCommand::SolidQuad(SolidQuad {
        dst: RectF::new(10.0, 20.0, 100.0, 50.0),
        color: [1.0, 0.0, 0.0, 1.0],
    });
    match cmd {
        DrawCommand::SolidQuad(q) => {
            assert_eq!(q.dst.x, 10.0);
            assert_eq!(q.dst.y, 20.0);
            assert_eq!(q.dst.w, 100.0);
            assert_eq!(q.dst.h, 50.0);
        }
        _ => panic!("expected SolidQuad"),
    }
}

#[test]
fn zero_size_dst_is_not_drawable() {
    let r = RectF::new(0.0, 0.0, 0.0, 50.0);
    assert!(!r.is_drawable(), "zero width rect must not be drawable");
    let r2 = RectF::new(0.0, 0.0, 50.0, 0.0);
    assert!(!r2.is_drawable(), "zero height rect must not be drawable");
    let r3 = RectF::new(0.0, 0.0, 10.0, 10.0);
    assert!(r3.is_drawable(), "positive rect must be drawable");
}

#[test]
fn render_node_count_matches_sprite_count() {
    let sprites = textured_sprite_system();
    assert_eq!(sprites.len(), sprites.render_node_count());
}

#[test]
fn surface_count_matches_inserted_textures() {
    let sprites = textured_sprite_system();
    assert_eq!(sprites.surface_count(), 1);
}

#[test]
fn pal_top_left_pixel_maps_to_clip_top_left() {
    let quad = pal_device_rect_to_clip_quad(RectF::new(0.0, 0.0, 100.0, 50.0), [1920, 1080]);
    assert_eq!(quad[0], [-1.0, 1.0]);
    assert!((quad[1][0] - -0.8958333).abs() < 0.0001);
    assert!((quad[2][1] - 0.9074074).abs() < 0.0001);
}

#[test]
fn sprite_transform_reports_device_rect_and_uv_from_pal_pixels() {
    let mut sprites = SpriteSystem::new();
    sprites.insert_texture(SceneTexture::rgba8(
        SceneTextureId(7),
        1,
        200,
        100,
        vec![255; 200 * 100 * 4],
    ));
    sprites.create(SpriteDesc {
        texture_id: SceneTextureId(7),
        texture_width: 200,
        texture_height: 100,
        cell_width: 50,
        cell_height: 25,
        source_rect: Some(PalRect::new(50, 25, 100, 50)),
        position: PalPoint3::new(10, 20, 5),
        offset: PalPoint2::new(3, 4),
        visible: true,
        base_priority: 7,
        ..SpriteDesc::new(SceneTextureId(7), 200, 100)
    });

    let commands = sprites.commands();
    let DrawCommand::Sprite(draw) = &commands[0] else {
        panic!("expected sprite draw");
    };
    assert_eq!(draw.dst, RectF::new(13.0, 24.0, 50.0, 25.0));
    assert_eq!(draw.priority, 12);
    assert_eq!(draw.src, RectF::new(0.25, 0.25, 0.25, 0.25));
    assert_eq!(draw.source_rect, [50, 25, 100, 50]);

    let metrics = RenderTargetMetrics::new([3840, 2160], [1920, 1080]);
    let transform = sprite_transform_debug(draw, &metrics);
    assert_eq!(transform.dst_device, RectF::new(26.0, 48.0, 100.0, 50.0));
    assert_eq!(transform.uv, RectF::new(0.25, 0.25, 0.25, 0.25));
}
