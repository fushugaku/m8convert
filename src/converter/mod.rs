use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod events;
pub mod hvlfile;
pub mod m8;
pub mod modfile;
pub mod render;
pub mod s3mfile;
pub mod wav;
pub mod xmfile;

use crate::converter::hvlfile::{HvlError, HvlModule, is_hvl, parse_hvl};
use crate::converter::m8::{
    M8Error, M8Report, ReferenceRenderReport, SourceUnsupportedEffect, export_editable_m8,
    export_hvl_editable_m8, export_reference_stem_m8, export_s3m_editable_m8, s3m_sample_filename,
    sample_filename,
};
use crate::converter::modfile::{ModError, Module, parse_mod};
use crate::converter::s3mfile::{S3mError, S3mModule, is_s3m, parse_s3m};
use crate::converter::wav::{encode_s3m_sample_as_wav, encode_sample_as_wav};
use crate::converter::xmfile::{XmError, XmModule, is_xm, parse_xm};
use render::{DEFAULT_REFERENCE_MAX_SECONDS, DEFAULT_REFERENCE_SAMPLE_RATE, render_reference_mix};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceRenderMode {
    Never,
    #[default]
    WhenNeeded,
    Always,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversionOptions {
    pub song_name: Option<String>,
    pub bundle_directory: Option<String>,
    #[serde(default)]
    pub reference_render: ReferenceRenderMode,
    pub reference_sample_rate: Option<u32>,
    pub reference_max_seconds: Option<u32>,
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
    Xm(#[from] XmError),
    #[error(transparent)]
    M8(#[from] M8Error),
    #[error("could not serialize conversion report: {0}")]
    Report(serde_json::Error),
    #[error("could not render reference mix: {0}")]
    ReferenceRender(String),
}

pub fn convert_tracker(
    input: &[u8],
    options: ConversionOptions,
) -> Result<ConvertedBundle, ConvertError> {
    if is_hvl(input) {
        convert_hvl(input, options)
    } else if is_s3m(input) {
        convert_s3m(input, options)
    } else if is_xm(input) {
        convert_xm(input, options)
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
    let mut m8_export = export_editable_m8(&module, &m8_options)?;

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

    let needs_reference = report_needs_reference(&m8_export.report);
    append_reference_render(
        input,
        &options,
        &project_name,
        &bundle_root,
        needs_reference,
        &mut files,
        &mut m8_export.report,
    )?;

    let report = ConversionReport {
        source: source_report(&module),
        m8: m8_export.report,
        notes: vec![
            "Editable conversion simulates MOD pattern flow, maps playback rows to M8 song rows, MOD channels to M8 tracks, and writes M8 SNG hops for detected playback loops/restart positions where possible.".to_string(),
            "Samples are exported as unsigned 8-bit mono WAV files with loop metadata, loop end, and MOD finetune mapped into M8 sampler parameters when present.".to_string(),
            "ProTracker tick effects are mapped to phrase FX or short M8 tables where possible, including slides, vibrato, tremolo, retrigger, and finetune/pan subcommands; remaining commands are listed in unsupported_effects, and near-matches are listed in approximations.".to_string(),
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
    let mut m8_export = export_hvl_editable_m8(&module, &m8_options)?;

    let mut files = vec![bundle_file(
        format!("{bundle_root}{project_name}.m8s"),
        "application/octet-stream",
        m8_export.song_bytes,
    )];
    append_hvl_source_fallback(
        input,
        &options,
        &bundle_root,
        &mut files,
        &mut m8_export.report,
    );

    let report = ConversionReport {
        source: hvl_source_report(&module),
        m8: m8_export.report,
        notes: vec![
            "Editable HVL conversion follows position jumps, breaks, stop/restart flow, maps visited positions to M8 song rows, HVL tracks to chains/phrases, and applies position transpose once through M8 chains.".to_string(),
            "HVL instruments are synthesized; this converter approximates them as M8 WavSynth patches, maps envelopes/vibrato into modulators, and maps performance-list commands to M8 tables where possible.".to_string(),
            "HVL track commands are mapped for pitch/portamento, filter override, square offset, panning, volume slides, fine slides, note cut, speed, and restart-loop cases; remaining commands are listed in unsupported_effects/warnings, and the original source is retained under Reference/ when fallback is enabled.".to_string(),
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
    let mut m8_export = export_s3m_editable_m8(&module, &m8_options)?;

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

    let has_opl = module
        .instruments
        .iter()
        .any(|instrument| !matches!(instrument.kind, 0 | 1));
    let needs_reference = has_opl || report_needs_reference(&m8_export.report);
    append_reference_render(
        input,
        &options,
        &project_name,
        &bundle_root,
        needs_reference,
        &mut files,
        &mut m8_export.report,
    )?;

    let report = ConversionReport {
        source: s3m_source_report(&module),
        m8: m8_export.report,
        notes: vec![
            "Experimental editable S3M conversion simulates pattern flow, maps playback rows to M8 song rows, enabled S3M channels to M8 tracks, and writes M8 SNG hops for detected playback loops where possible.".to_string(),
            "PCM instruments are exported as 16-bit mono or stereo WAV files at each S3M instrument's C2SPD rate with loop metadata/loop end so M8 transposition starts from the same C-4 tuning; AdLib/OPL instruments are reported in the editable project and preserved by the reference-render stem when enabled.".to_string(),
            "S3M tick effects are mapped to M8 phrase FX or short tables where possible, including pitch bend, portamento, vibrato, tremor/tremolo, panning, speed-aware volume slides, global volume, volume-column shortcuts, sample offset memory, and retrigger; remaining commands are listed in unsupported_effects, and near-matches are listed in approximations.".to_string(),
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

pub fn convert_xm(
    input: &[u8],
    options: ConversionOptions,
) -> Result<ConvertedBundle, ConvertError> {
    let module = parse_xm(input)?;
    let xm_adapter = module.to_s3m_adapter();
    let s3m_module = &xm_adapter.module;
    let project_name =
        project_name_from_title(&module.title, options.song_name.as_deref(), "converted_xm");
    let bundle_root = bundle_root(&project_name);
    let m8_options = m8_options(&options, &project_name);
    let mut m8_export = export_s3m_editable_m8(s3m_module, &m8_options)?;
    m8_export.report.warnings.extend(xm_adapter.warnings);
    m8_export.report.source_unsupported_effects.extend(
        xm_adapter
            .unsupported_effects
            .into_iter()
            .map(|effect| SourceUnsupportedEffect {
                format: "XM".to_string(),
                order: effect.order,
                pattern: effect.pattern,
                row: effect.row,
                channel: effect.channel,
                effect: effect.effect,
                param: effect.param,
                detail: effect.detail,
            }),
    );
    m8_export
        .report
        .approximations
        .extend(xm_adapter.approximations.into_iter().map(|approx| {
            crate::converter::m8::Approximation {
                order: approx.order,
                pattern: approx.pattern,
                row: approx.row,
                channel: approx.channel,
                category: approx.category,
                detail: approx.detail,
            }
        }));

    let mut files = vec![bundle_file(
        format!("{bundle_root}{project_name}.m8s"),
        "application/octet-stream",
        m8_export.song_bytes,
    )];

    for (index, sample) in s3m_module.instruments.iter().enumerate() {
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

    let needs_reference = report_needs_reference(&m8_export.report);
    append_reference_render(
        input,
        &options,
        &project_name,
        &bundle_root,
        needs_reference,
        &mut files,
        &mut m8_export.report,
    )?;

    let report = ConversionReport {
        source: xm_source_report(&module),
        m8: m8_export.report,
        notes: vec![
            "Experimental editable XM conversion parses FastTracker patterns through a native XM order/row timeline before adapting to M8, including instruments, PCM sample data, speed, tempo, note-off events, panning, and common tracker effects.".to_string(),
            "XM instruments are adapted through the S3M-compatible M8 exporter; populated samples are flattened to M8 sampler instruments, note sample maps are applied with channel memory, sample panning and ping-pong loop mode are preserved, and relative-note/finetune is folded into each WAV sample rate.".to_string(),
            "Common XM effects and volume-column commands are mapped to matching S3M/M8 effect paths where possible, including panning slides and volume-column vibrato approximations; unsupported or format-specific commands are surfaced in unsupported_effects/approximations with XM order/pattern/row context.".to_string(),
            "Simple XM volume envelopes, fadeout, and auto-vibrato are approximated as M8 sampler modulation metadata; complex envelopes still need an audio render/stem path for exact playback.".to_string(),
        ],
    };

    let report_bytes = serde_json::to_vec_pretty(&report).map_err(ConvertError::Report)?;
    files.push(bundle_file(
        format!("{bundle_root}conversion-report.json"),
        "application/json",
        report_bytes,
    ));

    let manifest = xm_manifest_json(&module, s3m_module, &project_name)?;
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

fn reference_render_requested(options: &ConversionOptions, needed: bool) -> bool {
    match options.reference_render {
        ReferenceRenderMode::Never => false,
        ReferenceRenderMode::WhenNeeded => needed,
        ReferenceRenderMode::Always => true,
    }
}

fn report_needs_reference(report: &M8Report) -> bool {
    report.dropped_tracks != 0
        || report.dropped_phrases != 0
        || report.dropped_chains != 0
        || report.dropped_tables != 0
        || !report.unsupported_effects.is_empty()
        || !report.source_unsupported_effects.is_empty()
        || !report.approximations.is_empty()
}

fn append_reference_render(
    input: &[u8],
    options: &ConversionOptions,
    project_name: &str,
    bundle_root: &str,
    needed: bool,
    files: &mut Vec<BundleFile>,
    report: &mut M8Report,
) -> Result<(), ConvertError> {
    if !reference_render_requested(options, needed) {
        return Ok(());
    }

    let sample_rate = options
        .reference_sample_rate
        .unwrap_or(DEFAULT_REFERENCE_SAMPLE_RATE);
    let max_seconds = options
        .reference_max_seconds
        .unwrap_or(DEFAULT_REFERENCE_MAX_SECONDS);
    let render = match render_reference_mix(input, sample_rate, max_seconds) {
        Ok(render) => render,
        Err(error) if options.reference_render == ReferenceRenderMode::Always => {
            return Err(ConvertError::ReferenceRender(error));
        }
        Err(error) => {
            report.warnings.push(format!(
                "reference render was requested but failed: {error}"
            ));
            return Ok(());
        }
    };

    let wav_name = "reference-mix.wav";
    let wav_path = format!("{bundle_root}Samples/{wav_name}");
    let reference_song_name = format!("{project_name}-reference");
    let m8_path = format!("{bundle_root}{reference_song_name}.m8s");
    let bundle_directory = options
        .bundle_directory
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| format!("/Bundles/{project_name}/"));
    let reference_song = export_reference_stem_m8(
        &reference_song_name,
        Some(&bundle_directory),
        &format!("Samples/{wav_name}"),
    )?;

    let duration_seconds = render.frames as f64 / render.sample_rate as f64;
    report.reference_render = Some(ReferenceRenderReport {
        renderer: "xmrsplayer 0.14".to_string(),
        sample_rate: render.sample_rate,
        frames: render.frames,
        duration_seconds,
        truncated: render.truncated,
        wav_path: wav_path.clone(),
        m8_path: m8_path.clone(),
    });
    if render.truncated {
        report.warnings.push(format!(
            "reference render reached the configured {max_seconds}s safety limit and was truncated"
        ));
    }
    files.push(bundle_file(wav_path, "audio/wav", render.wav_bytes));
    files.push(bundle_file(
        m8_path,
        "application/octet-stream",
        reference_song,
    ));
    Ok(())
}

fn append_hvl_source_fallback(
    input: &[u8],
    options: &ConversionOptions,
    bundle_root: &str,
    files: &mut Vec<BundleFile>,
    report: &mut M8Report,
) {
    if !reference_render_requested(options, true) {
        return;
    }
    let path = format!("{bundle_root}Reference/source.hvl");
    files.push(bundle_file(
        path.clone(),
        "application/octet-stream",
        input.to_vec(),
    ));
    report.source_fallback_path = Some(path);
    report.warnings.push(
        "the original HVL was preserved under Reference/ because its synthesis cannot yet be represented exactly by an editable M8 WavSynth patch"
            .to_string(),
    );
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

fn xm_source_report(module: &XmModule) -> SourceReport {
    SourceReport {
        format: "XM".to_string(),
        title: module.title.clone(),
        signature: Some("Extended Module".to_string()),
        channel_count: module.channel_count,
        order_count: module.orders.len(),
        pattern_count: module.patterns.len(),
        sample_count: module
            .instruments
            .iter()
            .flat_map(|instrument| instrument.samples.iter())
            .filter(|sample| !sample.data.is_empty())
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
        exported_sample_rate: u32,
        is_16bit: bool,
        is_stereo: bool,
        ping_pong_loop: bool,
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
            exported_sample_rate: sample.c5_speed.max(1),
            is_16bit: sample.is_16bit(),
            is_stereo: sample.is_stereo(),
            ping_pong_loop: sample.ping_pong_loop,
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

fn xm_manifest_json(
    module: &XmModule,
    s3m_module: &S3mModule,
    project_name: &str,
) -> Result<Vec<u8>, ConvertError> {
    #[derive(Serialize)]
    struct Manifest<'a> {
        project_name: &'a str,
        source_title: &'a str,
        format: &'static str,
        tracker_name: &'a str,
        channels: usize,
        orders: usize,
        patterns: usize,
        initial_speed: u8,
        initial_tempo: u8,
        instruments: usize,
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
        exported_sample_rate: u32,
        is_16bit: bool,
        ping_pong_loop: bool,
    }

    let samples = s3m_module
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
            exported_sample_rate: sample.c5_speed.max(1),
            is_16bit: sample.is_16bit(),
            ping_pong_loop: sample.ping_pong_loop,
        })
        .collect();

    serde_json::to_vec_pretty(&Manifest {
        project_name,
        source_title: &module.title,
        format: "XM",
        tracker_name: &module.tracker_name,
        channels: module.channel_count,
        orders: module.orders.len(),
        patterns: module.patterns.len(),
        initial_speed: module.initial_speed,
        initial_tempo: module.initial_tempo,
        instruments: module.instruments.len(),
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
        reference_render: options.reference_render,
        reference_sample_rate: options.reference_sample_rate,
        reference_max_seconds: options.reference_max_seconds,
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
    use m8_file_parser::{Instrument, Song};

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
        assert_tempo(song.tempo, 125.0);
        assert_ne!(song.song.steps[0], 0xff);
        assert_sampler_pitch_defaults(&song);
    }

    #[test]
    fn always_mode_adds_reference_wav_and_m8_stem_project() {
        let input = minimal_mod();
        let bundle = convert_mod(
            &input,
            ConversionOptions {
                reference_render: ReferenceRenderMode::Always,
                reference_sample_rate: Some(8_000),
                reference_max_seconds: Some(1),
                ..ConversionOptions::default()
            },
        )
        .expect("conversion succeeds");

        assert!(
            bundle
                .files
                .iter()
                .any(|file| file.path.ends_with("Samples/reference-mix.wav"))
        );
        assert!(
            bundle
                .files
                .iter()
                .any(|file| file.path.ends_with("TEST-reference.m8s"))
        );
        let reference = bundle
            .report
            .m8
            .reference_render
            .expect("reference render metadata");
        assert_eq!(reference.sample_rate, 8_000);
        assert_eq!(reference.frames, 8_000);
        assert!(reference.truncated);
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
    fn hvl_position_transpose_is_applied_once_by_the_chain() {
        let mut input = minimal_hvl();
        input[17] = 2;
        let bundle =
            convert_tracker(&input, ConversionOptions::default()).expect("conversion succeeds");
        let song_file = bundle
            .files
            .iter()
            .find(|file| file.path.ends_with(".m8s"))
            .expect("m8s file");
        let song_bytes = STANDARD.decode(&song_file.data_base64).expect("base64");
        let mut reader: &[u8] = &song_bytes;
        let song = Song::read(&mut reader).expect("m8 song is readable");
        let chain = &song.chains[song.song.steps[0] as usize];
        let phrase = &song.phrases[chain.steps[0].phrase as usize];

        assert_eq!(phrase.steps[0].note.0, 24);
        assert_eq!(chain.steps[0].transpose, 2);
    }

    #[test]
    fn hvl_speed_effect_updates_m8_tempo() {
        let mut input = minimal_hvl();
        input[26] = 0xf0;
        input[27] = 3;
        let bundle =
            convert_tracker(&input, ConversionOptions::default()).expect("conversion succeeds");
        let song_file = bundle
            .files
            .iter()
            .find(|file| file.path.ends_with(".m8s"))
            .expect("m8s file");
        let song_bytes = STANDARD.decode(&song_file.data_base64).expect("base64");
        let mut reader: &[u8] = &song_bytes;
        let song = Song::read(&mut reader).expect("m8 song is readable");
        let chain = &song.chains[song.song.steps[0] as usize];
        let phrase = &song.phrases[chain.steps[0].phrase as usize];

        assert_tempo(song.tempo, 125.0);
        assert!(
            [
                phrase.steps[0].fx1,
                phrase.steps[0].fx2,
                phrase.steps[0].fx3
            ]
            .iter()
            .any(|fx| fx.command == 0x18 && fx.value == 250)
        );
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
        assert_tempo(song.tempo, 125.0);
        assert_ne!(song.song.steps[0], 0xff);
        assert_sampler_pitch_defaults(&song);
    }

    #[test]
    fn converts_minimal_xm_to_readable_m8_bundle() {
        let input = minimal_xm();
        let bundle =
            convert_tracker(&input, ConversionOptions::default()).expect("conversion succeeds");

        assert_eq!(bundle.report.source.format, "XM");
        assert_eq!(bundle.project_name, "XMTEST");
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
        assert_tempo(song.tempo, 125.0);
        assert_ne!(song.song.steps[0], 0xff);
        assert_sampler_pitch_defaults(&song);
    }

    #[test]
    fn converts_s3m_speed_to_m8_tempo() {
        let mut input = minimal_s3m();
        input[0x31] = 3;
        input[0x32] = 125;
        let bundle =
            convert_tracker(&input, ConversionOptions::default()).expect("conversion succeeds");
        let song_file = bundle
            .files
            .iter()
            .find(|file| file.path.ends_with(".m8s"))
            .expect("m8s file");
        let song_bytes = STANDARD.decode(&song_file.data_base64).expect("base64");
        let mut reader: &[u8] = &song_bytes;
        let song = Song::read(&mut reader).expect("m8 song is readable");
        assert_tempo(song.tempo, 250.0);
        assert_tempo(bundle.report.m8.applied_tempo_bpm.unwrap(), 250.0);
    }

    fn assert_sampler_pitch_defaults(song: &Song) {
        let sampler = song
            .instruments
            .iter()
            .find_map(|instrument| match instrument {
                Instrument::Sampler(sampler) => Some(sampler),
                _ => None,
            })
            .expect("sampler instrument");
        assert_eq!(sampler.synth_params.pitch, 0);
        assert_eq!(sampler.synth_params.fine_tune, 0x80);
    }

    fn assert_tempo(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.01, "{actual} != {expected}");
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
        crate::converter::s3mfile::tests::minimal_s3m()
    }

    fn minimal_xm() -> Vec<u8> {
        crate::converter::xmfile::tests::minimal_xm()
    }
}
