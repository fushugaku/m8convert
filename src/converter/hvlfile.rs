use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HvlError {
    #[error("file is too short to be a HVL module")]
    TooShort,
    #[error("unsupported HVL signature or version")]
    UnsupportedSignature,
    #[error("invalid HVL header: {0}")]
    InvalidHeader(String),
    #[error("HVL data is truncated while reading {0}")]
    Truncated(&'static str),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HvlModule {
    pub title: String,
    pub version: u8,
    pub position_count: usize,
    pub channel_count: usize,
    pub restart_position: u16,
    pub speed_multiplier: u8,
    pub track_length: usize,
    pub track_count: usize,
    pub instrument_count: usize,
    pub subsong_count: usize,
    pub positions: Vec<HvlPosition>,
    pub tracks: Vec<HvlTrack>,
    pub instruments: Vec<HvlInstrument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HvlPosition {
    pub tracks: Vec<u8>,
    pub transposes: Vec<i8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HvlTrack {
    pub rows: Vec<HvlStep>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HvlStep {
    pub note: u8,
    pub instrument: u8,
    pub fx: u8,
    pub fx_param: u8,
    pub fx_b: u8,
    pub fx_b_param: u8,
}

impl HvlStep {
    pub fn is_empty(self) -> bool {
        self.note == 0
            && self.instrument == 0
            && self.fx == 0
            && self.fx_param == 0
            && self.fx_b == 0
            && self.fx_b_param == 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HvlInstrument {
    pub name: String,
    pub volume: u8,
    pub wave_length: u8,
    pub filter_speed: u8,
    pub envelope: HvlEnvelope,
    pub vibrato_delay: u8,
    pub vibrato_depth: u8,
    pub vibrato_speed: u8,
    pub square_lower_limit: u8,
    pub square_upper_limit: u8,
    pub square_speed: u8,
    pub filter_lower_limit: u8,
    pub filter_upper_limit: u8,
    pub plist_speed: u8,
    pub plist: Vec<HvlPListEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HvlEnvelope {
    pub attack_frames: u8,
    pub attack_volume: u8,
    pub decay_frames: u8,
    pub decay_volume: u8,
    pub sustain_frames: u8,
    pub release_frames: u8,
    pub release_volume: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HvlPListEntry {
    pub waveform: u8,
    pub fixed: bool,
    pub note: u8,
    pub fx: [u8; 2],
    pub fx_param: [u8; 2],
}

pub fn parse_hvl(input: &[u8]) -> Result<HvlModule, HvlError> {
    if input.len() < 16 {
        return Err(HvlError::TooShort);
    }
    if &input[0..3] != b"HVL" || input[3] > 1 {
        return Err(HvlError::UnsupportedSignature);
    }

    let version = input[3];
    let names_offset = read_be_u16(&input[4..6]) as usize;
    let position_count = (((input[6] & 0x0f) as usize) << 8) | input[7] as usize;
    let empty_track_compressed = (input[6] & 0x80) != 0;
    let speed_multiplier = ((input[6] >> 5) & 0x03) + 1;
    let channel_count = (input[8] >> 2) as usize + 4;
    let restart_position = (((input[8] & 0x03) as u16) << 8) | input[9] as u16;
    let track_length = input[10] as usize;
    let track_count = input[11] as usize;
    let instrument_count = input[12] as usize;
    let subsong_count = input[13] as usize;

    if names_offset >= input.len() {
        return Err(HvlError::InvalidHeader(
            "name table offset is outside the file".to_string(),
        ));
    }
    if position_count > 1000 {
        return Err(HvlError::InvalidHeader(format!(
            "position count {position_count} exceeds HVL limit"
        )));
    }
    if !(1..=64).contains(&track_length) {
        return Err(HvlError::InvalidHeader(format!(
            "track length {track_length} is outside 1..=64"
        )));
    }
    if instrument_count > 64 {
        return Err(HvlError::InvalidHeader(format!(
            "instrument count {instrument_count} exceeds HVL limit"
        )));
    }
    if channel_count > 16 {
        return Err(HvlError::InvalidHeader(format!(
            "channel count {channel_count} exceeds HVL limit"
        )));
    }

    let mut pos = 16usize;
    let mut subsongs = Vec::with_capacity(subsong_count);
    for _ in 0..subsong_count {
        let bytes = read(input, &mut pos, 2, "subsongs")?;
        subsongs.push(read_be_u16(bytes));
    }

    let mut positions = Vec::with_capacity(position_count);
    for _ in 0..position_count {
        let mut tracks = Vec::with_capacity(channel_count);
        let mut transposes = Vec::with_capacity(channel_count);
        for _ in 0..channel_count {
            let bytes = read(input, &mut pos, 2, "position list")?;
            tracks.push(bytes[0]);
            transposes.push(bytes[1] as i8);
        }
        positions.push(HvlPosition { tracks, transposes });
    }

    let mut tracks = vec![
        HvlTrack {
            rows: vec![empty_step(); track_length],
        };
        track_count + 1
    ];
    for (track_id, track) in tracks.iter_mut().enumerate().take(track_count + 1) {
        if empty_track_compressed && track_id == 0 {
            continue;
        }

        let mut rows = Vec::with_capacity(track_length);
        for _ in 0..track_length {
            let first = *read(input, &mut pos, 1, "tracks")?
                .first()
                .ok_or(HvlError::Truncated("tracks"))?;
            if first == 0x3f {
                rows.push(empty_step());
            } else {
                let rest = read(input, &mut pos, 4, "tracks")?;
                rows.push(HvlStep {
                    note: first,
                    instrument: rest[0],
                    fx: rest[1] >> 4,
                    fx_b: rest[1] & 0x0f,
                    fx_param: rest[2],
                    fx_b_param: rest[3],
                });
            }
        }
        track.rows = rows;
    }

    let mut names_pos = names_offset;
    let title = read_c_string(input, &mut names_pos);
    let mut instruments = Vec::with_capacity(instrument_count);
    for _ in 0..instrument_count {
        let name = read_c_string(input, &mut names_pos);
        let header = read(input, &mut pos, 22, "instruments")?;
        let plist_len = header[21] as usize;

        let mut plist = Vec::with_capacity(plist_len);
        for _ in 0..plist_len {
            let entry = read(input, &mut pos, 5, "instrument performance list")?;
            plist.push(HvlPListEntry {
                fx: [entry[0] & 0x0f, (entry[1] >> 3) & 0x0f],
                waveform: entry[1] & 0x07,
                fixed: (entry[2] & 0x40) != 0,
                note: entry[2] & 0x3f,
                fx_param: [entry[3], entry[4]],
            });
        }

        instruments.push(HvlInstrument {
            name,
            volume: header[0].min(64),
            filter_speed: ((header[1] >> 3) & 0x1f) | ((header[12] >> 2) & 0x20),
            wave_length: header[1] & 0x07,
            envelope: HvlEnvelope {
                attack_frames: header[2],
                attack_volume: header[3],
                decay_frames: header[4],
                decay_volume: header[5],
                sustain_frames: header[6],
                release_frames: header[7],
                release_volume: header[8],
            },
            filter_lower_limit: header[12] & 0x7f,
            vibrato_delay: header[13],
            vibrato_depth: header[14] & 0x0f,
            vibrato_speed: header[15],
            square_lower_limit: header[16],
            square_upper_limit: header[17],
            square_speed: header[18],
            filter_upper_limit: header[19] & 0x3f,
            plist_speed: header[20],
            plist,
        });
    }

    Ok(HvlModule {
        title,
        version,
        position_count,
        channel_count,
        restart_position,
        speed_multiplier,
        track_length,
        track_count,
        instrument_count,
        subsong_count,
        positions,
        tracks,
        instruments,
    })
}

pub fn is_hvl(input: &[u8]) -> bool {
    input.len() >= 4 && &input[0..3] == b"HVL" && input[3] <= 1
}

fn empty_step() -> HvlStep {
    HvlStep {
        note: 0,
        instrument: 0,
        fx: 0,
        fx_param: 0,
        fx_b: 0,
        fx_b_param: 0,
    }
}

fn read<'a>(
    input: &'a [u8],
    pos: &mut usize,
    len: usize,
    context: &'static str,
) -> Result<&'a [u8], HvlError> {
    let end = pos.saturating_add(len);
    let bytes = input.get(*pos..end).ok_or(HvlError::Truncated(context))?;
    *pos = end;
    Ok(bytes)
}

fn read_be_u16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

fn read_c_string(input: &[u8], pos: &mut usize) -> String {
    if *pos >= input.len() {
        return String::new();
    }

    let tail = &input[*pos..];
    let len = tail
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(tail.len());
    let value = String::from_utf8_lossy(&tail[..len]).trim_end().to_string();
    *pos = (*pos + len + 1).min(input.len());
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_hvl() {
        let bytes = minimal_hvl();
        let module = parse_hvl(&bytes).expect("valid hvl");
        assert_eq!(module.title, "HVLTEST");
        assert_eq!(module.channel_count, 4);
        assert_eq!(module.positions[0].tracks[0], 1);
        assert_eq!(module.tracks[1].rows[0].note, 25);
        assert_eq!(module.instruments[0].name, "SYNTH");
    }

    pub fn minimal_hvl() -> Vec<u8> {
        let track_len = 16usize;
        let compressed_track_len = 5 + (track_len - 1);
        let names_offset = 16 + 4 * 2 + compressed_track_len + 22;
        let mut out = vec![
            b'H',
            b'V',
            b'L',
            1,
            (names_offset >> 8) as u8,
            names_offset as u8,
            0x80,
            1,
            0,
            0,
            track_len as u8,
            1,
            1,
            0,
            100,
            1,
        ];

        out.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
        for row in 0..track_len {
            if row == 0 {
                out.extend_from_slice(&[25, 1, 0, 0, 0]);
            } else {
                out.push(0x3f);
            }
        }

        let mut instrument = [0u8; 22];
        instrument[0] = 64;
        instrument[1] = 3;
        instrument[2] = 1;
        instrument[4] = 1;
        instrument[6] = 1;
        instrument[7] = 1;
        instrument[21] = 0;
        out.extend_from_slice(&instrument);
        out.extend_from_slice(b"HVLTEST\0SYNTH\0");
        out
    }
}
