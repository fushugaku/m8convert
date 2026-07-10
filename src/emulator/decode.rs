use super::memory::ExecutionMemory;
use super::*;

#[path = "decode16.rs"]
mod decode16;
#[path = "decode32.rs"]
mod decode32;
#[path = "isa_helpers.rs"]
mod isa_helpers;
#[path = "vfp.rs"]
mod vfp;

pub(super) use decode16::decode_thumb;

#[cfg(test)]
pub(super) use decode32::decode_thumb32;

#[cfg(test)]
pub(super) use isa_helpers::{get_fpu_d, set_fpu_d, thumb_expand_imm};

#[cfg(test)]
pub(super) use vfp::vfp_expand_imm_f32_bits;

use isa_helpers::{condition_met, condition_name, is_thumb32_prefix, take_it_condition};

pub(super) fn step_thumb_trace(memory: &mut ExecutionMemory, cpu: &mut CpuState) -> TraceEvent {
    let pc = cpu.registers[15];
    memory.set_current_instruction(pc, cpu.cycles);
    if !memory.is_executable_address(pc) {
        let lr_target = cpu.registers[14] & !1;
        if memory.has_executable_halfword(lr_target) {
            cpu.registers[15] = lr_target;
            return TraceEvent {
                cycle: cpu.cycles,
                pc,
                opcode16: memory.read_u16(pc),
                opcode32: None,
                note: format!(
                    "PC 0x{pc:08x} points to non-executable RAM/data, stubbed external callback, returning to LR=0x{:08x}",
                    cpu.registers[14]
                ),
            };
        }
    }
    let Some(opcode16) = memory.read_u16(pc) else {
        return TraceEvent {
            cycle: cpu.cycles,
            pc,
            opcode16: None,
            opcode32: None,
            note: "PC points outside loaded firmware memory".to_string(),
        };
    };
    if let Some(condition) = take_it_condition(cpu) {
        let is_32 = is_thumb32_prefix(opcode16);
        let opcode32 = if is_32 {
            memory
                .read_u16(pc + 2)
                .map(|second| ((opcode16 as u32) << 16) | second as u32)
        } else {
            None
        };
        if !condition_met(condition, cpu.apsr) {
            cpu.registers[15] = pc + if is_32 { 4 } else { 2 };
            return TraceEvent {
                cycle: cpu.cycles,
                pc,
                opcode16: Some(opcode16),
                opcode32,
                note: format!(
                    "IT skipped {} instruction because {} is false",
                    if is_32 { "Thumb32" } else { "Thumb16" },
                    condition_name(condition)
                ),
            };
        }
        cpu.it_suppresses_flags = true;
    }

    let mut decoded = decode_thumb(memory, pc, opcode16, cpu);
    cpu.it_suppresses_flags = false;
    if let Some(unmapped) = memory.take_pending_unmapped_read() {
        decoded.note.push_str(&format!(
            " ; unmapped data read 0x{:08x} size {} at pc 0x{:08x} returned default 0",
            unmapped.address, unmapped.size, unmapped.pc
        ));
    }
    cpu.registers[15] = decoded.next_pc;
    TraceEvent {
        cycle: cpu.cycles,
        pc,
        opcode16: Some(opcode16),
        opcode32: decoded.opcode32,
        note: decoded.note,
    }
}

pub(super) struct ThumbDecode {
    pub(super) next_pc: u32,
    pub(super) opcode32: Option<u32>,
    pub(super) note: String,
}
