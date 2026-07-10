use std::collections::{HashMap, HashSet};

use m8_file_parser::{
    AHDEnv, Chain, ChainStep, FX, Instrument, LFO, LfoShape, LfoTriggerMode, LimitType, Note,
    Phrase, SamplePlayMode, Sampler, Song, Step, SynthParams, Table, WavShape, WavSynth,
    reader::Reader, writer::Writer,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::converter::ConversionOptions;
use crate::converter::hvlfile::{HvlInstrument, HvlModule, HvlStep};
use crate::converter::modfile::{Cell, Module, Pattern, period_to_note};
use crate::converter::s3mfile::{
    S3mCell, S3mModule, S3mPattern, s3m_command_pan_to_m8, s3m_note_to_m8, s3m_pan_nibble_to_m8,
};

mod effects;
mod header;
mod instruments;
mod naming;
mod playback;
mod steps;
mod timing;

use effects::*;
use header::*;
use instruments::*;
use naming::{fallback_sample_name, truncate_ascii};
pub use naming::{s3m_sample_filename, sample_filename};
use playback::*;
use steps::*;
use timing::*;

const TEMPLATE: &[u8] = include_bytes!("../../assets/templates/V6_2EMPTY.m8s");
const M8_TRACKS: usize = 8;
const M8_PHRASE_ROWS: usize = 16;
const EMPTY: u8 = 0xff;
const NOTE_OFF: u8 = 0x80;
const FX_ARP: u8 = 0x00;
const FX_DEL: u8 = 0x02;
const FX_KIL: u8 = 0x05;
const FX_RET: u8 = 0x08;
const FX_PSL: u8 = 0x0c;
const FX_PBN: u8 = 0x0d;
const FX_PVB: u8 = 0x0e;
const FX_SNG: u8 = 0x13;
const FX_TBL: u8 = 0x14;
const FX_TIC: u8 = 0x16;
const FX_TPO: u8 = 0x18;
const FX_SAMPLER_FIN: u8 = 0x82;
const FX_SAMPLER_STA: u8 = 0x84;
const FX_SAMPLER_PAN: u8 = 0x8d;
const FX_WAVSYNTH_OSC: u8 = 0x83;
const FX_WAVSYNTH_SIZ: u8 = 0x84;
const FX_WAVSYNTH_CUT: u8 = 0x89;
const TABLE_TICK_PER_TRACKER_TICK: u8 = 0x01;
const SAMPLER_DEFAULT_AMP: u8 = 0x00;
const SAMPLER_DEFAULT_DRY: u8 = 0xc0;
const MAX_PLAYBACK_BLOCKS: usize = 256;
const MAX_FLOW_VISITS: usize = 4096;

#[derive(Debug, Error)]
pub enum M8Error {
    #[error("could not parse embedded M8 template: {0}")]
    Template(String),
    #[error("could not write M8 project: {0}")]
    Write(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct M8Export {
    pub song_bytes: Vec<u8>,
    pub report: M8Report,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct M8Report {
    pub used_tracks: usize,
    pub used_phrases: usize,
    pub used_chains: usize,
    pub dropped_tracks: usize,
    pub dropped_phrases: usize,
    pub dropped_chains: usize,
    pub used_tables: usize,
    pub dropped_tables: usize,
    pub applied_tempo_bpm: Option<f32>,
    pub unsupported_effects: Vec<UnsupportedEffect>,
    pub source_unsupported_effects: Vec<SourceUnsupportedEffect>,
    pub approximations: Vec<Approximation>,
    pub warnings: Vec<String>,
    pub reference_render: Option<ReferenceRenderReport>,
    pub source_fallback_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceRenderReport {
    pub renderer: String,
    pub sample_rate: u32,
    pub frames: usize,
    pub duration_seconds: f64,
    pub truncated: bool,
    pub wav_path: String,
    pub m8_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsupportedEffect {
    pub order: usize,
    pub pattern: u8,
    pub row: usize,
    pub channel: usize,
    pub effect: u8,
    pub param: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceUnsupportedEffect {
    pub format: String,
    pub order: Option<usize>,
    pub pattern: u8,
    pub row: usize,
    pub channel: usize,
    pub effect: String,
    pub param: u8,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approximation {
    pub order: usize,
    pub pattern: u8,
    pub row: usize,
    pub channel: usize,
    pub category: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy)]
struct EffectLocation {
    order: usize,
    pattern: u8,
    row: usize,
    channel: usize,
}

impl EffectLocation {
    fn approximation(
        self,
        report: &mut M8Report,
        category: impl Into<String>,
        detail: impl Into<String>,
    ) {
        report.approximations.push(Approximation {
            order: self.order,
            pattern: self.pattern,
            row: self.row,
            channel: self.channel,
            category: category.into(),
            detail: detail.into(),
        });
    }
}

pub(crate) fn mod_playback_rows_for_events(module: &Module) -> Vec<(usize, u8, usize, bool)> {
    let mut report = M8Report::default();
    build_mod_playback_blocks(module, &mut report)
        .into_iter()
        .flat_map(|block| block.rows)
        .map(|row| {
            (
                row.order_index,
                row.pattern_id,
                row.source_row,
                row.emit_cells,
            )
        })
        .collect()
}

pub(crate) fn s3m_playback_rows_for_events(
    module: &S3mModule,
) -> Vec<(usize, u8, usize, bool, u8)> {
    let mut report = M8Report::default();
    let blocks = build_s3m_playback_blocks(module, &mut report);
    let volumes = s3m_global_volume_contexts(module, &blocks);
    blocks
        .into_iter()
        .enumerate()
        .flat_map(|(block_index, block)| {
            let block_volumes = volumes.get(block_index).cloned().unwrap_or_default();
            block
                .rows
                .into_iter()
                .enumerate()
                .map(move |(row_index, row)| {
                    (
                        row.order_index,
                        row.pattern_id,
                        row.source_row,
                        row.emit_cells,
                        block_volumes
                            .get(row_index)
                            .map(|volume| volume.current)
                            .unwrap_or(module.global_volume),
                    )
                })
        })
        .collect()
}

pub(crate) fn hvl_playback_rows_for_events(module: &HvlModule) -> Vec<(usize, usize)> {
    let mut report = M8Report::default();
    build_hvl_playback_blocks(module, &mut report)
        .into_iter()
        .flat_map(|block| block.rows)
        .map(|row| (row.order_index, row.source_row))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PackedStep {
    note: u8,
    velocity: u8,
    instrument: u8,
    fx: [(u8, u8); 3],
}

pub fn export_reference_stem_m8(
    song_name: &str,
    bundle_directory: Option<&str>,
    sample_path: &str,
) -> Result<Vec<u8>, M8Error> {
    let template = TEMPLATE.to_vec();
    let mut reader = Reader::new(template.clone());
    let mut song = Song::read_from_reader(&mut reader)
        .map_err(|error| M8Error::Template(format!("{error:?}")))?;
    let version = song.version;
    clear_song(&mut song);

    song.instruments[0] = Instrument::Sampler(Sampler {
        number: 0,
        name: truncate_ascii("REFERENCE MIX", 12),
        transpose: false,
        table_tick: TABLE_TICK_PER_TRACKER_TICK,
        synth_params: sampler_params(64),
        sample_path: sample_path.to_string(),
        play_mode: SamplePlayMode::FWD,
        slice: 0,
        start: 0,
        loop_start: 0,
        length: 0xff,
        degrade: 0,
    });

    let mut phrase = Phrase::default_ver(version);
    phrase.clear();
    phrase.steps[0] = Step {
        note: Note(60),
        instrument: 0,
        velocity: 0xff,
        ..empty_step()
    };
    song.phrases[0] = phrase;
    let mut chain = Chain::default();
    chain.steps[0] = ChainStep {
        phrase: 0,
        transpose: 0,
    };
    song.chains[0] = chain;
    song.song.steps[0] = 0;

    let mut writer = Writer::new(template);
    song.write(&mut writer).map_err(M8Error::Write)?;
    let mut song_bytes = writer.finish();
    patch_header(&mut song_bytes, song_name, bundle_directory, Some(120.0));
    Ok(song_bytes)
}

pub fn export_editable_m8(
    module: &Module,
    options: &ConversionOptions,
) -> Result<M8Export, M8Error> {
    let template = TEMPLATE.to_vec();
    let mut reader = Reader::new(template.clone());
    let mut song =
        Song::read_from_reader(&mut reader).map_err(|err| M8Error::Template(format!("{err:?}")))?;
    let version = song.version;

    let mut report = M8Report {
        used_tracks: module.channel_count.min(M8_TRACKS),
        dropped_tracks: module.channel_count.saturating_sub(M8_TRACKS),
        applied_tempo_bpm: Some(mod_m8_tempo(module)),
        ..M8Report::default()
    };

    clear_song(&mut song);
    install_sampler_instruments(&mut song, module);

    let playback_blocks = build_mod_playback_blocks(module, &mut report);
    let timing_contexts = mod_timing_contexts(module, &playback_blocks);
    let mut table_allocator = TableAllocator::new();
    let mut phrase_cache: HashMap<[PackedStep; M8_PHRASE_ROWS], u8> = HashMap::new();
    let mut chain_cache: HashMap<[u8; 4], u8> = HashMap::new();
    let mut next_phrase = 0u8;
    let mut next_chain = 0u8;
    let mut current_instruments = vec![EMPTY; module.channel_count];
    let mut current_velocities = vec![EMPTY; module.channel_count];
    let mut volume_slide_memory = vec![0u8; module.channel_count];
    let mut vibrato_memory = vec![0u8; module.channel_count];
    let mut tremolo_memory = vec![0u8; module.channel_count];
    let mut pitch_slide_memory = vec![PitchSlideMemory::default(); module.channel_count];
    let mut tone_portamento = vec![TonePortamentoState::default(); module.channel_count];
    let mut retrigger_memory = vec![RetriggerMemory::default(); module.channel_count];
    let mut sample_offset_memory = vec![SampleOffsetMemory::default(); module.channel_count];
    let mut waveform_state = vec![TrackerWaveformState::default(); module.channel_count];
    let mut current_pans = (0..module.channel_count)
        .map(mod_default_channel_pan)
        .collect::<Vec<_>>();
    let mut active_table_effects = vec![false; module.channel_count];
    let mut active_pitch_bends = vec![false; module.channel_count];

    for (playback_index, block) in playback_blocks.iter().enumerate() {
        for channel in 0..module.channel_count.min(M8_TRACKS) {
            let mut phrase_ids = [EMPTY; 4];

            for chunk in 0..4 {
                let mut packed = [empty_packed_step(); M8_PHRASE_ROWS];
                let mut phrase = Phrase::default_ver(version);
                phrase.clear();

                for row_in_chunk in 0..M8_PHRASE_ROWS {
                    let playback_row_index = chunk * M8_PHRASE_ROWS + row_in_chunk;
                    let Some(playback_row) = block.rows.get(playback_row_index).copied() else {
                        continue;
                    };
                    let source_cell = module
                        .patterns
                        .get(playback_row.pattern_id as usize)
                        .and_then(|pattern| pattern.rows.get(playback_row.source_row))
                        .and_then(|row| row.get(channel))
                        .copied()
                        .unwrap_or_else(silent_mod_cell);
                    let cell = if playback_row.emit_cells {
                        source_cell
                    } else {
                        continued_mod_cell(source_cell)
                    };
                    let mut step = convert_cell(
                        module,
                        cell,
                        &mut current_instruments[channel],
                        playback_row.order_index,
                        playback_row.pattern_id,
                        playback_row.source_row,
                        channel,
                        timing_contexts
                            .get(playback_index)
                            .and_then(|block| block.get(playback_row_index))
                            .and_then(|row| row.get(channel))
                            .copied()
                            .unwrap_or_default(),
                        &mut table_allocator,
                        &mut song.tables,
                        &mut current_velocities[channel],
                        &mut volume_slide_memory[channel],
                        &mut vibrato_memory[channel],
                        &mut tremolo_memory[channel],
                        &mut pitch_slide_memory[channel],
                        &mut tone_portamento[channel],
                        &mut retrigger_memory[channel],
                        &mut sample_offset_memory[channel],
                        &mut waveform_state[channel],
                        &mut current_pans[channel],
                        &mut active_table_effects[channel],
                        &mut active_pitch_bends[channel],
                        &mut report,
                    );
                    if channel == 0
                        && chunk * M8_PHRASE_ROWS + row_in_chunk + 1 == block.rows.len()
                        && let Some(song_hop) = block.song_hop
                    {
                        push_song_hop(&mut step, song_hop, &mut report, "MOD");
                    }
                    packed[row_in_chunk] = pack_step(&step);
                    phrase.steps[row_in_chunk] = step;
                }

                let phrase_id = if let Some(existing) = phrase_cache.get(&packed) {
                    *existing
                } else if next_phrase as usize >= Song::N_PHRASES {
                    report.dropped_phrases += 1;
                    EMPTY
                } else {
                    let id = next_phrase;
                    phrase_cache.insert(packed, id);
                    song.phrases[id as usize] = phrase;
                    next_phrase = next_phrase.saturating_add(1);
                    id
                };

                phrase_ids[chunk] = phrase_id;
            }

            let chain_id = if phrase_ids.iter().all(|id| *id == EMPTY) {
                EMPTY
            } else if let Some(existing) = chain_cache.get(&phrase_ids) {
                *existing
            } else if next_chain as usize >= Song::N_CHAINS {
                report.dropped_chains += 1;
                EMPTY
            } else {
                let id = next_chain;
                let mut chain = Chain::default();
                for (index, phrase_id) in phrase_ids.iter().enumerate() {
                    chain.steps[index] = ChainStep {
                        phrase: *phrase_id,
                        transpose: 0,
                    };
                }
                chain_cache.insert(phrase_ids, id);
                song.chains[id as usize] = chain;
                next_chain = next_chain.saturating_add(1);
                id
            };

            let song_index = playback_index * M8_TRACKS + channel;
            if song_index < song.song.steps.len() {
                song.song.steps[song_index] = chain_id;
            }
        }
    }

    report.used_phrases = next_phrase as usize;
    report.used_chains = next_chain as usize;
    report.used_tables = table_allocator.next_table;
    report.dropped_tables = table_allocator.dropped;

    if report.dropped_tracks > 0 {
        report.warnings.push(format!(
            "MOD has {} channels; M8 has 8 monophonic tracks, so {} channels were not exported",
            module.channel_count, report.dropped_tracks
        ));
    }
    if report.dropped_phrases > 0 || report.dropped_chains > 0 {
        report.warnings.push(
            "M8 phrase/chain limits were exceeded; use stem-render mode for audio-exact conversion of this module"
                .to_string(),
        );
    }
    let mut writer = Writer::new(template);
    song.write(&mut writer).map_err(M8Error::Write)?;
    let mut song_bytes = writer.finish();
    patch_header(
        &mut song_bytes,
        options.song_name.as_deref().unwrap_or(&module.title),
        options.bundle_directory.as_deref(),
        report.applied_tempo_bpm,
    );

    Ok(M8Export { song_bytes, report })
}

pub fn s3m_volume_column_pan_for_events(volume: u8) -> Option<u8> {
    s3m_volume_column_pan(volume)
}

pub fn export_hvl_editable_m8(
    module: &HvlModule,
    options: &ConversionOptions,
) -> Result<M8Export, M8Error> {
    let template = TEMPLATE.to_vec();
    let mut reader = Reader::new(template.clone());
    let mut song =
        Song::read_from_reader(&mut reader).map_err(|err| M8Error::Template(format!("{err:?}")))?;
    let version = song.version;

    let mut report = M8Report {
        used_tracks: module.channel_count.min(M8_TRACKS),
        dropped_tracks: module.channel_count.saturating_sub(M8_TRACKS),
        applied_tempo_bpm: Some(hvl_rows_to_m8_bpm(module.speed_multiplier, 6)),
        ..M8Report::default()
    };

    clear_song(&mut song);
    install_hvl_wavsynth_instruments(&mut song, module);
    let hvl_table_ids = install_hvl_performance_tables(&mut song, module);
    let playback_blocks = build_hvl_playback_blocks(module, &mut report);
    let timing_contexts = hvl_timing_contexts(module, &playback_blocks);
    let dynamic_table_start = module.instruments.len().min(Song::N_TABLES);
    let mut table_allocator = TableAllocator::starting_at(dynamic_table_start);
    let mut current_velocities = vec![EMPTY; module.channel_count];
    let mut tone_portamento = vec![TonePortamentoState::default(); module.channel_count];

    let mut phrase_cache: HashMap<[PackedStep; M8_PHRASE_ROWS], u8> = HashMap::new();
    let mut chain_cache: HashMap<[(u8, u8); 4], u8> = HashMap::new();
    let mut next_phrase = 0u8;
    let mut next_chain = 0u8;
    for (playback_index, block) in playback_blocks.iter().enumerate() {
        let Some(position_index) = block.rows.first().map(|row| row.order_index) else {
            continue;
        };
        let Some(position) = module.positions.get(position_index) else {
            continue;
        };
        let chunks_per_track = block.rows.len().div_ceil(M8_PHRASE_ROWS).min(4);
        for channel in 0..module.channel_count.min(M8_TRACKS) {
            let track_id = position.tracks[channel] as usize;
            let transpose = position.transposes[channel];
            let Some(track) = module.tracks.get(track_id) else {
                report.warnings.push(format!(
                    "position {position_index}, channel {channel}: HVL track {track_id} is outside parsed data"
                ));
                continue;
            };

            let mut phrase_ids = [(EMPTY, 0u8); 4];
            for (chunk, phrase_slot) in phrase_ids.iter_mut().enumerate().take(chunks_per_track) {
                let mut packed = [empty_packed_step(); M8_PHRASE_ROWS];
                let mut phrase = Phrase::default_ver(version);
                phrase.clear();

                for row_in_chunk in 0..M8_PHRASE_ROWS {
                    let playback_row_index = chunk * M8_PHRASE_ROWS + row_in_chunk;
                    let Some(playback_row) = block.rows.get(playback_row_index) else {
                        continue;
                    };
                    let row = playback_row.source_row;
                    let source = track.rows.get(row).copied().unwrap_or_else(empty_hvl_step);
                    let mut step = convert_hvl_step(
                        module,
                        source,
                        position_index,
                        track_id,
                        row,
                        channel,
                        timing_contexts
                            .get(playback_index)
                            .and_then(|block| block.get(playback_row_index))
                            .and_then(|row| row.get(channel))
                            .copied()
                            .unwrap_or_default(),
                        &hvl_table_ids,
                        &mut table_allocator,
                        &mut song.tables,
                        &mut current_velocities[channel],
                        &mut tone_portamento[channel],
                        &mut report,
                    );
                    if channel == 0
                        && playback_row_index + 1 == block.rows.len()
                        && let Some(song_hop) = block.song_hop
                    {
                        push_song_hop(&mut step, song_hop, &mut report, "HVL");
                    }
                    packed[row_in_chunk] = pack_step(&step);
                    phrase.steps[row_in_chunk] = step;
                }

                let phrase_id = if let Some(existing) = phrase_cache.get(&packed) {
                    *existing
                } else if next_phrase as usize >= Song::N_PHRASES {
                    report.dropped_phrases += 1;
                    EMPTY
                } else {
                    let id = next_phrase;
                    phrase_cache.insert(packed, id);
                    song.phrases[id as usize] = phrase;
                    next_phrase = next_phrase.saturating_add(1);
                    id
                };

                *phrase_slot = (phrase_id, transpose as u8);
            }

            let chain_id = if phrase_ids.iter().all(|(id, _)| *id == EMPTY) {
                EMPTY
            } else if let Some(existing) = chain_cache.get(&phrase_ids) {
                *existing
            } else if next_chain as usize >= Song::N_CHAINS {
                report.dropped_chains += 1;
                EMPTY
            } else {
                let id = next_chain;
                let mut chain = Chain::default();
                for (index, (phrase_id, transpose)) in phrase_ids.iter().enumerate() {
                    chain.steps[index] = ChainStep {
                        phrase: *phrase_id,
                        transpose: *transpose,
                    };
                }
                chain_cache.insert(phrase_ids, id);
                song.chains[id as usize] = chain;
                next_chain = next_chain.saturating_add(1);
                id
            };

            let song_index = playback_index * M8_TRACKS + channel;
            if song_index < song.song.steps.len() {
                song.song.steps[song_index] = chain_id;
            }
        }
    }

    report.used_phrases = next_phrase as usize;
    report.used_chains = next_chain as usize;
    let performance_table_count = hvl_table_ids
        .iter()
        .filter(|table_id| table_id.is_some())
        .count();
    report.used_tables = performance_table_count
        + table_allocator
            .next_table
            .saturating_sub(dynamic_table_start);
    report.dropped_tables = table_allocator.dropped;

    if report.dropped_tracks > 0 {
        report.warnings.push(format!(
            "HVL has {} channels; M8 has 8 monophonic tracks, so {} channels were not exported",
            module.channel_count, report.dropped_tracks
        ));
    }
    if report.dropped_phrases > 0 || report.dropped_chains > 0 {
        report
            .warnings
            .push("M8 phrase/chain limits were exceeded while mapping HVL tracks".to_string());
    }
    report.warnings.push(
        "HVL instruments are approximated with M8 WavSynth instruments; exact HivelyTracker synthesis requires a replayer/render path"
            .to_string(),
    );

    let mut writer = Writer::new(template);
    song.write(&mut writer).map_err(M8Error::Write)?;
    let mut song_bytes = writer.finish();
    patch_header(
        &mut song_bytes,
        options.song_name.as_deref().unwrap_or(&module.title),
        options.bundle_directory.as_deref(),
        report.applied_tempo_bpm,
    );

    Ok(M8Export { song_bytes, report })
}

pub fn export_s3m_editable_m8(
    module: &S3mModule,
    options: &ConversionOptions,
) -> Result<M8Export, M8Error> {
    let template = TEMPLATE.to_vec();
    let mut reader = Reader::new(template.clone());
    let mut song =
        Song::read_from_reader(&mut reader).map_err(|err| M8Error::Template(format!("{err:?}")))?;
    let version = song.version;

    let mut report = M8Report {
        used_tracks: module.active_channels.len().min(M8_TRACKS),
        dropped_tracks: module.active_channels.len().saturating_sub(M8_TRACKS),
        applied_tempo_bpm: Some(tracker_rows_to_m8_bpm(
            module.initial_speed,
            module.initial_tempo,
        )),
        ..M8Report::default()
    };

    clear_song(&mut song);
    let playback_blocks = build_s3m_playback_blocks(module, &mut report);
    install_s3m_sampler_instruments(&mut song, module, &mut report);
    let pan_plan =
        install_s3m_static_pan_instruments(&mut song, module, &playback_blocks, &mut report);
    let timing_contexts = s3m_timing_contexts(module, &playback_blocks);
    let global_volume_contexts = s3m_global_volume_contexts(module, &playback_blocks);
    let mut table_allocator = TableAllocator::new();
    let mut phrase_cache: HashMap<[PackedStep; M8_PHRASE_ROWS], u8> = HashMap::new();
    let mut chain_cache: HashMap<[u8; 4], u8> = HashMap::new();
    let mut next_phrase = 0u8;
    let mut next_chain = 0u8;
    let mut current_instruments = vec![EMPTY; module.active_channels.len()];
    let mut current_velocities = vec![EMPTY; module.active_channels.len()];
    let mut voice_active = vec![false; module.active_channels.len()];
    let mut current_channel_volumes = vec![64; module.active_channels.len()];
    let mut channel_volume_slide_memory = vec![0; module.active_channels.len()];
    let mut arpeggio_memory = vec![0u8; module.active_channels.len()];
    let mut volume_slide_memory = vec![0u8; module.active_channels.len()];
    let mut vibrato_memory = vec![0u8; module.active_channels.len()];
    let mut tremor_memory = vec![0u8; module.active_channels.len()];
    let mut tremolo_memory = vec![0u8; module.active_channels.len()];
    let mut pitch_slide_memory = vec![PitchSlideMemory::default(); module.active_channels.len()];
    let mut tone_portamento = vec![TonePortamentoState::default(); module.active_channels.len()];
    let mut retrigger_memory = vec![RetriggerMemory::default(); module.active_channels.len()];
    let mut sample_offset_memory =
        vec![SampleOffsetMemory::default(); module.active_channels.len()];
    let mut waveform_state = vec![TrackerWaveformState::default(); module.active_channels.len()];
    let mut current_pans = module
        .active_channels
        .iter()
        .map(|channel| module.channel_pans.get(*channel).copied().unwrap_or(0x80))
        .collect::<Vec<_>>();
    let mut pan_slide_memory = vec![0; module.active_channels.len()];
    let mut panbrello_state = vec![PanbrelloState::default(); module.active_channels.len()];
    let mut active_table_effects = vec![false; module.active_channels.len()];
    let mut active_pitch_bends = vec![false; module.active_channels.len()];

    for (playback_index, block) in playback_blocks.iter().enumerate() {
        for (track, source_channel) in module.active_channels.iter().take(M8_TRACKS).enumerate() {
            let mut phrase_ids = [EMPTY; 4];
            for (chunk, phrase_slot) in phrase_ids.iter_mut().enumerate() {
                let mut packed = [empty_packed_step(); M8_PHRASE_ROWS];
                let mut phrase = Phrase::default_ver(version);
                phrase.clear();

                for row_in_chunk in 0..M8_PHRASE_ROWS {
                    let playback_row_index = chunk * M8_PHRASE_ROWS + row_in_chunk;
                    let Some(playback_row) = block.rows.get(playback_row_index).copied() else {
                        continue;
                    };
                    let source_cell = module
                        .patterns
                        .get(playback_row.pattern_id as usize)
                        .and_then(|pattern| pattern.rows.get(playback_row.source_row))
                        .and_then(|row| row.get(*source_channel))
                        .copied()
                        .unwrap_or(S3mCell::EMPTY);
                    let cell = if playback_row.emit_cells {
                        source_cell
                    } else {
                        continued_s3m_cell(source_cell)
                    };
                    let mut step = convert_s3m_cell(
                        module,
                        cell,
                        &mut current_instruments[track],
                        playback_row.order_index,
                        playback_row.pattern_id,
                        playback_row.source_row,
                        *source_channel,
                        timing_contexts
                            .get(playback_index)
                            .and_then(|block| block.get(playback_row_index))
                            .and_then(|row| row.get(track))
                            .copied()
                            .unwrap_or_default(),
                        global_volume_contexts
                            .get(playback_index)
                            .and_then(|block| block.get(playback_row_index))
                            .copied()
                            .unwrap_or(GlobalVolumeContext {
                                previous: module.global_volume,
                                current: module.global_volume,
                            }),
                        &mut table_allocator,
                        &mut song.tables,
                        &mut current_velocities[track],
                        &mut voice_active[track],
                        &mut current_channel_volumes[track],
                        &mut channel_volume_slide_memory[track],
                        &mut arpeggio_memory[track],
                        &mut volume_slide_memory[track],
                        &mut vibrato_memory[track],
                        &mut tremor_memory[track],
                        &mut tremolo_memory[track],
                        &mut pitch_slide_memory[track],
                        &mut tone_portamento[track],
                        &mut retrigger_memory[track],
                        &mut sample_offset_memory[track],
                        &mut waveform_state[track],
                        &mut current_pans[track],
                        &mut pan_slide_memory[track],
                        &mut panbrello_state[track],
                        pan_plan
                            .channel_static_pans
                            .get(*source_channel)
                            .copied()
                            .flatten(),
                        &pan_plan.instrument_remaps,
                        &mut active_table_effects[track],
                        &mut active_pitch_bends[track],
                        &mut report,
                    );
                    if track == 0
                        && chunk * M8_PHRASE_ROWS + row_in_chunk + 1 == block.rows.len()
                        && let Some(song_hop) = block.song_hop
                    {
                        push_song_hop(&mut step, song_hop, &mut report, "S3M");
                    }
                    packed[row_in_chunk] = pack_step(&step);
                    phrase.steps[row_in_chunk] = step;
                }

                let phrase_id = if let Some(existing) = phrase_cache.get(&packed) {
                    *existing
                } else if next_phrase as usize >= Song::N_PHRASES {
                    report.dropped_phrases += 1;
                    EMPTY
                } else {
                    let id = next_phrase;
                    phrase_cache.insert(packed, id);
                    song.phrases[id as usize] = phrase;
                    next_phrase = next_phrase.saturating_add(1);
                    id
                };

                *phrase_slot = phrase_id;
            }

            let chain_id = if phrase_ids.iter().all(|id| *id == EMPTY) {
                EMPTY
            } else if let Some(existing) = chain_cache.get(&phrase_ids) {
                *existing
            } else if next_chain as usize >= Song::N_CHAINS {
                report.dropped_chains += 1;
                EMPTY
            } else {
                let id = next_chain;
                let mut chain = Chain::default();
                for (index, phrase_id) in phrase_ids.iter().enumerate() {
                    chain.steps[index] = ChainStep {
                        phrase: *phrase_id,
                        transpose: 0,
                    };
                }
                chain_cache.insert(phrase_ids, id);
                song.chains[id as usize] = chain;
                next_chain = next_chain.saturating_add(1);
                id
            };

            let song_index = playback_index * M8_TRACKS + track;
            if song_index < song.song.steps.len() {
                song.song.steps[song_index] = chain_id;
            }
        }
    }

    report.used_phrases = next_phrase as usize;
    report.used_chains = next_chain as usize;
    report.used_tables = table_allocator.next_table;
    report.dropped_tables = table_allocator.dropped;

    if report.dropped_tracks > 0 {
        report.warnings.push(format!(
            "S3M has {} enabled channels; M8 has 8 monophonic tracks, so {} channels were not exported",
            module.active_channels.len(),
            report.dropped_tracks
        ));
    }
    if report.dropped_phrases > 0 || report.dropped_chains > 0 {
        report
            .warnings
            .push("M8 phrase/chain limits were exceeded while mapping S3M patterns".to_string());
    }

    let mut writer = Writer::new(template);
    song.write(&mut writer).map_err(M8Error::Write)?;
    let mut song_bytes = writer.finish();
    patch_header(
        &mut song_bytes,
        options.song_name.as_deref().unwrap_or(&module.title),
        options.bundle_directory.as_deref(),
        report.applied_tempo_bpm,
    );

    Ok(M8Export { song_bytes, report })
}

#[cfg(test)]
mod tests;
