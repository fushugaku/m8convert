use xmrs::core::module::Module as XmrsModule;
use xmrsplayer::xmrsplayer::XmrsPlayer;

pub const DEFAULT_REFERENCE_SAMPLE_RATE: u32 = 44_100;
pub const DEFAULT_REFERENCE_MAX_SECONDS: u32 = 180;

#[derive(Debug, Clone)]
pub struct ReferenceRender {
    pub wav_bytes: Vec<u8>,
    pub sample_rate: u32,
    pub frames: usize,
    pub truncated: bool,
}

pub fn render_reference_mix(
    input: &[u8],
    sample_rate: u32,
    max_seconds: u32,
) -> Result<ReferenceRender, String> {
    let sample_rate = sample_rate.clamp(8_000, 96_000);
    let max_seconds = max_seconds.clamp(1, 600);
    let module = XmrsModule::load(input).map_err(|error| error.to_string())?;
    let mut player = XmrsPlayer::new(&module, sample_rate, 0);
    let frame_limit = sample_rate as usize * max_seconds as usize;
    let mut samples = Vec::with_capacity(frame_limit.min(sample_rate as usize * 30));
    let mut reached_end = false;

    for _ in 0..frame_limit {
        match player.sample(true) {
            Some(sample) => samples.push(sample),
            None => {
                reached_end = true;
                break;
            }
        }
    }

    if samples.is_empty() {
        return Err("reference replayer produced no audio frames".to_string());
    }
    let wav_bytes = encode_stereo_i16_wav(sample_rate, &samples);
    Ok(ReferenceRender {
        wav_bytes,
        sample_rate,
        frames: samples.len(),
        truncated: !reached_end && samples.len() == frame_limit,
    })
}

pub fn encode_stereo_i16_wav(sample_rate: u32, samples: &[(i16, i16)]) -> Vec<u8> {
    let data_len = samples.len().saturating_mul(4).min(u32::MAX as usize) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&36u32.saturating_add(data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&sample_rate.saturating_mul(4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for (left, right) in samples.iter().take(data_len as usize / 4) {
        out.extend_from_slice(&left.to_le_bytes());
        out.extend_from_slice(&right.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_wav_header_matches_pcm_payload() {
        let wav = encode_stereo_i16_wav(44_100, &[(1, -1), (2, -2)]);
        assert_eq!(&wav[..12], b"RIFF,\0\0\0WAVE");
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 44_100);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        assert_eq!(wav.len(), 52);
    }
}
