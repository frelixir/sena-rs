use std::path::PathBuf;
use std::time::{Duration, Instant};

use pal_asset::{Nls, ResourceManager};

use crate::assets::CoreAssets;
use crate::audio::{AudioConfig, AudioHandle, AudioSystem, PalSoundGroup};
use crate::config::EngineStartupConfig;
use crate::debug::{collect_frame_dump, pal_debug_enabled, print_dump};
use crate::event::PalEvent;
use crate::input::PalInputState;
use crate::runtime::{RuntimeStatus, RuntimeTick, ScriptRuntime, ScriptRuntimeConfig, WaitRequest};
use crate::scene::FrameScene;
use crate::sprite::SpriteSystem;
use crate::task::TaskSystem;

#[derive(Clone, Debug, Default)]
pub struct TraceConfig {
    pub script: bool,
    pub scene: bool,
    pub sprites: bool,
    pub assets: bool,
    pub render: bool,
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
    pal_debug: bool,
}

impl Engine {
    pub fn new(config: EngineConfig) -> anyhow::Result<Self> {
        let (startup_config, resource_manager, core_assets, runtime, last_runtime_status) =
            match &config.game_root {
                Some(root) => {
                    let startup_config = EngineStartupConfig::load(root)?;
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
                    // Initialise the writable Mem.dat shadow used by MemDatDirect writes.
                    runtime.load_mem_dat(&core_assets.mem_dat.bytes);
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
        let width = self
            .startup_config
            .as_ref()
            .map_or(fallback_width, |config| config.window_width(fallback_width));
        let height = self
            .startup_config
            .as_ref()
            .map_or(fallback_height, |config| {
                config.window_height(fallback_height)
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
            return self.window_size_from_config(fallback_width, fallback_height);
        };
        let width = system_ini
            .get("graphics")
            .and_then(|section| section.get("def_cg_width"))
            .and_then(|value| value.as_int())
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(fallback_width);
        let height = system_ini
            .get("graphics")
            .and_then(|section| section.get("def_cg_height"))
            .and_then(|value| value.as_int())
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(fallback_height);
        (width.max(1), height.max(1))
    }

    /// Clear per-frame transient input state. Must be called from app before new events arrive.
    pub fn input_begin_frame(&mut self) {
        self.input.begin_frame();
    }

    pub fn handle_event(&mut self, event: PalEvent) {
        match event {
            PalEvent::Input(ref input_event) => {
                self.input.handle_input_event(input_event);
            }
            PalEvent::CloseRequested => {}
            PalEvent::Resized { .. } => {}
            PalEvent::ScaleFactorChanged { .. } => {}
            PalEvent::RedrawRequested => {}
        }
    }

    pub fn update(&mut self) -> anyhow::Result<EngineFrame> {
        let timing = self.frame_clock.tick();

        // Update PAL cached time used by all task timing (animation delays, wait timeouts).
        self.task_system
            .set_pal_time(timing.elapsed.as_millis().min(u32::MAX as u128) as u32);

        // Process all tasks: animations update sprite source_rect, wait tasks check input.
        self.task_system.process(&mut self.sprites, &self.input);
        self.sprites
            .advance_transitions(timing.delta.as_millis().min(u32::MAX as u128) as u32);
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.advance_msprites(
                &mut self.sprites,
                timing.delta.as_millis().min(u32::MAX as u128) as u32,
            );
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
                let handle = match wait_req {
                    WaitRequest::Frame(n) => self.task_system.create_wait_frame(*n),
                    WaitRequest::Time(ms) => self.task_system.create_wait_time(*ms),
                    WaitRequest::Click => self.task_system.create_wait_click(),
                };
                match handle {
                    Some(h) => {
                        if let Some(runtime) = self.runtime.as_mut() {
                            runtime.set_wait_handle(h);
                        }
                    }
                    None => {
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

        let scene = self.compose_scene(timing.elapsed);

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

    fn compose_scene(&self, elapsed: Duration) -> FrameScene {
        let (logical_width, logical_height) = self.logical_size_from_config(
            FrameScene::PAL_DEFAULT_WIDTH,
            FrameScene::PAL_DEFAULT_HEIGHT,
        );
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

    fn tick(&mut self) -> FrameTiming {
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.previous);
        let elapsed = now.saturating_duration_since(self.start);
        self.previous = now;
        self.frame_index += 1;
        FrameTiming {
            frame_index: self.frame_index,
            delta,
            elapsed,
        }
    }
}
