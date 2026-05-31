use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::hvlfile::{HvlError, HvlModule, is_hvl, parse_hvl};
use crate::m8::{
    M8Error, M8Report, export_editable_m8, export_hvl_editable_m8, export_s3m_editable_m8,
    s3m_sample_filename, sample_filename,
};
use crate::modfile::{ModError, Module, parse_mod};
use crate::s3mfile::{S3mError, S3mModule, is_s3m, parse_s3m};
use crate::wav::{encode_s3m_sample_as_wav, encode_sample_as_wav};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversionOptions {
    pub song_name: Option<String>,
    pub bundle_directory: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertedBundle {
    pub project_name: String,
    pub files: Vec<BundleFile>,
    pub report: ConversionReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleFile {
    pub path: String,
    pub mime_type: String,
    pub size: usize,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionReport {
    pub source: SourceReport,
    pub m8: M8Report,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceReport {
    pub format: String,
    pub title: String,
    pub signature: Option<String>,
    pub channel_count: usize,
    pub order_count: usize,
    pub pattern_count: usize,
    pub sample_count: usize,
}

#[derive(Debug, Error)]
pub enum ConvertError {
    #[error(transparent)]
    Mod(#[from] ModError),
    #[error(transparent)]
    Hvl(#[from] HvlError),
    #[error(transparent)]
    S3m(#[from] S3mError),
    #[error(transparent)]
    M8(#[from] M8Error),
    #[error("could not serialize conversion report: {0}")]
    Report(serde_json::Error),
}

pub fn convert_tracker(
    input: &[u8],
    options: ConversionOptions,
) -> Result<ConvertedBundle, ConvertError> {
    if is_hvl(input) {
        convert_hvl(input, options)
    } else if is_s3m(input) {
        convert_s3m(input, options)
    } else {
        convert_mod(input, options)
    }
}

pub fn convert_mod(
    input: &[u8],
    options: ConversionOptions,
) -> Result<ConvertedBundle, ConvertError> {
    let module = parse_mod(input)?;
    let project_name = project_name(&module, options.song_name.as_deref());
    let bundle_root = bundle_root(&project_name);
    let m8_options = m8_options(&options, &project_name);
    let m8_export = export_editable_m8(&module, &m8_options)?;

    let mut files = vec![bundle_file(
        format!("{bundle_root}{project_name}.m8s"),
        "application/octet-stream",
        m8_export.song_bytes,
    )];

    for (index, sample) in module.samples.iter().enumerate() {
        if sample.length_bytes == 0 {
            continue;
        }

        files.push(bundle_file(
            format!(
                "{bundle_root}Samples/{}",
                sample_filename(index, &sample.name)
            ),
            "audio/wav",
            encode_sample_as_wav(sample),
        ));
    }

    let report = ConversionReport {
        source: source_report(&module),
        m8: m8_export.report,
        notes: vec![
            "Editable conversion maps MOD order rows to M8 song rows, MOD channels to M8 tracks, and MOD rows to phrases/chains.".to_string(),
            "Samples are exported as unsigned 8-bit mono WAV files with loop metadata when present; M8 song files reference those external samples.".to_string(),
            "ProTracker tick effects are listed in unsupported_effects unless they can be represented as a static phrase value.".to_string(),
        ],
    };

    let report_bytes = serde_json::to_vec_pretty(&report).map_err(ConvertError::Report)?;
    files.push(bundle_file(
        format!("{bundle_root}conversion-report.json"),
        "application/json",
        report_bytes,
    ));

    let manifest = manifest_json(&module, &project_name)?;
    files.push(bundle_file(
        format!("{bundle_root}m8convert-project.json"),
        "application/json",
        manifest,
    ));

    Ok(ConvertedBundle {
        project_name,
        files,
        report,
    })
}

pub fn convert_hvl(
    input: &[u8],
    options: ConversionOptions,
) -> Result<ConvertedBundle, ConvertError> {
    let module = parse_hvl(input)?;
    let project_name =
        project_name_from_title(&module.title, options.song_name.as_deref(), "converted_hvl");
    let bundle_root = bundle_root(&project_name);
    let m8_options = m8_options(&options, &project_name);
    let m8_export = export_hvl_editable_m8(&module, &m8_options)?;

    let mut files = vec![bundle_file(
        format!("{bundle_root}{project_name}.m8s"),
        "application/octet-stream",
        m8_export.song_bytes,
    )];

    let report = ConversionReport {
        source: hvl_source_report(&module),
        m8: m8_export.report,
        notes: vec![
            "Experimental editable HVL conversion maps positions to M8 song rows, HVL tracks to chains/phrases, and HVL notes to M8 notes.".to_string(),
            "HVL instruments are synthesized; this converter approximates them as M8 WavSynth patches and does not render the exact HivelyTracker engine.".to_string(),
            "HVL track commands and performance-list behavior are listed in unsupported_effects/warnings when they cannot be represented directly.".to_string(),
        ],
    };

    let report_bytes = serde_json::to_vec_pretty(&report).map_err(ConvertError::Report)?;
    files.push(bundle_file(
        format!("{bundle_root}conversion-report.json"),
        "application/json",
        report_bytes,
    ));

    let manifest = hvl_manifest_json(&module, &project_name)?;
    files.push(bundle_file(
        format!("{bundle_root}m8convert-project.json"),
        "application/json",
        manifest,
    ));

    Ok(ConvertedBundle {
        project_name,
        files,
        report,
    })
}

pub fn convert_s3m(
    input: &[u8],
    options: ConversionOptions,
) -> Result<ConvertedBundle, ConvertError> {
    let module = parse_s3m(input)?;
    let project_name =
        project_name_from_title(&module.title, options.song_name.as_deref(), "converted_s3m");
    let bundle_root = bundle_root(&project_name);
    let m8_options = m8_options(&options, &project_name);
    let m8_export = export_s3m_editable_m8(&module, &m8_options)?;

    let mut files = vec![bundle_file(
        format!("{bundle_root}{project_name}.m8s"),
        "application/octet-stream",
        m8_export.song_bytes,
    )];

    for (index, sample) in module.instruments.iter().enumerate() {
        if !sample.is_pcm() {
            continue;
        }
        files.push(bundle_file(
            format!(
                "{bundle_root}Samples/{}",
                s3m_sample_filename(index, &sample.name)
            ),
            "audio/wav",
            encode_s3m_sample_as_wav(sample),
        ));
    }

    let report = ConversionReport {
        source: s3m_source_report(&module),
        m8: m8_export.report,
        notes: vec![
            "Experimental editable S3M conversion maps order rows to M8 song rows, enabled S3M channels to M8 tracks, and packed pattern rows to phrases/chains.".to_string(),
            "PCM instruments are exported as mono WAV files; AdLib/OPL instruments are reported and skipped.".to_string(),
            "S3M tick effects are listed in unsupported_effects unless they can be represented as a static phrase value.".to_string(),
        ],
    };

    let report_bytes = serde_json::to_vec_pretty(&report).map_err(ConvertError::Report)?;
    files.push(bundle_file(
        format!("{bundle_root}conversion-report.json"),
        "application/json",
        report_bytes,
    ));

    let manifest = s3m_manifest_json(&module, &project_name)?;
    files.push(bundle_file(
        format!("{bundle_root}m8convert-project.json"),
        "application/json",
        manifest,
    ));

    Ok(ConvertedBundle {
        project_name,
        files,
        report,
    })
}

fn source_report(module: &Module) -> SourceReport {
    SourceReport {
        format: "MOD".to_string(),
        title: module.title.clone(),
        signature: module.signature.clone(),
        channel_count: module.channel_count,
        order_count: module.orders.len(),
        pattern_count: module.pattern_count,
        sample_count: module
            .samples
            .iter()
            .filter(|sample| sample.length_bytes > 0)
            .count(),
    }
}

fn hvl_source_report(module: &HvlModule) -> SourceReport {
    SourceReport {
        format: format!("HVL{}", module.version),
        title: module.title.clone(),
        signature: Some(format!("HVL{}", module.version)),
        channel_count: module.channel_count,
        order_count: module.position_count,
        pattern_count: module.track_count,
        sample_count: module.instrument_count,
    }
}

fn s3m_source_report(module: &S3mModule) -> SourceReport {
    SourceReport {
        format: "S3M".to_string(),
        title: module.title.clone(),
        signature: Some("SCRM".to_string()),
        channel_count: module.active_channels.len(),
        order_count: module.orders.len(),
        pattern_count: module.patterns.len(),
        sample_count: module
            .instruments
            .iter()
            .filter(|sample| sample.is_pcm())
            .count(),
    }
}

fn manifest_json(module: &Module, project_name: &str) -> Result<Vec<u8>, ConvertError> {
    #[derive(Serialize)]
    struct Manifest<'a> {
        project_name: &'a str,
        source_title: &'a str,
        samples: Vec<ManifestSample<'a>>,
    }

    #[derive(Serialize)]
    struct ManifestSample<'a> {
        index: usize,
        name: &'a str,
        filename: String,
        length_bytes: usize,
        finetune: i8,
        volume: u8,
        loop_start_bytes: usize,
        loop_length_bytes: usize,
    }

    let samples = module
        .samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| sample.length_bytes > 0)
        .map(|(index, sample)| ManifestSample {
            index: index + 1,
            name: &sample.name,
            filename: format!("Samples/{}", sample_filename(index, &sample.name)),
            length_bytes: sample.length_bytes,
            finetune: sample.finetune,
            volume: sample.volume,
            loop_start_bytes: sample.loop_start_bytes,
            loop_length_bytes: sample.loop_length_bytes,
        })
        .collect();

    serde_json::to_vec_pretty(&Manifest {
        project_name,
        source_title: &module.title,
        samples,
    })
    .map_err(ConvertError::Report)
}

fn hvl_manifest_json(module: &HvlModule, project_name: &str) -> Result<Vec<u8>, ConvertError> {
    #[derive(Serialize)]
    struct Manifest<'a> {
        project_name: &'a str,
        source_title: &'a str,
        format: &'static str,
        channels: usize,
        positions: usize,
        track_length: usize,
        instruments: Vec<ManifestInstrument<'a>>,
    }

    #[derive(Serialize)]
    struct ManifestInstrument<'a> {
        index: usize,
        name: &'a str,
        volume: u8,
        wave_length: u8,
        plist_length: usize,
    }

    let instruments = module
        .instruments
        .iter()
        .enumerate()
        .map(|(index, instrument)| ManifestInstrument {
            index: index + 1,
            name: &instrument.name,
            volume: instrument.volume,
            wave_length: instrument.wave_length,
            plist_length: instrument.plist.len(),
        })
        .collect();

    serde_json::to_vec_pretty(&Manifest {
        project_name,
        source_title: &module.title,
        format: "HVL",
        channels: module.channel_count,
        positions: module.position_count,
        track_length: module.track_length,
        instruments,
    })
    .map_err(ConvertError::Report)
}

fn s3m_manifest_json(module: &S3mModule, project_name: &str) -> Result<Vec<u8>, ConvertError> {
    #[derive(Serialize)]
    struct Manifest<'a> {
        project_name: &'a str,
        source_title: &'a str,
        format: &'static str,
        channels: usize,
        orders: usize,
        patterns: usize,
        initial_speed: u8,
        initial_tempo: u8,
        samples: Vec<ManifestSample<'a>>,
    }

    #[derive(Serialize)]
    struct ManifestSample<'a> {
        index: usize,
        name: &'a str,
        filename: String,
        length: u32,
        volume: u8,
        c5_speed: u32,
        loop_start: u32,
        loop_end: u32,
        is_16bit: bool,
        is_stereo: bool,
    }

    let samples = module
        .instruments
        .iter()
        .enumerate()
        .filter(|(_, sample)| sample.is_pcm())
        .map(|(index, sample)| ManifestSample {
            index: index + 1,
            name: &sample.name,
            filename: format!("Samples/{}", s3m_sample_filename(index, &sample.name)),
            length: sample.length,
            volume: sample.volume,
            c5_speed: sample.c5_speed,
            loop_start: sample.loop_start,
            loop_end: sample.loop_end,
            is_16bit: sample.is_16bit(),
            is_stereo: sample.is_stereo(),
        })
        .collect();

    serde_json::to_vec_pretty(&Manifest {
        project_name,
        source_title: &module.title,
        format: "S3M",
        channels: module.active_channels.len(),
        orders: module.orders.len(),
        patterns: module.patterns.len(),
        initial_speed: module.initial_speed,
        initial_tempo: module.initial_tempo,
        samples,
    })
    .map_err(ConvertError::Report)
}

fn bundle_file(path: impl Into<String>, mime_type: impl Into<String>, data: Vec<u8>) -> BundleFile {
    BundleFile {
        path: path.into(),
        mime_type: mime_type.into(),
        size: data.len(),
        data_base64: STANDARD.encode(data),
    }
}

fn bundle_root(project_name: &str) -> String {
    format!("Bundles/{project_name}/")
}

fn m8_options(options: &ConversionOptions, project_name: &str) -> ConversionOptions {
    ConversionOptions {
        song_name: Some(project_name.to_string()),
        bundle_directory: options
            .bundle_directory
            .clone()
            .or_else(|| Some(format!("/Bundles/{project_name}/"))),
    }
}

fn project_name(module: &Module, override_name: Option<&str>) -> String {
    project_name_from_title(&module.title, override_name, "converted_mod")
}

fn project_name_from_title(title: &str, override_name: Option<&str>, fallback: &str) -> String {
    let raw = override_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(title);
    let sanitized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    let name = sanitized.trim_matches('_');
    if name.is_empty() {
        fallback.to_string()
    } else {
        name.chars().take(32).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use m8_file_parser::Song;

    #[test]
    fn converts_minimal_mod_to_readable_m8_bundle() {
        let input = minimal_mod();
        let bundle =
            convert_mod(&input, ConversionOptions::default()).expect("conversion succeeds");

        assert!(bundle.files.iter().any(|file| file.path.ends_with(".m8s")));
        assert!(bundle.files.iter().any(|file| file.path.ends_with(".wav")));
        assert!(
            bundle
                .files
                .iter()
                .all(|file| file.path.starts_with("Bundles/TEST/"))
        );
        assert_eq!(bundle.report.m8.unsupported_effects.len(), 0);

        let song_file = bundle
            .files
            .iter()
            .find(|file| file.path.ends_with(".m8s"))
            .expect("m8s file");
        let song_bytes = STANDARD.decode(&song_file.data_base64).expect("base64");
        let mut reader: &[u8] = &song_bytes;
        let song = Song::read(&mut reader).expect("m8 song is readable");
        assert_eq!(song.directory, "/Bundles/TEST/");
        assert_ne!(song.song.steps[0], 0xff);
    }

    #[test]
    fn converts_minimal_hvl_to_readable_m8_bundle() {
        let input = minimal_hvl();
        let bundle =
            convert_tracker(&input, ConversionOptions::default()).expect("conversion succeeds");

        assert_eq!(bundle.report.source.format, "HVL1");
        assert!(bundle.files.iter().any(|file| file.path.ends_with(".m8s")));
        assert!(!bundle.files.iter().any(|file| file.path.ends_with(".wav")));

        let song_file = bundle
            .files
            .iter()
            .find(|file| file.path.ends_with(".m8s"))
            .expect("m8s file");
        let song_bytes = STANDARD.decode(&song_file.data_base64).expect("base64");
        let mut reader: &[u8] = &song_bytes;
        let song = Song::read(&mut reader).expect("m8 song is readable");
        assert_ne!(song.song.steps[0], 0xff);
    }

    #[test]
    fn converts_minimal_s3m_to_readable_m8_bundle() {
        let input = minimal_s3m();
        let bundle =
            convert_tracker(&input, ConversionOptions::default()).expect("conversion succeeds");

        assert_eq!(bundle.report.source.format, "S3M");
        assert!(bundle.files.iter().any(|file| file.path.ends_with(".m8s")));
        assert!(bundle.files.iter().any(|file| file.path.ends_with(".wav")));

        let song_file = bundle
            .files
            .iter()
            .find(|file| file.path.ends_with(".m8s"))
            .expect("m8s file");
        let song_bytes = STANDARD.decode(&song_file.data_base64).expect("base64");
        let mut reader: &[u8] = &song_bytes;
        let song = Song::read(&mut reader).expect("m8 song is readable");
        assert_ne!(song.song.steps[0], 0xff);
    }

    fn minimal_mod() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut title = [0u8; 20];
        title[..4].copy_from_slice(b"TEST");
        bytes.extend_from_slice(&title);

        for index in 0..31 {
            let mut descriptor = [0u8; 30];
            if index == 0 {
                descriptor[..4].copy_from_slice(b"BEEP");
                descriptor[22..24].copy_from_slice(&2u16.to_be_bytes());
                descriptor[25] = 64;
            }
            bytes.extend_from_slice(&descriptor);
        }

        bytes.push(1);
        bytes.push(0);
        bytes.extend_from_slice(&[0u8; 128]);
        bytes.extend_from_slice(b"M.K.");

        let mut pattern = vec![0u8; 64 * 4 * 4];
        pattern[0] = 0x03;
        pattern[1] = 0x58;
        pattern[2] = 0x10;
        pattern[3] = 0x00;
        bytes.extend_from_slice(&pattern);
        bytes.extend_from_slice(&[0x80, 0xff, 0x00, 0x7f]);
        bytes
    }

    fn minimal_hvl() -> Vec<u8> {
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
        out.extend_from_slice(&instrument);
        out.extend_from_slice(b"HVLTEST\0SYNTH\0");
        out
    }

    fn minimal_s3m() -> Vec<u8> {
        crate::s3mfile::tests::minimal_s3m()
    }
}
