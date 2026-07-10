use super::*;

pub(super) fn patch_header(
    song_bytes: &mut [u8],
    title: &str,
    bundle_directory: Option<&str>,
    tempo_bpm: Option<f32>,
) {
    let directory_offset = 14;
    if let Some(directory) = bundle_directory {
        patch_fixed_ascii(song_bytes, directory_offset, 128, directory);
    }

    let tempo_offset = 14 + 128 + 1;
    if let Some(tempo_bpm) = tempo_bpm {
        patch_f32(song_bytes, tempo_offset, tempo_bpm);
    }

    let name_offset = 14 + 128 + 1 + 4 + 1;
    patch_fixed_ascii(song_bytes, name_offset, 12, title);
}

pub(super) fn patch_f32(bytes: &mut [u8], offset: usize, value: f32) {
    if bytes.len() < offset + 4 {
        return;
    }

    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn patch_fixed_ascii(bytes: &mut [u8], offset: usize, len: usize, value: &str) {
    if bytes.len() < offset + len {
        return;
    }

    let value = truncate_ascii(value, len);
    bytes[offset..offset + len].fill(0);
    let value_bytes = value.as_bytes();
    bytes[offset..offset + value_bytes.len()].copy_from_slice(value_bytes);
}
