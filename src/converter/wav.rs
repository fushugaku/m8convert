use crate::converter::modfile::Sample;
use crate::converter::s3mfile::S3mInstrument;

const SAMPLE_RATE: u32 = 8363;

pub fn encode_sample_as_wav(sample: &Sample) -> Vec<u8> {
    let mut data: Vec<u8> = sample
        .data
        .iter()
        .map(|value| (*value as i16 + 128).clamp(0, 255) as u8)
        .collect();

    if data.len() % 2 != 0 {
        data.push(128);
    }

    let smpl_chunk = sample.has_loop().then(|| smpl_chunk(sample));
    let smpl_len = smpl_chunk.as_ref().map(|chunk| chunk.len()).unwrap_or(0);
    let riff_size = 4 + (8 + 16) + (8 + data.len()) + smpl_len;

    let mut out = Vec::with_capacity(8 + riff_size);
    out.extend_from_slice(b"RIFF");
    write_u32(&mut out, riff_size as u32);
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    write_u32(&mut out, 16);
    write_u16(&mut out, 1);
    write_u16(&mut out, 1);
    write_u32(&mut out, SAMPLE_RATE);
    write_u32(&mut out, SAMPLE_RATE);
    write_u16(&mut out, 1);
    write_u16(&mut out, 8);

    if let Some(chunk) = smpl_chunk {
        out.extend_from_slice(&chunk);
    }

    out.extend_from_slice(b"data");
    write_u32(&mut out, data.len() as u32);
    out.extend_from_slice(&data);
    out
}

pub fn encode_s3m_sample_as_wav(sample: &S3mInstrument) -> Vec<u8> {
    let source_rate = sample.c5_speed.max(1);
    let channels = if sample.right_data.is_some() {
        2u16
    } else {
        1u16
    };
    let block_align = channels * 2;
    let mut data = Vec::with_capacity(sample.data.len() * block_align as usize);
    for (index, left) in sample.data.iter().enumerate() {
        data.extend_from_slice(&left.to_le_bytes());
        if let Some(right) = &sample.right_data {
            data.extend_from_slice(&right.get(index).copied().unwrap_or_default().to_le_bytes());
        }
    }

    let smpl_chunk = sample.is_looped().then(|| {
        let loop_start = sample.loop_start;
        let loop_end = sample
            .loop_end
            .saturating_sub(1)
            .min(sample.data.len().saturating_sub(1) as u32);
        smpl_chunk_from_points(
            loop_start.min(sample.data.len().saturating_sub(1) as u32),
            loop_end,
            u32::from(sample.ping_pong_loop),
        )
    });
    let smpl_len = smpl_chunk.as_ref().map(|chunk| chunk.len()).unwrap_or(0);
    let riff_size = 4 + (8 + 16) + (8 + data.len()) + smpl_len;

    let mut out = Vec::with_capacity(8 + riff_size);
    out.extend_from_slice(b"RIFF");
    write_u32(&mut out, riff_size as u32);
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    write_u32(&mut out, 16);
    write_u16(&mut out, 1);
    write_u16(&mut out, channels);
    write_u32(&mut out, source_rate);
    write_u32(&mut out, source_rate.saturating_mul(u32::from(block_align)));
    write_u16(&mut out, block_align);
    write_u16(&mut out, 16);

    if let Some(chunk) = smpl_chunk {
        out.extend_from_slice(&chunk);
    }

    out.extend_from_slice(b"data");
    write_u32(&mut out, data.len() as u32);
    out.extend_from_slice(&data);
    out
}

fn smpl_chunk(sample: &Sample) -> Vec<u8> {
    let loop_start = sample.loop_start_bytes as u32;
    let loop_end = (sample.loop_start_bytes + sample.loop_length_bytes)
        .saturating_sub(1)
        .min(sample.length_bytes.saturating_sub(1)) as u32;
    smpl_chunk_from_points(loop_start, loop_end, 0)
}

fn smpl_chunk_from_points(loop_start: u32, loop_end: u32, loop_type: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(60);
    write_u32(&mut payload, 0);
    write_u32(&mut payload, loop_type);
    write_u32(&mut payload, 0);
    write_u32(&mut payload, 60);
    write_u32(&mut payload, 0);
    write_u32(&mut payload, 0);
    write_u32(&mut payload, 0);
    write_u32(&mut payload, 1);
    write_u32(&mut payload, 0);

    write_u32(&mut payload, 0);
    write_u32(&mut payload, 0);
    write_u32(&mut payload, loop_start);
    write_u32(&mut payload, loop_end);
    write_u32(&mut payload, 0);
    write_u32(&mut payload, 0);

    let mut chunk = Vec::with_capacity(8 + payload.len());
    chunk.extend_from_slice(b"smpl");
    write_u32(&mut chunk, payload.len() as u32);
    chunk.extend_from_slice(&payload);
    chunk
}

fn write_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_riff_wav_header() {
        let sample = Sample {
            name: "kick".to_string(),
            length_bytes: 2,
            finetune: 0,
            volume: 64,
            loop_start_bytes: 0,
            loop_length_bytes: 0,
            data: vec![-128, 127],
        };
        let wav = encode_sample_as_wav(&sample);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[wav.len() - 2..], &[0, 255]);
    }

    #[test]
    fn s3m_wav_preserves_c2spd_sample_rate() {
        let sample = S3mInstrument {
            kind: 1,
            name: "tone".to_string(),
            length: 4,
            loop_start: 1,
            loop_end: 3,
            volume: 64,
            flags: 1,
            c5_speed: 11_025,
            pack: 0,
            default_pan: None,
            amp_envelope: None,
            auto_vibrato: None,
            data: vec![0, 1000, -1000, 0],
            right_data: None,
            ping_pong_loop: false,
        };

        let wav = encode_s3m_sample_as_wav(&sample);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            sample.c5_speed
        );
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(wav.len(), 44 + 68 + sample.data.len() * 2);
    }

    #[test]
    fn s3m_wav_preserves_stereo_channels() {
        let sample = S3mInstrument {
            kind: 1,
            name: "stereo".to_string(),
            length: 2,
            loop_start: 0,
            loop_end: 0,
            volume: 64,
            flags: 0x02,
            c5_speed: 8_000,
            pack: 0,
            default_pan: None,
            amp_envelope: None,
            auto_vibrato: None,
            data: vec![1, 2],
            right_data: Some(vec![3, 4]),
            ping_pong_loop: false,
        };

        let wav = encode_s3m_sample_as_wav(&sample);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 4);
        assert_eq!(&wav[44..52], &[1, 0, 3, 0, 2, 0, 4, 0]);
    }
}
