use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use pal_script::{
    disassemble_script, format_script_header, parse_usize_number, DisassembleOptions, PointTable,
    ScriptImage,
};

#[derive(Debug, Parser)]
#[command(name = "pal-disassembled")]
#[command(about = "Disassemble PAL/Game.exe Sv20 Script.src bytecode")]
struct Cli {
    #[arg(value_name = "SCRIPT_SRC")]
    script: PathBuf,

    #[arg(long, value_name = "entry|code|OFFSET")]
    start: Option<String>,

    #[arg(long, value_name = "OFFSET")]
    end: Option<String>,

    #[arg(long = "point-dat", value_name = "POINT_DAT")]
    point_dat: Option<PathBuf>,

    #[arg(long)]
    no_header: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let script_bytes = fs::read(&cli.script)
        .with_context(|| format!("failed to read script file {}", cli.script.display()))?;
    let script = ScriptImage::parse(&script_bytes).context("failed to parse Script.src header")?;

    let start = match cli.start.as_deref() {
        Some(value) => parse_start(value, &script).context("failed to parse --start")?,
        None => script.entry_pc() as usize,
    };
    let end = cli
        .end
        .as_deref()
        .map(parse_usize_number)
        .transpose()
        .context("failed to parse --end")?;

    let point_table_storage = match &cli.point_dat {
        Some(path) => {
            let bytes = fs::read(path)
                .with_context(|| format!("failed to read Point.dat file {}", path.display()))?;
            Some(PointTable::parse(&bytes).context("failed to parse Point.dat")?)
        }
        None => None,
    };

    let options = DisassembleOptions {
        start,
        end,
        point_table: point_table_storage.as_ref(),
    };

    if !cli.no_header {
        println!("{}", format_script_header(&script));
    }

    for instruction in
        disassemble_script(&script, options).context("failed to disassemble script")?
    {
        println!("{instruction}");
    }

    Ok(())
}

fn parse_start(value: &str, script: &ScriptImage<'_>) -> pal_script::Result<usize> {
    match value {
        "entry" => Ok(script.entry_pc() as usize),
        "code" => Ok(script.code_base() as usize),
        _ => parse_usize_number(value),
    }
}
