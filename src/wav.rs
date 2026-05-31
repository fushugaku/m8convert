use crate::modfile::Sample;
use crate::s3mfile::S3mInstrument;

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
    let mut data = Vec::with_capacity(sample.data.len() * 2);
    for value in &sample.data {
        data.extend_from_slice(&value.to_le_bytes());
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
    write_u16(&mut out, 1);
    write_u32(&mut out, source_rate);
    write_u32(&mut out, source_rate * 2);
    write_u16(&mut out, 2);
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
    smpl_chunk_from_points(loop_start, loop_end)
}

fn smpl_chunk_from_points(loop_start: u32, loop_end: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(60);
    write_u32(&mut payload, 0);
    write_u32(&mut payload, 0);
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
            data: vec![0, 1000, -1000, 0],
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
}
