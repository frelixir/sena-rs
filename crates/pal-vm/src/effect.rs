use crate::scene::{RectF, SolidQuad};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PalEffectSystem {
    enabled: bool,
    active: Option<PalEffectState>,
}

impl Default for PalEffectSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl PalEffectSystem {
    pub fn new() -> Self {
        Self {
            enabled: true,
            active: None,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn active(&self) -> bool {
        self.active.is_some()
    }

    /// PalEffectEx(id, duration, arg, wait): id 0 is a no-op, >0x31 fails.
    pub fn effect_ex(
        &mut self,
        effect_id: u32,
        duration_ms: u32,
        arg: i32,
        wait: bool,
        now_ms: u32,
    ) -> i32 {
        if !self.enabled || effect_id == 0 {
            return 1;
        }
        if effect_id > 0x31 || self.active.is_some() {
            return 0;
        }
        self.active = Some(PalEffectState {
            effect_id,
            duration_ms: duration_ms.max(1),
            arg,
            wait,
            start_ms: now_ms,
            state: 1,
        });
        1
    }

    pub fn effect(&mut self, effect_id: u32, duration_ms: u32, now_ms: u32) -> i32 {
        self.effect_ex(effect_id, duration_ms, 0, true, now_ms)
    }

    pub fn tick(&mut self, now_ms: u32) {
        let Some(active) = self.active else {
            return;
        };
        if now_ms.wrapping_sub(active.start_ms) >= active.duration_ms {
            self.active = None;
        }
    }

    pub fn overlay_quad(
        &self,
        logical_width: u32,
        logical_height: u32,
        now_ms: u32,
    ) -> Option<SolidQuad> {
        let active = self.active?;
        let elapsed = now_ms.wrapping_sub(active.start_ms).min(active.duration_ms);
        let t = elapsed as f32 / active.duration_ms.max(1) as f32;
        let alpha = effect_alpha(active.effect_id, t);
        if alpha <= 0.0 {
            return None;
        }
        let color = if active.arg < 0 {
            [1.0, 1.0, 1.0, alpha]
        } else {
            [0.0, 0.0, 0.0, alpha]
        };
        Some(SolidQuad {
            dst: RectF::new(
                0.0,
                0.0,
                logical_width.max(1) as f32,
                logical_height.max(1) as f32,
            ),
            color,
        })
    }

    pub fn state(&self) -> Option<PalEffectState> {
        self.active
    }
}

fn effect_alpha(effect_id: u32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match effect_id % 4 {
        1 => 1.0 - t,
        2 => t,
        3 => (1.0 - ((t - 0.5).abs() * 2.0)).clamp(0.0, 1.0),
        _ => 1.0 - t,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PalEffectState {
    pub effect_id: u32,
    pub duration_ms: u32,
    pub arg: i32,
    pub wait: bool,
    pub start_ms: u32,
    pub state: i32,
}
