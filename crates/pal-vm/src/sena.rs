use std::path::PathBuf;

use clap::Parser;
use pal_asset::Nls;
use pal_vm::{run_sena, AudioConfig, FrameScene, SenaConfig};

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

    #[arg(long, default_value_t = 4096)]
    script_budget_per_frame: usize,

    #[arg(long)]
    no_audio: bool,
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
        script_budget_per_frame: args.script_budget_per_frame,
        audio: AudioConfig {
            enabled: !args.no_audio,
        },
        ..SenaConfig::default()
    })
}
