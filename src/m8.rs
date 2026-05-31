use std::collections::HashMap;

use m8_file_parser::{
    AHDEnv, Chain, ChainStep, FX, Instrument, LimitType, Note, Phrase, SamplePlayMode, Sampler,
    Song, Step, SynthParams, Table, WavShape, WavSynth, reader::Reader, writer::Writer,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::convert::ConversionOptions;
use crate::hvlfile::{HvlInstrument, HvlModule, HvlStep};
use crate::modfile::{Cell, Module, period_to_note};
use crate::s3mfile::{S3mCell, S3mModule, s3m_note_to_m8};

const TEMPLATE: &[u8] = include_bytes!("../assets/templates/V6_2EMPTY.m8s");
const M8_TRACKS: usize = 8;
const M8_PHRASE_ROWS: usize = 16;
const EMPTY: u8 = 0xff;
const FX_ARP: u8 = 0x00;
const FX_DEL: u8 = 0x02;
const FX_KIL: u8 = 0x05;
const FX_RET: u8 = 0x08;
const FX_PVB: u8 = 0x0e;
const FX_TBL: u8 = 0x14;
const FX_TPO: u8 = 0x18;
const FX_SAMPLER_FIN: u8 = 0x82;
const FX_SAMPLER_STA: u8 = 0x84;
const FX_SAMPLER_PAN: u8 = 0x8d;
const TABLE_TICK_PER_TRACKER_TICK: u8 = 0x01;
const SAMPLER_DEFAULT_AMP: u8 = 0x00;
const SAMPLER_DEFAULT_DRY: u8 = 0xc0;

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
    pub warnings: Vec<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PackedStep {
    note: u8,
    velocity: u8,
    instrument: u8,
    fx: [(u8, u8); 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TimingContext {
    speed: u8,
    tpo: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TableSpec {
    Empty,
    VolumeSlide { start: u8, delta: i8, rows: u8 },
    FineSlide { delta: i8, rows: u8 },
}

struct TableAllocator {
    next_table: u8,
    cache: HashMap<TableSpec, u8>,
    dropped: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct PitchSlideMemory {
    up: u8,
    down: u8,
}

impl Default for TimingContext {
    fn default() -> Self {
        Self {
            speed: 6,
            tpo: None,
        }
    }
}

impl TableAllocator {
    fn new() -> Self {
        Self {
            next_table: 0,
            cache: HashMap::new(),
            dropped: 0,
        }
    }

    fn allocate(&mut self, tables: &mut [Table], spec: TableSpec) -> Option<u8> {
        if let Some(table_id) = self.cache.get(&spec) {
            return Some(*table_id);
        }
        if self.next_table as usize >= tables.len() {
            self.dropped += 1;
            return None;
        }

        let table_id = self.next_table;
        self.next_table = self.next_table.saturating_add(1);

        let mut table = tables[table_id as usize].clone();
        table.clear();
        build_table(&mut table, spec);
        tables[table_id as usize] = table;
        self.cache.insert(spec, table_id);
        Some(table_id)
    }

    fn empty_table(&mut self, tables: &mut [Table]) -> Option<u8> {
        self.allocate(tables, TableSpec::Empty)
    }
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

    let timing_contexts = mod_timing_contexts(module);
    let mut table_allocator = TableAllocator::new();
    let mut phrase_cache: HashMap<[PackedStep; M8_PHRASE_ROWS], u8> = HashMap::new();
    let mut chain_cache: HashMap<[u8; 4], u8> = HashMap::new();
    let mut next_phrase = 0u8;
    let mut next_chain = 0u8;
    let mut current_instruments = vec![EMPTY; module.channel_count];
    let mut current_velocities = vec![EMPTY; module.channel_count];
    let mut volume_slide_memory = vec![0u8; module.channel_count];
    let mut vibrato_memory = vec![0u8; module.channel_count];
    let mut pitch_slide_memory = vec![PitchSlideMemory::default(); module.channel_count];
    let mut active_table_effects = vec![false; module.channel_count];

    for (order_index, pattern_id) in module.orders.iter().enumerate() {
        let Some(pattern) = module.patterns.get(*pattern_id as usize) else {
            report.warnings.push(format!(
                "order {order_index}: pattern {pattern_id} is outside parsed pattern data"
            ));
            continue;
        };

        for channel in 0..module.channel_count.min(M8_TRACKS) {
            let mut phrase_ids = [EMPTY; 4];

            for chunk in 0..4 {
                let mut packed = [empty_packed_step(); M8_PHRASE_ROWS];
                let mut phrase = Phrase::default_ver(version);
                phrase.clear();

                for row_in_chunk in 0..M8_PHRASE_ROWS {
                    let row = chunk * M8_PHRASE_ROWS + row_in_chunk;
                    let cell = pattern.rows[row][channel];
                    let step = convert_cell(
                        module,
                        cell,
                        &mut current_instruments[channel],
                        order_index,
                        *pattern_id,
                        row,
                        channel,
                        timing_contexts
                            .get(&(order_index, row, channel))
                            .copied()
                            .unwrap_or_default(),
                        &mut table_allocator,
                        &mut song.tables,
                        &mut current_velocities[channel],
                        &mut volume_slide_memory[channel],
                        &mut vibrato_memory[channel],
                        &mut pitch_slide_memory[channel],
                        &mut active_table_effects[channel],
                        &mut report,
                    );
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

            let song_index = order_index * M8_TRACKS + channel;
            if song_index < song.song.steps.len() {
                song.song.steps[song_index] = chain_id;
            }
        }
    }

    report.used_phrases = next_phrase as usize;
    report.used_chains = next_chain as usize;
    report.used_tables = table_allocator.next_table as usize;
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
    if module.samples.iter().any(|sample| sample.finetune != 0) {
        report.warnings.push(
            "MOD finetune values are reported but not pitch-calibrated in the M8 sampler yet"
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
        ..M8Report::default()
    };

    clear_song(&mut song);
    install_hvl_wavsynth_instruments(&mut song, module);

    let mut phrase_cache: HashMap<[PackedStep; M8_PHRASE_ROWS], u8> = HashMap::new();
    let mut chain_cache: HashMap<[(u8, u8); 4], u8> = HashMap::new();
    let mut next_phrase = 0u8;
    let mut next_chain = 0u8;
    let chunks_per_track = module.track_length.div_ceil(M8_PHRASE_ROWS).min(4);

    for (position_index, position) in module.positions.iter().enumerate() {
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
                    let row = chunk * M8_PHRASE_ROWS + row_in_chunk;
                    let source = track.rows.get(row).copied().unwrap_or_else(empty_hvl_step);
                    let step = convert_hvl_step(
                        module,
                        source,
                        transpose,
                        position_index,
                        track_id,
                        row,
                        channel,
                        &mut report,
                    );
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

            let song_index = position_index * M8_TRACKS + channel;
            if song_index < song.song.steps.len() {
                song.song.steps[song_index] = chain_id;
            }
        }
    }

    report.used_phrases = next_phrase as usize;
    report.used_chains = next_chain as usize;

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
        None,
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
    install_s3m_sampler_instruments(&mut song, module, &mut report);

    let timing_contexts = s3m_timing_contexts(module);
    let mut table_allocator = TableAllocator::new();
    let mut phrase_cache: HashMap<[PackedStep; M8_PHRASE_ROWS], u8> = HashMap::new();
    let mut chain_cache: HashMap<[u8; 4], u8> = HashMap::new();
    let mut next_phrase = 0u8;
    let mut next_chain = 0u8;
    let mut current_instruments = vec![EMPTY; module.active_channels.len()];
    let mut current_velocities = vec![EMPTY; module.active_channels.len()];
    let mut volume_slide_memory = vec![0u8; module.active_channels.len()];
    let mut vibrato_memory = vec![0u8; module.active_channels.len()];
    let mut pitch_slide_memory = vec![PitchSlideMemory::default(); module.active_channels.len()];
    let mut active_table_effects = vec![false; module.active_channels.len()];

    for (order_index, pattern_id) in module.orders.iter().enumerate() {
        let Some(pattern) = module.patterns.get(*pattern_id as usize) else {
            report.warnings.push(format!(
                "order {order_index}: S3M pattern {pattern_id} is outside parsed pattern data"
            ));
            continue;
        };

        for (track, source_channel) in module.active_channels.iter().take(M8_TRACKS).enumerate() {
            let mut phrase_ids = [EMPTY; 4];
            for (chunk, phrase_slot) in phrase_ids.iter_mut().enumerate() {
                let mut packed = [empty_packed_step(); M8_PHRASE_ROWS];
                let mut phrase = Phrase::default_ver(version);
                phrase.clear();

                for row_in_chunk in 0..M8_PHRASE_ROWS {
                    let row = chunk * M8_PHRASE_ROWS + row_in_chunk;
                    let cell = pattern.rows[row][*source_channel];
                    let step = convert_s3m_cell(
                        module,
                        cell,
                        &mut current_instruments[track],
                        order_index,
                        *pattern_id,
                        row,
                        *source_channel,
                        timing_contexts
                            .get(&(order_index, row, *source_channel))
                            .copied()
                            .unwrap_or_default(),
                        &mut table_allocator,
                        &mut song.tables,
                        &mut current_velocities[track],
                        &mut volume_slide_memory[track],
                        &mut vibrato_memory[track],
                        &mut pitch_slide_memory[track],
                        &mut active_table_effects[track],
                        &mut report,
                    );
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

            let song_index = order_index * M8_TRACKS + track;
            if song_index < song.song.steps.len() {
                song.song.steps[song_index] = chain_id;
            }
        }
    }

    report.used_phrases = next_phrase as usize;
    report.used_chains = next_chain as usize;
    report.used_tables = table_allocator.next_table as usize;
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

fn clear_song(song: &mut Song) {
    song.song.steps.fill(EMPTY);
    for phrase in &mut song.phrases {
        phrase.clear();
    }
    for chain in &mut song.chains {
        chain.clear();
    }
    for table in &mut song.tables {
        table.clear();
    }
    for instrument in &mut song.instruments {
        *instrument = Instrument::None;
    }
}

fn install_sampler_instruments(song: &mut Song, module: &Module) {
    for (index, sample) in module.samples.iter().enumerate().take(Song::N_INSTRUMENTS) {
        if sample.length_bytes == 0 {
            continue;
        }

        let loop_start = if sample.length_bytes == 0 {
            0
        } else {
            ((sample.loop_start_bytes.saturating_mul(255)) / sample.length_bytes).min(255) as u8
        };

        song.instruments[index] = Instrument::Sampler(Sampler {
            number: index as u8,
            name: truncate_ascii(&fallback_sample_name(index, &sample.name), 12),
            transpose: true,
            table_tick: TABLE_TICK_PER_TRACKER_TICK,
            synth_params: sampler_params(sample.volume),
            sample_path: format!("Samples/{}", sample_filename(index, &sample.name)),
            play_mode: if sample.has_loop() {
                SamplePlayMode::FWDLOOP
            } else {
                SamplePlayMode::FWD
            },
            slice: 0,
            start: 0,
            loop_start,
            length: 0xff,
            degrade: 0,
        });
    }
}

fn install_hvl_wavsynth_instruments(song: &mut Song, module: &HvlModule) {
    for (index, instrument) in module
        .instruments
        .iter()
        .enumerate()
        .take(Song::N_INSTRUMENTS)
    {
        song.instruments[index] = Instrument::WavSynth(WavSynth {
            number: index as u8,
            name: truncate_ascii(&fallback_sample_name(index, &instrument.name), 12),
            transpose: true,
            table_tick: instrument.plist_speed,
            synth_params: hvl_synth_params(instrument),
            shape: hvl_wave_shape(instrument),
            size: instrument.wave_length.saturating_mul(32).clamp(1, 0xff),
            mult: 0x80,
            warp: 0,
            scan: 0,
        });
    }
}

fn install_s3m_sampler_instruments(song: &mut Song, module: &S3mModule, report: &mut M8Report) {
    for (index, sample) in module
        .instruments
        .iter()
        .enumerate()
        .take(Song::N_INSTRUMENTS)
    {
        if sample.kind != 0 && sample.kind != 1 {
            report.warnings.push(format!(
                "S3M instrument {} is AdLib/OPL and was not exported",
                index + 1
            ));
            continue;
        }
        if !sample.is_pcm() {
            continue;
        }
        if sample.pack != 0 {
            report.warnings.push(format!(
                "S3M instrument {} uses packed sample data and was not exported",
                index + 1
            ));
            continue;
        }
        if sample.is_16bit() {
            report.warnings.push(format!(
                "S3M instrument {} was converted from 16-bit PCM to WAV",
                index + 1
            ));
        }
        if sample.is_stereo() {
            report.warnings.push(format!(
                "S3M instrument {} was downmixed from stereo to mono WAV",
                index + 1
            ));
        }

        let loop_start = if sample.data.is_empty() {
            0
        } else {
            ((sample.loop_start as usize).saturating_mul(255) / sample.data.len()).min(255) as u8
        };

        song.instruments[index] = Instrument::Sampler(Sampler {
            number: index as u8,
            name: truncate_ascii(&fallback_sample_name(index, &sample.name), 12),
            transpose: true,
            table_tick: TABLE_TICK_PER_TRACKER_TICK,
            synth_params: sampler_params(sample.volume),
            sample_path: format!("Samples/{}", s3m_sample_filename(index, &sample.name)),
            play_mode: if sample.is_looped() {
                SamplePlayMode::FWDLOOP
            } else {
                SamplePlayMode::FWD
            },
            slice: 0,
            start: 0,
            loop_start,
            length: 0xff,
            degrade: 0,
        });
    }
}

fn convert_cell(
    module: &Module,
    cell: Cell,
    current_instrument: &mut u8,
    order: usize,
    pattern: u8,
    row: usize,
    channel: usize,
    timing: TimingContext,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: &mut u8,
    volume_slide_memory: &mut u8,
    vibrato_memory: &mut u8,
    pitch_slide_memory: &mut PitchSlideMemory,
    active_table_effect: &mut bool,
    report: &mut M8Report,
) -> Step {
    if cell.sample_number > 0 {
        *current_instrument = cell.sample_number.saturating_sub(1);
    }

    let mut step = empty_step();
    if let Some(note) = period_to_note(cell.period) {
        step.note = Note(note);
        step.instrument = *current_instrument;
        step.velocity = velocity_for_cell(module, cell, *current_instrument);
        *current_velocity = step.velocity;
    } else if cell.effect == 0x0c {
        step.velocity = volume_to_velocity(cell.effect_param.min(64));
        *current_velocity = step.velocity;
    }

    let mut used_table_effect = false;
    let mapped = map_mod_effect(
        cell,
        timing,
        &mut step,
        table_allocator,
        tables,
        current_velocity,
        volume_slide_memory,
        vibrato_memory,
        pitch_slide_memory,
        &mut used_table_effect,
    );
    update_active_table_effect(
        &mut step,
        table_allocator,
        tables,
        used_table_effect,
        active_table_effect,
    );
    if !mapped && should_report_effect(cell) {
        report.unsupported_effects.push(UnsupportedEffect {
            order,
            pattern,
            row,
            channel,
            effect: cell.effect,
            param: cell.effect_param,
        });
    }

    step
}

fn convert_hvl_step(
    module: &HvlModule,
    source: HvlStep,
    transpose: i8,
    position: usize,
    track: usize,
    row: usize,
    channel: usize,
    report: &mut M8Report,
) -> Step {
    if source.fx != 0 {
        report.unsupported_effects.push(UnsupportedEffect {
            order: position,
            pattern: track as u8,
            row,
            channel,
            effect: source.fx,
            param: source.fx_param,
        });
    }
    if source.fx_b != 0 {
        report.unsupported_effects.push(UnsupportedEffect {
            order: position,
            pattern: track as u8,
            row,
            channel,
            effect: source.fx_b,
            param: source.fx_b_param,
        });
    }

    let mut step = empty_step();
    if source.note == 0 {
        return step;
    }

    step.note = Note((source.note as i16 - 1 + transpose as i16).clamp(0, 0x7f) as u8);
    if source.instrument > 0 {
        step.instrument = source.instrument.saturating_sub(1);
        step.velocity = module
            .instruments
            .get(step.instrument as usize)
            .map(|instrument| volume_to_velocity(instrument.volume))
            .unwrap_or(0xff);
    } else {
        step.velocity = 0xff;
    }

    step
}

fn convert_s3m_cell(
    module: &S3mModule,
    cell: S3mCell,
    current_instrument: &mut u8,
    order: usize,
    pattern: u8,
    row: usize,
    channel: usize,
    timing: TimingContext,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: &mut u8,
    volume_slide_memory: &mut u8,
    vibrato_memory: &mut u8,
    pitch_slide_memory: &mut PitchSlideMemory,
    active_table_effect: &mut bool,
    report: &mut M8Report,
) -> Step {
    if cell.instrument > 0 {
        *current_instrument = cell.instrument.saturating_sub(1);
    }
    let mut step = empty_step();
    if let Some(note) = s3m_note_to_m8(cell.note) {
        step.note = Note(note);
        if note == 0x80 {
            return step;
        }
        step.instrument = *current_instrument;
        step.velocity = if cell.volume <= 64 {
            volume_to_velocity(cell.volume)
        } else {
            module
                .instruments
                .get(*current_instrument as usize)
                .map(|sample| volume_to_velocity(sample.volume))
                .unwrap_or(0xff)
        };
        *current_velocity = step.velocity;
    } else if cell.volume <= 64 {
        step.velocity = volume_to_velocity(cell.volume);
        *current_velocity = step.velocity;
    }

    let mut used_table_effect = false;
    let mapped = map_s3m_effect(
        cell,
        timing,
        &mut step,
        table_allocator,
        tables,
        current_velocity,
        volume_slide_memory,
        vibrato_memory,
        pitch_slide_memory,
        &mut used_table_effect,
    );
    update_active_table_effect(
        &mut step,
        table_allocator,
        tables,
        used_table_effect,
        active_table_effect,
    );
    if !mapped && cell.command != 0 {
        report.unsupported_effects.push(UnsupportedEffect {
            order,
            pattern,
            row,
            channel,
            effect: cell.command,
            param: cell.info,
        });
    }

    step
}

fn should_report_effect(cell: Cell) -> bool {
    if cell.effect == 0 && cell.effect_param == 0 {
        return false;
    }

    !matches!(cell.effect, 0x0c)
}

fn update_active_table_effect(
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    used_table_effect: bool,
    active_table_effect: &mut bool,
) {
    if used_table_effect {
        *active_table_effect = true;
        return;
    }

    if !*active_table_effect {
        return;
    }

    if let Some(table) = table_allocator.empty_table(tables) {
        if push_fx(step, FX_TBL, table) {
            *active_table_effect = false;
        }
    }
}

fn map_mod_effect(
    cell: Cell,
    timing: TimingContext,
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: &mut u8,
    volume_slide_memory: &mut u8,
    vibrato_memory: &mut u8,
    pitch_slide_memory: &mut PitchSlideMemory,
    used_table_effect: &mut bool,
) -> bool {
    match cell.effect {
        0x00 if cell.effect_param != 0 => push_fx(step, FX_ARP, cell.effect_param),
        0x01 => map_pitch_slide(
            step,
            table_allocator,
            tables,
            cell.effect_param,
            timing.speed,
            true,
            pitch_slide_memory,
            used_table_effect,
        ),
        0x02 => map_pitch_slide(
            step,
            table_allocator,
            tables,
            cell.effect_param,
            timing.speed,
            false,
            pitch_slide_memory,
            used_table_effect,
        ),
        0x04 => map_vibrato(step, cell.effect_param, vibrato_memory),
        0x06 => {
            let vibrato_mapped = map_vibrato(step, 0, vibrato_memory);
            let start = if step.velocity == EMPTY {
                *current_velocity
            } else {
                step.velocity
            };
            let volume_mapped = map_volume_slide(
                step,
                table_allocator,
                tables,
                start,
                cell.effect_param,
                timing.speed,
                current_velocity,
                volume_slide_memory,
                used_table_effect,
            );
            vibrato_mapped && volume_mapped
        }
        0x0a => {
            let start = if step.velocity == EMPTY {
                *current_velocity
            } else {
                step.velocity
            };
            map_volume_slide(
                step,
                table_allocator,
                tables,
                start,
                cell.effect_param,
                timing.speed,
                current_velocity,
                volume_slide_memory,
                used_table_effect,
            )
        }
        0x08 => push_fx(step, FX_SAMPLER_PAN, cell.effect_param),
        0x09 => push_fx(step, FX_SAMPLER_STA, cell.effect_param),
        0x0c => true,
        0x0f => timing
            .tpo
            .map(|tempo| push_fx(step, FX_TPO, tempo))
            .unwrap_or(false),
        0x0e => match cell.effect_param >> 4 {
            0x01 => push_fx(
                step,
                FX_SAMPLER_FIN,
                relative_fx_value(positive_delta(cell.effect_param & 0x0f)),
            ),
            0x02 => push_fx(
                step,
                FX_SAMPLER_FIN,
                relative_fx_value(negative_delta(cell.effect_param & 0x0f)),
            ),
            0x08 => push_fx(step, FX_SAMPLER_PAN, (cell.effect_param & 0x0f) * 17),
            0x09 => push_fx(step, FX_RET, (cell.effect_param & 0x0f) << 4),
            0x0a => map_fine_volume_slide(step, cell.effect_param & 0x0f, current_velocity, true),
            0x0b => map_fine_volume_slide(step, cell.effect_param & 0x0f, current_velocity, false),
            0x0c => push_fx(step, FX_KIL, cell.effect_param & 0x0f),
            0x0d => push_fx(step, FX_DEL, cell.effect_param & 0x0f),
            _ => false,
        },
        _ => false,
    }
}

fn map_s3m_effect(
    cell: S3mCell,
    timing: TimingContext,
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: &mut u8,
    volume_slide_memory: &mut u8,
    vibrato_memory: &mut u8,
    pitch_slide_memory: &mut PitchSlideMemory,
    used_table_effect: &mut bool,
) -> bool {
    match cell.command {
        0 => true,
        1 | 20 => timing
            .tpo
            .map(|tempo| push_fx(step, FX_TPO, tempo))
            .unwrap_or(false),
        4 => {
            let start = if step.velocity == EMPTY {
                *current_velocity
            } else {
                step.velocity
            };
            map_volume_slide(
                step,
                table_allocator,
                tables,
                start,
                cell.info,
                timing.speed,
                current_velocity,
                volume_slide_memory,
                used_table_effect,
            )
        }
        5 => map_pitch_slide(
            step,
            table_allocator,
            tables,
            cell.info,
            timing.speed,
            false,
            pitch_slide_memory,
            used_table_effect,
        ),
        6 => map_pitch_slide(
            step,
            table_allocator,
            tables,
            cell.info,
            timing.speed,
            true,
            pitch_slide_memory,
            used_table_effect,
        ),
        8 | 21 => map_vibrato(step, cell.info, vibrato_memory),
        10 => push_fx(step, FX_ARP, cell.info),
        11 => {
            let vibrato_mapped = map_vibrato(step, 0, vibrato_memory);
            let start = if step.velocity == EMPTY {
                *current_velocity
            } else {
                step.velocity
            };
            let volume_mapped = map_volume_slide(
                step,
                table_allocator,
                tables,
                start,
                cell.info,
                timing.speed,
                current_velocity,
                volume_slide_memory,
                used_table_effect,
            );
            vibrato_mapped && volume_mapped
        }
        15 => push_fx(step, FX_SAMPLER_STA, cell.info),
        17 => push_fx(step, FX_RET, cell.info),
        19 => match cell.info >> 4 {
            0x08 => push_fx(step, FX_SAMPLER_PAN, (cell.info & 0x0f) * 17),
            0x0c => push_fx(step, FX_KIL, cell.info & 0x0f),
            0x0d => push_fx(step, FX_DEL, cell.info & 0x0f),
            _ => false,
        },
        _ => false,
    }
}

fn map_volume_slide(
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    start_velocity: u8,
    param: u8,
    ticks: u8,
    current_velocity: &mut u8,
    volume_slide_memory: &mut u8,
    used_table_effect: &mut bool,
) -> bool {
    let param = if param == 0 {
        *volume_slide_memory
    } else {
        *volume_slide_memory = param;
        param
    };
    if param == 0 {
        return true;
    }
    if start_velocity == EMPTY {
        return true;
    }

    if is_s3m_fine_volume_slide(param) {
        return map_fine_volume_slide(step, param & 0x0f, current_velocity, param >> 4 != 0x0f);
    }

    let up = param >> 4;
    let down = param & 0x0f;
    let delta = if up != 0 && down == 0 {
        positive_delta(up.saturating_mul(4))
    } else if down != 0 && up == 0 {
        negative_delta(down.saturating_mul(4))
    } else {
        return false;
    };

    let rows = tracker_effect_rows(ticks);
    if rows == 0 {
        return true;
    }
    *current_velocity = stepped_value(start_velocity, delta, rows as usize - 1);

    map_table(
        step,
        table_allocator,
        tables,
        TableSpec::VolumeSlide {
            start: start_velocity,
            delta,
            rows,
        },
        used_table_effect,
    )
}

fn map_pitch_slide(
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    amount: u8,
    speed: u8,
    upward: bool,
    memory: &mut PitchSlideMemory,
    used_table_effect: &mut bool,
) -> bool {
    let amount = if amount == 0 {
        if upward { memory.up } else { memory.down }
    } else {
        if upward {
            memory.up = amount;
        } else {
            memory.down = amount;
        }
        amount
    };
    if amount == 0 {
        return true;
    }

    let rows = tracker_effect_rows(speed);
    if rows == 0 {
        return true;
    }

    map_table(
        step,
        table_allocator,
        tables,
        TableSpec::FineSlide {
            delta: if upward {
                positive_delta(amount)
            } else {
                negative_delta(amount)
            },
            rows,
        },
        used_table_effect,
    )
}

fn map_table(
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    spec: TableSpec,
    used_table_effect: &mut bool,
) -> bool {
    let mapped = table_allocator
        .allocate(tables, spec)
        .map(|table| push_fx(step, FX_TBL, table))
        .unwrap_or(false);
    if mapped {
        *used_table_effect = true;
    }
    mapped
}

fn positive_delta(value: u8) -> i8 {
    value.min(63) as i8
}

fn negative_delta(value: u8) -> i8 {
    -(value.min(63) as i8)
}

fn relative_fx_value(delta: i8) -> u8 {
    if delta >= 0 {
        delta as u8
    } else {
        0u8.wrapping_sub(delta.unsigned_abs())
    }
}

fn map_vibrato(step: &mut Step, param: u8, vibrato_memory: &mut u8) -> bool {
    let param = if param == 0 {
        *vibrato_memory
    } else {
        *vibrato_memory = param;
        param
    };
    if param == 0 {
        return true;
    }
    push_fx(step, FX_PVB, param)
}

fn map_fine_volume_slide(
    step: &mut Step,
    amount: u8,
    current_velocity: &mut u8,
    upward: bool,
) -> bool {
    if amount == 0 || *current_velocity == EMPTY {
        return true;
    }
    let delta = if upward {
        positive_delta(amount.saturating_mul(4))
    } else {
        negative_delta(amount.saturating_mul(4))
    };
    *current_velocity = stepped_value(*current_velocity, delta, 0);
    step.velocity = *current_velocity;
    true
}

fn is_s3m_fine_volume_slide(param: u8) -> bool {
    let up = param >> 4;
    let down = param & 0x0f;
    (down == 0x0f && up != 0) || (up == 0x0f && down != 0)
}

fn build_table(table: &mut Table, spec: TableSpec) {
    match spec {
        TableSpec::Empty => {}
        TableSpec::VolumeSlide { start, delta, rows } => {
            for (index, step) in table.steps.iter_mut().take(rows as usize).enumerate() {
                step.velocity = stepped_value(start, delta, index);
            }
        }
        TableSpec::FineSlide { delta, rows } => {
            for step in table.steps.iter_mut().take(rows as usize) {
                step.fx1 = FX {
                    command: FX_SAMPLER_FIN,
                    value: relative_fx_value(delta),
                };
            }
        }
    }
}

fn tracker_effect_rows(speed: u8) -> u8 {
    speed.saturating_sub(1).min(16)
}

fn stepped_value(start: u8, delta: i8, index: usize) -> u8 {
    let next = start as i16 + delta as i16 * (index as i16 + 1);
    next.clamp(0, 255) as u8
}

fn push_fx(step: &mut Step, command: u8, value: u8) -> bool {
    let fx = FX { command, value };
    if step.fx1.is_empty() {
        step.fx1 = fx;
        true
    } else if step.fx2.is_empty() {
        step.fx2 = fx;
        true
    } else if step.fx3.is_empty() {
        step.fx3 = fx;
        true
    } else {
        false
    }
}

fn velocity_for_cell(module: &Module, cell: Cell, instrument: u8) -> u8 {
    if cell.effect == 0x0c {
        return volume_to_velocity(cell.effect_param.min(64));
    }

    module
        .samples
        .get(instrument as usize)
        .map(|sample| volume_to_velocity(sample.volume))
        .unwrap_or(0xff)
}

fn volume_to_velocity(volume: u8) -> u8 {
    ((volume.min(64) as u16 * 255) / 64) as u8
}

fn sampler_params(volume: u8) -> SynthParams {
    SynthParams {
        volume: volume_to_velocity(volume),
        pitch: 0,
        fine_tune: 0x80,
        filter_type: 0,
        filter_cutoff: 0xff,
        filter_res: 0,
        amp: SAMPLER_DEFAULT_AMP,
        limit: LimitType::try_from(0).expect("limit type 0 is valid"),
        mixer_pan: 0x80,
        mixer_dry: SAMPLER_DEFAULT_DRY,
        mixer_mfx: 0,
        mixer_delay: 0,
        mixer_reverb: 0,
        associated_eq: 0x80,
        mods: [
            AHDEnv::default().to_mod(),
            AHDEnv::default().to_mod(),
            AHDEnv::default().to_mod(),
            AHDEnv::default().to_mod(),
        ],
    }
}

fn hvl_synth_params(instrument: &HvlInstrument) -> SynthParams {
    let mut params = sampler_params(instrument.volume);
    params.filter_cutoff = instrument.filter_upper_limit.saturating_mul(4).max(1);
    params.filter_res = instrument.filter_speed.saturating_mul(4);
    params
}

fn hvl_wave_shape(instrument: &HvlInstrument) -> WavShape {
    let waveform = instrument
        .plist
        .iter()
        .find(|entry| entry.waveform != 0)
        .map(|entry| entry.waveform)
        .unwrap_or(0);

    match waveform {
        0 => WavShape::TRIANGLE,
        1 => WavShape::SAW,
        2 => WavShape::PULSE50,
        3 => WavShape::NOISE,
        _ => WavShape::PULSE50,
    }
}

fn empty_step() -> Step {
    let mut step = Step::default();
    step.clear();
    step
}

fn empty_packed_step() -> PackedStep {
    pack_step(&empty_step())
}

fn pack_step(step: &Step) -> PackedStep {
    PackedStep {
        note: step.note.0,
        velocity: step.velocity,
        instrument: step.instrument,
        fx: [pack_fx(step.fx1), pack_fx(step.fx2), pack_fx(step.fx3)],
    }
}

fn pack_fx(fx: FX) -> (u8, u8) {
    (fx.command, fx.value)
}

fn empty_hvl_step() -> HvlStep {
    HvlStep {
        note: 0,
        instrument: 0,
        fx: 0,
        fx_param: 0,
        fx_b: 0,
        fx_b_param: 0,
    }
}

fn mod_m8_tempo(module: &Module) -> f32 {
    let mut speed = 6;
    let mut tempo = 125;

    if let Some(pattern_id) = module.orders.first() {
        if let Some(pattern) = module.patterns.get(*pattern_id as usize) {
            for row in &pattern.rows {
                for cell in row.iter().take(module.channel_count) {
                    if cell.effect == 0x0f && cell.effect_param != 0 {
                        if cell.effect_param <= 32 {
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
    }

    tracker_rows_to_m8_bpm(speed, tempo)
}

fn mod_timing_contexts(module: &Module) -> HashMap<(usize, usize, usize), TimingContext> {
    let mut out = HashMap::new();
    let mut speed = 6;
    let mut tempo = 125;

    for (order_index, pattern_id) in module.orders.iter().enumerate() {
        let Some(pattern) = module.patterns.get(*pattern_id as usize) else {
            continue;
        };
        for (row_index, row) in pattern.rows.iter().enumerate() {
            for (channel, cell) in row.iter().take(module.channel_count).enumerate() {
                let mut tpo = None;
                if cell.effect == 0x0f && cell.effect_param != 0 {
                    if cell.effect_param <= 32 {
                        speed = cell.effect_param;
                    } else {
                        tempo = cell.effect_param;
                    }
                    tpo = Some(tracker_rows_to_m8_tpo(speed, tempo));
                }
                out.insert(
                    (order_index, row_index, channel),
                    TimingContext { speed, tpo },
                );
            }
        }
    }

    out
}

fn s3m_timing_contexts(module: &S3mModule) -> HashMap<(usize, usize, usize), TimingContext> {
    let mut out = HashMap::new();
    let mut speed = module.initial_speed;
    let mut tempo = module.initial_tempo;

    for (order_index, pattern_id) in module.orders.iter().enumerate() {
        let Some(pattern) = module.patterns.get(*pattern_id as usize) else {
            continue;
        };
        for (row_index, row) in pattern.rows.iter().enumerate() {
            for source_channel in &module.active_channels {
                let cell = row[*source_channel];
                let mut tpo = None;
                match cell.command {
                    1 if cell.info != 0 => {
                        speed = cell.info;
                        tpo = Some(tracker_rows_to_m8_tpo(speed, tempo));
                    }
                    20 if cell.info != 0 => {
                        tempo = cell.info;
                        tpo = Some(tracker_rows_to_m8_tpo(speed, tempo));
                    }
                    _ => {}
                }
                out.insert(
                    (order_index, row_index, *source_channel),
                    TimingContext { speed, tpo },
                );
            }
        }
    }

    out
}

fn tracker_rows_to_m8_bpm(speed: u8, tempo: u8) -> f32 {
    let speed = speed.max(1) as f32;
    let tempo = tempo.max(32) as f32;
    (tempo * 6.0 / speed).clamp(20.0, 999.0)
}

fn tracker_rows_to_m8_tpo(speed: u8, tempo: u8) -> u8 {
    tracker_rows_to_m8_bpm(speed, tempo)
        .round()
        .clamp(20.0, 255.0) as u8
}

fn patch_header(
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

fn patch_f32(bytes: &mut [u8], offset: usize, value: f32) {
    if bytes.len() < offset + 4 {
        return;
    }

    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn patch_fixed_ascii(bytes: &mut [u8], offset: usize, len: usize, value: &str) {
    if bytes.len() < offset + len {
        return;
    }

    let value = truncate_ascii(value, len);
    bytes[offset..offset + len].fill(0);
    let value_bytes = value.as_bytes();
    bytes[offset..offset + value_bytes.len()].copy_from_slice(value_bytes);
}

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

fn fallback_sample_name(index: usize, name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        format!("sample_{:02}", index + 1)
    } else {
        trimmed.to_string()
    }
}

fn sanitize_name(name: &str) -> String {
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

fn truncate_ascii(value: &str, max_len: usize) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii() && !ch.is_ascii_control())
        .take(max_len)
        .collect()
}
