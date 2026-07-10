use super::super::{ApsrFlags, CpuState};
use super::ThumbDecode;

pub(super) fn thumb32_branch_target(pc: u32, first: u16, second: u16) -> u32 {
    let s = ((first >> 10) & 1) as u32;
    let j1 = ((second >> 13) & 1) as u32;
    let j2 = ((second >> 11) & 1) as u32;
    let i1 = (!(j1 ^ s)) & 1;
    let i2 = (!(j2 ^ s)) & 1;
    let imm10 = (first & 0x03ff) as u32;
    let imm11 = (second & 0x07ff) as u32;
    let imm25 = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1);
    ((pc + 4) as i64 + sign_extend(imm25, 25) as i64) as u32
}

pub(super) fn thumb32_cond_branch_target(pc: u32, first: u16, second: u16) -> u32 {
    let s = ((first >> 10) & 1) as u32;
    let j1 = ((second >> 13) & 1) as u32;
    let j2 = ((second >> 11) & 1) as u32;
    let imm6 = (first & 0x003f) as u32;
    let imm11 = (second & 0x07ff) as u32;
    let imm21 = (s << 20) | (j2 << 19) | (j1 << 18) | (imm6 << 12) | (imm11 << 1);
    ((pc + 4) as i64 + sign_extend(imm21, 21) as i64) as u32
}

pub(crate) fn thumb_expand_imm(imm12: u32) -> u32 {
    let imm8 = imm12 & 0xff;
    if imm12 >> 10 == 0 {
        match (imm12 >> 8) & 0x3 {
            0 => imm8,
            1 => (imm8 << 16) | imm8,
            2 => (imm8 << 24) | (imm8 << 8),
            _ => (imm8 << 24) | (imm8 << 16) | (imm8 << 8) | imm8,
        }
    } else {
        let unrotated = 0x80 | (imm12 & 0x7f);
        unrotated.rotate_right((imm12 >> 7) & 0x1f)
    }
}

pub(super) fn thumb16_alu_decode(
    pc: u32,
    mnemonic: &'static str,
    rdn: usize,
    rm: usize,
    value: u32,
) -> ThumbDecode {
    ThumbDecode {
        next_pc: pc + 2,
        opcode32: None,
        note: format!(
            "{mnemonic} {}, {} ; {}=0x{value:08x}",
            register_name(rdn),
            register_name(rm),
            register_name(rdn)
        ),
    }
}

pub(super) fn shifted_register_value(
    value: u32,
    shift_kind: u16,
    shift: u32,
) -> (&'static str, u32) {
    match shift_kind {
        0 => ("LSL", value.wrapping_shl(shift)),
        1 => (
            "LSR",
            if shift == 0 {
                value
            } else {
                value.wrapping_shr(shift)
            },
        ),
        2 => (
            "ASR",
            if shift == 0 {
                value
            } else {
                ((value as i32) >> shift) as u32
            },
        ),
        _ => (
            "ROR",
            if shift == 0 {
                value
            } else {
                value.rotate_right(shift)
            },
        ),
    }
}

pub(super) fn signed_halfword(value: u32, high: bool) -> i16 {
    let halfword = if high { value >> 16 } else { value } as u16;
    halfword as i16
}

pub(super) fn register_shifted_register_value(
    value: u32,
    shift_kind: u16,
    shift: u32,
) -> (&'static str, u32) {
    match shift_kind {
        0 => (
            "LSL",
            if shift >= 32 {
                0
            } else {
                value.wrapping_shl(shift)
            },
        ),
        1 => (
            "LSR",
            if shift >= 32 {
                0
            } else {
                value.wrapping_shr(shift)
            },
        ),
        2 => (
            "ASR",
            if shift >= 32 {
                if value & 0x8000_0000 != 0 {
                    u32::MAX
                } else {
                    0
                }
            } else {
                ((value as i32) >> shift) as u32
            },
        ),
        _ => {
            let rotate = shift % 32;
            (
                "ROR",
                if rotate == 0 {
                    value
                } else {
                    value.rotate_right(rotate)
                },
            )
        }
    }
}

pub(super) fn is_thumb32_prefix(opcode: u16) -> bool {
    matches!(opcode & 0xf800, 0xe800 | 0xf000 | 0xf800)
}

pub(super) fn sign_extend(value: u32, bits: u8) -> i32 {
    let shift = 32 - bits;
    ((value << shift) as i32) >> shift
}

pub(super) fn align4(value: u32) -> u32 {
    value & !3
}

pub(super) fn thumb32_memory_base(cpu: &CpuState, pc: u32, register: usize) -> u32 {
    if register == 15 {
        align4(pc + 4)
    } else {
        cpu.registers[register]
    }
}

pub(super) fn register_name(index: usize) -> &'static str {
    match index {
        0 => "r0",
        1 => "r1",
        2 => "r2",
        3 => "r3",
        4 => "r4",
        5 => "r5",
        6 => "r6",
        7 => "r7",
        8 => "r8",
        9 => "r9",
        10 => "r10",
        11 => "r11",
        12 => "r12",
        13 => "sp",
        14 => "lr",
        _ => "pc",
    }
}

pub(super) fn single_register_name(index: usize) -> String {
    format!("s{index}")
}

pub(super) fn double_register_name(index: usize) -> String {
    format!("d{index}")
}

pub(crate) fn set_fpu_d(cpu: &mut CpuState, index: usize, bits: u64) {
    let low = (bits & 0xffff_ffff) as u32;
    let high = (bits >> 32) as u32;
    let s = index * 2;
    if s + 1 < cpu.fpu_s.len() {
        cpu.fpu_s[s] = low;
        cpu.fpu_s[s + 1] = high;
    }
}

pub(crate) fn get_fpu_d(cpu: &CpuState, index: usize) -> u64 {
    let s = index * 2;
    if s + 1 < cpu.fpu_s.len() {
        ((cpu.fpu_s[s + 1] as u64) << 32) | cpu.fpu_s[s] as u64
    } else {
        0
    }
}

pub(super) fn vfp_multiple_start_address(base: u32, bytes: u32, pre_index: bool, up: bool) -> u32 {
    match (pre_index, up) {
        (false, true) => base,
        (true, false) => base.wrapping_sub(bytes),
        (true, true) => base.wrapping_add(4),
        (false, false) => base.wrapping_sub(bytes).wrapping_add(4),
    }
}

pub(super) fn vfp_multiple_mnemonic(
    load: bool,
    pre_index: bool,
    up: bool,
    rn: usize,
    single: bool,
) -> &'static str {
    match (load, pre_index, up, rn, single) {
        (false, true, false, 13, false) => "VPUSH",
        (true, false, true, 13, false) => "VPOP",
        (false, true, false, 13, true) => "VPUSH",
        (true, false, true, 13, true) => "VPOP",
        (false, false, true, _, _) => "VSTMIA",
        (true, false, true, _, _) => "VLDMIA",
        (false, true, false, _, _) => "VSTMDB",
        (true, true, false, _, _) => "VLDMDB",
        (false, true, true, _, _) => "VSTMIB",
        (true, true, true, _, _) => "VLDMIB",
        (false, false, false, _, _) => "VSTMDA",
        (true, false, false, _, _) => "VLDMDA",
    }
}

pub(super) fn single_register_list_name(start: usize, count: usize) -> String {
    if count <= 1 {
        single_register_name(start)
    } else {
        format!(
            "{}-{}",
            single_register_name(start),
            single_register_name(start + count - 1)
        )
    }
}

pub(super) fn double_register_list_name(start: usize, count: usize) -> String {
    if count <= 1 {
        double_register_name(start)
    } else {
        format!(
            "{}-{}",
            double_register_name(start),
            double_register_name(start + count - 1)
        )
    }
}

pub(super) fn special_register_name(sysm: u8) -> &'static str {
    match sysm {
        0 => "APSR",
        1 => "IAPSR",
        2 => "EAPSR",
        3 => "XPSR",
        5 => "IPSR",
        6 => "EPSR",
        7 => "IEPSR",
        8 => "MSP",
        9 => "PSP",
        16 => "PRIMASK",
        17 => "BASEPRI",
        18 => "BASEPRI_MAX",
        19 => "FAULTMASK",
        20 => "CONTROL",
        _ => "SYSm",
    }
}

pub(super) fn barrier_option_name(option: u16) -> &'static str {
    match option {
        0x2 => "OSHST",
        0x3 => "OSH",
        0x6 => "NSHST",
        0x7 => "NSH",
        0xa => "ISHST",
        0xb => "ISH",
        0xe => "ST",
        0xf => "SY",
        _ => "reserved",
    }
}

pub(super) fn condition_name(condition: u16) -> &'static str {
    match condition {
        0x0 => "EQ",
        0x1 => "NE",
        0x2 => "CS",
        0x3 => "CC",
        0x4 => "MI",
        0x5 => "PL",
        0x6 => "VS",
        0x7 => "VC",
        0x8 => "HI",
        0x9 => "LS",
        0xa => "GE",
        0xb => "LT",
        0xc => "GT",
        0xd => "LE",
        _ => "",
    }
}

pub(super) fn condition_met(condition: u16, flags: ApsrFlags) -> bool {
    match condition {
        0x0 => flags.z,
        0x1 => !flags.z,
        0x2 => flags.c,
        0x3 => !flags.c,
        0x4 => flags.n,
        0x5 => !flags.n,
        0x6 => flags.v,
        0x7 => !flags.v,
        0x8 => flags.c && !flags.z,
        0x9 => !flags.c || flags.z,
        0xa => flags.n == flags.v,
        0xb => flags.n != flags.v,
        0xc => !flags.z && flags.n == flags.v,
        0xd => flags.z || flags.n != flags.v,
        _ => true,
    }
}

pub(super) fn vsel_condition(flags: ApsrFlags, selector: u16) -> (&'static str, bool) {
    match selector {
        0 => ("EQ", flags.z),
        1 => ("VS", flags.v),
        2 => ("GE", flags.n == flags.v),
        _ => ("GT", !flags.z && flags.n == flags.v),
    }
}

pub(super) fn set_it_conditions(cpu: &mut CpuState, condition: u16, mask: u16) {
    cpu.it_conditions = [0; 4];
    cpu.it_remaining = match mask {
        0x8 => {
            cpu.it_conditions[0] = condition;
            1
        }
        0xc => {
            cpu.it_conditions[0] = condition;
            cpu.it_conditions[1] = inverse_condition(condition);
            2
        }
        0xe => {
            cpu.it_conditions[0] = condition;
            cpu.it_conditions[1] = condition;
            cpu.it_conditions[2] = inverse_condition(condition);
            3
        }
        0xf => {
            cpu.it_conditions = [condition; 4];
            4
        }
        _ => {
            cpu.it_conditions[0] = condition;
            1
        }
    };
}

pub(super) fn take_it_condition(cpu: &mut CpuState) -> Option<u16> {
    if cpu.it_remaining == 0 {
        return None;
    }

    let condition = cpu.it_conditions[0];
    cpu.it_conditions.rotate_left(1);
    cpu.it_conditions[3] = 0;
    cpu.it_remaining -= 1;
    Some(condition)
}

pub(super) fn inverse_condition(condition: u16) -> u16 {
    if condition <= 0x0d {
        condition ^ 1
    } else {
        condition
    }
}

pub(super) fn set_nz_flags(cpu: &mut CpuState, value: u32) {
    cpu.apsr.n = value & 0x8000_0000 != 0;
    cpu.apsr.z = value == 0;
}

pub(super) fn set_nz_flags_if_permitted(cpu: &mut CpuState, value: u32) {
    if !cpu.it_suppresses_flags {
        set_nz_flags(cpu, value);
    }
}

pub(super) fn set_vfp_compare_flags(cpu: &mut CpuState, ordering: Option<std::cmp::Ordering>) {
    let (n, z, c, v) = match ordering {
        Some(std::cmp::Ordering::Greater) => (false, false, true, false),
        Some(std::cmp::Ordering::Equal) => (false, true, true, false),
        Some(std::cmp::Ordering::Less) => (true, false, false, false),
        None => (false, false, true, true),
    };
    cpu.apsr.n = n;
    cpu.apsr.z = z;
    cpu.apsr.c = c;
    cpu.apsr.v = v;
    cpu.fpscr = (cpu.fpscr & !0xf000_0000)
        | ((n as u32) << 31)
        | ((z as u32) << 30)
        | ((c as u32) << 29)
        | ((v as u32) << 28);
}

pub(super) fn apsr_to_xpsr_bits(apsr: ApsrFlags) -> u32 {
    apsr.to_xpsr_bits()
}

pub(super) fn set_add_flags(cpu: &mut CpuState, lhs: u32, rhs: u32, value: u32) {
    set_nz_flags(cpu, value);
    cpu.apsr.c = (lhs as u64 + rhs as u64) > u32::MAX as u64;
    cpu.apsr.v = ((lhs ^ value) & (rhs ^ value) & 0x8000_0000) != 0;
}

pub(super) fn set_add_flags_if_permitted(cpu: &mut CpuState, lhs: u32, rhs: u32, value: u32) {
    if !cpu.it_suppresses_flags {
        set_add_flags(cpu, lhs, rhs, value);
    }
}

pub(super) fn set_sub_flags(cpu: &mut CpuState, lhs: u32, rhs: u32, value: u32) {
    set_nz_flags(cpu, value);
    cpu.apsr.c = lhs >= rhs;
    cpu.apsr.v = ((lhs ^ rhs) & (lhs ^ value) & 0x8000_0000) != 0;
}

pub(super) fn set_sub_flags_if_permitted(cpu: &mut CpuState, lhs: u32, rhs: u32, value: u32) {
    if !cpu.it_suppresses_flags {
        set_sub_flags(cpu, lhs, rhs, value);
    }
}
