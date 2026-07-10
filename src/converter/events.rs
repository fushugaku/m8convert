use base64::{Engine, engine::general_purpose::STANDARD};
use m8_file_parser::{FX, Instrument, Song, reader::Reader};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::converter::hvlfile::{HvlError, HvlModule, HvlStep, is_hvl, parse_hvl};
use crate::converter::modfile::{ModError, parse_mod, period_to_note};
use crate::converter::s3mfile::{S3mError, is_s3m, parse_s3m, s3m_note_to_m8};
use crate::converter::xmfile::{XmError, is_xm, parse_xm};
use crate::converter::{
    ConversionOptions, ConvertError, ConvertedBundle, ReferenceRenderMode, convert_tracker,
};

const EMPTY: u8 = 0xff;
const NOTE_OFF: u8 = 0x80;
const M8_TRACKS: usize = 8;
const FX_PSL: u8 = 0x0c;
const FX_PBN: u8 = 0x0d;
const FX_PVB: u8 = 0x0e;
const FX_SAMPLER_PAN: u8 = 0x8d;

#[derive(Debug, Error)]
pub enum EventHarnessError {
    #[error(transparent)]
    Convert(#[from] ConvertError),
    #[error(transparent)]
    Decode(#[from] base64::DecodeError),
    #[error(transparent)]
    Mod(#[from] ModError),
    #[error(transparent)]
    Hvl(#[from] HvlError),
    #[error(transparent)]
    S3m(#[from] S3mError),
    #[error(transparent)]
    Xm(#[from] XmError),
    #[error("converted bundle does not contain an M8 song file")]
    MissingM8,
    #[error("could not parse converted M8 song: {0}")]
    M8(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRegressionReport {
    pub source_format: String,
    pub source_title: String,
    pub source_initial_bpm: f32,
    pub m8_initial_bpm: f32,
    pub source_events: Vec<TrackerEvent>,
    pub m8_events: Vec<M8Event>,
    pub comparison: EventComparison,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerEvent {
    pub playback_row: usize,
    pub order: usize,
    pub pattern: u8,
    pub row: usize,
    pub channel: usize,
    pub note: Option<u8>,
    pub note_off: bool,
    pub instrument: Option<u8>,
    pub volume: Option<u8>,
    pub expected_velocity: Option<u8>,
    pub pan: Option<u8>,
    pub pitch: bool,
    pub effect: Option<EventFx>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct M8Event {
    pub song_row: usize,
    pub track: usize,
    pub chain_row: usize,
    pub phrase_row: usize,
    pub absolute_row: usize,
    pub note: Option<u8>,
    pub note_off: bool,
    pub instrument: Option<u8>,
    pub velocity: Option<u8>,
    pub pan: Option<u8>,
    pub effects: Vec<EventFx>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFx {
    pub command: u8,
    pub value: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventComparison {
    pub source_note_events: usize,
    pub m8_note_events: usize,
    pub source_note_off_events: usize,
    pub m8_note_off_events: usize,
    pub source_volume_events: usize,
    pub m8_velocity_events: usize,
    pub source_pan_events: usize,
    pub m8_pan_events: usize,
    pub source_pitch_events: usize,
    pub m8_pitch_events: usize,
    pub source_effect_events: usize,
    pub m8_effect_events: usize,
    pub matched_note_events: usize,
    pub note_value_mismatches: usize,
    pub note_row_mismatches: usize,
    pub instrument_value_mismatches: usize,
    pub velocity_value_mismatches: usize,
    pub pan_value_mismatches: usize,
    pub source_initial_bpm: f32,
    pub m8_initial_bpm: f32,
    pub initial_bpm_delta: f32,
    pub differences: Vec<String>,
}

pub fn build_event_regression_report(
    input: &[u8],
    options: ConversionOptions,
) -> Result<EventRegressionReport, EventHarnessError> {
    let source = source_events(input)?;
    let mut editable_options = options;
    editable_options.reference_render = ReferenceRenderMode::Never;
    let bundle = convert_tracker(input, editable_options)?;
    let song_bytes = converted_m8_bytes(&bundle)?;
    let (m8_events, m8_initial_bpm) = m8_events(&song_bytes)?;
    let comparison = compare_events(
        &source.events,
        &m8_events,
        source.initial_bpm,
        m8_initial_bpm,
    );

    Ok(EventRegressionReport {
        source_format: source.format,
        source_title: source.title,
        source_initial_bpm: source.initial_bpm,
        m8_initial_bpm,
        source_events: source.events,
        m8_events,
        comparison,
    })
}

struct SourceEvents {
    format: String,
    title: String,
    events: Vec<TrackerEvent>,
    initial_bpm: f32,
}

fn source_events(input: &[u8]) -> Result<SourceEvents, EventHarnessError> {
    if is_hvl(input) {
        let module = parse_hvl(input)?;
        let mut events = Vec::new();
        for (playback_row, (order, row)) in
            crate::converter::m8::hvl_playback_rows_for_events(&module)
                .into_iter()
                .enumerate()
        {
            let Some(position) = module.positions.get(order) else {
                continue;
            };
            for channel in 0..module.channel_count.min(M8_TRACKS) {
                let track_id = position.tracks[channel] as usize;
                let Some(step) = module
                    .tracks
                    .get(track_id)
                    .and_then(|track| track.rows.get(row))
                else {
                    continue;
                };
                if step.note == 0 && step.instrument == 0 && step.fx == 0 && step.fx_b == 0 {
                    continue;
                }
                let volume = hvl_event_volume(&module, *step);
                events.push(TrackerEvent {
                    playback_row,
                    order,
                    pattern: track_id.min(0xff) as u8,
                    row,
                    channel,
                    note: (step.note > 0).then_some(
                        (step.note as i16 - 1 + position.transposes[channel] as i16).clamp(0, 0x7f)
                            as u8,
                    ),
                    note_off: false,
                    instrument: step.instrument.checked_sub(1),
                    volume,
                    expected_velocity: volume.map(volume_to_velocity),
                    pan: hvl_effect_pan(step.fx, step.fx_param)
                        .or_else(|| hvl_effect_pan(step.fx_b, step.fx_b_param)),
                    pitch: hvl_pitch_command(step.fx, step.fx_param)
                        || hvl_pitch_command(step.fx_b, step.fx_b_param),
                    effect: tracker_fx(step.fx, step.fx_param)
                        .or_else(|| tracker_fx(step.fx_b, step.fx_b_param)),
                });
            }
        }
        return Ok(SourceEvents {
            format: format!("HVL{}", module.version),
            title: module.title,
            events,
            initial_bpm: (750.0 * module.speed_multiplier.max(1) as f32 / 6.0).clamp(20.0, 999.0),
        });
    }

    if is_s3m(input) {
        let module = parse_s3m(input)?;
        let mut events = Vec::new();
        let mut current_pans = module.channel_pans.clone();
        let mut current_instruments = vec![None; module.active_channels.len()];
        let mut channel_volumes = vec![64u8; module.active_channels.len()];
        let mut channel_volume_slide_memory = vec![0u8; module.active_channels.len()];
        let mut speed = module.initial_speed.max(1);
        for (playback_row, (order, pattern_id, row_index, emit_cells, global_volume)) in
            crate::converter::m8::s3m_playback_rows_for_events(&module)
                .into_iter()
                .enumerate()
        {
            if !emit_cells {
                continue;
            }
            let Some(row) = module
                .patterns
                .get(pattern_id as usize)
                .and_then(|pattern| pattern.rows.get(row_index))
            else {
                continue;
            };
            for (track, source_channel) in module.active_channels.iter().take(M8_TRACKS).enumerate()
            {
                let cell = row[*source_channel];
                if cell.command == 1 && cell.info != 0 {
                    speed = cell.info;
                }
                update_event_channel_volume(
                    cell.command,
                    cell.info,
                    speed,
                    &mut channel_volumes[track],
                    &mut channel_volume_slide_memory[track],
                );
                if cell.instrument > 0 {
                    current_instruments[track] = cell.instrument.checked_sub(1);
                }
                let explicit_pan = crate::converter::m8::s3m_volume_column_pan_for_events(
                    cell.volume,
                )
                .or_else(|| s3m_effect_pan(cell.command, cell.info, module.pan_command_uses_8bit));
                if let Some(pan) = explicit_pan
                    && let Some(current) = current_pans.get_mut(*source_channel)
                {
                    *current = pan;
                }
                if cell == crate::converter::s3mfile::S3mCell::EMPTY {
                    continue;
                }
                let note = s3m_note_to_m8(cell.note);
                let volume = if cell.volume <= 64 {
                    Some(cell.volume)
                } else if note.is_some_and(|note| note != NOTE_OFF) {
                    current_instruments[track]
                        .and_then(|instrument| module.instruments.get(instrument as usize))
                        .map(|instrument| instrument.volume)
                } else {
                    None
                };
                let pan = explicit_pan.or_else(|| {
                    note.filter(|note| *note != NOTE_OFF)
                        .and_then(|_| current_pans.get(*source_channel).copied())
                });
                events.push(TrackerEvent {
                    playback_row,
                    order,
                    pattern: pattern_id,
                    row: row_index,
                    channel: track,
                    note: note.filter(|value| *value != NOTE_OFF),
                    note_off: note == Some(NOTE_OFF),
                    instrument: cell.instrument.checked_sub(1),
                    volume,
                    expected_velocity: volume.map(|volume| {
                        scaled_tracker_velocity(volume, global_volume, channel_volumes[track])
                    }),
                    pan,
                    pitch: s3m_pitch_command(cell.command, cell.info)
                        || s3m_volume_column_pitch(cell.volume),
                    effect: tracker_fx(cell.command, cell.info),
                });
            }
        }
        return Ok(SourceEvents {
            format: "S3M".to_string(),
            title: module.title,
            events,
            initial_bpm: tracker_bpm(module.initial_speed, module.initial_tempo),
        });
    }

    if is_xm(input) {
        let module = parse_xm(input)?;
        let title = module.title.clone();
        let mut events = Vec::new();
        let mut current_instruments = vec![None; module.channel_count];
        let mut current_pans = vec![0x80; module.channel_count];
        let mut global_volume = 64u8;
        let mut global_slide_memory = 0u8;
        let mut speed = module.initial_speed.max(1);
        for (playback_row, (order, pattern_id, row_index, emit_cells, cells)) in
            module.playback_rows_for_events().into_iter().enumerate()
        {
            if !emit_cells {
                continue;
            }
            for (channel, cell) in cells.into_iter().take(M8_TRACKS).enumerate() {
                if cell.effect == 0x0f && (1..0x20).contains(&cell.effect_param) {
                    speed = cell.effect_param;
                }
                update_xm_global_volume(
                    cell.effect,
                    cell.effect_param,
                    speed,
                    &mut global_volume,
                    &mut global_slide_memory,
                );
                if cell.instrument > 0 {
                    current_instruments[channel] = cell.instrument.checked_sub(1).map(usize::from);
                }
                let note = match cell.note {
                    1..=96 => Some(cell.note - 1),
                    97 => Some(NOTE_OFF),
                    _ => None,
                };
                let sample = current_instruments[channel]
                    .and_then(|instrument| xm_sample_for_note(&module, instrument, cell.note));
                let explicit_pan = xm_effect_pan(cell.effect, cell.effect_param)
                    .or_else(|| xm_volume_column_pan(cell.volume));
                if let Some(pan) = explicit_pan {
                    current_pans[channel] = pan;
                } else if note.is_some_and(|note| note != NOTE_OFF)
                    && let Some(sample) = sample
                {
                    current_pans[channel] = sample.panning;
                }
                if cell == crate::converter::xmfile::XmCell::EMPTY {
                    continue;
                }
                let volume = match cell.volume {
                    0x10..=0x50 => Some(cell.volume - 0x10),
                    _ if note.is_some_and(|note| note != NOTE_OFF) => {
                        sample.map(|sample| sample.volume)
                    }
                    _ => None,
                };
                let instrument = if cell.instrument > 0 && note.is_some_and(|note| note != NOTE_OFF)
                {
                    xm_flattened_instrument_for_note(
                        &module,
                        cell.instrument.saturating_sub(1) as usize,
                        cell.note,
                    )
                } else {
                    None
                };
                events.push(TrackerEvent {
                    playback_row,
                    order,
                    pattern: pattern_id,
                    row: row_index,
                    channel,
                    note: note.filter(|note| *note != NOTE_OFF),
                    note_off: note == Some(NOTE_OFF),
                    instrument,
                    volume,
                    expected_velocity: volume
                        .map(|volume| scaled_tracker_velocity(volume, global_volume, 64)),
                    pan: explicit_pan.or_else(|| {
                        note.filter(|note| *note != NOTE_OFF)
                            .map(|_| current_pans[channel])
                    }),
                    pitch: xm_pitch_command(cell.effect, cell.effect_param)
                        || xm_volume_column_pitch(cell.volume),
                    effect: tracker_fx(cell.effect, cell.effect_param),
                });
            }
        }
        return Ok(SourceEvents {
            format: "XM".to_string(),
            title,
            events,
            initial_bpm: tracker_bpm(module.initial_speed, module.initial_tempo),
        });
    }

    let module = parse_mod(input)?;
    let mut events = Vec::new();
    let mut current_instruments = vec![None; module.channel_count];
    let mut current_pans = (0..module.channel_count)
        .map(mod_default_pan)
        .collect::<Vec<_>>();
    for (playback_row, (order, pattern_id, row_index, emit_cells)) in
        crate::converter::m8::mod_playback_rows_for_events(&module)
            .into_iter()
            .enumerate()
    {
        if !emit_cells {
            continue;
        }
        let Some(row) = module
            .patterns
            .get(pattern_id as usize)
            .and_then(|pattern| pattern.rows.get(row_index))
        else {
            continue;
        };
        for (channel, cell) in row
            .iter()
            .copied()
            .take(module.channel_count.min(M8_TRACKS))
            .enumerate()
        {
            if cell.sample_number > 0 {
                current_instruments[channel] = cell.sample_number.checked_sub(1);
            }
            let explicit_pan = mod_effect_pan(cell.effect, cell.effect_param);
            if let Some(pan) = explicit_pan {
                current_pans[channel] = pan;
            }
            if cell.is_empty() {
                continue;
            }
            let note = period_to_note(cell.period);
            let volume = if cell.effect == 0x0c {
                Some(cell.effect_param.min(64))
            } else if note.is_some() {
                current_instruments[channel]
                    .and_then(|instrument| module.samples.get(instrument as usize))
                    .map(|sample| sample.volume)
            } else {
                None
            };
            events.push(TrackerEvent {
                playback_row,
                order,
                pattern: pattern_id,
                row: row_index,
                channel,
                note,
                note_off: cell.effect == 0x0e && cell.effect_param >> 4 == 0x0c,
                instrument: cell.sample_number.checked_sub(1),
                volume,
                expected_velocity: volume.map(volume_to_velocity),
                pan: explicit_pan.or_else(|| note.map(|_| current_pans[channel])),
                pitch: mod_pitch_command(cell.effect, cell.effect_param),
                effect: tracker_fx(cell.effect, cell.effect_param),
            });
        }
    }

    let initial_bpm = mod_initial_bpm(&module);
    Ok(SourceEvents {
        format: module.signature.unwrap_or_else(|| "MOD".to_string()),
        title: module.title,
        events,
        initial_bpm,
    })
}

fn converted_m8_bytes(bundle: &ConvertedBundle) -> Result<Vec<u8>, EventHarnessError> {
    let file = bundle
        .files
        .iter()
        .find(|file| file.path.ends_with(".m8s"))
        .ok_or(EventHarnessError::MissingM8)?;
    Ok(STANDARD.decode(&file.data_base64)?)
}

fn m8_events(input: &[u8]) -> Result<(Vec<M8Event>, f32), EventHarnessError> {
    let mut reader = Reader::new(input.to_vec());
    let song = Song::read_from_reader(&mut reader)
        .map_err(|err| EventHarnessError::M8(format!("{err:?}")))?;
    let mut out = Vec::new();

    for (song_step_index, chain_id) in song.song.steps.iter().copied().enumerate() {
        if chain_id == EMPTY {
            continue;
        }
        let song_row = song_step_index / M8_TRACKS;
        let track = song_step_index % M8_TRACKS;
        let Some(chain) = song.chains.get(chain_id as usize) else {
            continue;
        };

        for (chain_row, chain_step) in chain.steps.iter().enumerate() {
            if chain_step.phrase == EMPTY {
                continue;
            }
            let Some(phrase) = song.phrases.get(chain_step.phrase as usize) else {
                continue;
            };
            for (phrase_row, step) in phrase.steps.iter().enumerate() {
                if step.is_empty() {
                    continue;
                }
                let effects = [step.fx1, step.fx2, step.fx3]
                    .into_iter()
                    .filter_map(m8_fx)
                    .collect::<Vec<_>>();
                let note = (!step.note.is_empty() && step.note.0 != NOTE_OFF).then(|| {
                    (i16::from(step.note.0) + i16::from(chain_step.transpose as i8)).clamp(0, 0x7f)
                        as u8
                });
                let pan = effects
                    .iter()
                    .rev()
                    .find(|effect| effect.command == FX_SAMPLER_PAN)
                    .map(|effect| effect.value)
                    .or_else(|| {
                        (step.instrument != EMPTY)
                            .then_some(step.instrument)
                            .and_then(|instrument| song.instruments.get(instrument as usize))
                            .and_then(|instrument| match instrument {
                                Instrument::Sampler(instrument) => {
                                    Some(instrument.synth_params.mixer_pan)
                                }
                                Instrument::WavSynth(instrument) => {
                                    Some(instrument.synth_params.mixer_pan)
                                }
                                _ => None,
                            })
                    });
                out.push(M8Event {
                    song_row,
                    track,
                    chain_row,
                    phrase_row,
                    absolute_row: song_row * 64 + chain_row * 16 + phrase_row,
                    note,
                    note_off: step.note.0 == NOTE_OFF,
                    instrument: (step.instrument != EMPTY).then_some(step.instrument),
                    velocity: (step.velocity != EMPTY).then_some(step.velocity),
                    pan,
                    effects,
                });
            }
        }
    }

    out.sort_by_key(|event| (event.absolute_row, event.track));
    Ok((out, song.tempo))
}

fn compare_events(
    source_events: &[TrackerEvent],
    m8_events: &[M8Event],
    source_initial_bpm: f32,
    m8_initial_bpm: f32,
) -> EventComparison {
    let mut comparison = EventComparison {
        source_note_events: source_events
            .iter()
            .filter(|event| event.note.is_some())
            .count(),
        m8_note_events: m8_events
            .iter()
            .filter(|event| event.note.is_some())
            .count(),
        source_note_off_events: source_events.iter().filter(|event| event.note_off).count(),
        m8_note_off_events: m8_events.iter().filter(|event| event.note_off).count(),
        source_volume_events: source_events
            .iter()
            .filter(|event| event.volume.is_some())
            .count(),
        m8_velocity_events: m8_events
            .iter()
            .filter(|event| event.velocity.is_some())
            .count(),
        source_pan_events: source_events
            .iter()
            .filter(|event| event.pan.is_some())
            .count(),
        m8_pan_events: m8_events.iter().filter(|event| event.pan.is_some()).count(),
        source_pitch_events: source_events.iter().filter(|event| event.pitch).count(),
        m8_pitch_events: m8_events
            .iter()
            .filter(|event| {
                event
                    .effects
                    .iter()
                    .any(|fx| matches!(fx.command, FX_PSL | FX_PBN | FX_PVB))
            })
            .count(),
        source_effect_events: source_events
            .iter()
            .filter(|event| event.effect.is_some())
            .count(),
        m8_effect_events: m8_events.iter().map(|event| event.effects.len()).sum(),
        matched_note_events: 0,
        note_value_mismatches: 0,
        note_row_mismatches: 0,
        instrument_value_mismatches: 0,
        velocity_value_mismatches: 0,
        pan_value_mismatches: 0,
        source_initial_bpm,
        m8_initial_bpm,
        initial_bpm_delta: (source_initial_bpm - m8_initial_bpm).abs(),
        differences: Vec::new(),
    };

    push_count_difference(
        &mut comparison.differences,
        "note events",
        comparison.source_note_events,
        comparison.m8_note_events,
    );
    push_count_difference(
        &mut comparison.differences,
        "note-off events",
        comparison.source_note_off_events,
        comparison.m8_note_off_events,
    );
    push_count_difference(
        &mut comparison.differences,
        "pan events",
        comparison.source_pan_events,
        comparison.m8_pan_events,
    );
    push_count_difference(
        &mut comparison.differences,
        "pitch events",
        comparison.source_pitch_events,
        comparison.m8_pitch_events,
    );

    if comparison.initial_bpm_delta > 0.01 {
        comparison.differences.push(format!(
            "initial BPM: source expects {:.3}, converted M8 has {:.3}",
            source_initial_bpm, m8_initial_bpm
        ));
    }

    for track in 0..M8_TRACKS {
        let source_notes = source_events
            .iter()
            .filter(|event| event.channel == track && event.note.is_some())
            .collect::<Vec<_>>();
        let m8_notes = m8_events
            .iter()
            .filter(|event| event.track == track && event.note.is_some())
            .collect::<Vec<_>>();
        let matched = source_notes.len().min(m8_notes.len());
        comparison.matched_note_events += matched;
        for index in 0..matched {
            let source = source_notes[index];
            let converted = m8_notes[index];
            if source.note != converted.note {
                comparison.note_value_mismatches += 1;
                push_value_difference(
                    &mut comparison.differences,
                    format!("track {track} note {index}"),
                    source.note,
                    converted.note,
                );
            }
            if source.playback_row != converted.absolute_row {
                comparison.note_row_mismatches += 1;
                push_value_difference(
                    &mut comparison.differences,
                    format!("track {track} note {index} playback row"),
                    Some(source.playback_row),
                    Some(converted.absolute_row),
                );
            }
            if let Some(instrument) = source.instrument
                && converted.instrument != Some(instrument)
            {
                comparison.instrument_value_mismatches += 1;
                push_value_difference(
                    &mut comparison.differences,
                    format!("track {track} note {index} instrument"),
                    Some(instrument),
                    converted.instrument,
                );
            }
            if let Some(expected_velocity) = source.expected_velocity {
                let actual_velocity = converted.velocity.unwrap_or(0xff);
                if expected_velocity.abs_diff(actual_velocity) > 1 {
                    comparison.velocity_value_mismatches += 1;
                    push_value_difference(
                        &mut comparison.differences,
                        format!("track {track} note {index} velocity"),
                        Some(expected_velocity),
                        Some(actual_velocity),
                    );
                }
            }
            if let Some(expected_pan) = source.pan
                && converted.pan != Some(expected_pan)
            {
                comparison.pan_value_mismatches += 1;
                push_value_difference(
                    &mut comparison.differences,
                    format!("track {track} note {index} pan"),
                    Some(expected_pan),
                    converted.pan,
                );
            }
        }
    }

    comparison
}

fn push_value_difference<T: std::fmt::Debug>(
    differences: &mut Vec<String>,
    label: String,
    source: T,
    converted: T,
) {
    if differences.len() < 128 {
        differences.push(format!(
            "{label}: source has {source:?}, converted M8 has {converted:?}"
        ));
    }
}

fn push_count_difference(
    differences: &mut Vec<String>,
    label: &str,
    source_count: usize,
    m8_count: usize,
) {
    if source_count != m8_count {
        differences.push(format!(
            "{label}: source has {source_count}, converted M8 has {m8_count}"
        ));
    }
}

fn volume_to_velocity(volume: u8) -> u8 {
    ((u16::from(volume.min(64)) * 255) / 64) as u8
}

fn scaled_tracker_velocity(volume: u8, global_volume: u8, channel_volume: u8) -> u8 {
    let numerator = u32::from(volume.min(64))
        * u32::from(global_volume.min(64))
        * u32::from(channel_volume.min(64))
        * 255;
    (numerator / (64 * 64 * 64)).min(255) as u8
}

fn tracker_bpm(speed: u8, tempo: u8) -> f32 {
    (tempo.max(32) as f32 * 6.0 / speed.max(1) as f32).clamp(20.0, 999.0)
}

fn mod_initial_bpm(module: &crate::converter::modfile::Module) -> f32 {
    let mut speed = 6;
    let mut tempo = 125;
    if let Some(pattern_id) = module.orders.first()
        && let Some(pattern) = module.patterns.get(*pattern_id as usize)
    {
        for row in &pattern.rows {
            for cell in row.iter().take(module.channel_count) {
                if cell.effect == 0x0f && cell.effect_param != 0 {
                    if cell.effect_param < 0x20 {
                        speed = cell.effect_param;
                    } else {
                        tempo = cell.effect_param;
                    }
                }
            }
            if row.iter().any(|cell| !cell.is_empty()) {
                break;
            }
        }
    }
    tracker_bpm(speed, tempo)
}

fn mod_default_pan(channel: usize) -> u8 {
    if matches!(channel % 4, 0 | 3) {
        0
    } else {
        0xff
    }
}

fn mod_effect_pan(command: u8, param: u8) -> Option<u8> {
    match command {
        0x08 => Some(param),
        0x0e if param >> 4 == 0x08 => Some(crate::converter::s3mfile::s3m_pan_nibble_to_m8(
            param & 0x0f,
        )),
        _ => None,
    }
}

fn hvl_event_volume(module: &HvlModule, step: HvlStep) -> Option<u8> {
    for (command, param) in [(step.fx, step.fx_param), (step.fx_b, step.fx_b_param)] {
        if command == 0x0c {
            return match param {
                0x00..=0x40 => Some(param),
                0x50..=0x90 => Some(param - 0x50),
                0xa0..=0xe0 => Some(param - 0xa0),
                _ => None,
            };
        }
    }
    (step.note != 0)
        .then(|| step.instrument.checked_sub(1))
        .flatten()
        .and_then(|instrument| module.instruments.get(instrument as usize))
        .map(|instrument| instrument.volume)
}

fn hvl_effect_pan(command: u8, param: u8) -> Option<u8> {
    (command == 0x07).then_some(param.wrapping_add(128))
}

fn update_event_channel_volume(
    command: u8,
    param: u8,
    speed: u8,
    current: &mut u8,
    memory: &mut u8,
) {
    if command == 13 {
        *current = param.min(64);
        return;
    }
    if command != 14 {
        return;
    }
    let param = if param == 0 {
        *memory
    } else {
        *memory = param;
        param
    };
    let up = param >> 4;
    let down = param & 0x0f;
    let delta = if down == 0x0f && up != 0 {
        i16::from(up)
    } else if up == 0x0f && down != 0 {
        -i16::from(down)
    } else if down != 0 {
        -i16::from(down) * i16::from(speed.saturating_sub(1))
    } else {
        i16::from(up) * i16::from(speed.saturating_sub(1))
    };
    *current = (i16::from(*current) + delta).clamp(0, 64) as u8;
}

fn update_xm_global_volume(effect: u8, param: u8, speed: u8, current: &mut u8, memory: &mut u8) {
    if effect == 0x10 {
        *current = param.min(64);
        return;
    }
    if effect != 0x11 {
        return;
    }
    let param = if param == 0 {
        *memory
    } else {
        *memory = param;
        param
    };
    let up = param >> 4;
    let down = param & 0x0f;
    let delta = if down == 0x0f && up != 0 {
        i16::from(up)
    } else if up == 0x0f && down != 0 {
        -i16::from(down)
    } else if up != 0 {
        i16::from(up) * i16::from(speed.saturating_sub(1))
    } else {
        -i16::from(down) * i16::from(speed.saturating_sub(1))
    };
    *current = (i16::from(*current) + delta).clamp(0, 64) as u8;
}

fn xm_sample_for_note(
    module: &crate::converter::xmfile::XmModule,
    instrument: usize,
    note: u8,
) -> Option<&crate::converter::xmfile::XmSample> {
    let instrument = module.instruments.get(instrument)?;
    let sample_index = if (1..=96).contains(&note) {
        instrument
            .sample_map
            .get(note.saturating_sub(1) as usize)
            .copied()
            .unwrap_or(0) as usize
    } else {
        0
    };
    instrument.samples.get(sample_index)
}

fn xm_flattened_instrument_for_note(
    module: &crate::converter::xmfile::XmModule,
    instrument_index: usize,
    note: u8,
) -> Option<u8> {
    let instrument = module.instruments.get(instrument_index)?;
    let sample_index = instrument
        .sample_map
        .get(note.saturating_sub(1).min(95) as usize)
        .copied()
        .unwrap_or(0) as usize;
    if instrument.samples.get(sample_index)?.data.is_empty() {
        return None;
    }
    let prior = module
        .instruments
        .iter()
        .take(instrument_index)
        .flat_map(|instrument| &instrument.samples)
        .filter(|sample| !sample.data.is_empty())
        .count();
    let within = instrument
        .samples
        .iter()
        .take(sample_index)
        .filter(|sample| !sample.data.is_empty())
        .count();
    let slot = prior + within;
    (slot < 128).then_some(slot as u8)
}

fn xm_effect_pan(effect: u8, param: u8) -> Option<u8> {
    (effect == 0x08).then_some(param)
}

fn xm_volume_column_pan(volume: u8) -> Option<u8> {
    (0xc0..=0xcf)
        .contains(&volume)
        .then_some((((volume & 0x0f) as u16 * 255 + 7) / 15) as u8)
}

fn xm_pitch_command(effect: u8, param: u8) -> bool {
    matches!(effect, 0x00 if param != 0)
        || matches!(effect, 0x01..=0x07 | 0x1b | 0x1d | 0x21)
        || (effect == 0x0e && matches!(param >> 4, 0x01..=0x07))
}

fn xm_volume_column_pitch(volume: u8) -> bool {
    matches!(volume, 0xa0..=0xbf | 0xf0..=0xff)
}

fn tracker_fx(command: u8, value: u8) -> Option<EventFx> {
    (command != 0 || value != 0).then_some(EventFx { command, value })
}

fn m8_fx(fx: FX) -> Option<EventFx> {
    (!fx.is_empty()).then_some(EventFx {
        command: fx.command,
        value: fx.value,
    })
}

fn s3m_effect_pan(command: u8, value: u8, pan_command_uses_8bit: bool) -> Option<u8> {
    match command {
        19 if value >> 4 == 0x08 => Some(crate::converter::s3mfile::s3m_pan_nibble_to_m8(
            value & 0x0f,
        )),
        24 => Some(crate::converter::s3mfile::s3m_command_pan_to_m8(
            value,
            pan_command_uses_8bit,
        )),
        _ => None,
    }
}

fn hvl_pitch_command(command: u8, param: u8) -> bool {
    matches!(command, 1 | 2 | 3 | 5)
        || (command == 0x0e && matches!(param >> 4, 0x01 | 0x02 | 0x04))
}

fn mod_pitch_command(command: u8, value: u8) -> bool {
    matches!(command, 0x00 if value != 0)
        || matches!(command, 0x01..=0x07)
        || (command == 0x0e && matches!(value >> 4, 0x01 | 0x02 | 0x03 | 0x04 | 0x05 | 0x07))
}

fn s3m_pitch_command(command: u8, _value: u8) -> bool {
    matches!(command, 5 | 6 | 7 | 8 | 10 | 11 | 12 | 18 | 21)
}

fn s3m_volume_column_pitch(volume: u8) -> bool {
    matches!(volume, 105..=124 | 193..=212)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_event_report_for_minimal_s3m() {
        let input = crate::converter::s3mfile::tests::minimal_s3m();
        let report = build_event_regression_report(&input, ConversionOptions::default())
            .expect("event report");

        assert_eq!(report.source_format, "S3M");
        assert_eq!(report.source_events.len(), 1);
        assert!(report.comparison.source_note_events >= 1);
        assert!(report.comparison.m8_note_events >= 1);
        assert_eq!(report.comparison.note_value_mismatches, 0);
        assert_eq!(report.comparison.note_row_mismatches, 0);
        assert_eq!(report.comparison.instrument_value_mismatches, 0);
        assert_eq!(report.comparison.velocity_value_mismatches, 0);
        assert_eq!(report.comparison.pan_value_mismatches, 0);
        assert!(report.comparison.initial_bpm_delta < 0.01);
    }
}
