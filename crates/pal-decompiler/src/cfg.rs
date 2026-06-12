//! Control-flow graph construction from a flat instruction list.

use std::collections::{BTreeMap, BTreeSet};

use pal_script::{Argument, Instruction};

// ─── Block ────────────────────────────────────────────────────────────────────

/// A basic block in the CFG.
#[derive(Debug, Clone)]
pub struct Block {
    pub start_pc: u32,
    pub instrs: Vec<Instruction>,
    /// PC values of successor blocks (static targets only).
    pub succs: Vec<u32>,
}

impl Block {
    pub fn last_instr(&self) -> Option<&Instruction> {
        self.instrs.last()
    }

    /// PC of the first instruction that follows this block (used for fallthrough).
    pub fn end_pc(&self) -> Option<u32> {
        self.instrs.last().map(instr_end_pc)
    }
}

/// Compute the PC of the first byte after `instr` (i.e. next instruction's PC).
pub fn instr_end_pc(instr: &Instruction) -> u32 {
    match instr {
        Instruction::DataWord { pc, .. } => pc + 4,
        Instruction::Primary {
            pc, opcode, args, ..
        } => {
            // header word (4) + one word per decoded argument
            let n_words = 1 + decoded_arg_words(*opcode, args);
            pc + n_words as u32 * 4
        }
    }
}

fn decoded_arg_words(opcode: u16, args: &[Argument]) -> usize {
    match opcode {
        10 => 2,      // Point word + condition operand
        23 => 2,      // ExtCall raw + DestinationSlot
        9 | 11 => 1,  // Point word
        20 | 29 => 1, // RawSlot
        _ => args.len(),
    }
}

// ─── CFG building ─────────────────────────────────────────────────────────────

/// Build a BTreeMap<start_pc, Block> from a flat instruction list.
pub fn build_cfg(instructions: &[Instruction], entry_pc: u32) -> BTreeMap<u32, Block> {
    if instructions.is_empty() {
        return BTreeMap::new();
    }

    // Pass 1: collect all leader PCs.
    let mut leaders: BTreeSet<u32> = BTreeSet::new();
    leaders.insert(entry_pc);

    for instr in instructions {
        match instr {
            Instruction::Primary {
                opcode, args, pc, ..
            } => {
                match opcode {
                    9 | 10 | 11 => {
                        // jmp / jf / gosub — static target is a leader
                        if let Some(tgt) = extract_static_target_pc(args.first()) {
                            leaders.insert(tgt);
                        }
                        // For jf and gosub, the fallthrough is also a leader
                        if *opcode == 10 || *opcode == 11 {
                            let end = instr_end_pc(instr);
                            leaders.insert(end);
                        }
                    }
                    21 | 24 => {
                        // end / ret — next PC is a leader (unreachable but structural)
                        let end = instr_end_pc(instr);
                        leaders.insert(end);
                    }
                    _ => {}
                }
                let _ = pc;
            }
            Instruction::DataWord { .. } => {}
        }
    }

    // Pass 2: partition instructions into blocks.
    let mut blocks: BTreeMap<u32, Block> = BTreeMap::new();
    let mut current_start: Option<u32> = None;
    let mut current_instrs: Vec<Instruction> = Vec::new();

    for instr in instructions {
        let pc = instr_pc(instr);

        if leaders.contains(&pc) {
            // Flush the current block if any
            if let Some(start) = current_start.take() {
                let succs = compute_succs(&current_instrs);
                blocks.insert(
                    start,
                    Block {
                        start_pc: start,
                        instrs: current_instrs,
                        succs,
                    },
                );
                current_instrs = Vec::new();
            }
            current_start = Some(pc);
        }

        current_instrs.push(instr.clone());

        // If this is a terminating instruction, close the block now
        if is_terminator(instr) {
            if let Some(start) = current_start.take() {
                let succs = compute_succs(&current_instrs);
                blocks.insert(
                    start,
                    Block {
                        start_pc: start,
                        instrs: current_instrs,
                        succs,
                    },
                );
                current_instrs = Vec::new();
            }
        }
    }

    // Flush any remaining instructions
    if let Some(start) = current_start {
        let succs = compute_succs(&current_instrs);
        blocks.insert(
            start,
            Block {
                start_pc: start,
                instrs: current_instrs,
                succs,
            },
        );
    }

    blocks
}

pub fn instr_pc(instr: &Instruction) -> u32 {
    match instr {
        Instruction::DataWord { pc, .. } => *pc,
        Instruction::Primary { pc, .. } => *pc,
    }
}

fn is_terminator(instr: &Instruction) -> bool {
    match instr {
        Instruction::Primary { opcode, .. } => matches!(opcode, 9 | 10 | 11 | 21 | 24),
        _ => false,
    }
}

fn compute_succs(instrs: &[Instruction]) -> Vec<u32> {
    let mut succs = Vec::new();
    if let Some(last) = instrs.last() {
        match last {
            Instruction::Primary { opcode, args, .. } => {
                match opcode {
                    9 => {
                        // jmp: only static target (handles both Point and PointOperand)
                        if let Some(tgt) = extract_static_target_pc(args.first()) {
                            succs.push(tgt);
                        }
                    }
                    10 => {
                        // jf: fallthrough (true) and branch target (false)
                        let fall = instr_end_pc(last);
                        succs.push(fall);
                        if let Some(Argument::Point {
                            target_pc: Some(tgt),
                            ..
                        }) = args.first()
                        {
                            if *tgt != fall {
                                succs.push(*tgt);
                            }
                        }
                    }
                    11 => {
                        // gosub: fallthrough only (the callee is a separate function)
                        let fall = instr_end_pc(last);
                        succs.push(fall);
                    }
                    21 | 24 => {
                        // end / ret: no successors
                    }
                    _ => {
                        // Fallthrough
                        succs.push(instr_end_pc(last));
                    }
                }
            }
            _ => {
                succs.push(instr_end_pc(last));
            }
        }
    }
    succs
}

/// Collect all gosub targets from the instruction list.
pub fn collect_gosub_targets(instructions: &[Instruction]) -> BTreeSet<u32> {
    let mut targets = BTreeSet::new();
    for instr in instructions {
        if let Instruction::Primary {
            opcode: 11, args, ..
        } = instr
        {
            if let Some(tgt) = extract_static_target_pc(args.first()) {
                targets.insert(tgt);
            }
        }
    }
    targets
}

/// Extract a statically-known target PC from a jump argument.
/// Handles both `Argument::Point` (opcodes 10, 11) and
/// `Argument::PointOperand` (opcode 9).
pub fn extract_static_target_pc(arg: Option<&Argument>) -> Option<u32> {
    match arg {
        Some(Argument::Point {
            target_pc: Some(tgt),
            ..
        }) => Some(*tgt),
        Some(Argument::PointOperand {
            target_pc: Some(tgt),
            ..
        }) => Some(*tgt),
        _ => None,
    }
}
