//! Top-level decompilation driver.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use pal_asset::Nls;
use pal_script::{
    disassemble_script, Argument, DisassembleOptions, Instruction, PointTable, ScriptImage,
    SCRIPT_CODE_BASE,
};

use crate::assets::ScriptAssets;
use crate::cfg::{build_cfg, Block};
use crate::codegen::emit_lua;

// ─── DecompileContext ─────────────────────────────────────────────────────────

/// All context needed by the code generator.
pub struct DecompileContext<'a> {
    pub blocks: BTreeMap<u32, Block>,
    pub gosub_targets: BTreeSet<u32>,
    /// Map from PC → point_id (for naming).
    pub pc_to_point: BTreeMap<u32, u32>,
    pub file_entries: &'a [String],
    pub text_entries: &'a [(u32, String)],
    /// Raw (decrypted) FILE.DAT bytes for signature-driven resource resolution.
    pub file_bytes: &'a [u8],
    /// Raw (decrypted) TEXT.DAT bytes for signature-driven text resolution.
    pub text_bytes: &'a [u8],
    pub nls: Option<Nls>,
}

impl<'a> DecompileContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        blocks: BTreeMap<u32, Block>,
        point_table: &PointTable,
        file_entries: &'a [String],
        text_entries: &'a [(u32, String)],
        file_bytes: &'a [u8],
        text_bytes: &'a [u8],
        nls: Option<Nls>,
    ) -> Self {
        // Build pc → point_id reverse map
        let entries = point_table.len();
        let mut pc_to_point = BTreeMap::new();
        for id in 1..=entries as u32 {
            if let Ok(Some(pc)) = point_table.resolve_target_pc(id) {
                pc_to_point.insert(pc, id);
            }
        }

        // Collect gosub targets from blocks
        let mut gosub_targets = BTreeSet::new();
        for block in blocks.values() {
            for instr in &block.instrs {
                if let pal_script::Instruction::Primary {
                    opcode: 11, args, ..
                } = instr
                {
                    if let Some(pal_script::Argument::Point {
                        target_pc: Some(tgt),
                        ..
                    }) = args.first()
                    {
                        gosub_targets.insert(*tgt);
                    }
                }
            }
        }

        Self {
            blocks,
            gosub_targets,
            pc_to_point,
            file_entries,
            text_entries,
            file_bytes,
            text_bytes,
            nls,
        }
    }

    /// Convenience constructor with empty raw bytes (for tests that don't need resolution).
    pub fn new_simple(
        blocks: BTreeMap<u32, Block>,
        point_table: &PointTable,
        file_entries: &'a [String],
        text_entries: &'a [(u32, String)],
        nls: Option<Nls>,
    ) -> Self {
        Self::new(
            blocks,
            point_table,
            file_entries,
            text_entries,
            &[],
            &[],
            nls,
        )
    }

    /// Generate the Lua function name for a PC.
    pub fn proc_name(&self, pc: u32) -> String {
        match self.pc_to_point.get(&pc) {
            Some(&id) => format!("proc_p{id:04X}_{pc:08X}"),
            None => format!("proc_{pc:08X}"),
        }
    }

    /// Generate a Lua jump label for a PC.
    pub fn label_name(&self, pc: u32) -> String {
        match self.pc_to_point.get(&pc) {
            Some(&id) => format!("L_p{id:04X}_{pc:08X}"),
            None => format!("L_{pc:08X}"),
        }
    }
}

// ─── Entry point ─────────────────────────────────────────────────────────────

/// Decompile the given script assets and return the Lua source string.
pub fn decompile(assets: &ScriptAssets, nls: Nls) -> Result<String> {
    let script = ScriptImage::parse(&assets.script_bytes).context("parse SCRIPT.SRC header")?;

    let point_table = PointTable::parse(&assets.point_bytes).context("parse POINT.DAT")?;

    // Disassemble without point table to avoid PointIdOutOfRange errors on
    // dynamic jump operands (e.g., 0x4000_0001 is a VariableSlot operand,
    // not a point_id).  We then post-process to resolve point targets statically.
    let options = DisassembleOptions {
        start: SCRIPT_CODE_BASE as usize,
        end: None,
        point_table: None,
    };

    let mut instructions =
        disassemble_script(&script, options).context("disassemble SCRIPT.SRC")?;

    // Post-process: resolve static point targets (ignore out-of-range/zero ids).
    resolve_static_points(&mut instructions, &point_table);

    let blocks = build_cfg(&instructions, script.entry_pc());

    let ctx = DecompileContext::new(
        blocks,
        &point_table,
        &assets.file_entries,
        &assets.text_entries,
        &assets.file_bytes,
        &assets.text_bytes,
        Some(nls),
    );

    let lua = emit_lua(&ctx, script.entry_pc());
    Ok(lua)
}

/// Walk all instructions and fill in `target_pc` for Point arguments where possible.
///
/// - `Point { id=0 }`: dynamic target (id=0 convention) — left as None, will output
///   `goto_point(0) -- dynamic` in codegen.
/// - `Point { id=N }` where N is out of range: invalid — silently left as None.
///   A comment-annotated fallback is emitted by codegen.
/// - `PointOperand { operand=VariableSlot, point_id=None }`: genuinely dynamic jump
///   via runtime variable — left as-is, codegen outputs `goto_point(v_X) -- dynamic`.
/// - `PointOperand { operand=Immediate(N), point_id=Some(N) }` where N is out of range:
///   static but unresolvable — silently left as None, codegen outputs a comment.
pub fn resolve_static_points(instructions: &mut [Instruction], point_table: &PointTable) {
    for instr in instructions.iter_mut() {
        if let Instruction::Primary { args, .. } = instr {
            for arg in args.iter_mut() {
                match arg {
                    // opcodes 10, 11 — direct point reference
                    Argument::Point { id, target_pc } => {
                        if target_pc.is_none() {
                            if let Ok(Some(pc)) = point_table.resolve_target_pc(*id) {
                                *target_pc = Some(pc);
                            }
                            // id=0 → dynamic; out-of-range → silently skip (see doc)
                        }
                    }
                    // opcode 9 (jmp) — encodes target as a PointOperand with a point_id
                    Argument::PointOperand {
                        point_id: Some(pid),
                        target_pc,
                        ..
                    } => {
                        if target_pc.is_none() {
                            if let Ok(Some(pc)) = point_table.resolve_target_pc(*pid) {
                                *target_pc = Some(pc);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
