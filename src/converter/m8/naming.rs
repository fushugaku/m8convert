pub fn sample_filename(index: usize, name: &str) -> String {
    format!(
        "{:02}_{}.wav",
        index + 1,
        sanitize_name(&fallback_sample_name(index, name))
    )
}

pub fn s3m_sample_filename(index: usize, name: &str) -> String {
    format!(
        "{:02}_{}.wav",
        index + 1,
        sanitize_name(&fallback_sample_name(index, name))
    )
}

pub(super) fn fallback_sample_name(index: usize, name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        format!("sample_{:02}", index + 1)
    } else {
        trimmed.to_string()
    }
}

pub(super) fn sanitize_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    truncate_ascii(&sanitized, 48)
}

pub(super) fn truncate_ascii(value: &str, max_len: usize) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii() && !ch.is_ascii_control())
        .take(max_len)
        .collect()
}
