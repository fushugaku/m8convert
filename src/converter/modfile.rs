use serde::{Deserialize, Serialize};
use thiserror::Error;

const TITLE_LEN: usize = 20;
const SAMPLE_NAME_LEN: usize = 22;
const ORDER_TABLE_LEN: usize = 128;
const ROWS_PER_PATTERN: usize = 64;

#[derive(Debug, Error)]
pub enum ModError {
    #[error("file is too short to be a MOD module")]
    TooShort,
    #[error("unsupported MOD signature {0:?}")]
    UnsupportedSignature(String),
    #[error("pattern data is truncated")]
    TruncatedPatternData,
    #[error("sample data is truncated")]
    TruncatedSampleData,
    #[error("invalid song length {0}")]
    InvalidSongLength(u8),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    pub title: String,
    pub channel_count: usize,
    pub restart_position: u8,
    pub orders: Vec<u8>,
    pub pattern_count: usize,
    pub samples: Vec<Sample>,
    pub patterns: Vec<Pattern>,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub name: String,
    pub length_bytes: usize,
    pub finetune: i8,
    pub volume: u8,
    pub loop_start_bytes: usize,
    pub loop_length_bytes: usize,
    pub data: Vec<i8>,
}

impl Sample {
    pub fn has_loop(&self) -> bool {
        self.loop_length_bytes > 2 && self.loop_start_bytes < self.length_bytes
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub rows: Vec<Vec<Cell>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cell {
    pub period: u16,
    pub sample_number: u8,
    pub effect: u8,
    pub effect_param: u8,
}

impl Cell {
    pub fn is_empty(self) -> bool {
        self.period == 0 && self.sample_number == 0 && self.effect == 0 && self.effect_param == 0
    }
}

pub fn parse_mod(input: &[u8]) -> Result<Module, ModError> {
    if input.len() < 600 {
        return Err(ModError::TooShort);
    }

    let signature = input.get(1080..1084).map(decode_fixed_string);
    let (sample_count, channel_count, pattern_offset) = match signature.as_deref() {
        Some("M.K.") | Some("M!K!") | Some("M&K!") | Some("N.T.") | Some("FLT4") | Some("4CHN") => {
            (31, 4, 1084)
        }
        Some(sig) if sig.ends_with("CHN") => {
            let channels = sig[..sig.len() - 3]
                .parse::<usize>()
                .map_err(|_| ModError::UnsupportedSignature(sig.to_string()))?;
            (31, channels, 1084)
        }
        Some(sig) if sig.ends_with("CH") => {
            let channels = sig[..sig.len() - 2]
                .parse::<usize>()
                .map_err(|_| ModError::UnsupportedSignature(sig.to_string()))?;
            (31, channels, 1084)
        }
        _ => (15, 4, 600),
    };

    if channel_count == 0 || channel_count > 32 {
        return Err(ModError::UnsupportedSignature(
            signature.unwrap_or_else(|| "<none>".to_string()),
        ));
    }

    let title = decode_fixed_string(&input[..TITLE_LEN]);
    let mut offset = TITLE_LEN;
    let mut samples = Vec::with_capacity(sample_count);

    for _ in 0..sample_count {
        let descriptor = input.get(offset..offset + 30).ok_or(ModError::TooShort)?;
        let name = decode_fixed_string(&descriptor[..SAMPLE_NAME_LEN]);
        let length_bytes = read_be_u16(&descriptor[22..24]) as usize * 2;
        let finetune = signed_finetune(descriptor[24] & 0x0f);
        let volume = descriptor[25].min(64);
        let loop_start_bytes = read_be_u16(&descriptor[26..28]) as usize * 2;
        let loop_length_bytes = read_be_u16(&descriptor[28..30]) as usize * 2;

        samples.push(Sample {
            name,
            length_bytes,
            finetune,
            volume,
            loop_start_bytes,
            loop_length_bytes,
            data: Vec::new(),
        });
        offset += 30;
    }

    let song_length = *input.get(offset).ok_or(ModError::TooShort)?;
    if song_length == 0 || song_length as usize > ORDER_TABLE_LEN {
        return Err(ModError::InvalidSongLength(song_length));
    }
    let restart_position = *input.get(offset + 1).ok_or(ModError::TooShort)?;
    offset += 2;

    let order_table = input
        .get(offset..offset + ORDER_TABLE_LEN)
        .ok_or(ModError::TooShort)?;
    let orders = order_table[..song_length as usize].to_vec();
    let pattern_count = orders
        .iter()
        .max()
        .map(|max| *max as usize + 1)
        .unwrap_or(0);

    let pattern_bytes = pattern_count * ROWS_PER_PATTERN * channel_count * 4;
    let pattern_end = pattern_offset + pattern_bytes;
    let pattern_data = input
        .get(pattern_offset..pattern_end)
        .ok_or(ModError::TruncatedPatternData)?;

    let mut patterns = Vec::with_capacity(pattern_count);
    let mut pattern_pos = 0;
    for _ in 0..pattern_count {
        let mut rows = Vec::with_capacity(ROWS_PER_PATTERN);
        for _ in 0..ROWS_PER_PATTERN {
            let mut row = Vec::with_capacity(channel_count);
            for _ in 0..channel_count {
                let bytes = &pattern_data[pattern_pos..pattern_pos + 4];
                row.push(parse_cell(bytes));
                pattern_pos += 4;
            }
            rows.push(row);
        }
        patterns.push(Pattern { rows });
    }

    let mut sample_offset = pattern_end;
    for sample in &mut samples {
        let sample_end = sample_offset + sample.length_bytes;
        let data = input
            .get(sample_offset..sample_end)
            .ok_or(ModError::TruncatedSampleData)?;
        sample.data = data.iter().map(|byte| *byte as i8).collect();
        sample_offset = sample_end;
    }

    Ok(Module {
        title,
        channel_count,
        restart_position,
        orders,
        pattern_count,
        samples,
        patterns,
        signature,
    })
}

pub fn period_to_note(period: u16) -> Option<u8> {
    if period == 0 {
        return None;
    }

    PERIODS
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| period.abs_diff(**candidate))
        .map(|(index, _)| index as u8)
}

fn parse_cell(bytes: &[u8]) -> Cell {
    let period = (((bytes[0] & 0x0f) as u16) << 8) | bytes[1] as u16;
    let sample_number = (bytes[0] & 0xf0) | (bytes[2] >> 4);
    let effect = bytes[2] & 0x0f;
    let effect_param = bytes[3];

    Cell {
        period,
        sample_number,
        effect,
        effect_param,
    }
}

fn read_be_u16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

fn signed_finetune(value: u8) -> i8 {
    if value <= 7 {
        value as i8
    } else {
        value as i8 - 16
    }
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

const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302,
    285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cell_sample_and_period() {
        let cell = parse_cell(&[0x13, 0x58, 0x21, 0x7f]);
        assert_eq!(cell.sample_number, 0x12);
        assert_eq!(cell.period, 0x358);
        assert_eq!(cell.effect, 0x01);
        assert_eq!(cell.effect_param, 0x7f);
    }

    #[test]
    fn maps_periods_to_mod_note_numbers() {
        assert_eq!(period_to_note(856), Some(0));
        assert_eq!(period_to_note(428), Some(12));
        assert_eq!(period_to_note(113), Some(35));
        assert_eq!(period_to_note(0), None);
    }
}
