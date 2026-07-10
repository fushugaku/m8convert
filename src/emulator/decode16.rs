use super::super::memory::ExecutionMemory;
use super::super::{ApsrFlags, CpuState};
use super::ThumbDecode;
use super::decode32::decode_thumb32;
use super::isa_helpers::*;

pub(crate) fn decode_thumb(
    memory: &mut ExecutionMemory,
    pc: u32,
    opcode: u16,
    cpu: &mut CpuState,
) -> ThumbDecode {
    if opcode == 0xbe00 {
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: "BKPT #0".to_string(),
        };
    }

    if opcode == 0xbf00 {
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: "NOP".to_string(),
        };
    }

    if opcode & 0xff0f == 0xbf00 {
        let hint = match (opcode >> 4) & 0x0f {
            0x1 => "YIELD",
            0x2 => "WFE",
            0x3 => "WFI",
            0x4 => "SEV",
            0x5 => "SEVL",
            value => {
                return ThumbDecode {
                    next_pc: pc + 2,
                    opcode32: None,
                    note: format!("HINT #0x{value:x}"),
                };
            }
        };
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: hint.to_string(),
        };
    }

    if opcode & 0xff00 == 0xbf00 && opcode & 0x000f != 0 {
        let condition = (opcode >> 4) & 0x0f;
        let mask = opcode & 0x0f;
        set_it_conditions(cpu, condition, mask);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "IT {} mask 0x{mask:x} ; {} conditional instruction(s)",
                condition_name(condition),
                cpu.it_remaining
            ),
        };
    }

    if opcode & 0xff87 == 0x4700 {
        let rm = ((opcode >> 3) & 0x0f) as usize;
        let raw_target = cpu.registers[rm];
        if is_exception_return(raw_target)
            && let Some(note) = return_from_exception(memory, cpu, raw_target)
        {
            return ThumbDecode {
                next_pc: cpu.registers[15],
                opcode32: None,
                note,
            };
        }
        let target = raw_target & !1;
        let lr_target = cpu.registers[14] & !1;
        if !memory.has_executable_halfword(target) && memory.has_executable_halfword(lr_target) {
            return ThumbDecode {
                next_pc: lr_target,
                opcode32: None,
                note: format!(
                    "BX {} ; target 0x{target:08x} has no executable loaded code, stubbed external callback, returning to LR=0x{:08x}",
                    register_name(rm),
                    cpu.registers[14]
                ),
            };
        }
        return ThumbDecode {
            next_pc: target,
            opcode32: None,
            note: format!("BX {} ; target 0x{target:08x}", register_name(rm)),
        };
    }

    if opcode & 0xff87 == 0x4780 {
        let rm = ((opcode >> 3) & 0x0f) as usize;
        let target = cpu.registers[rm] & !1;
        cpu.registers[14] = pc + 3;
        if !memory.has_executable_halfword(target) {
            return ThumbDecode {
                next_pc: pc + 2,
                opcode32: None,
                note: format!(
                    "BLX {} ; target 0x{target:08x} has no executable loaded code, stubbed external callback, LR=0x{:08x}",
                    register_name(rm),
                    cpu.registers[14]
                ),
            };
        }
        return ThumbDecode {
            next_pc: target,
            opcode32: None,
            note: format!(
                "BLX {} ; target 0x{target:08x}, LR=0x{:08x}",
                register_name(rm),
                cpu.registers[14]
            ),
        };
    }

    if opcode & 0xf800 == 0x1800 {
        let immediate = opcode & 0x0400 != 0;
        let subtract = opcode & 0x0200 != 0;
        let rn = ((opcode >> 3) & 0x7) as usize;
        let rd = (opcode & 0x7) as usize;
        let operand = if immediate {
            ((opcode >> 6) & 0x7) as u32
        } else {
            cpu.registers[((opcode >> 6) & 0x7) as usize]
        };
        let lhs = cpu.registers[rn];
        let value = if subtract {
            lhs.wrapping_sub(operand)
        } else {
            lhs.wrapping_add(operand)
        };
        cpu.registers[rd] = value;
        if subtract {
            set_sub_flags_if_permitted(cpu, lhs, operand, value);
        } else {
            set_add_flags_if_permitted(cpu, lhs, operand, value);
        }
        let operand_note = if immediate {
            format!("#0x{operand:x}")
        } else {
            register_name(((opcode >> 6) & 0x7) as usize).to_string()
        };
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "{} {}, {}, {} ; {}=0x{value:08x}",
                if subtract { "SUBS" } else { "ADDS" },
                register_name(rd),
                register_name(rn),
                operand_note,
                register_name(rd)
            ),
        };
    }

    if opcode & 0xe000 == 0x0000 {
        let operation = (opcode >> 11) & 0x3;
        let imm5 = ((opcode >> 6) & 0x1f) as u32;
        let rm = ((opcode >> 3) & 0x7) as usize;
        let rd = (opcode & 0x7) as usize;
        let input = cpu.registers[rm];
        let (mnemonic, value, carry) = match operation {
            0 => {
                let value = input.wrapping_shl(imm5);
                let carry = if imm5 == 0 {
                    cpu.apsr.c
                } else {
                    input & (1 << (32 - imm5)) != 0
                };
                ("LSLS", value, carry)
            }
            1 => {
                let shift = if imm5 == 0 { 32 } else { imm5 };
                let value = if shift == 32 { 0 } else { input >> shift };
                let carry = input & (1 << (shift - 1)) != 0;
                ("LSRS", value, carry)
            }
            _ => {
                let shift = if imm5 == 0 { 32 } else { imm5 };
                let value = if shift == 32 {
                    if input & 0x8000_0000 != 0 {
                        u32::MAX
                    } else {
                        0
                    }
                } else {
                    ((input as i32) >> shift) as u32
                };
                let carry = input & (1 << (shift - 1)) != 0;
                ("ASRS", value, carry)
            }
        };
        cpu.registers[rd] = value;
        if !cpu.it_suppresses_flags {
            set_nz_flags(cpu, value);
            cpu.apsr.c = carry;
        }
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "{mnemonic} {}, {}, #{} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                imm5,
                register_name(rd)
            ),
        };
    }

    if opcode & 0xfc00 == 0x4000 {
        let operation = (opcode >> 6) & 0x0f;
        let rm = ((opcode >> 3) & 0x07) as usize;
        let rdn = (opcode & 0x07) as usize;
        let lhs = cpu.registers[rdn];
        let rhs = cpu.registers[rm];
        match operation {
            0x0 => {
                let value = lhs & rhs;
                cpu.registers[rdn] = value;
                set_nz_flags_if_permitted(cpu, value);
                return thumb16_alu_decode(pc, "ANDS", rdn, rm, value);
            }
            0x1 => {
                let value = lhs ^ rhs;
                cpu.registers[rdn] = value;
                set_nz_flags_if_permitted(cpu, value);
                return thumb16_alu_decode(pc, "EORS", rdn, rm, value);
            }
            0x2 => {
                let shift = rhs & 0xff;
                let value = if shift >= 32 { 0 } else { lhs << shift };
                cpu.registers[rdn] = value;
                set_nz_flags_if_permitted(cpu, value);
                return thumb16_alu_decode(pc, "LSLS", rdn, rm, value);
            }
            0x3 => {
                let shift = rhs & 0xff;
                let value = if shift >= 32 { 0 } else { lhs >> shift };
                cpu.registers[rdn] = value;
                set_nz_flags_if_permitted(cpu, value);
                return thumb16_alu_decode(pc, "LSRS", rdn, rm, value);
            }
            0x4 => {
                let shift = rhs & 0xff;
                let value = if shift >= 32 {
                    if lhs & 0x8000_0000 != 0 { u32::MAX } else { 0 }
                } else {
                    ((lhs as i32) >> shift) as u32
                };
                cpu.registers[rdn] = value;
                set_nz_flags_if_permitted(cpu, value);
                return thumb16_alu_decode(pc, "ASRS", rdn, rm, value);
            }
            0x5 => {
                let carry = u32::from(cpu.apsr.c);
                let value = lhs.wrapping_add(rhs).wrapping_add(carry);
                cpu.registers[rdn] = value;
                set_add_flags_if_permitted(cpu, lhs, rhs.wrapping_add(carry), value);
                return thumb16_alu_decode(pc, "ADCS", rdn, rm, value);
            }
            0x6 => {
                let borrow = u32::from(!cpu.apsr.c);
                let value = lhs.wrapping_sub(rhs).wrapping_sub(borrow);
                cpu.registers[rdn] = value;
                set_sub_flags_if_permitted(cpu, lhs, rhs.wrapping_add(borrow), value);
                return thumb16_alu_decode(pc, "SBCS", rdn, rm, value);
            }
            0x7 => {
                let shift = rhs & 0xff;
                let value = if shift == 0 {
                    lhs
                } else {
                    lhs.rotate_right(shift & 0x1f)
                };
                cpu.registers[rdn] = value;
                if !cpu.it_suppresses_flags {
                    set_nz_flags(cpu, value);
                    if shift != 0 {
                        cpu.apsr.c = value & 0x8000_0000 != 0;
                    }
                }
                return thumb16_alu_decode(pc, "RORS", rdn, rm, value);
            }
            0x8 => {
                let value = lhs & rhs;
                set_nz_flags(cpu, value);
                return ThumbDecode {
                    next_pc: pc + 2,
                    opcode32: None,
                    note: format!("TST {}, {}", register_name(rdn), register_name(rm)),
                };
            }
            0xa => {
                let value = lhs.wrapping_sub(rhs);
                set_sub_flags(cpu, lhs, rhs, value);
                return ThumbDecode {
                    next_pc: pc + 2,
                    opcode32: None,
                    note: format!("CMP {}, {}", register_name(rdn), register_name(rm)),
                };
            }
            0x9 => {
                let value = 0u32.wrapping_sub(rhs);
                cpu.registers[rdn] = value;
                set_sub_flags_if_permitted(cpu, 0, rhs, value);
                return ThumbDecode {
                    next_pc: pc + 2,
                    opcode32: None,
                    note: format!(
                        "RSBS {}, {}, #0 ; {}=0x{value:08x}",
                        register_name(rdn),
                        register_name(rm),
                        register_name(rdn)
                    ),
                };
            }
            0xb => {
                let value = lhs.wrapping_add(rhs);
                set_add_flags(cpu, lhs, rhs, value);
                return ThumbDecode {
                    next_pc: pc + 2,
                    opcode32: None,
                    note: format!("CMN {}, {}", register_name(rdn), register_name(rm)),
                };
            }
            0xc => {
                let value = lhs | rhs;
                cpu.registers[rdn] = value;
                set_nz_flags_if_permitted(cpu, value);
                return thumb16_alu_decode(pc, "ORRS", rdn, rm, value);
            }
            0xd => {
                let value = lhs.wrapping_mul(rhs);
                cpu.registers[rdn] = value;
                set_nz_flags_if_permitted(cpu, value);
                return thumb16_alu_decode(pc, "MULS", rdn, rm, value);
            }
            0xe => {
                let value = lhs & !rhs;
                cpu.registers[rdn] = value;
                set_nz_flags_if_permitted(cpu, value);
                return thumb16_alu_decode(pc, "BICS", rdn, rm, value);
            }
            0xf => {
                let value = !rhs;
                cpu.registers[rdn] = value;
                set_nz_flags_if_permitted(cpu, value);
                return thumb16_alu_decode(pc, "MVNS", rdn, rm, value);
            }
            _ => {}
        }
    }

    if opcode & 0xfc00 == 0x4400 {
        let operation = (opcode >> 8) & 0x03;
        let rm = (((opcode >> 3) & 0x07) | (((opcode >> 6) & 1) << 3)) as usize;
        let rdn = ((opcode & 0x07) | (((opcode >> 7) & 1) << 3)) as usize;
        match operation {
            0 => {
                let value = cpu.registers[rdn].wrapping_add(cpu.registers[rm]);
                cpu.registers[rdn] = value;
                return ThumbDecode {
                    next_pc: pc + 2,
                    opcode32: None,
                    note: format!(
                        "ADD {}, {} ; {}=0x{value:08x}",
                        register_name(rdn),
                        register_name(rm),
                        register_name(rdn)
                    ),
                };
            }
            1 => {
                let lhs = cpu.registers[rdn];
                let rhs = cpu.registers[rm];
                let value = lhs.wrapping_sub(rhs);
                set_sub_flags(cpu, lhs, rhs, value);
                return ThumbDecode {
                    next_pc: pc + 2,
                    opcode32: None,
                    note: format!("CMP {}, {}", register_name(rdn), register_name(rm)),
                };
            }
            2 => {
                cpu.registers[rdn] = cpu.registers[rm];
                return ThumbDecode {
                    next_pc: pc + 2,
                    opcode32: None,
                    note: format!("MOV {}, {}", register_name(rdn), register_name(rm)),
                };
            }
            _ => {}
        }
    }

    if opcode & 0xf800 == 0x4800 {
        let rt = ((opcode >> 8) & 0x7) as usize;
        let literal_address = align4(pc + 4) + ((opcode & 0xff) as u32) * 4;
        let value = memory.read_u32_or_zero(literal_address);
        cpu.registers[rt] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "LDR {}, [PC, #0x{:x}] ; [0x{literal_address:08x}] = 0x{value:08x}",
                register_name(rt),
                ((opcode & 0xff) as u32) * 4
            ),
        };
    }

    if opcode & 0xf800 == 0x6000 {
        let imm5 = ((opcode >> 6) & 0x1f) as u32;
        let rn = ((opcode >> 3) & 0x7) as usize;
        let rt = (opcode & 0x7) as usize;
        let address = cpu.registers[rn].wrapping_add(imm5 * 4);
        memory.write_u32(address, cpu.registers[rt]);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "STR {}, [{}, #0x{:x}] ; [0x{:08x}] = 0x{:08x}",
                register_name(rt),
                register_name(rn),
                imm5 * 4,
                address,
                cpu.registers[rt]
            ),
        };
    }

    if opcode & 0xf800 == 0x6800 {
        let imm5 = ((opcode >> 6) & 0x1f) as u32;
        let rn = ((opcode >> 3) & 0x7) as usize;
        let rt = (opcode & 0x7) as usize;
        let address = cpu.registers[rn].wrapping_add(imm5 * 4);
        let mut sanitize_note = String::new();
        if next_instructions_tail_call_through_loaded_pointer(memory, pc, rt)
            && let Some(sanitized) =
                memory.sanitize_virtual_dispatch_receiver_base(address, cpu.registers[rn])
        {
            cpu.registers[rt] = sanitized;
            return ThumbDecode {
                next_pc: cpu.registers[14] & !1,
                opcode32: None,
                note: format!(
                    "LDR {}, [{}, #0x{:x}] ; [0x{address:08x}] = 0x{sanitized:08x} ; virtual dispatch receiver base sanitized from 0x{:08x}, returning to LR=0x{:08x}",
                    register_name(rt),
                    register_name(rn),
                    imm5 * 4,
                    cpu.registers[rn],
                    cpu.registers[14]
                ),
            };
        }
        let raw_value = if let Some(sanitized) =
            memory.sanitize_display_cell_effective_address(cpu.registers[rn], imm5 * 4, address)
        {
            sanitize_note = format!(
                " ; effective address base sanitized from 0x{:08x}",
                cpu.registers[rn]
            );
            sanitized
        } else {
            memory.read_u32_or_zero(address)
        };
        let mut value = raw_value;
        if next_instructions_tail_call_through_loaded_pointer(memory, pc, rt)
            && let Some(sanitized) = memory.sanitize_virtual_dispatch_receiver(address, raw_value)
        {
            cpu.registers[rt] = sanitized;
            return ThumbDecode {
                next_pc: cpu.registers[14] & !1,
                opcode32: None,
                note: format!(
                    "LDR {}, [{}, #0x{:x}] ; [0x{address:08x}] = 0x{sanitized:08x} ; virtual dispatch receiver sanitized from 0x{raw_value:08x}, returning to LR=0x{:08x}",
                    register_name(rt),
                    register_name(rn),
                    imm5 * 4,
                    cpu.registers[14]
                ),
            };
        }
        if next_instruction_checks_register_for_null(memory, pc, rt)
            && let Some(sanitized) = memory.sanitize_optional_pointer_load(address, raw_value)
        {
            value = sanitized;
            sanitize_note = format!(" ; optional pointer sanitized from 0x{raw_value:08x}");
        }
        cpu.registers[rt] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "LDR {}, [{}, #0x{:x}] ; [0x{address:08x}] = 0x{value:08x}{sanitize_note}",
                register_name(rt),
                register_name(rn),
                imm5 * 4
            ),
        };
    }

    if opcode & 0xf800 == 0x7000 {
        let imm5 = ((opcode >> 6) & 0x1f) as u32;
        let rn = ((opcode >> 3) & 0x7) as usize;
        let rt = (opcode & 0x7) as usize;
        let address = cpu.registers[rn].wrapping_add(imm5);
        let value = cpu.registers[rt] as u8;
        memory.write_u8(address, value);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "STRB {}, [{}, #0x{imm5:x}] ; [0x{address:08x}] = 0x{value:02x}",
                register_name(rt),
                register_name(rn)
            ),
        };
    }

    if opcode & 0xf800 == 0x7800 {
        let imm5 = ((opcode >> 6) & 0x1f) as u32;
        let rn = ((opcode >> 3) & 0x7) as usize;
        let rt = (opcode & 0x7) as usize;
        let address = cpu.registers[rn].wrapping_add(imm5);
        let value = memory.read_u8_or_zero(address) as u32;
        cpu.registers[rt] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "LDRB {}, [{}, #0x{imm5:x}] ; [0x{address:08x}] = 0x{value:02x}",
                register_name(rt),
                register_name(rn)
            ),
        };
    }

    if opcode & 0xf800 == 0x8000 {
        let imm5 = ((opcode >> 6) & 0x1f) as u32;
        let rn = ((opcode >> 3) & 0x7) as usize;
        let rt = (opcode & 0x7) as usize;
        let offset = imm5 * 2;
        let address = cpu.registers[rn].wrapping_add(offset);
        let value = cpu.registers[rt] as u16;
        memory.write_u16(address, value);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "STRH {}, [{}, #0x{offset:x}] ; [0x{address:08x}] = 0x{value:04x}",
                register_name(rt),
                register_name(rn)
            ),
        };
    }

    if opcode & 0xf800 == 0x8800 {
        let imm5 = ((opcode >> 6) & 0x1f) as u32;
        let rn = ((opcode >> 3) & 0x7) as usize;
        let rt = (opcode & 0x7) as usize;
        let offset = imm5 * 2;
        let address = cpu.registers[rn].wrapping_add(offset);
        let value = memory.read_u16_or_zero(address) as u32;
        cpu.registers[rt] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "LDRH {}, [{}, #0x{offset:x}] ; [0x{address:08x}] = 0x{value:04x}",
                register_name(rt),
                register_name(rn)
            ),
        };
    }

    if opcode & 0xf000 == 0xa000 {
        let use_sp = opcode & 0x0800 != 0;
        let rd = ((opcode >> 8) & 0x7) as usize;
        let immediate = ((opcode & 0xff) as u32) * 4;
        let base = if use_sp {
            cpu.registers[13]
        } else {
            (pc + 4) & !3
        };
        let value = base.wrapping_add(immediate);
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "{} {}, {}, #0x{immediate:x} ; {}=0x{value:08x}",
                if use_sp { "ADD" } else { "ADR" },
                register_name(rd),
                if use_sp { "sp" } else { "pc" },
                register_name(rd)
            ),
        };
    }

    if opcode & 0xf000 == 0x9000 {
        let load = opcode & 0x0800 != 0;
        let rt = ((opcode >> 8) & 0x7) as usize;
        let immediate = ((opcode & 0xff) as u32) * 4;
        let address = cpu.registers[13].wrapping_add(immediate);
        let note = if load {
            let value = memory.read_u32_or_zero(address);
            cpu.registers[rt] = value;
            format!(
                "LDR {}, [sp, #0x{immediate:x}] ; [0x{address:08x}] = 0x{value:08x}",
                register_name(rt)
            )
        } else {
            let value = cpu.registers[rt];
            memory.write_u32(address, value);
            format!(
                "STR {}, [sp, #0x{immediate:x}] ; [0x{address:08x}] = 0x{value:08x}",
                register_name(rt)
            )
        };
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note,
        };
    }

    if opcode & 0xff00 == 0x4600 {
        let dst = (((opcode >> 4) & 0x8) | (opcode & 0x7)) as usize;
        let src = ((opcode >> 3) & 0x0f) as usize;
        cpu.registers[dst] = cpu.registers[src];
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!("MOV {}, {}", register_name(dst), register_name(src)),
        };
    }

    if opcode & 0xf800 == 0x2000 {
        let rd = ((opcode >> 8) & 0x7) as usize;
        let imm8 = (opcode & 0xff) as u32;
        cpu.registers[rd] = imm8;
        set_nz_flags_if_permitted(cpu, imm8);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!("MOVS {}, #0x{imm8:x}", register_name(rd)),
        };
    }

    if opcode & 0xf800 == 0x3000 {
        let rd = ((opcode >> 8) & 0x7) as usize;
        let imm8 = (opcode & 0xff) as u32;
        let lhs = cpu.registers[rd];
        let value = lhs.wrapping_add(imm8);
        cpu.registers[rd] = value;
        set_add_flags_if_permitted(cpu, lhs, imm8, value);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "ADDS {}, #0x{imm8:x} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rd)
            ),
        };
    }

    if opcode & 0xf800 == 0x3800 {
        let rd = ((opcode >> 8) & 0x7) as usize;
        let imm8 = (opcode & 0xff) as u32;
        let lhs = cpu.registers[rd];
        let value = lhs.wrapping_sub(imm8);
        cpu.registers[rd] = value;
        set_sub_flags_if_permitted(cpu, lhs, imm8, value);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "SUBS {}, #0x{imm8:x} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rd)
            ),
        };
    }

    if opcode & 0xf000 == 0x5000 {
        let operation = (opcode >> 9) & 0x07;
        let rm = ((opcode >> 6) & 0x7) as usize;
        let rn = ((opcode >> 3) & 0x7) as usize;
        let rt = (opcode & 0x7) as usize;
        let address = cpu.registers[rn].wrapping_add(cpu.registers[rm]);
        let note = match operation {
            0 => {
                let value = cpu.registers[rt];
                memory.write_u32(address, value);
                format!(
                    "STR {}, [{}, {}] ; [0x{address:08x}] = 0x{value:08x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm)
                )
            }
            1 => {
                let value = cpu.registers[rt] as u16;
                memory.write_u16(address, value);
                format!(
                    "STRH {}, [{}, {}] ; [0x{address:08x}] = 0x{value:04x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm)
                )
            }
            2 => {
                let value = cpu.registers[rt] as u8;
                memory.write_u8(address, value);
                format!(
                    "STRB {}, [{}, {}] ; [0x{address:08x}] = 0x{value:02x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm)
                )
            }
            3 => {
                let value = ((memory.read_u8_or_zero(address) as i8) as i32) as u32;
                cpu.registers[rt] = value;
                format!(
                    "LDRSB {}, [{}, {}] ; [0x{address:08x}] = 0x{value:08x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm)
                )
            }
            4 => {
                let (value, sanitize_note) = if let Some(sanitized) = memory
                    .sanitize_display_cell_effective_address(
                        cpu.registers[rn],
                        cpu.registers[rm],
                        address,
                    ) {
                    (
                        sanitized,
                        format!(
                            " ; effective address base sanitized from 0x{:08x}",
                            cpu.registers[rn]
                        ),
                    )
                } else {
                    (memory.read_u32_or_zero(address), String::new())
                };
                cpu.registers[rt] = value;
                format!(
                    "LDR {}, [{}, {}] ; [0x{address:08x}] = 0x{value:08x}{sanitize_note}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm)
                )
            }
            5 => {
                let value = memory.read_u16_or_zero(address) as u32;
                cpu.registers[rt] = value;
                format!(
                    "LDRH {}, [{}, {}] ; [0x{address:08x}] = 0x{value:04x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm)
                )
            }
            6 => {
                let value = memory.read_u8_or_zero(address) as u32;
                cpu.registers[rt] = value;
                format!(
                    "LDRB {}, [{}, {}] ; [0x{address:08x}] = 0x{value:02x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm)
                )
            }
            _ => {
                let value = ((memory.read_u16_or_zero(address) as i16) as i32) as u32;
                cpu.registers[rt] = value;
                format!(
                    "LDRSH {}, [{}, {}] ; [0x{address:08x}] = 0x{value:08x}",
                    register_name(rt),
                    register_name(rn),
                    register_name(rm)
                )
            }
        };
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note,
        };
    }

    if opcode & 0xf000 == 0xc000 {
        let load = opcode & 0x0800 != 0;
        let rn = ((opcode >> 8) & 0x07) as usize;
        let register_list = opcode & 0x00ff;
        let base = cpu.registers[rn];
        let mut address = base;
        for register in 0..8 {
            if register_list & (1 << register) != 0 {
                if load {
                    cpu.registers[register] = memory.read_u32_or_zero(address);
                } else {
                    memory.write_u32(address, cpu.registers[register]);
                }
                address = address.wrapping_add(4);
            }
        }
        cpu.registers[rn] = address;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "{} {}!, register-list 0x{register_list:02x} ; start=0x{base:08x}, {}=0x{address:08x}",
                if load { "LDMIA" } else { "STMIA" },
                register_name(rn),
                register_name(rn)
            ),
        };
    }

    if opcode & 0xf800 == 0x2800 {
        let rn = ((opcode >> 8) & 0x7) as usize;
        let imm8 = (opcode & 0xff) as u32;
        let lhs = cpu.registers[rn];
        let value = lhs.wrapping_sub(imm8);
        set_sub_flags(cpu, lhs, imm8, value);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!("CMP {}, #0x{imm8:x}", register_name(rn)),
        };
    }

    if opcode & 0xffc0 == 0x4280 {
        let rm = ((opcode >> 3) & 0x7) as usize;
        let rn = (opcode & 0x7) as usize;
        let lhs = cpu.registers[rn];
        let rhs = cpu.registers[rm];
        let value = lhs.wrapping_sub(rhs);
        set_sub_flags(cpu, lhs, rhs, value);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!("CMP {}, {}", register_name(rn), register_name(rm)),
        };
    }

    if opcode & 0xf000 == 0xd000 && opcode & 0x0f00 != 0x0f00 {
        let condition = (opcode >> 8) & 0x0f;
        let offset = sign_extend(((opcode & 0xff) as u32) << 1, 9);
        let target = ((pc + 4) as i64 + offset as i64) as u32;
        let taken = condition_met(condition, cpu.apsr);
        return ThumbDecode {
            next_pc: if taken { target } else { pc + 2 },
            opcode32: None,
            note: format!(
                "B{} 0x{target:08x} ; {}",
                condition_name(condition),
                if taken { "taken" } else { "not taken" }
            ),
        };
    }

    if opcode & 0xf800 == 0xe000 {
        let offset = sign_extend(((opcode & 0x07ff) as u32) << 1, 12);
        let target = ((pc + 4) as i64 + offset as i64) as u32;
        return ThumbDecode {
            next_pc: target,
            opcode32: None,
            note: format!("B 0x{target:08x}"),
        };
    }

    if opcode & 0xffe8 == 0xb660 || opcode & 0xffe8 == 0xb670 {
        let disable = opcode & 0x0010 != 0;
        let mut masks = Vec::new();
        if opcode & 0x0004 != 0 {
            masks.push("A");
        }
        if opcode & 0x0002 != 0 {
            masks.push("I");
        }
        if opcode & 0x0001 != 0 {
            masks.push("F");
        }
        if opcode & 0x0002 != 0 {
            cpu.primask = u32::from(disable);
        }
        if opcode & 0x0001 != 0 {
            cpu.faultmask = u32::from(disable);
        }
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "{} {} ; PRIMASK={}, FAULTMASK={}",
                if disable { "CPSID" } else { "CPSIE" },
                if masks.is_empty() {
                    "-".to_string()
                } else {
                    masks.join("")
                },
                cpu.primask,
                cpu.faultmask
            ),
        };
    }

    if opcode & 0xf500 == 0xb100 {
        let rn = (opcode & 0x0007) as usize;
        let nonzero = opcode & 0x0800 != 0;
        let offset = ((((opcode >> 3) & 0x1f) as u32) << 1) | ((((opcode >> 9) & 1) as u32) << 6);
        let target = pc.wrapping_add(4).wrapping_add(offset);
        let value = cpu.registers[rn];
        let taken = if nonzero { value != 0 } else { value == 0 };
        return ThumbDecode {
            next_pc: if taken { target } else { pc + 2 },
            opcode32: None,
            note: format!(
                "{} {}, 0x{target:08x} ; {}",
                if nonzero { "CBNZ" } else { "CBZ" },
                register_name(rn),
                if taken { "taken" } else { "not taken" }
            ),
        };
    }

    if opcode & 0xfe00 == 0xb400 {
        let register_list = opcode & 0xff;
        let has_lr = opcode & 0x0100 != 0;
        let register_count = register_list.count_ones() + u32::from(has_lr);
        let new_sp = cpu.registers[13].wrapping_sub(register_count * 4);
        let mut address = new_sp;
        for register in 0..8 {
            if register_list & (1 << register) != 0 {
                memory.write_u32(address, cpu.registers[register]);
                address = address.wrapping_add(4);
            }
        }
        if has_lr {
            memory.write_u32(address, cpu.registers[14]);
        }
        cpu.registers[13] = new_sp;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "PUSH register-list 0x{register_list:02x}{} ; SP=0x{:08x}",
                if has_lr { " + LR" } else { "" },
                cpu.registers[13]
            ),
        };
    }

    if opcode & 0xff80 == 0xb000 {
        let immediate = ((opcode & 0x7f) as u32) * 4;
        cpu.registers[13] = cpu.registers[13].wrapping_add(immediate);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!("ADD sp, #0x{immediate:x} ; SP=0x{:08x}", cpu.registers[13]),
        };
    }

    if opcode & 0xff80 == 0xb080 {
        let immediate = ((opcode & 0x7f) as u32) * 4;
        cpu.registers[13] = cpu.registers[13].wrapping_sub(immediate);
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!("SUB sp, #0x{immediate:x} ; SP=0x{:08x}", cpu.registers[13]),
        };
    }

    if opcode & 0xffc0 == 0xb200 {
        let rm = ((opcode >> 3) & 0x07) as usize;
        let rd = (opcode & 0x07) as usize;
        let value = ((cpu.registers[rm] as u16) as i16 as i32) as u32;
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "SXTH {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if opcode & 0xffc0 == 0xb240 {
        let rm = ((opcode >> 3) & 0x07) as usize;
        let rd = (opcode & 0x07) as usize;
        let value = ((cpu.registers[rm] as u8) as i8 as i32) as u32;
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "SXTB {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if opcode & 0xffc0 == 0xb280 {
        let rm = ((opcode >> 3) & 0x07) as usize;
        let rd = (opcode & 0x07) as usize;
        let value = cpu.registers[rm] & 0x0000_ffff;
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "UXTH {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if opcode & 0xffc0 == 0xb2c0 {
        let rm = ((opcode >> 3) & 0x07) as usize;
        let rd = (opcode & 0x07) as usize;
        let value = cpu.registers[rm] & 0x0000_00ff;
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "UXTB {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if opcode & 0xffc0 == 0xba00 {
        let rm = ((opcode >> 3) & 0x07) as usize;
        let rd = (opcode & 0x07) as usize;
        let value = cpu.registers[rm].swap_bytes();
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "REV {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if opcode & 0xffc0 == 0xba40 {
        let rm = ((opcode >> 3) & 0x07) as usize;
        let rd = (opcode & 0x07) as usize;
        let input = cpu.registers[rm];
        let value = ((input & 0x00ff_00ff) << 8) | ((input & 0xff00_ff00) >> 8);
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "REV16 {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if opcode & 0xffc0 == 0xbac0 {
        let rm = ((opcode >> 3) & 0x07) as usize;
        let rd = (opcode & 0x07) as usize;
        let input = cpu.registers[rm];
        let reversed = ((input & 0x00ff) << 8) | ((input & 0xff00) >> 8);
        let value = (reversed as u16 as i16 as i32) as u32;
        cpu.registers[rd] = value;
        return ThumbDecode {
            next_pc: pc + 2,
            opcode32: None,
            note: format!(
                "REVSH {}, {} ; {}=0x{value:08x}",
                register_name(rd),
                register_name(rm),
                register_name(rd)
            ),
        };
    }

    if opcode & 0xfe00 == 0xbc00 {
        let register_list = opcode & 0xff;
        let has_pc = opcode & 0x0100 != 0;
        let register_count = register_list.count_ones() + u32::from(has_pc);
        let mut next_pc = pc + 2;
        let mut address = cpu.registers[13];
        let mut pc_note = String::new();
        for register in 0..8 {
            if register_list & (1 << register) != 0 {
                cpu.registers[register] = memory.read_u32_or_zero(address);
                address = address.wrapping_add(4);
            }
        }
        if has_pc {
            let pc_address = address;
            let pc_value = memory.read_u32_or_zero(pc_address);
            if is_exception_return(pc_value) {
                let restored_sp = cpu.registers[13].wrapping_add(register_count * 4);
                cpu.registers[13] = restored_sp;
                if let Some(note) = return_from_exception(memory, cpu, pc_value) {
                    return ThumbDecode {
                        next_pc: cpu.registers[15],
                        opcode32: None,
                        note,
                    };
                }
                cpu.registers[13] = cpu.registers[13].wrapping_sub(register_count * 4);
            }
            next_pc = pc_value & !1;
            pc_note = format!(" + PC ; PC<-[0x{pc_address:08x}]=0x{pc_value:08x}");
        }
        cpu.registers[13] = cpu.registers[13].wrapping_add(register_count * 4);
        return ThumbDecode {
            next_pc,
            opcode32: None,
            note: format!(
                "POP register-list 0x{register_list:02x}{} ; SP=0x{:08x}",
                pc_note, cpu.registers[13]
            ),
        };
    }

    if is_thumb32_prefix(opcode)
        && let Some(second) = memory.read_u16(pc + 2)
    {
        return decode_thumb32(memory, pc, opcode, second, cpu);
    }

    ThumbDecode {
        next_pc: pc + 2,
        opcode32: None,
        note: format!("Thumb16 0x{opcode:04x} ; decoded semantics not implemented"),
    }
}

fn next_instruction_checks_register_for_null(
    memory: &ExecutionMemory,
    pc: u32,
    register: usize,
) -> bool {
    let Some(next) = memory.read_u16(pc + 2) else {
        return false;
    };
    next & 0xf500 == 0xb100 && (next & 0x0007) as usize == register
}

fn next_instructions_tail_call_through_loaded_pointer(
    memory: &ExecutionMemory,
    pc: u32,
    register: usize,
) -> bool {
    let Some(load_vtable_slot) = memory.read_u16(pc + 2) else {
        return false;
    };
    let Some(branch) = memory.read_u16(pc + 4) else {
        return false;
    };
    thumb16_ldr_uses_register_as_base_and_target(load_vtable_slot, register)
        && thumb16_branch_uses_register(branch, register)
}

fn thumb16_ldr_uses_register_as_base_and_target(opcode: u16, register: usize) -> bool {
    opcode & 0xf800 == 0x6800
        && ((opcode >> 3) & 0x7) as usize == register
        && (opcode & 0x7) as usize == register
}

fn thumb16_branch_uses_register(opcode: u16, register: usize) -> bool {
    matches!(opcode & 0xff87, 0x4700 | 0x4780) && ((opcode >> 3) & 0x0f) as usize == register
}

fn is_exception_return(value: u32) -> bool {
    value & 0xffff_ff00 == 0xffff_ff00
        && matches!(value & 0x1f, 0x01 | 0x09 | 0x0d | 0x11 | 0x19 | 0x1d)
}

fn return_from_exception(
    memory: &mut ExecutionMemory,
    cpu: &mut CpuState,
    exc_return: u32,
) -> Option<String> {
    let exception = cpu.active_exception?;
    let sp = cpu.registers[13];
    let r0 = memory.read_u32(sp)?;
    let r1 = memory.read_u32(sp + 4)?;
    let r2 = memory.read_u32(sp + 8)?;
    let r3 = memory.read_u32(sp + 12)?;
    let r12 = memory.read_u32(sp + 16)?;
    let lr = memory.read_u32(sp + 20)?;
    let pc = memory.read_u32(sp + 24)?;
    let xpsr = memory.read_u32(sp + 28)?;

    cpu.registers[0] = r0;
    cpu.registers[1] = r1;
    cpu.registers[2] = r2;
    cpu.registers[3] = r3;
    cpu.registers[12] = r12;
    cpu.registers[13] = sp + 32;
    cpu.registers[14] = lr;
    cpu.registers[15] = pc & !1;
    cpu.xpsr = xpsr | 0x0100_0000;
    cpu.apsr = ApsrFlags::from_xpsr_bits(xpsr);
    cpu.exception_depth = cpu.exception_depth.saturating_sub(1);
    cpu.active_exception = None;
    memory.finish_exception(exception);

    Some(format!(
        "Exception return 0x{exc_return:08x} ; exception {exception}, PC=0x{:08x}, LR=0x{lr:08x}, SP=0x{:08x}",
        cpu.registers[15], cpu.registers[13]
    ))
}
