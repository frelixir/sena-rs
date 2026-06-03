use std::fmt;

use crate::error::{PalError, Result};
use crate::opcodes::{ext_opcode, primary_opcode, PrimaryOpcode};
use crate::operand::Operand;
use crate::point::PointTable;
use crate::script::ScriptImage;

#[derive(Debug, Clone, Copy)]
pub struct DisassembleOptions<'a> {
    pub start: usize,
    pub end: Option<usize>,
    pub point_table: Option<&'a PointTable>,
}

impl<'a> DisassembleOptions<'a> {
    pub fn new(start: usize) -> Self {
        Self {
            start,
            end: None,
            point_table: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Argument {
    Operand(Operand),
    Point {
        id: u32,
        target_pc: Option<u32>,
    },
    RawSlot {
        slot: u16,
        raw: u32,
    },
    ExtCall {
        raw: u32,
        category: u16,
        index: u16,
        name: Option<&'static str>,
    },
    DestinationSlot {
        slot: u32,
        raw: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    DataWord {
        pc: u32,
        word: u32,
    },
    Primary {
        pc: u32,
        word: u32,
        opcode: u16,
        meta: Option<PrimaryOpcode>,
        args: Vec<Argument>,
    },
}

pub fn disassemble_script(
    script: &ScriptImage<'_>,
    options: DisassembleOptions<'_>,
) -> Result<Vec<Instruction>> {
    let len = script.len();
    let end = options.end.unwrap_or(len);
    if options.start > end || end > len {
        return Err(PalError::InvalidRange {
            start: options.start,
            end,
            len,
        });
    }

    let mut pc = options.start;
    let mut instructions = Vec::new();

    while pc.checked_add(4).ok_or(PalError::ArithmeticOverflow)? <= end {
        let insn_pc = pc;
        let word = read_instruction_u32(script, &mut pc, end)?;

        let hi = ((word >> 16) & 0xFFFF) as u16;
        let opcode = (word & 0xFFFF) as u16;

        if hi != 1 {
            instructions.push(Instruction::DataWord {
                pc: insn_pc as u32,
                word,
            });
            continue;
        }

        let meta = primary_opcode(opcode);
        let mut args = Vec::new();

        match opcode {
            10 => {
                let label = read_instruction_u32(script, &mut pc, end)?;
                let cond = read_instruction_u32(script, &mut pc, end)?;
                let target_pc = resolve_point(options.point_table, label)?;
                args.push(Argument::Point {
                    id: label,
                    target_pc,
                });
                args.push(Argument::Operand(Operand::decode(cond)));
            }
            20 | 29 => {
                let raw = read_instruction_u32(script, &mut pc, end)?;
                args.push(Argument::RawSlot {
                    slot: (raw & 0xFFFF) as u16,
                    raw,
                });
            }
            23 => {
                let raw = read_instruction_u32(script, &mut pc, end)?;
                let dst = read_instruction_u32(script, &mut pc, end)?;
                let category = ((raw >> 16) & 0xFFFF) as u16;
                let index = (raw & 0xFFFF) as u16;
                let ext = ext_opcode(category, index);
                args.push(Argument::ExtCall {
                    raw,
                    category,
                    index,
                    name: ext.and_then(|meta| meta.name),
                });
                args.push(Argument::DestinationSlot {
                    slot: dst,
                    raw: dst,
                });
            }
            9 | 11 => {
                let raw = read_instruction_u32(script, &mut pc, end)?;
                let id = raw;
                let target_pc = resolve_point(options.point_table, id)?;
                args.push(Argument::Point { id, target_pc });
            }
            _ => {
                let argc = meta.map(|item| item.argc).unwrap_or(0);
                for _ in 0..argc {
                    let raw = read_instruction_u32(script, &mut pc, end)?;
                    args.push(Argument::Operand(Operand::decode(raw)));
                }
            }
        }

        instructions.push(Instruction::Primary {
            pc: insn_pc as u32,
            word,
            opcode,
            meta,
            args,
        });
    }

    Ok(instructions)
}

fn read_instruction_u32(script: &ScriptImage<'_>, pc: &mut usize, limit: usize) -> Result<u32> {
    let end = pc.checked_add(4).ok_or(PalError::ArithmeticOverflow)?;
    if end > limit {
        return Err(PalError::InvalidRange {
            start: *pc,
            end: limit,
            len: script.len(),
        });
    }
    let value = crate::util::read_u32_le(script.bytes(), *pc)?;
    *pc = end;
    Ok(value)
}

fn resolve_point(point_table: Option<&PointTable>, id: u32) -> Result<Option<u32>> {
    match point_table {
        Some(table) => table.resolve_target_pc(id),
        None => Ok(None),
    }
}

impl fmt::Display for Argument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operand(operand) => write!(f, "{operand}"),
            Self::Point { id, target_pc } => match target_pc {
                Some(target) => write!(f, "point[{id}] -> 0x{target:08X}"),
                None => write!(f, "point[{id}]"),
            },
            Self::RawSlot { slot, raw } => write!(f, "slot[{slot}] raw=0x{raw:08X}"),
            Self::ExtCall {
                category,
                index,
                name,
                ..
            } => match name {
                Some(name) => write!(f, "ext_{category:04X}_{index:04X}.{name}"),
                None => write!(f, "ext_{category:04X}_{index:04X}"),
            },
            Self::DestinationSlot { slot, raw } => write!(f, "dst_slot[{slot}] raw=0x{raw:08X}"),
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DataWord { pc, word } => write!(f, "{pc:08X}: .word 0x{word:08X}"),
            Self::Primary {
                pc,
                word,
                opcode,
                meta,
                args,
            } => {
                let mut generated_name = String::new();
                let name = match meta {
                    Some(meta) => meta.name,
                    None => {
                        generated_name = format!("op_{opcode:04X}");
                        generated_name.as_str()
                    }
                };

                if args.is_empty() {
                    write!(f, "{pc:08X}: {name} ; word=0x{word:08X}")
                } else {
                    write!(f, "{pc:08X}: {name} ")?;
                    for (idx, arg) in args.iter().enumerate() {
                        if idx != 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{arg}")?;
                    }
                    write!(f, " ; word=0x{word:08X}")
                }
            }
        }
    }
}
