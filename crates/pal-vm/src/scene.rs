use std::time::Duration;

use crate::runtime::RuntimeStatus;

#[derive(Clone, Debug)]
pub struct FrameScene {
    pub clear_color: [f64; 4],
    pub diagnostic_label: String,
    pub logical_width: u32,
    pub logical_height: u32,
    pub textures: Vec<SceneTexture>,
    pub commands: Vec<DrawCommand>,
}

impl FrameScene {
    pub const PAL_DEFAULT_WIDTH: u32 = 1920;
    pub const PAL_DEFAULT_HEIGHT: u32 = 1080;

    pub fn boot() -> Self {
        Self {
            clear_color: [0.02, 0.02, 0.025, 1.0],
            diagnostic_label: "boot".to_owned(),
            logical_width: Self::PAL_DEFAULT_WIDTH,
            logical_height: Self::PAL_DEFAULT_HEIGHT,
            textures: Vec::new(),
            commands: Vec::new(),
        }
    }

    pub fn from_runtime_status(status: &RuntimeStatus, elapsed: Duration) -> Self {
        let pulse = ((elapsed.as_secs_f64() * 2.0).sin() * 0.5) + 0.5;
        let clear_color = match status {
            RuntimeStatus::NotBooted => [0.02, 0.02, 0.025, 1.0],
            RuntimeStatus::Running { .. } => [0.015, 0.020 + pulse * 0.010, 0.045, 1.0],
            RuntimeStatus::WaitFrame { .. } => [0.020, 0.030, 0.055, 1.0],
            RuntimeStatus::WaitClick { .. } => [0.035 + pulse * 0.015, 0.020, 0.050, 1.0],
            RuntimeStatus::Halted { .. } => [0.015, 0.045, 0.020, 1.0],
            RuntimeStatus::UnsupportedCommand { .. } | RuntimeStatus::UnsupportedExtCall { .. } => {
                [0.065, 0.040 + pulse * 0.015, 0.015, 1.0]
            }
            RuntimeStatus::Faulted { .. } => [0.070, 0.010, 0.010, 1.0],
        };
        Self {
            clear_color,
            diagnostic_label: status.to_string(),
            logical_width: Self::PAL_DEFAULT_WIDTH,
            logical_height: Self::PAL_DEFAULT_HEIGHT,
            textures: Vec::new(),
            commands: Vec::new(),
        }
    }

    pub fn with_logical_size(mut self, width: u32, height: u32) -> Self {
        self.logical_width = width.max(1);
        self.logical_height = height.max(1);
        self
    }

    pub fn with_texture(mut self, texture: SceneTexture) -> Self {
        self.textures.push(texture);
        self
    }

    pub fn with_command(mut self, command: DrawCommand) -> Self {
        self.commands.push(command);
        self
    }
}

impl Default for FrameScene {
    fn default() -> Self {
        Self::boot()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SceneTextureId(pub u64);

#[derive(Clone, Debug)]
pub struct SceneTexture {
    pub id: SceneTextureId,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub format: SceneTextureFormat,
    pub pixels: Vec<u8>,
}

impl SceneTexture {
    pub fn rgba8(
        id: SceneTextureId,
        generation: u64,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Self {
        Self {
            id,
            generation,
            width,
            height,
            format: SceneTextureFormat::Rgba8,
            pixels,
        }
    }

    pub fn checked_rgba8(
        id: SceneTextureId,
        generation: u64,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> anyhow::Result<Self> {
        let expected = width as usize * height as usize * 4;
        if pixels.len() != expected {
            return Err(anyhow::anyhow!(
                "RGBA texture {:?} has {} bytes, expected {} for {}x{}",
                id,
                pixels.len(),
                expected,
                width,
                height
            ));
        }
        Ok(Self::rgba8(id, generation, width, height, pixels))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneTextureFormat {
    Rgba8,
}

#[derive(Clone, Debug)]
pub enum DrawCommand {
    Sprite(SpriteDraw),
    SolidQuad(SolidQuad),
}

#[derive(Clone, Copy, Debug)]
pub struct SpriteDraw {
    pub texture_id: SceneTextureId,
    pub priority: i32,
    pub dst: RectF,
    pub src: RectF,
    pub source_rect: [i32; 4],
    pub texture_size: [u32; 2],
    pub cell_size: [u32; 2],
    pub position: [f32; 3],
    pub offset: [i32; 2],
    pub color: [f32; 4],
    pub scale: f32,
    pub rotation: [f32; 3],
    pub center_offset: [f32; 2],
    pub render_mode: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct SolidQuad {
    pub dst: RectF,
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn is_drawable(self) -> bool {
        self.w.abs() > f32::EPSILON && self.h.abs() > f32::EPSILON
    }
}
