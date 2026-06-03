use std::path::PathBuf;
use std::sync::Arc;

use pal_asset::Nls;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::audio::AudioConfig;
use crate::engine::{Engine, EngineConfig, TraceConfig};
use crate::event::{InputEvent, MouseButton, PalEvent};
use crate::renderer::{RenderOutcome, Renderer, RendererConfig};
use crate::runtime::ScriptRuntimeConfig;
use crate::scene::FrameScene;

#[derive(Clone, Debug)]
pub struct SenaConfig {
    pub game_root: Option<PathBuf>,
    pub nls: Nls,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub renderer: RendererConfig,
    pub prefer_config_window_size: bool,
    pub print_loaded_assets: bool,
    pub trace_script: bool,
    pub trace_scene: bool,
    pub trace_sprites: bool,
    pub trace_assets: bool,
    pub trace_render: bool,
    pub script_budget_per_frame: usize,
    pub audio: AudioConfig,
}

impl Default for SenaConfig {
    fn default() -> Self {
        Self {
            game_root: None,
            nls: Nls::ShiftJis,
            title: "sena".to_owned(),
            width: FrameScene::PAL_DEFAULT_WIDTH,
            height: FrameScene::PAL_DEFAULT_HEIGHT,
            renderer: RendererConfig::default(),
            prefer_config_window_size: true,
            print_loaded_assets: false,
            trace_script: false,
            trace_scene: false,
            trace_sprites: false,
            trace_assets: false,
            trace_render: false,
            script_budget_per_frame: ScriptRuntimeConfig::default().instructions_per_frame,
            audio: AudioConfig::default(),
        }
    }
}

pub fn run_sena(config: SenaConfig) -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = SenaApplication::new(config)?;
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct SenaApplication {
    config: SenaConfig,
    engine: Engine,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
}

impl SenaApplication {
    fn new(config: SenaConfig) -> anyhow::Result<Self> {
        let engine = Engine::new(EngineConfig {
            game_root: config.game_root.clone(),
            nls: config.nls,
            script_runtime: ScriptRuntimeConfig {
                instructions_per_frame: config.script_budget_per_frame,
                trace: config.trace_script,
            },
            audio: config.audio.clone(),
            trace: TraceConfig {
                script: config.trace_script,
                scene: config.trace_scene,
                sprites: config.trace_sprites,
                assets: config.trace_assets,
                render: config.trace_render,
            },
        })?;
        if config.print_loaded_assets {
            print_asset_summary(&engine);
        }
        Ok(Self {
            config,
            engine,
            window: None,
            renderer: None,
        })
    }

    fn create_window_and_renderer(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (width, height) = if self.config.prefer_config_window_size {
            self.engine
                .window_size_from_config(self.config.width, self.config.height)
        } else {
            (self.config.width, self.config.height)
        };
        let attrs = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(width as f64, height as f64));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("sena failed to create a winit window"),
        );
        let mut renderer_config = self.config.renderer;
        renderer_config.virtual_width = width.max(1);
        renderer_config.virtual_height = height.max(1);
        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            event_loop.owned_display_handle(),
            renderer_config,
        ))
        .expect("sena failed to initialize the software renderer");
        self.window = Some(window);
        self.renderer = Some(renderer);
    }

    fn handle_resize(&mut self, size: PhysicalSize<u32>) {
        self.engine.handle_event(PalEvent::Resized {
            width: size.width,
            height: size.height,
        });
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.resize(size);
        }
    }

    fn render(&mut self, event_loop: &ActiveEventLoop) {
        self.engine.handle_event(PalEvent::RedrawRequested);
        let frame = match self.engine.update() {
            Ok(frame) => frame,
            Err(err) => {
                // Log the error but keep the window open so the user can see what happened.
                log::error!("sena engine update failed: {err}");
                return;
            }
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        match renderer.render(&frame.scene) {
            RenderOutcome::Rendered => {}
            RenderOutcome::Skipped => {}
            RenderOutcome::Reconfigured => log::debug!("software surface was reconfigured"),
        }
    }
}

impl ApplicationHandler for SenaApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.create_window_and_renderer(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if window.id() != window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                self.engine.handle_event(PalEvent::CloseRequested);
                event_loop.exit();
            }
            WindowEvent::Resized(size) => self.handle_resize(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.engine
                    .handle_event(PalEvent::ScaleFactorChanged { scale_factor });
            }
            WindowEvent::RedrawRequested => self.render(event_loop),
            WindowEvent::CursorMoved { position, .. } => {
                self.engine
                    .handle_event(PalEvent::Input(InputEvent::CursorMoved {
                        x: position.x,
                        y: position.y,
                    }));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.engine
                    .handle_event(PalEvent::Input(InputEvent::MouseInput {
                        button: map_mouse_button(button),
                        pressed: state == ElementState::Pressed,
                    }));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (delta_x, delta_y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
                };
                self.engine
                    .handle_event(PalEvent::Input(InputEvent::MouseWheel { delta_x, delta_y }));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let key_name = key_name(&event.logical_key);
                if pressed && matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    event_loop.exit();
                }
                self.engine
                    .handle_event(PalEvent::Input(InputEvent::Keyboard { key_name, pressed }));
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Clear per-frame transient state before new events for this frame arrive.
        self.engine.input_begin_frame();
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

fn map_mouse_button(button: WinitMouseButton) -> MouseButton {
    match button {
        WinitMouseButton::Left => MouseButton::Left,
        WinitMouseButton::Right => MouseButton::Right,
        WinitMouseButton::Middle => MouseButton::Middle,
        WinitMouseButton::Back => MouseButton::Back,
        WinitMouseButton::Forward => MouseButton::Forward,
        WinitMouseButton::Other(value) => MouseButton::Other(value),
    }
}

fn key_name(key: &Key) -> String {
    match key {
        Key::Named(named) => format!("{named:?}"),
        Key::Character(text) => text.to_string(),
        Key::Unidentified(_) => "Unidentified".to_owned(),
        Key::Dead(ch) => match ch {
            Some(ch) => format!("Dead({ch})"),
            None => "Dead".to_owned(),
        },
    }
}

fn print_asset_summary(engine: &Engine) {
    let Some(assets) = engine.core_assets() else {
        println!("no game root was provided; core assets were not loaded");
        return;
    };
    println!(
        "Script.src: 0x{:X} bytes, check=0x{:08X}, entry=0x{:08X}",
        assets.script.bytes.len(),
        assets.script_check_value,
        assets.script_entry_pc
    );
    println!("File.dat:   0x{:X} bytes", assets.file_dat.bytes.len());
    println!("Text.dat:   0x{:X} bytes", assets.text_dat.bytes.len());
    println!("Mem.dat:    0x{:X} bytes", assets.mem_dat.bytes.len());
    println!(
        "Point.dat:  0x{:X} bytes, points={}",
        assets.point_dat.bytes.len(),
        assets.point_table.len()
    );
    println!("Runtime:    {}", engine.runtime_status());
    match assets.graphic_index.as_ref() {
        Some(index) => println!("graphic.dat: entries={}", index.len()),
        None => println!("graphic.dat: not present"),
    }
}
