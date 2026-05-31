use serde::{Deserialize, Serialize};
use thiserror::Error;

const S3M_MAGIC_OFFSET: usize = 0x2c;
const S3M_HEADER_LEN: usize = 0x60;
const S3M_CHANNELS: usize = 32;
const S3M_ROWS: usize = 64;

#[derive(Debug, Error)]
pub enum S3mError {
    #[error("file is too short to be an S3M module")]
    TooShort,
    #[error("missing SCRM signature at offset 0x2c")]
    MissingSignature,
    #[error("invalid S3M type byte {0:#04x}")]
    InvalidType(u8),
    #[error("S3M data is truncated while reading {0}")]
    Truncated(&'static str),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3mModule {
    pub title: String,
    pub orders: Vec<u8>,
    pub active_channels: Vec<usize>,
    pub channel_pans: Vec<u8>,
    pub instruments: Vec<S3mInstrument>,
    pub patterns: Vec<S3mPattern>,
    pub initial_speed: u8,
    pub initial_tempo: u8,
    pub global_volume: u8,
    pub tracker_version: u16,
    pub ffi: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3mInstrument {
    pub kind: u8,
    pub name: String,
    pub length: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub volume: u8,
    pub flags: u8,
    pub c5_speed: u32,
    pub pack: u8,
    pub data: Vec<i16>,
}

impl S3mInstrument {
    pub fn is_pcm(&self) -> bool {
        self.kind == 1 && !self.data.is_empty()
    }

    pub fn is_looped(&self) -> bool {
        self.flags & 0x01 != 0 && self.loop_end > self.loop_start
    }

    pub fn is_16bit(&self) -> bool {
        self.flags & 0x04 != 0
    }

    pub fn is_stereo(&self) -> bool {
        self.flags & 0x02 != 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3mPattern {
    pub rows: Vec<Vec<S3mCell>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct S3mCell {
    pub note: u8,
    pub instrument: u8,
    pub volume: u8,
    pub command: u8,
    pub info: u8,
}

impl S3mCell {
    pub const EMPTY: Self = Self {
        note: 0xff,
        instrument: 0,
        volume: 0xff,
        command: 0,
        info: 0,
    };

    pub fn has_note(self) -> bool {
        self.note != 0xff && self.note != 0x00
    }
}

#[derive(Debug, Clone)]
struct RawInstrument {
    kind: u8,
    name: String,
    sample_offset: usize,
    length: u32,
    loop_start: u32,
    loop_end: u32,
    volume: u8,
    pack: u8,
    flags: u8,
    c5_speed: u32,
}

pub fn is_s3m(input: &[u8]) -> bool {
    input.len() >= S3M_MAGIC_OFFSET + 4 && &input[S3M_MAGIC_OFFSET..S3M_MAGIC_OFFSET + 4] == b"SCRM"
}

pub fn parse_s3m(input: &[u8]) -> Result<S3mModule, S3mError> {
    if input.len() < S3M_HEADER_LEN {
        return Err(S3mError::TooShort);
    }
    if !is_s3m(input) {
        return Err(S3mError::MissingSignature);
    }
    if input[0x1d] != 0x10 {
        return Err(S3mError::InvalidType(input[0x1d]));
    }

    let title = decode_fixed_string(&input[0x00..0x1c]);
    let order_count = read_u16(input, 0x20)? as usize;
    let instrument_count = read_u16(input, 0x22)? as usize;
    let pattern_count = read_u16(input, 0x24)? as usize;
    let tracker_version = read_u16(input, 0x28)?;
    let ffi = read_u16(input, 0x2a)?;
    let global_volume = input[0x30];
    let initial_speed = input[0x31];
    let initial_tempo = input[0x32];

    let channel_settings = input
        .get(0x40..0x40 + S3M_CHANNELS)
        .ok_or(S3mError::Truncated("channel settings"))?;
    let active_channels: Vec<usize> = channel_settings
        .iter()
        .enumerate()
        .filter_map(|(index, setting)| (*setting != 0xff && *setting < 16).then_some(index))
        .collect();

    let mut pos = S3M_HEADER_LEN;
    let orders = input
        .get(pos..pos + order_count)
        .ok_or(S3mError::Truncated("order table"))?
        .iter()
        .copied()
        .take_while(|order| *order != 0xff)
        .filter(|order| *order != 0xfe)
        .collect::<Vec<_>>();
    pos += order_count;

    let instrument_paras =
        read_para_table(input, &mut pos, instrument_count, "instrument pointers")?;
    let pattern_paras = read_para_table(input, &mut pos, pattern_count, "pattern pointers")?;
    let channel_pans = read_channel_pans(input, &mut pos, input[0x35] == 0xfc, channel_settings)?;

    let signed_samples = ffi == 1;
    let instruments = instrument_paras
        .iter()
        .map(|para| parse_instrument(input, *para as usize * 16, signed_samples))
        .collect::<Result<Vec<_>, _>>()?;

    let patterns = pattern_paras
        .iter()
        .map(|para| {
            if *para == 0 {
                Ok(empty_pattern())
            } else {
                parse_pattern(input, *para as usize * 16)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(S3mModule {
        title,
        orders,
        active_channels,
        channel_pans,
        instruments,
        patterns,
        initial_speed,
        initial_tempo,
        global_volume,
        tracker_version,
        ffi,
    })
}

fn read_channel_pans(
    input: &[u8],
    pos: &mut usize,
    has_default_pans: bool,
    channel_settings: &[u8],
) -> Result<Vec<u8>, S3mError> {
    if has_default_pans {
        let bytes = input
            .get(*pos..*pos + S3M_CHANNELS)
            .ok_or(S3mError::Truncated("default channel panning"))?;
        *pos += S3M_CHANNELS;
        Ok(bytes
            .iter()
            .enumerate()
            .map(|(channel, value)| {
                if value & 0x20 != 0 {
                    s3m_pan_nibble_to_m8(value & 0x0f)
                } else {
                    default_channel_pan(channel_settings[channel])
                }
            })
            .collect())
    } else {
        Ok(channel_settings
            .iter()
            .copied()
            .map(default_channel_pan)
            .collect())
    }
}

fn default_channel_pan(channel_setting: u8) -> u8 {
    if channel_setting < 8 {
        s3m_pan_nibble_to_m8(3)
    } else if channel_setting < 16 {
        s3m_pan_nibble_to_m8(12)
    } else {
        0x80
    }
}

pub fn s3m_pan_nibble_to_m8(value: u8) -> u8 {
    let value = value.min(0x0f);
    if value == 0x08 {
        0x80
    } else {
        ((value as u16 * 255 + 7) / 15) as u8
    }
}

pub fn s3m_note_to_m8(note: u8) -> Option<u8> {
    match note {
        0x00 | 0xff => None,
        0xfe => Some(0x80),
        value => {
            let octave = value >> 4;
            let semitone = value & 0x0f;
            if semitone > 11 {
                None
            } else {
                Some(((octave as i16 - 1) * 12 + semitone as i16).clamp(0, 0x7f) as u8)
            }
        }
    }
}

fn parse_instrument(
    input: &[u8],
    offset: usize,
    signed_samples: bool,
) -> Result<S3mInstrument, S3mError> {
    if offset == 0 {
        return Ok(empty_instrument());
    }
    let header = input
        .get(offset..offset + 80)
        .ok_or(S3mError::Truncated("instrument header"))?;

    let mem_para = ((header[0x0d] as u32) << 16) | read_u16_slice(header, 0x0e) as u32;
    let raw = RawInstrument {
        kind: header[0],
        name: decode_fixed_string(&header[0x30..0x4c]),
        sample_offset: mem_para as usize * 16,
        length: read_u32_slice(header, 0x10),
        loop_start: read_u32_slice(header, 0x14),
        loop_end: read_u32_slice(header, 0x18),
        volume: header[0x1c].min(64),
        pack: header[0x1e],
        flags: header[0x1f],
        c5_speed: read_u32_slice(header, 0x20).max(1),
    };

    if raw.kind != 1 || raw.length == 0 || raw.pack != 0 {
        return Ok(S3mInstrument {
            kind: raw.kind,
            name: raw.name,
            length: raw.length,
            loop_start: raw.loop_start,
            loop_end: raw.loop_end,
            volume: raw.volume,
            flags: raw.flags,
            c5_speed: raw.c5_speed,
            pack: raw.pack,
            data: Vec::new(),
        });
    }

    let data = decode_sample(input, &raw, signed_samples);
    Ok(S3mInstrument {
        kind: raw.kind,
        name: raw.name,
        length: raw.length,
        loop_start: raw.loop_start.min(data.len() as u32),
        loop_end: raw.loop_end.min(data.len() as u32),
        volume: raw.volume,
        flags: raw.flags,
        c5_speed: raw.c5_speed,
        pack: raw.pack,
        data,
    })
}

fn parse_pattern(input: &[u8], offset: usize) -> Result<S3mPattern, S3mError> {
    if offset + 2 > input.len() {
        return Err(S3mError::Truncated("pattern length"));
    }
    let length = read_u16(input, offset)? as usize;
    let body_start = offset + 2;
    let body_end = (offset + length).min(input.len());
    if body_start > body_end {
        return Err(S3mError::Truncated("pattern body"));
    }

    let body = &input[body_start..body_end];
    let mut pattern = empty_pattern();
    let mut pos = 0usize;
    let mut row = 0usize;

    while row < S3M_ROWS && pos < body.len() {
        let flags = body[pos];
        pos += 1;
        if flags == 0 {
            row += 1;
            continue;
        }

        let channel = (flags & 0x1f) as usize;
        let mut cell = S3mCell::EMPTY;
        if flags & 0x20 != 0 {
            if pos + 2 > body.len() {
                break;
            }
            cell.note = body[pos];
            cell.instrument = body[pos + 1];
            pos += 2;
        }
        if flags & 0x40 != 0 {
            if pos >= body.len() {
                break;
            }
            cell.volume = body[pos];
            pos += 1;
        }
        if flags & 0x80 != 0 {
            if pos + 2 > body.len() {
                break;
            }
            cell.command = body[pos];
            cell.info = body[pos + 1];
            pos += 2;
        }
        if channel < S3M_CHANNELS {
            pattern.rows[row][channel] = cell;
        }
    }

    Ok(pattern)
}

fn decode_sample(input: &[u8], instrument: &RawInstrument, signed_samples: bool) -> Vec<i16> {
    let is_16bit = instrument.flags & 0x04 != 0;
    let is_stereo = instrument.flags & 0x02 != 0;
    let bytes_per_sample = if is_16bit { 2 } else { 1 };
    let channel_count = if is_stereo { 2 } else { 1 };
    let needed = instrument.length as usize * bytes_per_sample * channel_count;
    let Some(raw) =
        input.get(instrument.sample_offset..instrument.sample_offset.saturating_add(needed))
    else {
        return Vec::new();
    };

    let left_len = instrument.length as usize * bytes_per_sample;
    let left = &raw[..left_len.min(raw.len())];
    let right = if is_stereo && raw.len() > left_len {
        &raw[left_len..(left_len * 2).min(raw.len())]
    } else {
        &[][..]
    };

    let mut left_pcm = decode_pcm_channel(left, is_16bit, signed_samples);
    if is_stereo {
        let right_pcm = decode_pcm_channel(right, is_16bit, signed_samples);
        for (index, sample) in left_pcm.iter_mut().enumerate() {
            let right = *right_pcm.get(index).unwrap_or(&0) as i32;
            *sample = (((*sample as i32) + right) / 2) as i16;
        }
    }
    left_pcm
}

fn decode_pcm_channel(input: &[u8], is_16bit: bool, signed_samples: bool) -> Vec<i16> {
    if is_16bit {
        input
            .chunks_exact(2)
            .map(|bytes| {
                if signed_samples {
                    i16::from_le_bytes([bytes[0], bytes[1]])
                } else {
                    (u16::from_le_bytes([bytes[0], bytes[1]]) as i32 - 0x8000) as i16
                }
            })
            .collect()
    } else {
        input
            .iter()
            .map(|byte| {
                let sample = if signed_samples {
                    (*byte as i8 as i32) * 256
                } else {
                    (*byte as i32 - 128) * 256
                };
                sample.clamp(i16::MIN as i32, i16::MAX as i32) as i16
            })
            .collect()
    }
}

fn empty_pattern() -> S3mPattern {
    S3mPattern {
        rows: vec![vec![S3mCell::EMPTY; S3M_CHANNELS]; S3M_ROWS],
    }
}

fn empty_instrument() -> S3mInstrument {
    S3mInstrument {
        kind: 0,
        name: String::new(),
        length: 0,
        loop_start: 0,
        loop_end: 0,
        volume: 0,
        flags: 0,
        c5_speed: 8363,
        pack: 0,
        data: Vec::new(),
    }
}

fn read_para_table(
    input: &[u8],
    pos: &mut usize,
    count: usize,
    context: &'static str,
) -> Result<Vec<u16>, S3mError> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let value = input
            .get(*pos..*pos + 2)
            .ok_or(S3mError::Truncated(context))?;
        out.push(read_u16_slice(value, 0));
        *pos += 2;
    }
    Ok(out)
}

fn read_u16(input: &[u8], offset: usize) -> Result<u16, S3mError> {
    let bytes = input
        .get(offset..offset + 2)
        .ok_or(S3mError::Truncated("u16"))?;
    Ok(read_u16_slice(bytes, 0))
}

fn read_u16_slice(input: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([input[offset], input[offset + 1]])
}

fn read_u32_slice(input: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        input[offset],
        input[offset + 1],
        input[offset + 2],
        input[offset + 3],
    ])
}

fn decode_fixed_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end])
        .trim_end()
        .to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn parses_minimal_s3m() {
        let input = minimal_s3m();
        let module = parse_s3m(&input).expect("valid s3m");
        assert_eq!(module.title, "S3MTEST");
        assert_eq!(module.orders, vec![0]);
        assert_eq!(module.active_channels[0], 0);
        assert_eq!(module.channel_pans[0], s3m_pan_nibble_to_m8(3));
        assert_eq!(module.patterns[0].rows[0][0].note, 0x40);
        assert_eq!(module.instruments[0].data.len(), 4);
    }

    #[test]
    fn maps_s3m_notes_to_m8_notes() {
        assert_eq!(s3m_note_to_m8(0x40), Some(36));
        assert_eq!(s3m_note_to_m8(0x41), Some(37));
        assert_eq!(s3m_note_to_m8(0xfe), Some(0x80));
        assert_eq!(s3m_note_to_m8(0xff), None);
    }

    #[test]
    fn maps_s3m_pan_nibble_to_m8_pan() {
        assert_eq!(s3m_pan_nibble_to_m8(0x00), 0x00);
        assert_eq!(s3m_pan_nibble_to_m8(0x04), 0x44);
        assert_eq!(s3m_pan_nibble_to_m8(0x08), 0x80);
        assert_eq!(s3m_pan_nibble_to_m8(0x0b), 0xbb);
        assert_eq!(s3m_pan_nibble_to_m8(0x0f), 0xff);
    }

    pub fn minimal_s3m() -> Vec<u8> {
        let instrument_offset = 0x90usize;
        let pattern_offset = 0xf0usize;
        let sample_offset = 0x140usize;
        let mut out = vec![0u8; sample_offset + 4];

        out[0..7].copy_from_slice(b"S3MTEST");
        out[0x1c] = 0x1a;
        out[0x1d] = 0x10;
        out[0x20..0x22].copy_from_slice(&1u16.to_le_bytes());
        out[0x22..0x24].copy_from_slice(&1u16.to_le_bytes());
        out[0x24..0x26].copy_from_slice(&1u16.to_le_bytes());
        out[0x2a..0x2c].copy_from_slice(&2u16.to_le_bytes());
        out[0x2c..0x30].copy_from_slice(b"SCRM");
        out[0x30] = 64;
        out[0x31] = 6;
        out[0x32] = 125;
        out[0x33] = 0x80 | 48;
        out[0x40] = 0;
        for index in 1..S3M_CHANNELS {
            out[0x40 + index] = 0xff;
        }

        out[0x60] = 0;
        out[0x61..0x63].copy_from_slice(&((instrument_offset / 16) as u16).to_le_bytes());
        out[0x63..0x65].copy_from_slice(&((pattern_offset / 16) as u16).to_le_bytes());

        out[instrument_offset] = 1;
        let sample_para = sample_offset / 16;
        out[instrument_offset + 0x0d] = ((sample_para >> 16) & 0xff) as u8;
        out[instrument_offset + 0x0e..instrument_offset + 0x10]
            .copy_from_slice(&((sample_para & 0xffff) as u16).to_le_bytes());
        out[instrument_offset + 0x10..instrument_offset + 0x14]
            .copy_from_slice(&4u32.to_le_bytes());
        out[instrument_offset + 0x1c] = 64;
        out[instrument_offset + 0x20..instrument_offset + 0x24]
            .copy_from_slice(&8363u32.to_le_bytes());
        out[instrument_offset + 0x30..instrument_offset + 0x34].copy_from_slice(b"BEEP");
        out[instrument_offset + 0x4c..instrument_offset + 0x50].copy_from_slice(b"SCRS");

        let mut pattern = Vec::new();
        pattern.extend_from_slice(&0u16.to_le_bytes());
        pattern.push(0x60);
        pattern.push(0x40);
        pattern.push(1);
        pattern.push(64);
        pattern.push(0);
        for _ in 1..S3M_ROWS {
            pattern.push(0);
        }
        let len = pattern.len() as u16;
        pattern[0..2].copy_from_slice(&len.to_le_bytes());
        out[pattern_offset..pattern_offset + pattern.len()].copy_from_slice(&pattern);

        out[sample_offset..sample_offset + 4].copy_from_slice(&[0, 255, 128, 64]);
        out
    }
}
