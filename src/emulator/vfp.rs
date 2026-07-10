pub(crate) fn vfp_expand_imm_f32_bits(imm8: u8) -> u32 {
    let sign = ((imm8 as u32) >> 7) << 31;
    let bit6 = ((imm8 as u32) >> 6) & 1;
    let exponent = ((!bit6 & 1) << 7) | (bit6 * 0x7c) | (((imm8 as u32) >> 4) & 0x03);
    let fraction = (imm8 as u32 & 0x0f) << 19;
    sign | (exponent << 23) | fraction
}

pub(crate) fn vfp_expand_imm_f64_bits(imm8: u8) -> u64 {
    let sign = ((imm8 as u64) >> 7) << 63;
    let bit6 = ((imm8 as u64) >> 6) & 1;
    let exponent = ((!bit6 & 1) << 10) | (bit6 * 0x3fc) | (((imm8 as u64) >> 4) & 0x03);
    let fraction = (imm8 as u64 & 0x0f) << 48;
    sign | (exponent << 52) | fraction
}

pub(super) fn max_number_f32(lhs: f32, rhs: f32) -> f32 {
    if lhs.is_nan() {
        rhs
    } else if rhs.is_nan() {
        lhs
    } else {
        lhs.max(rhs)
    }
}

pub(super) fn max_number_f64(lhs: f64, rhs: f64) -> f64 {
    if lhs.is_nan() {
        rhs
    } else if rhs.is_nan() {
        lhs
    } else {
        lhs.max(rhs)
    }
}

pub(super) fn min_number_f32(lhs: f32, rhs: f32) -> f32 {
    if lhs.is_nan() {
        rhs
    } else if rhs.is_nan() {
        lhs
    } else {
        lhs.min(rhs)
    }
}

pub(super) fn min_number_f64(lhs: f64, rhs: f64) -> f64 {
    if lhs.is_nan() {
        rhs
    } else if rhs.is_nan() {
        lhs
    } else {
        lhs.min(rhs)
    }
}
