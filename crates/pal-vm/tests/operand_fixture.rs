use std::path::PathBuf;

use pal_asset::{AssetSource, LoadedAsset};
use pal_script::PointTable;
use pal_vm::{CoreAssets, RuntimeStatus, ScriptRuntime, ScriptRuntimeConfig};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn asset(name: &str, bytes: Vec<u8>) -> LoadedAsset {
    LoadedAsset {
        name: name.to_owned(),
        bytes,
        source: AssetSource::Loose {
            path: PathBuf::from(name),
        },
    }
}

/// Build an instruction word: hi=1, lo=opcode.
fn opcode(op: u16) -> [u8; 4] {
    (0x0001_0000u32 | op as u32).to_le_bytes()
}

fn word(value: u32) -> [u8; 4] {
    value.to_le_bytes()
}

/// Operand: immediate value (kind 0x0).
fn imm(value: i32) -> u32 {
    value as u32
}

/// Operand: variable slot (kind 0x4).
fn var(slot: u16) -> u32 {
    0x4000_0000u32 | slot as u32
}

fn ext_raw(category: u16, index: u16) -> u32 {
    ((category as u32) << 16) | index as u32
}

/// Operand: ArgumentBase (kind 0x9, raw = 0x90000000).
fn arg_base() -> u32 {
    0x9000_0000u32
}

/// Operand: ArgumentStack lo index (kind 0x8).
fn arg_stack(lo: u16) -> u32 {
    0x8000_0000u32 | lo as u32
}

/// Build a minimal CoreAssets from a script byte buffer.
/// The script header "Sv20" is prepended at offset 0; the real code starts at offset 8
/// (after the 4-byte magic + 4-byte entry-pc word).
fn assets_with_script(entry_pc: u32, script_body: Vec<u8>) -> CoreAssets {
    let mut script = Vec::new();
    // PAL script magic
    script.extend_from_slice(b"Sv20");
    // Placeholder for check value (offset 4..8 is NOT the entry pc; check ScriptImage::parse)
    // Actually the script image format: first 4 bytes = "Sv20", next 4 = check value,
    // next 4 = entry_pc offset. Let's look at how runtime_fixture does it:
    // In runtime_fixture: entry_pc = 12, script starts: "Sv20" + word(0) + word(12)
    // So offset 4 = check value = 0, offset 8 = entry_pc = 12.
    script.extend_from_slice(&0u32.to_le_bytes()); // check value
    script.extend_from_slice(&entry_pc.to_le_bytes()); // entry_pc
    script.extend_from_slice(&script_body);

    CoreAssets {
        script: asset("Script.src", script),
        file_dat: asset("File.dat", Vec::new()),
        text_dat: asset("Text.dat", Vec::new()),
        mem_dat: asset("Mem.dat", Vec::new()),
        point_dat: asset("Point.dat", Vec::new()),
        graphic_dat: None,
        script_check_value: 0,
        script_entry_pc: entry_pc,
        point_table: PointTable::parse(&[]).expect("empty Point.dat should parse"),
        graphic_index: None,
    }
}

/// Run to completion (Halted or UnsupportedCommand or Faulted).
fn run_to_stop(runtime: &mut ScriptRuntime, assets: &CoreAssets, config: &ScriptRuntimeConfig) {
    for _ in 0..1000 {
        let tick = runtime
            .run_frame(assets, config)
            .expect("run_frame should not propagate error");
        match &tick.status {
            RuntimeStatus::Running { .. } => continue,
            RuntimeStatus::WaitFrame { .. } | RuntimeStatus::WaitClick { .. } => {
                // Resolve waits immediately for test purposes.
                runtime.resolve_pending_wait();
                continue;
            }
            _ => break,
        }
    }
}

// ---------------------------------------------------------------------------
// ArgumentBase tests
// ---------------------------------------------------------------------------

/// ArgumentBase (kind 0x9) reads as 0 on a freshly booted runtime.
#[test]
fn argument_base_reads_zero_initially() {
    // Script: mov var[0] = arg_base; halt
    let entry_pc = 12u32;
    let mut body = Vec::new();
    body.extend_from_slice(&opcode(1)); // mov
    body.extend_from_slice(&word(var(0))); // dst = var[0]
    body.extend_from_slice(&word(arg_base())); // src = arg_base
    body.extend_from_slice(&opcode(21)); // halt

    let assets = assets_with_script(entry_pc, body);
    let config = ScriptRuntimeConfig::default();
    let mut runtime = ScriptRuntime::boot(entry_pc, config.clone());
    run_to_stop(&mut runtime, &assets, &config);

    assert_eq!(runtime.vars()[0], 0, "argument_base should be 0 on boot");
}

/// Write to ArgumentBase then read it back via a variable.
#[test]
fn argument_base_write_read_roundtrip() {
    // Script: mov arg_base = imm(42); mov var[0] = arg_base; halt
    let entry_pc = 12u32;
    let mut body = Vec::new();
    // mov arg_base = 42
    body.extend_from_slice(&opcode(1));
    body.extend_from_slice(&word(arg_base()));
    body.extend_from_slice(&word(imm(42)));
    // mov var[0] = arg_base
    body.extend_from_slice(&opcode(1));
    body.extend_from_slice(&word(var(0)));
    body.extend_from_slice(&word(arg_base()));
    // halt
    body.extend_from_slice(&opcode(21));

    let assets = assets_with_script(entry_pc, body);
    let config = ScriptRuntimeConfig::default();
    let mut runtime = ScriptRuntime::boot(entry_pc, config.clone());
    run_to_stop(&mut runtime, &assets, &config);

    assert_eq!(runtime.vars()[0], 42, "argument_base roundtrip failed");
}

// ---------------------------------------------------------------------------
// ArgumentStack / pack_args tests
// ---------------------------------------------------------------------------

/// Push three values, pack them, then read them via arg_stack operand.
/// After pack_args with reversal: lo=1 → last pushed (top), lo=3 → first pushed (bottom).
#[test]
fn pack_args_and_arg_stack_access() {
    // Push 10, 20, 30 onto stack; pack 3 args; read arg[1] → var[0], arg[2] → var[1], arg[3] → var[2]; halt
    let entry_pc = 12u32;
    let mut body = Vec::new();

    // push 10
    body.extend_from_slice(&opcode(31));
    body.extend_from_slice(&word(imm(10)));
    // push 20
    body.extend_from_slice(&opcode(31));
    body.extend_from_slice(&word(imm(20)));
    // push 30
    body.extend_from_slice(&opcode(31));
    body.extend_from_slice(&word(imm(30)));
    // pack_args 3
    body.extend_from_slice(&opcode(32));
    body.extend_from_slice(&word(imm(3)));

    // mov var[0] = arg_stack[1]  (top of stack push = 30)
    body.extend_from_slice(&opcode(1));
    body.extend_from_slice(&word(var(0)));
    body.extend_from_slice(&word(arg_stack(1)));
    // mov var[1] = arg_stack[2]  (middle = 20)
    body.extend_from_slice(&opcode(1));
    body.extend_from_slice(&word(var(1)));
    body.extend_from_slice(&word(arg_stack(2)));
    // mov var[2] = arg_stack[3]  (bottom = 10)
    body.extend_from_slice(&opcode(1));
    body.extend_from_slice(&word(var(2)));
    body.extend_from_slice(&word(arg_stack(3)));
    // halt
    body.extend_from_slice(&opcode(21));

    let assets = assets_with_script(entry_pc, body);
    let config = ScriptRuntimeConfig::default();
    let mut runtime = ScriptRuntime::boot(entry_pc, config.clone());
    run_to_stop(&mut runtime, &assets, &config);

    assert_eq!(
        runtime.vars()[0],
        30,
        "arg_stack[1] should be last-pushed value (30)"
    );
    assert_eq!(
        runtime.vars()[1],
        20,
        "arg_stack[2] should be middle value (20)"
    );
    assert_eq!(
        runtime.vars()[2],
        10,
        "arg_stack[3] should be first-pushed value (10)"
    );
}

/// pack_args then drop_args: argument stack depth returns to zero.
#[test]
fn pack_args_drop_args_cycle() {
    // Push 5 values; pack 5; drop 5; halt
    let entry_pc = 12u32;
    let mut body = Vec::new();
    for i in 0..5i32 {
        body.extend_from_slice(&opcode(31));
        body.extend_from_slice(&word(imm(i)));
    }
    body.extend_from_slice(&opcode(32));
    body.extend_from_slice(&word(imm(5)));
    body.extend_from_slice(&opcode(33));
    body.extend_from_slice(&word(imm(5)));
    body.extend_from_slice(&opcode(21));

    let assets = assets_with_script(entry_pc, body);
    let config = ScriptRuntimeConfig::default();
    let mut runtime = ScriptRuntime::boot(entry_pc, config.clone());
    run_to_stop(&mut runtime, &assets, &config);

    assert_eq!(
        runtime.argument_stack_depth(),
        0,
        "argument_stack should be empty after drop_args"
    );
    assert!(
        matches!(runtime.status(), RuntimeStatus::Halted { .. }),
        "runtime should be halted"
    );
}

// ---------------------------------------------------------------------------
// UnsupportedCommand sets faulted-like status without crashing
// ---------------------------------------------------------------------------

/// An unknown opcode (e.g. opcode 99) sets UnsupportedCommand status; the runtime stays alive.
#[test]
fn unsupported_command_sets_status_without_crash() {
    let entry_pc = 12u32;
    let mut body = Vec::new();
    // opcode 99 is not implemented
    body.extend_from_slice(&opcode(99));

    let assets = assets_with_script(entry_pc, body);
    let config = ScriptRuntimeConfig::default();
    let mut runtime = ScriptRuntime::boot(entry_pc, config.clone());
    run_to_stop(&mut runtime, &assets, &config);

    assert!(
        matches!(
            runtime.status(),
            RuntimeStatus::UnsupportedCommand { opcode: 99, .. }
        ),
        "expected UnsupportedCommand(99), got {:?}",
        runtime.status()
    );
}

// ---------------------------------------------------------------------------
// WaitClick creates task and runtime stays in WaitClick
// ---------------------------------------------------------------------------

/// Opcode 253 (WaitClick) emits a WaitRequest and leaves the runtime in WaitClick status.
/// The window must NOT close on this — it's a normal blocking state.
#[test]
fn wait_click_emits_request_and_status_is_wait_click() {
    let entry_pc = 12u32;
    let mut body = Vec::new();
    body.extend_from_slice(&opcode(253)); // WaitClick

    let assets = assets_with_script(entry_pc, body);
    let config = ScriptRuntimeConfig::default();
    let mut runtime = ScriptRuntime::boot(entry_pc, config.clone());

    let tick = runtime
        .run_frame(&assets, &config)
        .expect("run_frame should succeed on WaitClick");

    assert!(
        tick.wait_request
            .map_or(false, |r| r == pal_vm::WaitRequest::Click),
        "expected WaitRequest::Click"
    );
    assert!(
        matches!(runtime.status(), RuntimeStatus::WaitClick { .. }),
        "expected WaitClick status, got {:?}",
        runtime.status()
    );
}

#[test]
fn wait_sync_release_emits_timed_wait() {
    let entry_pc = 12u32;
    let mut body = Vec::new();
    body.extend_from_slice(&opcode(31));
    body.extend_from_slice(&word(imm(250)));
    body.extend_from_slice(&opcode(23));
    body.extend_from_slice(&word(ext_raw(7, 3)));
    body.extend_from_slice(&word(var(0)));

    let assets = assets_with_script(entry_pc, body);
    let config = ScriptRuntimeConfig::default();
    let mut runtime = ScriptRuntime::boot(entry_pc, config.clone());
    runtime.set_pal_time(1000);

    let tick = runtime
        .run_frame(&assets, &config)
        .expect("run_frame should succeed on wait_sync_release");

    assert_eq!(runtime.vars()[0], 1, "wait_sync_release should return 1");
    assert_eq!(tick.wait_request, Some(pal_vm::WaitRequest::Time(250)));
    assert!(
        matches!(runtime.status(), RuntimeStatus::WaitFrame { .. }),
        "expected WaitFrame status, got {:?}",
        runtime.status()
    );
}

#[test]
fn wait_sync_begin_and_get_time_are_stateful() {
    let entry_pc = 12u32;
    let mut body = Vec::new();
    body.extend_from_slice(&opcode(23));
    body.extend_from_slice(&word(ext_raw(7, 2)));
    body.extend_from_slice(&word(var(0)));
    body.extend_from_slice(&opcode(23));
    body.extend_from_slice(&word(ext_raw(7, 8)));
    body.extend_from_slice(&word(var(1)));
    body.extend_from_slice(&opcode(21));

    let assets = assets_with_script(entry_pc, body);
    let config = ScriptRuntimeConfig::default();
    let mut runtime = ScriptRuntime::boot(entry_pc, config.clone());
    runtime.set_pal_time(1234);
    run_to_stop(&mut runtime, &assets, &config);

    assert_eq!(runtime.vars()[0], 1, "wait_sync_begin should return 1");
    assert_eq!(
        runtime.vars()[1],
        0,
        "get_time in the same VM tick should see zero elapsed time"
    );
    assert!(
        matches!(runtime.status(), RuntimeStatus::Halted { .. }),
        "runtime should be halted"
    );
}
