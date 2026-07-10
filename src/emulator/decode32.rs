use super::super::memory::ExecutionMemory;
use super::super::{ARM_XPSR_THUMB, CpuState};
use super::ThumbDecode;
use super::isa_helpers::*;
use super::vfp::*;

pub(crate) fn decode_thumb32(
    memory: &mut ExecutionMemory,
    pc: u32,
    first: u16,
    second: u16,
    cpu: &mut CpuState,
) -> ThumbDecode {
    let opcode32 = ((first as u32) << 16) | second as u32;
    if first == 0xe8bd {
        let register_count = second.count_ones();
        let mut address = cpu.registers[13];
        let mut next_pc = pc + 4;
        let mut pc_note = String::new();
        for register in 0..16 {
            if second & (1 << register) != 0 {
                let value = memory.read_u32_or_zero(address);
                if register == 15 {
                    next_pc = value & !1;
                    pc_note = format!(" ; PC<-[0x{address:08x}]=0x{value:08x}");
                } else {
                    cpu.registers[register] = value;
                }
                address = address.wrapping_add(4);
            }
        }
        cpu.registers[13] = cpu.registers[13].wrapping_add(register_count * 4);
        return ThumbDecode {
            next_pc,
            opcode32: Some(opcode32),
            note: format!(
                "POP.W register-list 0x{second:04x}{pc_note} ; SP=0x{:08x}",
                cpu.registers[13]
            ),
        };
    }

    if first == 0xe92d {
        let register_count = second.count_ones();
        let new_sp = cpu.registers[13].wrapping_sub(register_count * 4);
        let mut address = new_sp;
        for register in 0..16 {
            if second & (1 << register) != 0 {
                memory.write_u32(address, cpu.registers[register]);
                address = address.wrapping_add(4);
            }
        }
        cpu.registers[13] = new_sp;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "PUSH.W register-list 0x{second:04x} ; SP=0x{:08x}",
                cpu.registers[13]
            ),
        };
    }

    if first & 0xfff0 == 0xe8d0 && second & 0xffe0 == 0xf000 {
        let rn = (first & 0x0f) as usize;
        let rm = (second & 0x000f) as usize;
        let halfword = second & 0x0010 != 0;
        let pc_base = pc + 4;
        let base = if rn == 15 { pc_base } else { cpu.registers[rn] };
        let index = cpu.registers[rm];
        let table_address = if halfword {
            base.wrapping_add(index.wrapping_mul(2))
        } else {
            base.wrapping_add(index)
        };
        let table_value = if halfword {
            memory.read_u16_or_zero(table_address) as u32
        } else {
            memory.read_u8_or_zero(table_address) as u32
        };
        let target = pc_base.wrapping_add(table_value.wrapping_mul(2));
        return ThumbDecode {
            next_pc: target,
            opcode32: Some(opcode32),
            note: format!(
                "{} [{}, {}{}] ; [0x{table_address:08x}] = 0x{table_value:x}, target=0x{target:08x}",
                if halfword { "TBH" } else { "TBB" },
                register_name(rn),
                register_name(rm),
                if halfword { ", LSL #1" } else { "" },
            ),
        };
    }

    if first & 0xfff0 == 0xe850 && second & 0x0f00 == 0x0f00 {
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let imm8 = (second & 0x00ff) as u32;
        let offset = imm8 * 4;
        let address = cpu.registers[rn].wrapping_add(offset);
        let value = memory.read_u32_or_zero(address);
        cpu.registers[rt] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "LDREX {}, [{}, #0x{offset:x}] ; [0x{address:08x}] = 0x{value:08x}",
                register_name(rt),
                register_name(rn)
            ),
        };
    }

    if first & 0xfff0 == 0xe840 {
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm8 = (second & 0x00ff) as u32;
        let offset = imm8 * 4;
        let address = cpu.registers[rn].wrapping_add(offset);
        let value = cpu.registers[rt];
        memory.write_u32(address, value);
        cpu.registers[rd] = 0;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "STREX {}, {}, [{}, #0x{offset:x}] ; [0x{address:08x}] = 0x{value:08x}, {}=0",
                register_name(rd),
                register_name(rt),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xffc0, 0xe880 | 0xe900) {
        let load = first & 0x0010 != 0;
        let writeback = first & 0x0020 != 0;
        let decrement_before = first & 0xffc0 == 0xe900;
        let rn = (first & 0x0f) as usize;
        let register_count = second.count_ones();
        let bytes = register_count * 4;
        let base = cpu.registers[rn];
        let mut address = if decrement_before {
            base.wrapping_sub(bytes)
        } else {
            base
        };
        let mut next_pc = pc + 4;
        for register in 0..16 {
            if second & (1 << register) != 0 {
                if load {
                    let value = memory.read_u32_or_zero(address);
                    if register == 15 {
                        next_pc = value & !1;
                    } else {
                        cpu.registers[register] = value;
                    }
                } else {
                    memory.write_u32(address, cpu.registers[register]);
                }
                address = address.wrapping_add(4);
            }
        }
        let next_base = if decrement_before {
            base.wrapping_sub(bytes)
        } else {
            base.wrapping_add(bytes)
        };
        if writeback {
            cpu.registers[rn] = next_base;
        }
        let mode = if decrement_before { "DB" } else { "IA" };
        let operation = match (load, decrement_before) {
            (true, true) => "LDMDB.W",
            (true, false) => "LDM.W",
            (false, true) => "STMDB.W",
            (false, false) => "STM.W",
        };
        return ThumbDecode {
            next_pc,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {}{}, register-list 0x{second:04x} ({mode}) ; start=0x{:08x}, {}=0x{next_base:08x}",
                register_name(rn),
                if writeback { "!" } else { "" },
                if decrement_before { next_base } else { base },
                register_name(rn)
            ),
        };
    }

    if first & 0xfe40 == 0xe840 {
        let load = first & 0x0010 != 0;
        let add = first & 0x0080 != 0;
        let pre_index = first & 0x0100 != 0;
        let writeback = first & 0x0020 != 0;
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let rt2 = ((second >> 8) & 0x0f) as usize;
        let imm8 = (second & 0x00ff) as u32;
        let offset = imm8 * 4;
        let base = cpu.registers[rn];
        let adjusted = if add {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let address = if pre_index { adjusted } else { base };
        let address_note = if pre_index {
            format!(
                "[{}, #{}0x{offset:x}]{}",
                register_name(rn),
                if add { "" } else { "-" },
                if writeback { "!" } else { "" }
            )
        } else {
            format!(
                "[{}], #{}0x{offset:x}",
                register_name(rn),
                if add { "" } else { "-" }
            )
        };
        let note = if load {
            let first_value = memory.read_u32_or_zero(address);
            let second_value = memory.read_u32_or_zero(address.wrapping_add(4));
            cpu.registers[rt] = first_value;
            cpu.registers[rt2] = second_value;
            format!(
                "LDRD {}, {}, {address_note} ; [0x{address:08x}] = 0x{first_value:08x}, [0x{:08x}] = 0x{second_value:08x}",
                register_name(rt),
                register_name(rt2),
                address.wrapping_add(4)
            )
        } else {
            memory.write_u32(address, cpu.registers[rt]);
            memory.write_u32(address.wrapping_add(4), cpu.registers[rt2]);
            format!(
                "STRD {}, {}, {address_note} ; [0x{address:08x}] = 0x{:08x}, [0x{:08x}] = 0x{:08x}",
                register_name(rt),
                register_name(rt2),
                cpu.registers[rt],
                address.wrapping_add(4),
                cpu.registers[rt2]
            )
        };
        if writeback {
            cpu.registers[rn] = adjusted;
        }
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: if writeback {
                format!("{note}, {}=0x{adjusted:08x}", register_name(rn))
            } else {
                note
            },
        };
    }

    if first == 0xf3bf && second & 0xfff0 == 0x8f40 {
        let barrier = format!("DSB {}", barrier_option_name(second & 0x000f));
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: barrier,
        };
    }

    if first == 0xf3bf && second & 0xfff0 == 0x8f50 {
        let barrier = format!("DMB {}", barrier_option_name(second & 0x000f));
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: barrier,
        };
    }

    if first == 0xf3bf && second & 0xfff0 == 0x8f60 {
        let barrier = format!("ISB {}", barrier_option_name(second & 0x000f));
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: barrier,
        };
    }

    if first == 0xf3af && second & 0xff00 == 0x8000 {
        let hint = match second & 0x00ff {
            0x00 => "NOP.W",
            0x01 => "YIELD.W",
            0x02 => "WFE.W",
            0x03 => "WFI.W",
            0x04 => "SEV.W",
            0x05 => "SEVL.W",
            value => {
                return ThumbDecode {
                    next_pc: pc + 4,
                    opcode32: Some(opcode32),
                    note: format!("HINT.W #0x{value:x}"),
                };
            }
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: hint.to_string(),
        };
    }

    if first == 0xf3ef && second & 0xf000 == 0x8000 {
        let rd = ((second >> 8) & 0x0f) as usize;
        let sysm = (second & 0x00ff) as u8;
        let value = match sysm {
            0..=3 => apsr_to_xpsr_bits(cpu.apsr),
            5 => cpu.xpsr & 0x1ff,
            6 => cpu.xpsr & ARM_XPSR_THUMB,
            7 => cpu.xpsr,
            8 => cpu.registers[13],
            9 => cpu.psp,
            16 => cpu.primask,
            17 | 18 => cpu.basepri,
            19 => cpu.faultmask,
            20 => cpu.control,
            _ => 0,
        };
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "MRS {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                special_register_name(sysm),
                register_name(rd)
            ),
        };
    }

    if first & 0xfff0 == 0xf380 && second & 0xff00 == 0x8800 {
        let rn = (first & 0x0f) as usize;
        let sysm = (second & 0x00ff) as u8;
        let value = cpu.registers[rn];
        match sysm {
            8 => cpu.registers[13] = value,
            9 => cpu.psp = value,
            16 => cpu.primask = value & 1,
            17 => cpu.basepri = value & 0xff,
            18 => {
                let new_basepri = value & 0xff;
                if cpu.basepri == 0 || (new_basepri != 0 && new_basepri < cpu.basepri) {
                    cpu.basepri = new_basepri;
                }
            }
            19 => cpu.faultmask = value & 1,
            20 => cpu.control = value & 0x7,
            _ => {}
        }
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "MSR {}, {} ; value=0x{value:08x}",
                special_register_name(sysm),
                register_name(rn)
            ),
        };
    }

    if first == 0xeef1 && second & 0x0fff == 0x0a10 {
        let rt = ((second >> 12) & 0x0f) as usize;
        if rt == 15 {
            cpu.apsr.n = cpu.fpscr & 0x8000_0000 != 0;
            cpu.apsr.z = cpu.fpscr & 0x4000_0000 != 0;
            cpu.apsr.c = cpu.fpscr & 0x2000_0000 != 0;
            cpu.apsr.v = cpu.fpscr & 0x1000_0000 != 0;
        } else {
            cpu.registers[rt] = cpu.fpscr;
        }
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: if rt == 15 {
                "VMRS APSR_nzcv, FPSCR".to_string()
            } else {
                format!(
                    "VMRS {}, FPSCR ; {}=0x{:08x}",
                    register_name(rt),
                    register_name(rt),
                    cpu.registers[rt]
                )
            },
        };
    }

    if first == 0xeee1 && second & 0x0fff == 0x0a10 {
        let rt = ((second >> 12) & 0x0f) as usize;
        cpu.fpscr = cpu.registers[rt];
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!("VMSR FPSCR, {}", register_name(rt)),
        };
    }

    if first & 0xf800 == 0xf000 && second & 0xd000 == 0x8000 {
        let condition = (first >> 6) & 0x0f;
        let target = thumb32_cond_branch_target(pc, first, second);
        let taken = condition_met(condition, cpu.apsr);
        return ThumbDecode {
            next_pc: if taken { target } else { pc + 4 },
            opcode32: Some(opcode32),
            note: format!(
                "B{}.W 0x{target:08x} ; {}",
                condition_name(condition),
                if taken { "taken" } else { "not taken" }
            ),
        };
    }

    if first & 0xf800 == 0xf000 && second & 0xc000 == 0x8000 && second & 0x1000 != 0 {
        let target = thumb32_branch_target(pc, first, second);
        return ThumbDecode {
            next_pc: target,
            opcode32: Some(opcode32),
            note: format!("B.W 0x{target:08x}"),
        };
    }

    if first & 0xf800 == 0xf000 && second & 0xd000 == 0xd000 {
        let target = thumb32_branch_target(pc, first, second);
        cpu.registers[14] = pc + 5;
        return ThumbDecode {
            next_pc: target,
            opcode32: Some(opcode32),
            note: format!("BL 0x{target:08x} ; LR=0x{:08x}", cpu.registers[14]),
        };
    }

    if matches!(first, 0xf04f | 0xf44f) && second & 0x8000 == 0 {
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let value = thumb_expand_imm(imm12);
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!("MOV.W {}, #0x{value:08x}", register_name(rd)),
        };
    }

    if matches!(first & 0xfbf0, 0xf240 | 0xf2c0) {
        let rd = ((second >> 8) & 0x0f) as usize;
        let immediate = (((first & 0x000f) as u32) << 12)
            | (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let move_top = first & 0xfbf0 == 0xf2c0;
        let value = if move_top {
            (cpu.registers[rd] & 0x0000_ffff) | (immediate << 16)
        } else {
            immediate
        };
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, #0x{immediate:04x} ; {}=0x{value:08x}",
                if move_top { "MOVT" } else { "MOVW" },
                register_name(rd),
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfbf0, 0xf020 | 0xf040 | 0xf060 | 0xf080) && second & 0x8000 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let operation = match first & 0xfbf0 {
            0xf020 => "BIC.W",
            0xf040 => "ORR.W",
            0xf060 if rn == 15 => "MVN.W",
            0xf060 => "ORN.W",
            _ => "EOR.W",
        };
        let value = match first & 0xfbf0 {
            0xf020 => cpu.registers[rn] & !immediate,
            0xf040 => cpu.registers[rn] | immediate,
            0xf060 if rn == 15 => !immediate,
            0xf060 => cpu.registers[rn] | !immediate,
            _ => cpu.registers[rn] ^ immediate,
        };
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {}, {}, #0x{immediate:08x} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfbf0, 0xf030 | 0xf050 | 0xf070 | 0xf090) && second & 0x8000 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let operation = match first & 0xfbf0 {
            0xf030 => "BICS.W",
            0xf050 if rn == 15 => "MOVS.W",
            0xf050 => "ORRS.W",
            0xf070 if rn == 15 => "MVNS.W",
            0xf070 => "ORNS.W",
            _ => "EORS.W",
        };
        let value = match first & 0xfbf0 {
            0xf030 => cpu.registers[rn] & !immediate,
            0xf050 if rn == 15 => immediate,
            0xf050 => cpu.registers[rn] | immediate,
            0xf070 if rn == 15 => !immediate,
            0xf070 => cpu.registers[rn] | !immediate,
            _ => cpu.registers[rn] ^ immediate,
        };
        cpu.registers[rd] = value;
        set_nz_flags(cpu, value);
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {}, {}, #0x{immediate:08x} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if first & 0xfbf0 == 0xf010 && second & 0x8f00 == 0x0f00 {
        let rn = (first & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let value = cpu.registers[rn] & immediate;
        set_nz_flags(cpu, value);
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "TST.W {}, #0x{immediate:08x} ; result=0x{value:08x}",
                register_name(rn)
            ),
        };
    }

    if first & 0xfbf0 == 0xf010 && second & 0x8000 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let value = cpu.registers[rn] & immediate;
        cpu.registers[rd] = value;
        set_nz_flags(cpu, value);
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "ANDS.W {}, {}, #0x{immediate:08x} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if first & 0xfbf0 == 0xf000 && second & 0x8000 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let value = cpu.registers[rn] & immediate;
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "AND.W {}, {}, #0x{immediate:08x} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfbf0, 0xf100 | 0xf1a0) && second & 0x8000 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let subtract = first & 0xfbf0 == 0xf1a0;
        let value = if subtract {
            cpu.registers[rn].wrapping_sub(immediate)
        } else {
            cpu.registers[rn].wrapping_add(immediate)
        };
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, {}, #0x{immediate:08x} ; {}=0x{value:08x}",
                if subtract { "SUB.W" } else { "ADD.W" },
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfbf0, 0xf140 | 0xf150 | 0xf160 | 0xf170) && second & 0x8000 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let op = first & 0xfbf0;
        let lhs = cpu.registers[rn];
        let carry = u32::from(cpu.apsr.c);
        let value = match op {
            0xf140 | 0xf150 => lhs.wrapping_add(immediate).wrapping_add(carry),
            _ => lhs
                .wrapping_sub(immediate)
                .wrapping_sub(u32::from(!cpu.apsr.c)),
        };
        cpu.registers[rd] = value;
        if matches!(op, 0xf150 | 0xf170) {
            if matches!(op, 0xf150) {
                set_add_flags(cpu, lhs, immediate.wrapping_add(carry), value);
            } else {
                set_sub_flags(
                    cpu,
                    lhs,
                    immediate.wrapping_add(u32::from(!cpu.apsr.c)),
                    value,
                );
            }
        }
        let operation = match op {
            0xf140 => "ADC.W",
            0xf150 => "ADCS.W",
            0xf160 => "SBC.W",
            _ => "SBCS.W",
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {}, {}, #0x{immediate:08x} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if first & 0xfbf0 == 0xf1c0 && second & 0x8000 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let value = immediate.wrapping_sub(cpu.registers[rn]);
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "RSB.W {}, {}, #0x{immediate:08x} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if first & 0xfbf0 == 0xf1b0 && second & 0x8f00 == 0x0f00 {
        let rn = (first & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let lhs = cpu.registers[rn];
        let value = lhs.wrapping_sub(immediate);
        set_sub_flags(cpu, lhs, immediate, value);
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!("CMP.W {}, #0x{immediate:x}", register_name(rn)),
        };
    }

    if matches!(first & 0xfbf0, 0xf110 | 0xf1b0) && second & 0x8000 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let imm12 = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let immediate = thumb_expand_imm(imm12);
        let lhs = cpu.registers[rn];
        let subtract = first & 0xfbf0 == 0xf1b0;
        let value = if subtract {
            lhs.wrapping_sub(immediate)
        } else {
            lhs.wrapping_add(immediate)
        };
        if subtract {
            set_sub_flags(cpu, lhs, immediate, value);
        } else {
            set_add_flags(cpu, lhs, immediate, value);
        }
        if rd != 15 {
            cpu.registers[rd] = value;
        }
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, {}, #0x{immediate:08x} ; {}=0x{value:08x}",
                if subtract { "SUBS.W" } else { "ADDS.W" },
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfff0, 0xf880 | 0xf890 | 0xf8a0 | 0xf8b0) {
        let load = first & 0x0010 != 0;
        let halfword = first & 0x0020 != 0;
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let imm12 = (second & 0x0fff) as u32;
        let address = thumb32_memory_base(cpu, pc, rn).wrapping_add(imm12);
        let note = match (load, halfword) {
            (false, false) => {
                let value = cpu.registers[rt] as u8;
                memory.write_u8(address, value);
                format!(
                    "STRB.W {}, [{}, #0x{imm12:x}] ; [0x{address:08x}] = 0x{value:02x}",
                    register_name(rt),
                    register_name(rn)
                )
            }
            (true, false) => {
                let value = memory.read_u8_or_zero(address) as u32;
                cpu.registers[rt] = value;
                format!(
                    "LDRB.W {}, [{}, #0x{imm12:x}] ; [0x{address:08x}] = 0x{value:02x}",
                    register_name(rt),
                    register_name(rn)
                )
            }
            (false, true) => {
                let value = cpu.registers[rt] as u16;
                memory.write_u16(address, value);
                format!(
                    "STRH.W {}, [{}, #0x{imm12:x}] ; [0x{address:08x}] = 0x{value:04x}",
                    register_name(rt),
                    register_name(rn)
                )
            }
            (true, true) => {
                let value = memory.read_u16_or_zero(address) as u32;
                cpu.registers[rt] = value;
                format!(
                    "LDRH.W {}, [{}, #0x{imm12:x}] ; [0x{address:08x}] = 0x{value:04x}",
                    register_name(rt),
                    register_name(rn)
                )
            }
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if matches!(first & 0xfff0, 0xf990 | 0xf9b0) {
        let halfword = first & 0x0020 != 0;
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let imm12 = (second & 0x0fff) as u32;
        let address = thumb32_memory_base(cpu, pc, rn).wrapping_add(imm12);
        let value = if halfword {
            ((memory.read_u16_or_zero(address) as i16) as i32) as u32
        } else {
            ((memory.read_u8_or_zero(address) as i8) as i32) as u32
        };
        cpu.registers[rt] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, [{}, #0x{imm12:x}] ; [0x{address:08x}] = 0x{value:08x}",
                if halfword { "LDRSH.W" } else { "LDRSB.W" },
                register_name(rt),
                register_name(rn)
            ),
        };
    }

    if matches!(first & 0xfff0, 0xf910 | 0xf930) && second & 0x0800 != 0 {
        let halfword = first & 0x0020 != 0;
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let pre_index = second & 0x0400 != 0;
        let add = second & 0x0200 != 0;
        let writeback = second & 0x0100 != 0;
        let imm8 = (second & 0x00ff) as u32;
        let base = thumb32_memory_base(cpu, pc, rn);
        let offset_base = if add {
            base.wrapping_add(imm8)
        } else {
            base.wrapping_sub(imm8)
        };
        let address = if pre_index { offset_base } else { base };
        let value = if halfword {
            ((memory.read_u16_or_zero(address) as i16) as i32) as u32
        } else {
            ((memory.read_u8_or_zero(address) as i8) as i32) as u32
        };
        cpu.registers[rt] = value;
        if writeback && rn != 15 {
            cpu.registers[rn] = offset_base;
        }
        let suffix = match (pre_index, writeback) {
            (true, true) => "!",
            _ => "",
        };
        let sign = if add { "" } else { "-" };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, [{}, #{sign}0x{imm8:x}]{suffix} ; [0x{address:08x}] = 0x{value:08x}",
                if halfword { "LDRSH.W" } else { "LDRSB.W" },
                register_name(rt),
                register_name(rn)
            ),
        };
    }

    if matches!(first & 0xfb00, 0xf200 | 0xf2a0) {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let immediate = (((first >> 10) as u32 & 1) << 11)
            | (((second >> 12) as u32 & 0x7) << 8)
            | (second as u32 & 0xff);
        let subtract = first & 0xfb00 == 0xf2a0;
        let value = if subtract {
            cpu.registers[rn].wrapping_sub(immediate)
        } else {
            cpu.registers[rn].wrapping_add(immediate)
        };
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, {}, #0x{immediate:03x} ; {}=0x{value:08x}",
                if subtract { "SUBW" } else { "ADDW" },
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if first & 0xfff0 == 0xf8c0 {
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let imm12 = (second & 0x0fff) as u32;
        let address = thumb32_memory_base(cpu, pc, rn).wrapping_add(imm12);
        memory.write_u32(address, cpu.registers[rt]);
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "STR.W {}, [{}, #0x{imm12:x}] ; [0x{address:08x}] = 0x{:08x}",
                register_name(rt),
                register_name(rn),
                cpu.registers[rt]
            ),
        };
    }

    if first & 0xfff0 == 0xf8d0 {
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let imm12 = (second & 0x0fff) as u32;
        let address = thumb32_memory_base(cpu, pc, rn).wrapping_add(imm12);
        let (value, sanitize_note) = if rn != 15 {
            if let Some(sanitized) =
                memory.sanitize_display_cell_effective_address(cpu.registers[rn], imm12, address)
            {
                (
                    sanitized,
                    format!(
                        " ; effective address base sanitized from 0x{:08x}",
                        cpu.registers[rn]
                    ),
                )
            } else {
                (memory.read_u32_or_zero(address), String::new())
            }
        } else {
            (memory.read_u32_or_zero(address), String::new())
        };
        cpu.registers[rt] = value;
        let next_pc = if rt == 15 { value & !1 } else { pc + 4 };
        return ThumbDecode {
            next_pc,
            opcode32: Some(opcode32),
            note: format!(
                "LDR.W {}, [{}, #0x{imm12:x}] ; [0x{address:08x}] = 0x{value:08x}{sanitize_note}",
                register_name(rt),
                register_name(rn)
            ),
        };
    }

    if first == 0xf85f {
        let rt = ((second >> 12) & 0x0f) as usize;
        let imm12 = (second & 0x0fff) as u32;
        let address = align4(pc + 4).wrapping_sub(imm12);
        let value = memory.read_u32_or_zero(address);
        cpu.registers[rt] = value;
        let next_pc = if rt == 15 { value & !1 } else { pc + 4 };
        return ThumbDecode {
            next_pc,
            opcode32: Some(opcode32),
            note: format!(
                "LDR.W {}, [PC, #-0x{imm12:x}] ; [0x{address:08x}] = 0x{value:08x}",
                register_name(rt)
            ),
        };
    }

    if matches!(first & 0xfff0, 0xf800 | 0xf810 | 0xf820 | 0xf830) && second & 0x0fc0 == 0 {
        let load = first & 0x0010 != 0;
        let halfword = first & 0x0020 != 0;
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let shift = ((second >> 4) & 0x03) as u32;
        let offset = cpu.registers[rm].wrapping_shl(shift);
        let address = thumb32_memory_base(cpu, pc, rn).wrapping_add(offset);
        let note = match (load, halfword) {
            (false, false) => {
                let value = cpu.registers[rt] as u8;
                memory.write_u8(address, value);
                format!(
                    "STRB.W {}, [{}, {}, LSL #{}] ; [0x{address:08x}] = 0x{value:02x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm),
                    shift
                )
            }
            (true, false) => {
                let value = memory.read_u8_or_zero(address) as u32;
                cpu.registers[rt] = value;
                format!(
                    "LDRB.W {}, [{}, {}, LSL #{}] ; [0x{address:08x}] = 0x{value:02x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm),
                    shift
                )
            }
            (false, true) => {
                let value = cpu.registers[rt] as u16;
                memory.write_u16(address, value);
                format!(
                    "STRH.W {}, [{}, {}, LSL #{}] ; [0x{address:08x}] = 0x{value:04x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm),
                    shift
                )
            }
            (true, true) => {
                let value = memory.read_u16_or_zero(address) as u32;
                cpu.registers[rt] = value;
                format!(
                    "LDRH.W {}, [{}, {}, LSL #{}] ; [0x{address:08x}] = 0x{value:04x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm),
                    shift
                )
            }
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if matches!(first & 0xfff0, 0xf800 | 0xf810) && second & 0x0f00 == 0x0b00 {
        let load = first & 0x0010 != 0;
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let imm8 = (second & 0x00ff) as u32;
        let address = cpu.registers[rn];
        let next_rn = cpu.registers[rn].wrapping_add(imm8);
        cpu.registers[rn] = next_rn;
        let note = if load {
            let value = memory.read_u8_or_zero(address) as u32;
            cpu.registers[rt] = value;
            format!(
                "LDRB.W {}, [{}], #0x{imm8:x} ; [0x{address:08x}] = 0x{value:02x}, {}=0x{next_rn:08x}",
                register_name(rt),
                register_name(rn),
                register_name(rn)
            )
        } else {
            let value = cpu.registers[rt] as u8;
            memory.write_u8(address, value);
            format!(
                "STRB.W {}, [{}], #0x{imm8:x} ; [0x{address:08x}] = 0x{value:02x}, {}=0x{next_rn:08x}",
                register_name(rt),
                register_name(rn),
                register_name(rn)
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if matches!(first & 0xfff0, 0xf840 | 0xf850) && second & 0x0f00 == 0x0b00 {
        let load = first & 0x0010 != 0;
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let imm8 = (second & 0x00ff) as u32;
        let address = cpu.registers[rn];
        let next_rn = cpu.registers[rn].wrapping_add(imm8);
        cpu.registers[rn] = next_rn;
        let mut next_pc = pc + 4;
        let note = if load {
            let value = memory.read_u32_or_zero(address);
            cpu.registers[rt] = value;
            if rt == 15 {
                next_pc = value & !1;
            }
            format!(
                "LDR.W {}, [{}], #0x{imm8:x} ; [0x{address:08x}] = 0x{value:08x}, {}=0x{next_rn:08x}",
                register_name(rt),
                register_name(rn),
                register_name(rn)
            )
        } else {
            memory.write_u32(address, cpu.registers[rt]);
            format!(
                "STR.W {}, [{}], #0x{imm8:x} ; [0x{address:08x}] = 0x{:08x}, {}=0x{next_rn:08x}",
                register_name(rt),
                register_name(rn),
                cpu.registers[rt],
                register_name(rn)
            )
        };
        return ThumbDecode {
            next_pc,
            opcode32: Some(opcode32),
            note,
        };
    }

    if matches!(first & 0xfff0, 0xf840 | 0xf850) && second & 0x0fc0 == 0 {
        let load = first & 0x0010 != 0;
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let shift = ((second >> 4) & 0x03) as u32;
        let offset = cpu.registers[rm].wrapping_shl(shift);
        let address = thumb32_memory_base(cpu, pc, rn).wrapping_add(offset);
        let mut next_pc = pc + 4;
        let note = if load {
            let value = memory.read_u32_or_zero(address);
            cpu.registers[rt] = value;
            if rt == 15 {
                next_pc = value & !1;
            }
            format!(
                "LDR.W {}, [{}, {}, LSL #{}] ; [0x{address:08x}] = 0x{value:08x}",
                register_name(rt),
                register_name(rn),
                register_name(rm),
                shift
            )
        } else {
            memory.write_u32(address, cpu.registers[rt]);
            format!(
                "STR.W {}, [{}, {}, LSL #{}] ; [0x{address:08x}] = 0x{:08x}",
                register_name(rt),
                register_name(rn),
                register_name(rm),
                shift,
                cpu.registers[rt]
            )
        };
        return ThumbDecode {
            next_pc,
            opcode32: Some(opcode32),
            note,
        };
    }

    if matches!(
        first & 0xfff0,
        0xf800 | 0xf810 | 0xf820 | 0xf830 | 0xf840 | 0xf850
    ) && second & 0x0800 != 0
    {
        let load = first & 0x0010 != 0;
        let halfword = first & 0x0020 != 0;
        let word = first & 0x0040 != 0;
        let rn = (first & 0x0f) as usize;
        let rt = ((second >> 12) & 0x0f) as usize;
        let imm8 = (second & 0x00ff) as u32;
        let pre_index = second & 0x0400 != 0;
        let add = second & 0x0200 != 0;
        let writeback = second & 0x0100 != 0;
        let base = thumb32_memory_base(cpu, pc, rn);
        let indexed = if add {
            base.wrapping_add(imm8)
        } else {
            base.wrapping_sub(imm8)
        };
        let address = if pre_index { indexed } else { base };
        if writeback {
            cpu.registers[rn] = indexed;
        }
        let operation = match (load, word, halfword) {
            (false, false, false) => "STRB.W",
            (true, false, false) => "LDRB.W",
            (false, false, true) => "STRH.W",
            (true, false, true) => "LDRH.W",
            (false, true, _) => "STR.W",
            (true, true, _) => "LDR.W",
        };
        let index_note = if pre_index {
            format!(
                "[{}, #{}0x{imm8:x}]{}",
                register_name(rn),
                if add { "" } else { "-" },
                if writeback { "!" } else { "" }
            )
        } else {
            format!(
                "[{}], #{}0x{imm8:x}",
                register_name(rn),
                if add { "" } else { "-" }
            )
        };
        let note = if load {
            let value = if word {
                memory.read_u32_or_zero(address)
            } else if halfword {
                memory.read_u16_or_zero(address) as u32
            } else {
                memory.read_u8_or_zero(address) as u32
            };
            cpu.registers[rt] = value;
            let next_pc = if word && rt == 15 { value & !1 } else { pc + 4 };
            let note = format!(
                "{operation} {}, {index_note} ; [0x{address:08x}] = 0x{value:08x}{}",
                register_name(rt),
                if writeback {
                    format!(", {}=0x{indexed:08x}", register_name(rn))
                } else {
                    String::new()
                }
            );
            return ThumbDecode {
                next_pc,
                opcode32: Some(opcode32),
                note,
            };
        } else {
            let value = cpu.registers[rt];
            if word {
                memory.write_u32(address, value);
            } else if halfword {
                memory.write_u16(address, value as u16);
            } else {
                memory.write_u8(address, value as u8);
            }
            format!(
                "{operation} {}, {index_note} ; [0x{address:08x}] = 0x{value:08x}{}",
                register_name(rt),
                if writeback {
                    format!(", {}=0x{indexed:08x}", register_name(rn))
                } else {
                    String::new()
                }
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if matches!(first & 0xfff0, 0xeb00 | 0xeb40 | 0xeb60 | 0xeba0 | 0xebc0) {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let shift_kind = (second >> 4) & 0x03;
        let shift = ((((second >> 12) & 0x07) << 2) | ((second >> 6) & 0x03)) as u32;
        let (shift_name, operand) = shifted_register_value(cpu.registers[rm], shift_kind, shift);
        let operation = match first & 0xfff0 {
            0xeb00 => "ADD.W",
            0xeb40 => "ADC.W",
            0xeb60 => "SBC.W",
            0xeba0 => "SUB.W",
            _ => "RSB.W",
        };
        let value = match first & 0xfff0 {
            0xeb00 => cpu.registers[rn].wrapping_add(operand),
            0xeb40 => cpu.registers[rn]
                .wrapping_add(operand)
                .wrapping_add(u32::from(cpu.apsr.c)),
            0xeb60 => cpu.registers[rn]
                .wrapping_sub(operand)
                .wrapping_sub(u32::from(!cpu.apsr.c)),
            0xeba0 => cpu.registers[rn].wrapping_sub(operand),
            _ => operand.wrapping_sub(cpu.registers[rn]),
        };
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {}, {}, {}, {} #{} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rm),
                shift_name,
                shift,
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfff0, 0xeb10 | 0xeb50 | 0xeb70 | 0xebb0 | 0xebd0)
        && second & 0x0f00 != 0x0f00
    {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let shift_kind = (second >> 4) & 0x03;
        let shift = ((((second >> 12) & 0x07) << 2) | ((second >> 6) & 0x03)) as u32;
        let (shift_name, operand) = shifted_register_value(cpu.registers[rm], shift_kind, shift);
        let lhs = cpu.registers[rn];
        let op = first & 0xfff0;
        let carry = u32::from(cpu.apsr.c);
        let value = match op {
            0xeb10 => lhs.wrapping_add(operand),
            0xeb50 => lhs.wrapping_add(operand).wrapping_add(carry),
            0xeb70 => lhs
                .wrapping_sub(operand)
                .wrapping_sub(u32::from(!cpu.apsr.c)),
            0xebb0 => lhs.wrapping_sub(operand),
            _ => operand.wrapping_sub(lhs),
        };
        cpu.registers[rd] = value;
        match op {
            0xeb10 => set_add_flags(cpu, lhs, operand, value),
            0xeb50 => set_add_flags(cpu, lhs, operand.wrapping_add(carry), value),
            0xeb70 => set_sub_flags(
                cpu,
                lhs,
                operand.wrapping_add(u32::from(!cpu.apsr.c)),
                value,
            ),
            0xebb0 => set_sub_flags(cpu, lhs, operand, value),
            _ => set_sub_flags(cpu, operand, lhs, value),
        }
        let operation = match op {
            0xeb10 => "ADDS.W",
            0xeb50 => "ADCS.W",
            0xeb70 => "SBCS.W",
            0xebb0 => "SUBS.W",
            _ => "RSBS.W",
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {}, {}, {}, {} #{} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rm),
                shift_name,
                shift,
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfff0, 0xeb10 | 0xebb0) && second & 0x0f00 == 0x0f00 {
        let rn = (first & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let shift_kind = (second >> 4) & 0x03;
        let shift = ((((second >> 12) & 0x07) << 2) | ((second >> 6) & 0x03)) as u32;
        let (shift_name, operand) = shifted_register_value(cpu.registers[rm], shift_kind, shift);
        let lhs = cpu.registers[rn];
        let subtract = first & 0xfff0 == 0xebb0;
        let value = if subtract {
            lhs.wrapping_sub(operand)
        } else {
            lhs.wrapping_add(operand)
        };
        if subtract {
            set_sub_flags(cpu, lhs, operand, value);
        } else {
            set_add_flags(cpu, lhs, operand, value);
        }
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, {}, {} #{} ; result=0x{value:08x}",
                if subtract { "CMP.W" } else { "CMN.W" },
                register_name(rn),
                register_name(rm),
                shift_name,
                shift
            ),
        };
    }

    if first == 0xea4f {
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let shift_kind = (second >> 4) & 0x03;
        let shift = ((((second >> 12) & 0x07) << 2) | ((second >> 6) & 0x03)) as u32;
        let (shift_name, value) = shifted_register_value(cpu.registers[rm], shift_kind, shift);
        let operation = if shift_kind == 0 && shift == 0 {
            "MOV.W"
        } else {
            match shift_kind {
                0 => "LSL.W",
                1 => "LSR.W",
                2 => "ASR.W",
                _ => "ROR.W",
            }
        };
        cpu.registers[rd] = value;
        let operand_note = if operation == "MOV.W" {
            register_name(rm).to_string()
        } else {
            format!("{}, #{shift}", register_name(rm))
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {}, {operand_note} ; {}=0x{value:08x} ({shift_name} #{shift})",
                register_name(rd),
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfff0, 0xea00 | 0xea20 | 0xea40 | 0xea60 | 0xea80) {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let shift_kind = (second >> 4) & 0x03;
        let shift = ((((second >> 12) & 0x07) << 2) | ((second >> 6) & 0x03)) as u32;
        let (shift_name, operand) = shifted_register_value(cpu.registers[rm], shift_kind, shift);
        let (operation, value) = match first & 0xfff0 {
            0xea00 => ("AND.W", cpu.registers[rn] & operand),
            0xea20 => ("BIC.W", cpu.registers[rn] & !operand),
            0xea40 => ("ORR.W", cpu.registers[rn] | operand),
            0xea60 => ("ORN.W", cpu.registers[rn] | !operand),
            _ => ("EOR.W", cpu.registers[rn] ^ operand),
        };
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {}, {}, {}, {} #{} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rm),
                shift_name,
                shift,
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfff0, 0xea10 | 0xea30 | 0xea50 | 0xea70 | 0xea90) {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let shift_kind = (second >> 4) & 0x03;
        let shift = ((((second >> 12) & 0x07) << 2) | ((second >> 6) & 0x03)) as u32;
        let (shift_name, operand) = shifted_register_value(cpu.registers[rm], shift_kind, shift);
        let (operation, value) = match first & 0xfff0 {
            0xea10 => ("ANDS.W", cpu.registers[rn] & operand),
            0xea30 => ("BICS.W", cpu.registers[rn] & !operand),
            0xea50 => ("ORRS.W", cpu.registers[rn] | operand),
            0xea70 => ("ORNS.W", cpu.registers[rn] | !operand),
            _ => ("EORS.W", cpu.registers[rn] ^ operand),
        };
        if rd != 15 {
            cpu.registers[rd] = value;
        }
        set_nz_flags(cpu, value);
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {}, {}, {}, {} #{} ; result=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rm),
                shift_name,
                shift
            ),
        };
    }

    if first & 0xfff0 == 0xfb00 && matches!(second & 0x00f0, 0x0000 | 0x0010) {
        let rn = (first & 0x0f) as usize;
        let ra = ((second >> 12) & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let product = cpu.registers[rn].wrapping_mul(cpu.registers[rm]);
        let subtract = second & 0x00f0 == 0x0010;
        let multiply_only = !subtract && ra == 15;
        let value = if multiply_only {
            product
        } else if subtract {
            cpu.registers[ra].wrapping_sub(product)
        } else {
            product.wrapping_add(cpu.registers[ra])
        };
        cpu.registers[rd] = value;
        let note = if multiply_only {
            format!(
                "MUL.W {}, {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rm),
                register_name(rd)
            )
        } else {
            format!(
                "{} {}, {}, {}, {} ; {}=0x{value:08x}",
                if subtract { "MLS" } else { "MLA" },
                register_name(rd),
                register_name(rn),
                register_name(rm),
                register_name(ra),
                register_name(rd)
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xfff0 == 0xfb10 && second & 0x00c0 == 0 {
        let rn = (first & 0x0f) as usize;
        let ra = ((second >> 12) & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let n_high = second & 0x0020 != 0;
        let m_high = second & 0x0010 != 0;
        let lhs = signed_halfword(cpu.registers[rn], n_high) as i32;
        let rhs = signed_halfword(cpu.registers[rm], m_high) as i32;
        let product = lhs.wrapping_mul(rhs);
        let multiply_only = ra == 15;
        let value = if multiply_only {
            product as u32
        } else {
            (cpu.registers[ra] as i32).wrapping_add(product) as u32
        };
        cpu.registers[rd] = value;
        let suffix = format!(
            "{}{}",
            if n_high { "T" } else { "B" },
            if m_high { "T" } else { "B" }
        );
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: if multiply_only {
                format!(
                    "SMUL{suffix} {}, {}, {} ; {}=0x{value:08x}",
                    register_name(rd),
                    register_name(rn),
                    register_name(rm),
                    register_name(rd)
                )
            } else {
                format!(
                    "SMLA{suffix} {}, {}, {}, {} ; {}=0x{value:08x}",
                    register_name(rd),
                    register_name(rn),
                    register_name(rm),
                    register_name(ra),
                    register_name(rd)
                )
            },
        };
    }

    if first & 0xfff0 == 0xfba0 && second & 0x00f0 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd_lo = ((second >> 12) & 0x0f) as usize;
        let rd_hi = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let product = (cpu.registers[rn] as u64) * (cpu.registers[rm] as u64);
        cpu.registers[rd_lo] = product as u32;
        cpu.registers[rd_hi] = (product >> 32) as u32;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "UMULL {}, {}, {}, {} ; value=0x{product:016x}",
                register_name(rd_lo),
                register_name(rd_hi),
                register_name(rn),
                register_name(rm)
            ),
        };
    }

    if first & 0xfff0 == 0xfb80 && second & 0x00f0 == 0 {
        let rn = (first & 0x0f) as usize;
        let rd_lo = ((second >> 12) & 0x0f) as usize;
        let rd_hi = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let product =
            (cpu.registers[rn] as i32 as i64).wrapping_mul(cpu.registers[rm] as i32 as i64);
        cpu.registers[rd_lo] = product as u32;
        cpu.registers[rd_hi] = (product >> 32) as u32;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "SMULL {}, {}, {}, {} ; value=0x{:016x}",
                register_name(rd_lo),
                register_name(rd_hi),
                register_name(rn),
                register_name(rm),
                product as u64
            ),
        };
    }

    if matches!(first & 0xfff0, 0xfb90 | 0xfbb0) && second & 0x00f0 == 0x00f0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let signed = first & 0xfff0 == 0xfb90;
        let divisor = cpu.registers[rm];
        let value = if divisor == 0 {
            0
        } else if signed {
            ((cpu.registers[rn] as i32).wrapping_div(divisor as i32)) as u32
        } else {
            cpu.registers[rn] / divisor
        };
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, {}, {} ; {}=0x{value:08x}",
                if signed { "SDIV" } else { "UDIV" },
                register_name(rd),
                register_name(rn),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if first & 0xfff0 == 0xfa80 && second & 0xf0f0 == 0xf040 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let mut value = 0u32;
        let mut ge = 0u8;
        for lane in 0..4 {
            let shift = lane * 8;
            let lhs = (cpu.registers[rn] >> shift) & 0xff;
            let rhs = (cpu.registers[rm] >> shift) & 0xff;
            let sum = lhs + rhs;
            if sum > 0xff {
                ge |= 1 << lane;
            }
            value |= (sum & 0xff) << shift;
        }
        cpu.registers[rd] = value;
        cpu.apsr.ge = ge;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "UADD8 {}, {}, {} ; {}=0x{value:08x}, GE=0x{ge:x}",
                register_name(rd),
                register_name(rn),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if first & 0xfff0 == 0xfaa0 && second & 0xf0f0 == 0xf080 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let mut value = 0u32;
        for lane in 0..4 {
            let shift = lane * 8;
            let source = if cpu.apsr.ge & (1 << lane) != 0 {
                cpu.registers[rn]
            } else {
                cpu.registers[rm]
            };
            value |= source & (0xff << shift);
        }
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "SEL {}, {}, {} ; {}=0x{value:08x}, GE=0x{:x}",
                register_name(rd),
                register_name(rn),
                register_name(rm),
                register_name(rd),
                cpu.apsr.ge
            ),
        };
    }

    if first & 0xfff0 == 0xfab0 && second & 0xf0f0 == 0xf080 {
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let value = cpu.registers[rm].leading_zeros();
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "CLZ {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if first & 0xfff0 == 0xfa90 && second & 0xf0f0 == 0xf0a0 {
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let value = cpu.registers[rm].reverse_bits();
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "RBIT {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xffe0, 0xfa00 | 0xfa20 | 0xfa40 | 0xfa60) && second & 0xf0f0 == 0xf000 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let shift_kind = (first >> 5) & 0x03;
        let shift = cpu.registers[rm] & 0xff;
        let (operation, value) =
            register_shifted_register_value(cpu.registers[rn], shift_kind, shift);
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation}.W {}, {}, {} ; shift={shift}, {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if matches!(first & 0xfff0, 0xfa00 | 0xfa10 | 0xfa40 | 0xfa50) && second & 0x00c0 == 0x0080 {
        let rn = (first & 0x000f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let rm = (second & 0x0f) as usize;
        let rotation = (((second >> 4) & 0x03) as u32) * 8;
        let rotated = cpu.registers[rm].rotate_right(rotation);
        let halfword = matches!(first & 0xfff0, 0xfa00 | 0xfa10);
        let unsigned = matches!(first & 0xfff0, 0xfa10 | 0xfa50);
        let extended = match (halfword, unsigned) {
            (true, true) => rotated & 0x0000_ffff,
            (true, false) => ((rotated as u16) as i16 as i32) as u32,
            (false, true) => rotated & 0x0000_00ff,
            (false, false) => ((rotated as u8) as i8 as i32) as u32,
        };
        let value = if rn == 15 {
            extended
        } else {
            cpu.registers[rn].wrapping_add(extended)
        };
        cpu.registers[rd] = value;
        let operation = match (rn == 15, halfword, unsigned) {
            (true, true, true) => "UXTH.W",
            (true, true, false) => "SXTH.W",
            (true, false, true) => "UXTB.W",
            (true, false, false) => "SXTB.W",
            (false, true, true) => "UXTAH",
            (false, true, false) => "SXTAH",
            (false, false, true) => "UXTAB",
            (false, false, false) => "SXTAB",
        };
        let rotate_note = if rotation == 0 {
            String::new()
        } else {
            format!(", ROR #{rotation}")
        };
        let operands = if rn == 15 {
            format!(
                "{}, {}{}",
                register_name(rd),
                register_name(rm),
                rotate_note
            )
        } else {
            format!(
                "{}, {}, {}{}",
                register_name(rd),
                register_name(rn),
                register_name(rm),
                rotate_note
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{operation} {operands} ; {}=0x{value:08x}",
                register_name(rd)
            ),
        };
    }

    if first & 0xfff0 == 0xf3c0 {
        let rn = (first & 0x0f) as usize;
        let rd = ((second >> 8) & 0x0f) as usize;
        let lsb = (((second >> 6) & 0x03) | (((second >> 12) & 0x07) << 2)) as u32;
        let width = ((second & 0x1f) + 1) as u32;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let value = (cpu.registers[rn] >> lsb) & mask;
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "UBFX {}, {}, #{lsb}, #{width} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rn),
                register_name(rd)
            ),
        };
    }

    if first & 0xfe00 == 0xec00 && first & 0x0020 != 0 && matches!(second & 0x0f00, 0x0a00 | 0x0b00)
    {
        let load = first & 0x0010 != 0;
        let up = first & 0x0080 != 0;
        let pre_index = first & 0x0100 != 0;
        let d = ((first >> 6) & 1) as usize;
        let rn = (first & 0x0f) as usize;
        let vd = ((second >> 12) & 0x0f) as usize;
        let single = second & 0x0f00 == 0x0a00;
        let imm8 = (second & 0x00ff) as usize;
        let bytes = (imm8 as u32) * 4;
        let base = cpu.registers[rn];
        let start_address = vfp_multiple_start_address(base, bytes, pre_index, up);
        let next_base = if up {
            base.wrapping_add(bytes)
        } else {
            base.wrapping_sub(bytes)
        };
        let note = if single {
            let sd = vd * 2 + d;
            let register_count = imm8;
            let list = single_register_list_name(sd, register_count);
            for offset in 0..register_count {
                let address = start_address.wrapping_add((offset as u32) * 4);
                let register = sd + offset;
                if load {
                    let bits = memory.read_u32_or_zero(address);
                    if register < cpu.fpu_s.len() {
                        cpu.fpu_s[register] = bits;
                    }
                } else if register < cpu.fpu_s.len() {
                    memory.write_u32(address, cpu.fpu_s[register]);
                }
            }
            if first & 0x0020 != 0 {
                cpu.registers[rn] = next_base;
            }
            format!(
                "{} {}{}, {{{list}}} ; start=0x{start_address:08x}, {}=0x{next_base:08x}",
                vfp_multiple_mnemonic(load, pre_index, up, rn, single),
                register_name(rn),
                if first & 0x0020 != 0 { "!" } else { "" },
                register_name(rn)
            )
        } else {
            let dd = vd + d * 16;
            let register_count = imm8 / 2;
            let list = double_register_list_name(dd, register_count);
            for offset in 0..register_count {
                let address = start_address.wrapping_add((offset as u32) * 8);
                let register = dd + offset;
                if load {
                    let low = memory.read_u32_or_zero(address);
                    let high = memory.read_u32_or_zero(address.wrapping_add(4));
                    set_fpu_d(cpu, register, ((high as u64) << 32) | low as u64);
                } else {
                    let bits = get_fpu_d(cpu, register);
                    memory.write_u32(address, bits as u32);
                    memory.write_u32(address.wrapping_add(4), (bits >> 32) as u32);
                }
            }
            if first & 0x0020 != 0 {
                cpu.registers[rn] = next_base;
            }
            format!(
                "{} {}{}, {{{list}}} ; start=0x{start_address:08x}, {}=0x{next_base:08x}",
                vfp_multiple_mnemonic(load, pre_index, up, rn, single),
                register_name(rn),
                if first & 0x0020 != 0 { "!" } else { "" },
                register_name(rn)
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xffa0 == 0xec00 && matches!(second & 0x0f00, 0x0a00 | 0x0b00) {
        let to_core = first & 0x0010 != 0;
        let rt = ((second >> 12) & 0x0f) as usize;
        let rt2 = (first & 0x0f) as usize;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let double = second & 0x0f00 == 0x0b00;
        let note = if double {
            let dm = vm + m * 16;
            if to_core {
                let bits = get_fpu_d(cpu, dm);
                cpu.registers[rt] = bits as u32;
                cpu.registers[rt2] = (bits >> 32) as u32;
                format!(
                    "VMOV {}, {}, {} ; {}=0x{:08x}, {}=0x{:08x}",
                    register_name(rt),
                    register_name(rt2),
                    double_register_name(dm),
                    register_name(rt),
                    cpu.registers[rt],
                    register_name(rt2),
                    cpu.registers[rt2]
                )
            } else {
                let bits = ((cpu.registers[rt2] as u64) << 32) | cpu.registers[rt] as u64;
                set_fpu_d(cpu, dm, bits);
                format!(
                    "VMOV {}, {}, {} ; {}=0x{bits:016x}",
                    double_register_name(dm),
                    register_name(rt),
                    register_name(rt2),
                    double_register_name(dm)
                )
            }
        } else {
            let sm = vm * 2 + m;
            if to_core {
                cpu.registers[rt] = cpu.fpu_s.get(sm).copied().unwrap_or(0);
                cpu.registers[rt2] = cpu.fpu_s.get(sm + 1).copied().unwrap_or(0);
                format!(
                    "VMOV {}, {}, {}, {} ; {}=0x{:08x}, {}=0x{:08x}",
                    register_name(rt),
                    register_name(rt2),
                    single_register_name(sm),
                    single_register_name(sm + 1),
                    register_name(rt),
                    cpu.registers[rt],
                    register_name(rt2),
                    cpu.registers[rt2]
                )
            } else {
                if sm < cpu.fpu_s.len() {
                    cpu.fpu_s[sm] = cpu.registers[rt];
                }
                if sm + 1 < cpu.fpu_s.len() {
                    cpu.fpu_s[sm + 1] = cpu.registers[rt2];
                }
                format!(
                    "VMOV {}, {}, {}, {} ; {}=0x{:08x}, {}=0x{:08x}",
                    single_register_name(sm),
                    single_register_name(sm + 1),
                    register_name(rt),
                    register_name(rt2),
                    single_register_name(sm),
                    cpu.registers[rt],
                    single_register_name(sm + 1),
                    cpu.registers[rt2]
                )
            }
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xff00 == 0xed00 && first & 0x0020 == 0 && second & 0x0f00 == 0x0b00 {
        let load = first & 0x0010 != 0;
        let up = first & 0x0080 != 0;
        let d = ((first >> 6) & 1) as usize;
        let rn = (first & 0x0f) as usize;
        let vd = ((second >> 12) & 0x0f) as usize;
        let dd = vd + d * 16;
        let offset = ((second & 0x00ff) as u32) * 4;
        let base = if rn == 15 {
            align4(pc + 4)
        } else {
            cpu.registers[rn]
        };
        let address = if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let note = if load {
            let low = memory.read_u32_or_zero(address);
            let high = memory.read_u32_or_zero(address.wrapping_add(4));
            let bits = ((high as u64) << 32) | low as u64;
            set_fpu_d(cpu, dd, bits);
            format!(
                "VLDR {}, [{}, #{}0x{offset:x}] ; [0x{address:08x}] = 0x{bits:016x} ({})",
                double_register_name(dd),
                register_name(rn),
                if up { "" } else { "-" },
                f64::from_bits(bits)
            )
        } else {
            let bits = get_fpu_d(cpu, dd);
            memory.write_u32(address, bits as u32);
            memory.write_u32(address.wrapping_add(4), (bits >> 32) as u32);
            format!(
                "VSTR {}, [{}, #{}0x{offset:x}] ; [0x{address:08x}] = 0x{bits:016x}",
                double_register_name(dd),
                register_name(rn),
                if up { "" } else { "-" }
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xff00 == 0xed00 && first & 0x0020 == 0 && second & 0x0f00 == 0x0a00 {
        let load = first & 0x0010 != 0;
        let up = first & 0x0080 != 0;
        let d = ((first >> 6) & 1) as usize;
        let rn = (first & 0x0f) as usize;
        let vd = ((second >> 12) & 0x0f) as usize;
        let sd = vd * 2 + d;
        let offset = ((second & 0x00ff) as u32) * 4;
        let base = if rn == 15 {
            align4(pc + 4)
        } else {
            cpu.registers[rn]
        };
        let address = if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let note = if load {
            let bits = memory.read_u32_or_zero(address);
            cpu.fpu_s[sd] = bits;
            format!(
                "VLDR {}, [{}, #{}0x{offset:x}] ; [0x{address:08x}] = 0x{bits:08x} ({})",
                single_register_name(sd),
                register_name(rn),
                if up { "" } else { "-" },
                f32::from_bits(bits)
            )
        } else {
            let bits = cpu.fpu_s[sd];
            memory.write_u32(address, bits);
            format!(
                "VSTR {}, [{}, #{}0x{offset:x}] ; [0x{address:08x}] = 0x{bits:08x}",
                single_register_name(sd),
                register_name(rn),
                if up { "" } else { "-" }
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xffe0 == 0xee00 && second & 0x0f7f == 0x0a10 {
        let to_core = first & 0x0010 != 0;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let sn = vn * 2 + n;
        let rt = ((second >> 12) & 0x0f) as usize;
        let note = if to_core {
            let bits = cpu.fpu_s[sn];
            cpu.registers[rt] = bits;
            format!(
                "VMOV {}, {} ; {}=0x{bits:08x}",
                register_name(rt),
                single_register_name(sn),
                register_name(rt)
            )
        } else {
            let bits = cpu.registers[rt];
            cpu.fpu_s[sn] = bits;
            format!(
                "VMOV {}, {} ; {}=0x{bits:08x}",
                single_register_name(sn),
                register_name(rt),
                single_register_name(sn)
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xffbe == 0xeeb4 && matches!(second & 0x0f40, 0x0a40 | 0x0b40) {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let extended = second & 0x0080 != 0;
        let mnemonic = if extended { "VCMPE" } else { "VCMP" };
        let note = if second & 0x0f00 == 0x0a00 {
            let sd = vd * 2 + d;
            let sm = vm * 2 + m;
            let lhs = f32::from_bits(cpu.fpu_s.get(sd).copied().unwrap_or(0));
            let rhs = f32::from_bits(cpu.fpu_s.get(sm).copied().unwrap_or(0));
            set_vfp_compare_flags(cpu, lhs.partial_cmp(&rhs));
            format!(
                "{mnemonic}.F32 {}, {} ; {} vs {}",
                single_register_name(sd),
                single_register_name(sm),
                lhs,
                rhs
            )
        } else {
            let dd = vd + d * 16;
            let dm = vm + m * 16;
            let lhs = f64::from_bits(get_fpu_d(cpu, dd));
            let rhs = f64::from_bits(get_fpu_d(cpu, dm));
            set_vfp_compare_flags(cpu, lhs.partial_cmp(&rhs));
            format!(
                "{mnemonic}.F64 {}, {} ; {} vs {}",
                double_register_name(dd),
                double_register_name(dm),
                lhs,
                rhs
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xffbf == 0xeeb8
        && matches!(second & 0x0f00, 0x0a00 | 0x0b00)
        && second & 0x0040 != 0
    {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let sm = vm * 2 + m;
        let signed = second & 0x0080 != 0;
        let source_bits = cpu.fpu_s.get(sm).copied().unwrap_or(0);
        if second & 0x0f00 == 0x0a00 {
            let sd = vd * 2 + d;
            let value = if signed {
                (source_bits as i32) as f32
            } else {
                source_bits as f32
            };
            if sd < cpu.fpu_s.len() {
                cpu.fpu_s[sd] = value.to_bits();
            }
            return ThumbDecode {
                next_pc: pc + 4,
                opcode32: Some(opcode32),
                note: format!(
                    "VCVT.F32.{} {}, {} ; {}={}",
                    if signed { "S32" } else { "U32" },
                    single_register_name(sd),
                    single_register_name(sm),
                    single_register_name(sd),
                    value
                ),
            };
        }

        let dd = vd + d * 16;
        let value = if signed {
            (source_bits as i32) as f64
        } else {
            source_bits as f64
        };
        set_fpu_d(cpu, dd, value.to_bits());
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VCVT.F64.{} {}, {} ; {}={}",
                if signed { "S32" } else { "U32" },
                double_register_name(dd),
                single_register_name(sm),
                double_register_name(dd),
                value
            ),
        };
    }

    if matches!(first & 0xffbf, 0xeebc | 0xeebd)
        && matches!(second & 0x0f50, 0x0a40 | 0x0b40)
        && second & 0x0080 != 0
    {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let sd = vd * 2 + d;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let unsigned = first & 0xffbf == 0xeebc;
        let double = second & 0x0f00 == 0x0b00;
        let (source_name, value) = if double {
            let dm = vm + m * 16;
            let source = f64::from_bits(get_fpu_d(cpu, dm));
            let value = if unsigned {
                source as u32
            } else {
                (source as i32) as u32
            };
            (double_register_name(dm), value)
        } else {
            let sm = vm * 2 + m;
            let source = f32::from_bits(cpu.fpu_s.get(sm).copied().unwrap_or(0));
            let value = if unsigned {
                source as u32
            } else {
                (source as i32) as u32
            };
            (single_register_name(sm), value)
        };
        if sd < cpu.fpu_s.len() {
            cpu.fpu_s[sd] = value;
        }
        let conversion = if double {
            if unsigned {
                "VCVT.U32.F64"
            } else {
                "VCVT.S32.F64"
            }
        } else if unsigned {
            "VCVT.U32.F32"
        } else {
            "VCVT.S32.F32"
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{conversion} {}, {} ; {}=0x{value:08x}",
                single_register_name(sd),
                source_name,
                single_register_name(sd)
            ),
        };
    }

    if first & 0xffbf == 0xeeb7 && second & 0x0f50 == 0x0a40 && second & 0x0080 != 0 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let dd = vd + d * 16;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let sm = vm * 2 + m;
        let value = f32::from_bits(cpu.fpu_s.get(sm).copied().unwrap_or(0)) as f64;
        set_fpu_d(cpu, dd, value.to_bits());
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VCVT.F64.F32 {}, {} ; {}={}",
                double_register_name(dd),
                single_register_name(sm),
                double_register_name(dd),
                value
            ),
        };
    }

    if first & 0xffbf == 0xeeb7 && second & 0x0f50 == 0x0b40 && second & 0x0080 != 0 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let sd = vd * 2 + d;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let dm = vm + m * 16;
        let value = f64::from_bits(get_fpu_d(cpu, dm)) as f32;
        if sd < cpu.fpu_s.len() {
            cpu.fpu_s[sd] = value.to_bits();
        }
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VCVT.F32.F64 {}, {} ; {}={}",
                single_register_name(sd),
                double_register_name(dm),
                single_register_name(sd),
                value
            ),
        };
    }

    if first & 0xffbf == 0xeeb0 && matches!(second & 0x0fc0, 0x0ac0 | 0x0bc0) {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let note = if second & 0x0f00 == 0x0a00 {
            let sd = vd * 2 + d;
            let sm = vm * 2 + m;
            let source = f32::from_bits(cpu.fpu_s.get(sm).copied().unwrap_or(0));
            let value = source.abs();
            if sd < cpu.fpu_s.len() {
                cpu.fpu_s[sd] = value.to_bits();
            }
            format!(
                "VABS.F32 {}, {} ; {}={value}",
                single_register_name(sd),
                single_register_name(sm),
                single_register_name(sd)
            )
        } else {
            let dd = vd + d * 16;
            let dm = vm + m * 16;
            let source = f64::from_bits(get_fpu_d(cpu, dm));
            let value = source.abs();
            set_fpu_d(cpu, dd, value.to_bits());
            format!(
                "VABS.F64 {}, {} ; {}={value}",
                double_register_name(dd),
                double_register_name(dm),
                double_register_name(dd)
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xffbf == 0xeeb1 && matches!(second & 0x0fc0, 0x0a40 | 0x0b40 | 0x0ac0 | 0x0bc0) {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let sqrt = second & 0x0080 != 0;
        let note = if second & 0x0f00 == 0x0a00 {
            let sd = vd * 2 + d;
            let sm = vm * 2 + m;
            let source = f32::from_bits(cpu.fpu_s.get(sm).copied().unwrap_or(0));
            let value = if sqrt { source.sqrt() } else { -source };
            if sd < cpu.fpu_s.len() {
                cpu.fpu_s[sd] = value.to_bits();
            }
            format!(
                "{}.F32 {}, {} ; {}={value}",
                if sqrt { "VSQRT" } else { "VNEG" },
                single_register_name(sd),
                single_register_name(sm),
                single_register_name(sd)
            )
        } else {
            let dd = vd + d * 16;
            let dm = vm + m * 16;
            let source = f64::from_bits(get_fpu_d(cpu, dm));
            let value = if sqrt { source.sqrt() } else { -source };
            set_fpu_d(cpu, dd, value.to_bits());
            format!(
                "{}.F64 {}, {} ; {}={value}",
                if sqrt { "VSQRT" } else { "VNEG" },
                double_register_name(dd),
                double_register_name(dm),
                double_register_name(dd)
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xffb0 == 0xee30 && second & 0x0f10 == 0x0a00 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let sd = vd * 2 + d;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let sn = vn * 2 + n;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let sm = vm * 2 + m;
        let lhs = f32::from_bits(cpu.fpu_s[sn]);
        let rhs = f32::from_bits(cpu.fpu_s[sm]);
        let subtract = second & 0x0040 != 0;
        let value = if subtract { lhs - rhs } else { lhs + rhs };
        cpu.fpu_s[sd] = value.to_bits();
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, {}, {} ; {}={value}",
                if subtract { "VSUB.F32" } else { "VADD.F32" },
                single_register_name(sd),
                single_register_name(sn),
                single_register_name(sm),
                single_register_name(sd)
            ),
        };
    }

    if first & 0xffb0 == 0xee20 && second & 0x0f10 == 0x0a00 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let sd = vd * 2 + d;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let sn = vn * 2 + n;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let sm = vm * 2 + m;
        let lhs = f32::from_bits(cpu.fpu_s[sn]);
        let rhs = f32::from_bits(cpu.fpu_s[sm]);
        let value = lhs * rhs;
        cpu.fpu_s[sd] = value.to_bits();
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VMUL.F32 {}, {}, {} ; {}={value}",
                single_register_name(sd),
                single_register_name(sn),
                single_register_name(sm),
                single_register_name(sd)
            ),
        };
    }

    if first & 0xffb0 == 0xee80 && second & 0x0f50 == 0x0a00 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let sd = vd * 2 + d;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let sn = vn * 2 + n;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let sm = vm * 2 + m;
        let lhs = f32::from_bits(cpu.fpu_s[sn]);
        let rhs = f32::from_bits(cpu.fpu_s[sm]);
        let value = lhs / rhs;
        cpu.fpu_s[sd] = value.to_bits();
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VDIV.F32 {}, {}, {} ; {}={value}",
                single_register_name(sd),
                single_register_name(sn),
                single_register_name(sm),
                single_register_name(sd)
            ),
        };
    }

    if first & 0xffb0 == 0xeea0 && second & 0x0f10 == 0x0a00 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let sd = vd * 2 + d;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let sn = vn * 2 + n;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let sm = vm * 2 + m;
        let accumulator = f32::from_bits(cpu.fpu_s[sd]);
        let lhs = f32::from_bits(cpu.fpu_s[sn]);
        let rhs = f32::from_bits(cpu.fpu_s[sm]);
        let subtract = second & 0x0040 != 0;
        let product = lhs * rhs;
        let value = if subtract {
            accumulator - product
        } else {
            lhs.mul_add(rhs, accumulator)
        };
        cpu.fpu_s[sd] = value.to_bits();
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{}.F32 {}, {}, {} ; {}={value}",
                if subtract { "VFMS" } else { "VFMA" },
                single_register_name(sd),
                single_register_name(sn),
                single_register_name(sm),
                single_register_name(sd)
            ),
        };
    }

    if first & 0xffb0 == 0xee30 && second & 0x0f10 == 0x0b00 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let dd = vd + d * 16;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let dn = vn + n * 16;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let dm = vm + m * 16;
        let lhs = f64::from_bits(get_fpu_d(cpu, dn));
        let rhs = f64::from_bits(get_fpu_d(cpu, dm));
        let subtract = second & 0x0040 != 0;
        let value = if subtract { lhs - rhs } else { lhs + rhs };
        set_fpu_d(cpu, dd, value.to_bits());
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, {}, {} ; {}={}",
                if subtract { "VSUB.F64" } else { "VADD.F64" },
                double_register_name(dd),
                double_register_name(dn),
                double_register_name(dm),
                double_register_name(dd),
                value
            ),
        };
    }

    if first & 0xffb0 == 0xee20 && second & 0x0f10 == 0x0b00 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let dd = vd + d * 16;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let dn = vn + n * 16;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let dm = vm + m * 16;
        let lhs = f64::from_bits(get_fpu_d(cpu, dn));
        let rhs = f64::from_bits(get_fpu_d(cpu, dm));
        let value = lhs * rhs;
        set_fpu_d(cpu, dd, value.to_bits());
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VMUL.F64 {}, {}, {} ; {}={}",
                double_register_name(dd),
                double_register_name(dn),
                double_register_name(dm),
                double_register_name(dd),
                value
            ),
        };
    }

    if first & 0xffb0 == 0xee80 && second & 0x0f50 == 0x0b00 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let dd = vd + d * 16;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let dn = vn + n * 16;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let dm = vm + m * 16;
        let lhs = f64::from_bits(get_fpu_d(cpu, dn));
        let rhs = f64::from_bits(get_fpu_d(cpu, dm));
        let value = lhs / rhs;
        set_fpu_d(cpu, dd, value.to_bits());
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VDIV.F64 {}, {}, {} ; {}={}",
                double_register_name(dd),
                double_register_name(dn),
                double_register_name(dm),
                double_register_name(dd),
                value
            ),
        };
    }

    if first & 0xffb0 == 0xeea0 && second & 0x0f10 == 0x0b00 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let dd = vd + d * 16;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let dn = vn + n * 16;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let dm = vm + m * 16;
        let accumulator = f64::from_bits(get_fpu_d(cpu, dd));
        let lhs = f64::from_bits(get_fpu_d(cpu, dn));
        let rhs = f64::from_bits(get_fpu_d(cpu, dm));
        let subtract = second & 0x0040 != 0;
        let product = lhs * rhs;
        let value = if subtract {
            accumulator - product
        } else {
            accumulator + product
        };
        set_fpu_d(cpu, dd, value.to_bits());
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "{} {}, {}, {} ; {}={}",
                if subtract { "VFMS.F64" } else { "VFMA.F64" },
                double_register_name(dd),
                double_register_name(dn),
                double_register_name(dm),
                double_register_name(dd),
                value
            ),
        };
    }

    if first & 0xffb0 == 0xee90 && matches!(second & 0x0f10, 0x0a00 | 0x0b00) {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let subtract = second & 0x0040 != 0;
        let operation = if subtract { "VFNMA" } else { "VFNMS" };
        let note = if second & 0x0f00 == 0x0a00 {
            let sd = vd * 2 + d;
            let sn = vn * 2 + n;
            let sm = vm * 2 + m;
            let accumulator = f32::from_bits(cpu.fpu_s[sd]);
            let lhs = f32::from_bits(cpu.fpu_s[sn]);
            let rhs = f32::from_bits(cpu.fpu_s[sm]);
            let product = lhs * rhs;
            let value = if subtract {
                -accumulator - product
            } else {
                product - accumulator
            };
            cpu.fpu_s[sd] = value.to_bits();
            format!(
                "{operation}.F32 {}, {}, {} ; {}={value}",
                single_register_name(sd),
                single_register_name(sn),
                single_register_name(sm),
                single_register_name(sd)
            )
        } else {
            let dd = vd + d * 16;
            let dn = vn + n * 16;
            let dm = vm + m * 16;
            let accumulator = f64::from_bits(get_fpu_d(cpu, dd));
            let lhs = f64::from_bits(get_fpu_d(cpu, dn));
            let rhs = f64::from_bits(get_fpu_d(cpu, dm));
            let product = lhs * rhs;
            let value = if subtract {
                -accumulator - product
            } else {
                product - accumulator
            };
            set_fpu_d(cpu, dd, value.to_bits());
            format!(
                "{operation}.F64 {}, {}, {} ; {}={value}",
                double_register_name(dd),
                double_register_name(dn),
                double_register_name(dm),
                double_register_name(dd)
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xff80 == 0xfe00 && matches!(second & 0x0f10, 0x0a00 | 0x0b00) {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let selector = (first >> 4) & 0x03;
        let (condition_name, take_first) = vsel_condition(cpu.apsr, selector);
        let note = if second & 0x0f00 == 0x0a00 {
            let sd = vd * 2 + d;
            let sn = vn * 2 + n;
            let sm = vm * 2 + m;
            let value = if take_first {
                cpu.fpu_s.get(sn).copied().unwrap_or(0)
            } else {
                cpu.fpu_s.get(sm).copied().unwrap_or(0)
            };
            if sd < cpu.fpu_s.len() {
                cpu.fpu_s[sd] = value;
            }
            format!(
                "VSEL{condition_name}.F32 {}, {}, {} ; {}={}",
                single_register_name(sd),
                single_register_name(sn),
                single_register_name(sm),
                single_register_name(sd),
                f32::from_bits(value)
            )
        } else {
            let dd = vd + d * 16;
            let dn = vn + n * 16;
            let dm = vm + m * 16;
            let value = if take_first {
                get_fpu_d(cpu, dn)
            } else {
                get_fpu_d(cpu, dm)
            };
            set_fpu_d(cpu, dd, value);
            format!(
                "VSEL{condition_name}.F64 {}, {}, {} ; {}={}",
                double_register_name(dd),
                double_register_name(dn),
                double_register_name(dm),
                double_register_name(dd),
                f64::from_bits(value)
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xffb0 == 0xfe80 && matches!(second & 0x0f10, 0x0a00 | 0x0b00) {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let vn = (first & 0x0f) as usize;
        let n = ((second >> 7) & 1) as usize;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let minimum = second & 0x0040 != 0;
        let operation = if minimum { "VMINNM" } else { "VMAXNM" };
        let note = if second & 0x0f00 == 0x0a00 {
            let sd = vd * 2 + d;
            let sn = vn * 2 + n;
            let sm = vm * 2 + m;
            let lhs = f32::from_bits(cpu.fpu_s[sn]);
            let rhs = f32::from_bits(cpu.fpu_s[sm]);
            let value = if minimum {
                min_number_f32(lhs, rhs)
            } else {
                max_number_f32(lhs, rhs)
            };
            cpu.fpu_s[sd] = value.to_bits();
            format!(
                "{operation}.F32 {}, {}, {} ; {}={value}",
                single_register_name(sd),
                single_register_name(sn),
                single_register_name(sm),
                single_register_name(sd)
            )
        } else {
            let dd = vd + d * 16;
            let dn = vn + n * 16;
            let dm = vm + m * 16;
            let lhs = f64::from_bits(get_fpu_d(cpu, dn));
            let rhs = f64::from_bits(get_fpu_d(cpu, dm));
            let value = if minimum {
                min_number_f64(lhs, rhs)
            } else {
                max_number_f64(lhs, rhs)
            };
            set_fpu_d(cpu, dd, value.to_bits());
            format!(
                "{operation}.F64 {}, {}, {} ; {}={value}",
                double_register_name(dd),
                double_register_name(dn),
                double_register_name(dm),
                double_register_name(dd)
            )
        };
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note,
        };
    }

    if first & 0xffbf == 0xeeb0 && second & 0x0fc0 == 0x0a40 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let sd = vd * 2 + d;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let sm = vm * 2 + m;
        let bits = cpu.fpu_s[sm];
        cpu.fpu_s[sd] = bits;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VMOV.F32 {}, {} ; {}=0x{bits:08x}",
                single_register_name(sd),
                single_register_name(sm),
                single_register_name(sd)
            ),
        };
    }

    if first & 0xffbf == 0xeeb0 && second & 0x0fc0 == 0x0b40 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let dd = vd + d * 16;
        let vm = (second & 0x0f) as usize;
        let m = ((second >> 5) & 1) as usize;
        let dm = vm + m * 16;
        let bits = get_fpu_d(cpu, dm);
        set_fpu_d(cpu, dd, bits);
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VMOV.F64 {}, {} ; {}=0x{bits:016x}",
                double_register_name(dd),
                double_register_name(dm),
                double_register_name(dd)
            ),
        };
    }

    if first & 0xffb0 == 0xeeb0 && second & 0x0f00 == 0x0a00 && second & 0x00f0 == 0 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let sd = vd * 2 + d;
        let imm8 = (((first & 0x0f) as u8) << 4) | (second & 0x0f) as u8;
        let bits = vfp_expand_imm_f32_bits(imm8);
        cpu.fpu_s[sd] = bits;
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VMOV.F32 {}, #{} ; bits=0x{bits:08x}",
                single_register_name(sd),
                f32::from_bits(bits)
            ),
        };
    }

    if first & 0xffb0 == 0xeeb0 && second & 0x0f00 == 0x0b00 && second & 0x00f0 == 0 {
        let vd = ((second >> 12) & 0x0f) as usize;
        let d = ((first >> 6) & 1) as usize;
        let dd = vd + d * 16;
        let imm8 = (((first & 0x0f) as u8) << 4) | (second & 0x0f) as u8;
        let bits = vfp_expand_imm_f64_bits(imm8);
        set_fpu_d(cpu, dd, bits);
        return ThumbDecode {
            next_pc: pc + 4,
            opcode32: Some(opcode32),
            note: format!(
                "VMOV.F64 {}, #{} ; bits=0x{bits:016x}",
                double_register_name(dd),
                f64::from_bits(bits)
            ),
        };
    }

    ThumbDecode {
        next_pc: pc + 4,
        opcode32: Some(opcode32),
        note: format!("Thumb32 0x{opcode32:08x} ; decoded semantics not implemented"),
    }
}
