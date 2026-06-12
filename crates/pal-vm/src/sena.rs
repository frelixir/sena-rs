use std::path::PathBuf;

use clap::Parser;
use pal_asset::Nls;
use pal_vm::{run_sena, AudioConfig, DiagnosticClick, FrameScene, SenaConfig};

#[derive(Debug, Parser)]
#[command(name = "sena")]
#[command(about = "PAL engine launcher")]
struct Args {
    #[arg(value_name = "GAME_ROOT")]
    game_root: Option<PathBuf>,

    #[arg(long, default_value = "sjis")]
    nls: Nls,

    #[arg(long, default_value_t = FrameScene::PAL_DEFAULT_WIDTH)]
    width: u32,

    #[arg(long, default_value_t = FrameScene::PAL_DEFAULT_HEIGHT)]
    height: u32,

    #[arg(long, default_value = "sena")]
    title: String,

    #[arg(long)]
    ignore_config_window_size: bool,

    #[arg(long)]
    print_loaded_assets: bool,

    /// Trace every script opcode and extcall (verbose).
    #[arg(long)]
    trace_script: bool,

    /// Trace scene composition (clear color, command count) each frame.
    #[arg(long)]
    trace_scene: bool,

    /// Trace sprite state (position, visibility, source rect) each frame.
    #[arg(long)]
    trace_sprites: bool,

    /// Trace asset open / decode / upload events.
    #[arg(long)]
    trace_assets: bool,

    /// Trace draw command submission each frame.
    #[arg(long)]
    trace_render: bool,

    /// Trace button state (group, index, name, rect, enabled, visible, hit) every frame.
    #[arg(long)]
    trace_buttons: bool,

    #[arg(long, default_value_t = 4096)]
    script_budget_per_frame: usize,

    #[arg(long)]
    no_audio: bool,

    /// Run the engine without creating a window or GPU renderer.
    #[arg(long, alias = "handless", alias = "gui-handless")]
    headless: bool,

    /// Number of frames to execute in --headless diagnostics.
    #[arg(long, default_value_t = 120)]
    diagnostic_frames: usize,

    /// Real-time delay between --headless diagnostic frames.
    #[arg(long, default_value_t = 16)]
    diagnostic_frame_ms: u64,

    /// Write the final --headless diagnostic frame to a PNG file.
    #[arg(long)]
    diagnostic_png: Option<PathBuf>,

    /// Inject headless clicks as frame:x:y in PAL logical coordinates. Can be passed more than once.
    #[arg(long, value_parser = parse_diagnostic_click)]
    diagnostic_click: Vec<DiagnosticClick>,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = Args::parse();
    run_sena(SenaConfig {
        game_root: args.game_root,
        nls: args.nls,
        title: args.title,
        width: args.width,
        height: args.height,
        prefer_config_window_size: !args.ignore_config_window_size,
        print_loaded_assets: args.print_loaded_assets,
        trace_script: args.trace_script,
        trace_scene: args.trace_scene,
        trace_sprites: args.trace_sprites,
        trace_assets: args.trace_assets,
        trace_render: args.trace_render,
        trace_buttons: args.trace_buttons,
        script_budget_per_frame: args.script_budget_per_frame,
        audio: AudioConfig {
            enabled: !args.no_audio,
        },
        headless: args.headless,
        diagnostic_frames: args.diagnostic_frames,
        diagnostic_frame_ms: args.diagnostic_frame_ms,
        diagnostic_png: args.diagnostic_png,
        diagnostic_clicks: args.diagnostic_click,
        ..SenaConfig::default()
    })
}

fn parse_diagnostic_click(raw: &str) -> Result<DiagnosticClick, String> {
    let mut parts = raw.split(':');
    let frame = parts
        .next()
        .ok_or_else(|| "missing frame".to_owned())?
        .parse::<usize>()
        .map_err(|err| format!("invalid frame: {err}"))?;
    let x = parts
        .next()
        .ok_or_else(|| "missing x".to_owned())?
        .parse::<i32>()
        .map_err(|err| format!("invalid x: {err}"))?;
    let y = parts
        .next()
        .ok_or_else(|| "missing y".to_owned())?
        .parse::<i32>()
        .map_err(|err| format!("invalid y: {err}"))?;
    if parts.next().is_some() {
        return Err("expected frame:x:y".to_owned());
    }
    Ok(DiagnosticClick { frame, x, y })
}
