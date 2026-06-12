use std::path::PathBuf;
use std::time::{Duration, Instant};

use pal_asset::{Nls, ResourceManager};

use crate::assets::CoreAssets;
use crate::audio::{AudioConfig, AudioHandle, AudioSystem, PalSoundGroup};
use crate::config::{ini_graphics_size, EngineStartupConfig};
use crate::debug::{collect_frame_dump, pal_debug_enabled, print_dump};
use crate::event::PalEvent;
use crate::input::PalInputState;
use crate::runtime::{RuntimeStatus, RuntimeTick, ScriptRuntime, ScriptRuntimeConfig, WaitRequest};
use crate::scene::FrameScene;
use crate::sprite::SpriteSystem;
use crate::task::TaskSystem;

const MAX_PAL_FRAME_DELTA: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Default)]
pub struct TraceConfig {
    pub script: bool,
    pub scene: bool,
    pub sprites: bool,
    pub assets: bool,
    pub render: bool,
    pub buttons: bool,
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub game_root: Option<PathBuf>,
    pub nls: Nls,
    pub script_runtime: ScriptRuntimeConfig,
    pub audio: AudioConfig,
    pub trace: TraceConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            game_root: None,
            nls: Nls::ShiftJis,
            script_runtime: ScriptRuntimeConfig::default(),
            audio: AudioConfig::default(),
            trace: TraceConfig::default(),
        }
    }
}

pub struct Engine {
    config: EngineConfig,
    startup_config: Option<EngineStartupConfig>,
    resource_manager: Option<ResourceManager>,
    core_assets: Option<CoreAssets>,
    runtime: Option<ScriptRuntime>,
    sprites: SpriteSystem,
    task_system: TaskSystem,
    audio: AudioSystem,
    frame_clock: FrameClock,
    last_runtime_status: RuntimeStatus,
    input: PalInputState,
    window_physical_size: (u32, u32),
    pal_debug: bool,
}

impl Engine {
    pub fn new(config: EngineConfig) -> anyhow::Result<Self> {
        let (startup_config, resource_manager, core_assets, runtime, last_runtime_status) =
            match &config.game_root {
                Some(root) => {
                    let startup_config = EngineStartupConfig::load(root, config.nls)?;
                    let mut resource_manager = ResourceManager::bootstrap(root, config.nls)?;
                    resource_manager.preload_pacs()?;
                    let core_assets = CoreAssets::load(
                        &mut resource_manager,
                        startup_config.system_dat.as_ref(),
                    )?;
                    log::info!(
                        "loaded PAL assets: script_check=0x{:08X}, entry=0x{:08X}, points={}, \
                         file_dat=0x{:X}, text_dat=0x{:X}, mem_dat=0x{:X}, graphic_entries={}",
                        core_assets.script_check_value,
                        core_assets.script_entry_pc,
                        core_assets.point_table.len(),
                        core_assets.file_dat.bytes.len(),
                        core_assets.text_dat.bytes.len(),
                        core_assets.mem_dat.bytes.len(),
                        core_assets
                            .graphic_index
                            .as_ref()
                            .map_or(0, |index| index.len())
                    );
                    let mut runtime = ScriptRuntime::boot(
                        core_assets.script_entry_pc,
                        config.script_runtime.clone(),
                    );
                    if let Some(system_ini) = startup_config.system_ini.clone() {
                        runtime.set_system_ini(system_ini);
                    }
                    // Initialise the writable Mem.dat shadow used by MemDatDirect writes.
                    runtime.load_mem_dat(&core_assets.mem_dat.bytes);
                    let (window_width, window_height) = startup_config.window_size(
                        FrameScene::PAL_DEFAULT_WIDTH,
                        FrameScene::PAL_DEFAULT_HEIGHT,
                    );
                    runtime.set_window_size(window_width, window_height);
                    let status = runtime.status().clone();
                    (
                        Some(startup_config),
                        Some(resource_manager),
                        Some(core_assets),
                        Some(runtime),
                        status,
                    )
                }
                None => (None, None, None, None, RuntimeStatus::NotBooted),
            };

        let audio = AudioSystem::new(config.audio.clone())?;
        let sprites = SpriteSystem::new();
        let task_system = TaskSystem::new();

        let fallback_size = startup_config.as_ref().map_or(
            (
                FrameScene::PAL_DEFAULT_WIDTH,
                FrameScene::PAL_DEFAULT_HEIGHT,
            ),
            |config| {
                config.window_size(
                    FrameScene::PAL_DEFAULT_WIDTH,
                    FrameScene::PAL_DEFAULT_HEIGHT,
                )
            },
        );

        Ok(Self {
            config,
            startup_config,
            resource_manager,
            core_assets,
            runtime,
            sprites,
            task_system,
            audio,
            frame_clock: FrameClock::new(),
            last_runtime_status,
            input: PalInputState::new(),
            window_physical_size: fallback_size,
            pal_debug: pal_debug_enabled(),
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    pub fn startup_config(&self) -> Option<&EngineStartupConfig> {
        self.startup_config.as_ref()
    }

    pub fn resource_manager(&mut self) -> Option<&mut ResourceManager> {
        self.resource_manager.as_mut()
    }

    pub fn core_assets(&self) -> Option<&CoreAssets> {
        self.core_assets.as_ref()
    }

    pub fn runtime(&self) -> Option<&ScriptRuntime> {
        self.runtime.as_ref()
    }

    pub fn runtime_status(&self) -> &RuntimeStatus {
        &self.last_runtime_status
    }

    pub fn sprites(&self) -> &SpriteSystem {
        &self.sprites
    }

    pub fn sprites_mut(&mut self) -> &mut SpriteSystem {
        &mut self.sprites
    }

    pub fn task_system(&self) -> &TaskSystem {
        &self.task_system
    }

    pub fn task_system_mut(&mut self) -> &mut TaskSystem {
        &mut self.task_system
    }

    pub fn audio(&self) -> &AudioSystem {
        &self.audio
    }

    pub fn audio_mut(&mut self) -> &mut AudioSystem {
        &mut self.audio
    }

    pub fn take_core_assets(&mut self) -> Option<CoreAssets> {
        self.core_assets.take()
    }

    pub fn load_sound_from_resource(
        &mut self,
        name: &str,
        group: PalSoundGroup,
    ) -> anyhow::Result<AudioHandle> {
        let resource_manager = self
            .resource_manager
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("cannot load sound {:?} without a game root", name))?;
        self.audio
            .load_static_from_resource(resource_manager, name, group)
    }

    pub fn play_sound(&mut self, handle: AudioHandle, looping: bool) -> anyhow::Result<()> {
        self.audio.play(handle, looping)
    }

    pub fn stop_sound(&mut self, handle: AudioHandle) -> anyhow::Result<()> {
        self.audio.stop(handle)
    }

    pub fn window_size_from_config(&self, fallback_width: u32, fallback_height: u32) -> (u32, u32) {
        let (default_width, default_height) = self
            .startup_config
            .as_ref()
            .map_or((fallback_width, fallback_height), |config| {
                config.logical_size(fallback_width, fallback_height)
            });
        let width = self
            .startup_config
            .as_ref()
            .map_or(default_width, |config| config.window_width(default_width));
        let height = self
            .startup_config
            .as_ref()
            .map_or(default_height, |config| {
                config.window_height(default_height)
            });
        (width.max(640), height.max(480))
    }

    pub fn logical_size_from_config(
        &self,
        fallback_width: u32,
        fallback_height: u32,
    ) -> (u32, u32) {
        let Some(system_ini) = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.parsed_system_ini())
        else {
            return self
                .startup_config
                .as_ref()
                .map_or((fallback_width.max(1), fallback_height.max(1)), |config| {
                    config.logical_size(fallback_width, fallback_height)
                });
        };
        ini_graphics_size(Some(system_ini), fallback_width, fallback_height)
    }

    /// Clear per-frame transient input state after the current frame has consumed it.
    pub fn input_begin_frame(&mut self) {
        self.input.begin_frame();
    }

    pub fn handle_event(&mut self, event: PalEvent) {
        match event {
            PalEvent::Input(ref input_event) => {
                self.input.handle_input_event(input_event);
            }
            PalEvent::CloseRequested => {}
            PalEvent::Resized { width, height } => {
                self.window_physical_size = (width.max(1), height.max(1));
            }
            PalEvent::ScaleFactorChanged { .. } => {}
            PalEvent::RedrawRequested => {}
        }
    }

    pub fn update(&mut self) -> anyhow::Result<EngineFrame> {
        let timing = self.frame_clock.tick_capped(MAX_PAL_FRAME_DELTA);
        self.update_with_timing(timing)
    }

    pub fn update_with_delta(&mut self, delta: Duration) -> anyhow::Result<EngineFrame> {
        let timing = self.frame_clock.tick_fixed(delta);
        self.update_with_timing(timing)
    }

    fn update_with_timing(&mut self, timing: FrameTiming) -> anyhow::Result<EngineFrame> {
        let (logical_width, logical_height) = self.logical_size_from_config(
            FrameScene::PAL_DEFAULT_WIDTH,
            FrameScene::PAL_DEFAULT_HEIGHT,
        );
        self.input.set_coordinate_space(
            self.window_physical_size.0,
            self.window_physical_size.1,
            logical_width,
            logical_height,
        );

        // Update PAL cached time used by all task timing (animation delays, wait timeouts).
        self.task_system
            .set_pal_time(timing.elapsed.as_millis().min(u32::MAX as u128) as u32);

        // Process all tasks: animations update sprite source_rect, wait tasks check input.
        self.task_system.process(&mut self.sprites, &self.input);
        let delta_ms = timing.delta.as_millis().min(u32::MAX as u128) as u32;
        self.sprites.advance_motion_entries(delta_ms);
        self.sprites.advance_transitions(delta_ms);
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.set_pal_time(self.task_system.pal_time_ms);
            runtime.advance_msprites(&mut self.sprites, delta_ms);
            if let Some(core_assets) = self.core_assets.as_ref() {
                let nls = self
                    .resource_manager
                    .as_ref()
                    .map(|manager| manager.nls())
                    .unwrap_or(Nls::ShiftJis);
                runtime.sync_text_sprite(
                    core_assets,
                    nls,
                    self.resource_manager.as_mut(),
                    &mut self.sprites,
                );
            }
            runtime.update_button_input_state(&mut self.sprites, &self.input);
            if self.config.trace.buttons {
                let (mx, my) = self.input.mouse_position();
                runtime.dump_button_states(&self.sprites, timing.frame_index, mx, my);
            }
        }

        // Check if the script runtime was waiting on a task that just completed.
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.set_pal_time(self.task_system.pal_time_ms);
            if let Some(handle) = runtime.pending_wait_handle() {
                if !self.task_system.is_alive(handle) {
                    runtime.resolve_pending_wait();
                }
            }
        }

        // Run script VM.
        let runtime_tick = match (
            self.runtime.as_mut(),
            self.core_assets.as_ref(),
            self.resource_manager.as_mut(),
        ) {
            (Some(runtime), Some(core_assets), resource_manager) => {
                match runtime.run_frame_with_resources(
                    core_assets,
                    resource_manager,
                    Some(&mut self.sprites),
                    Some(&mut self.task_system),
                    Some(&mut self.audio),
                    Some(&self.input),
                    &self.config.script_runtime,
                ) {
                    Ok(tick) => Some(tick),
                    Err(e) => {
                        log::error!("runtime error: {e}");
                        runtime.set_faulted(format!("{e}"));
                        None
                    }
                }
            }
            _ => None,
        };

        // Process any wait request emitted by the VM this tick.
        if let Some(tick) = runtime_tick.as_ref() {
            if let Some(wait_req) = &tick.wait_request {
                // The native PAL loop can observe the same input edge that caused
                // script execution to reach wait_click().  Creating a task after
                // task_system.process() without checking the current frame drops
                // that edge, making ADV text/buttons require an extra click.
                let input_satisfied_click_wait =
                    matches!(wait_req, WaitRequest::Click | WaitRequest::ClickOrTime(_))
                        && self.input.any_push();
                let handle = match wait_req {
                    WaitRequest::Click | WaitRequest::ClickOrTime(_)
                        if input_satisfied_click_wait =>
                    {
                        None
                    }
                    WaitRequest::Frame(n) => self.task_system.create_wait_frame(*n),
                    WaitRequest::Time(ms) => self.task_system.create_wait_time(*ms),
                    WaitRequest::Click => self.task_system.create_wait_click(),
                    WaitRequest::ClickOrTime(ms) => self.task_system.create_wait_click_or_time(*ms),
                };
                match (handle, input_satisfied_click_wait) {
                    (Some(h), _) => {
                        if let Some(runtime) = self.runtime.as_mut() {
                            runtime.set_wait_handle(h);
                        }
                    }
                    (None, true) => {
                        if let Some(runtime) = self.runtime.as_mut() {
                            runtime.resolve_pending_wait();
                        }
                    }
                    (None, false) => {
                        // Pool full: unblock VM immediately rather than hanging forever.
                        log::warn!(
                            "task pool full; wait request cannot be created, VM will continue"
                        );
                        if let Some(runtime) = self.runtime.as_mut() {
                            runtime.resolve_pending_wait();
                        }
                    }
                }
            }

            if tick.status != self.last_runtime_status {
                log::info!("script runtime: {}", tick.status);
            }
            self.last_runtime_status = tick.status.clone();
        }

        self.audio.update();

        let scene = self.compose_scene(timing.elapsed, logical_width, logical_height);

        if self.pal_debug {
            let frame_events = runtime_tick
                .as_ref()
                .map(|t| t.frame_events.as_slice())
                .unwrap_or(&[]);
            let dump = collect_frame_dump(
                timing.frame_index,
                self.task_system.pal_time_ms,
                timing.delta.as_millis() as u32,
                &self.last_runtime_status,
                frame_events,
                &self.task_system,
                &self.sprites,
                &scene,
            );
            print_dump(&dump);
        }

        if self.config.trace.sprites {
            log::debug!(
                "[trace-sprites] frame={} sprites={} surfaces={} render_nodes={}",
                timing.frame_index,
                self.sprites.len(),
                self.sprites.surface_count(),
                self.sprites.render_node_count(),
            );
            for (handle, sp) in self.sprites.iter_sprites() {
                log::debug!(
                    "[trace-sprites]   sprite={} vis={} pos=({:.0},{:.0}) src={:?} name={:?}",
                    handle.0,
                    sp.visible,
                    sp.position.x,
                    sp.position.y,
                    sp.source_name,
                    sp.source_name,
                );
            }
        }

        if self.config.trace.scene {
            log::debug!(
                "[trace-scene] frame={} draw_commands={} clear_color={:?}",
                timing.frame_index,
                scene.commands.len(),
                scene.clear_color,
            );
        }

        if self.config.trace.render {
            for (i, cmd) in scene.commands.iter().enumerate() {
                match cmd {
                    crate::scene::DrawCommand::Sprite(sp) => {
                        log::debug!(
                            "[trace-render] [{i}] sprite tex={} dst=({:.0},{:.0},{:.0}x{:.0}) src=({:.3},{:.3}) prio={}",
                            sp.texture_id.0, sp.dst.x, sp.dst.y, sp.dst.w, sp.dst.h,
                            sp.src.x, sp.src.y, sp.priority,
                        );
                    }
                    crate::scene::DrawCommand::SolidQuad(q) => {
                        log::debug!(
                            "[trace-render] [{i}] solid_quad dst=({:.0},{:.0},{:.0}x{:.0})",
                            q.dst.x,
                            q.dst.y,
                            q.dst.w,
                            q.dst.h,
                        );
                    }
                }
            }
        }

        Ok(EngineFrame {
            timing,
            runtime_tick,
            runtime_status: self.last_runtime_status.clone(),
            scene,
        })
    }

    fn compose_scene(
        &self,
        elapsed: Duration,
        logical_width: u32,
        logical_height: u32,
    ) -> FrameScene {
        let mut scene = FrameScene::from_runtime_status(&self.last_runtime_status, elapsed)
            .with_logical_size(logical_width, logical_height);
        for texture in self.sprites.textures() {
            scene.textures.push(texture.clone());
        }
        for command in self.sprites.commands() {
            scene.commands.push(command);
        }
        for command in self.sprites.transition_commands() {
            scene.commands.push(command);
        }
        if let Some(runtime) = self.runtime.as_ref() {
            if let Some(quad) = runtime.effect_overlay(logical_width, logical_height) {
                scene
                    .commands
                    .push(crate::scene::DrawCommand::SolidQuad(quad));
            }
        }
        scene
    }
}

#[derive(Clone, Debug)]
pub struct EngineFrame {
    pub timing: FrameTiming,
    pub runtime_tick: Option<RuntimeTick>,
    pub runtime_status: RuntimeStatus,
    pub scene: FrameScene,
}

#[derive(Clone, Copy, Debug)]
pub struct FrameTiming {
    pub frame_index: u64,
    pub delta: Duration,
    pub elapsed: Duration,
}

#[derive(Debug)]
struct FrameClock {
    start: Instant,
    previous: Instant,
    frame_index: u64,
}

impl FrameClock {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            previous: now,
            frame_index: 0,
        }
    }

    fn tick_capped(&mut self, max_delta: Duration) -> FrameTiming {
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.previous).min(max_delta);
        let elapsed = self
            .previous
            .saturating_duration_since(self.start)
            .saturating_add(delta);
        self.previous = now;
        self.frame_index += 1;
        FrameTiming {
            frame_index: self.frame_index,
            delta,
            elapsed,
        }
    }

    fn tick_fixed(&mut self, delta: Duration) -> FrameTiming {
        let delta = delta.min(MAX_PAL_FRAME_DELTA);
        let elapsed = self
            .previous
            .saturating_duration_since(self.start)
            .saturating_add(delta);
        self.previous = self.start + elapsed;
        self.frame_index += 1;
        FrameTiming {
            frame_index: self.frame_index,
            delta,
            elapsed,
        }
    }
}
