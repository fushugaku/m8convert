use std::collections::HashMap;

use m8_file_parser::{
    AHDEnv, Chain, ChainStep, FX, Instrument, LimitType, Note, Phrase, SamplePlayMode, Sampler,
    Song, Step, SynthParams, WavShape, WavSynth, reader::Reader, writer::Writer,
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
        ..M8Report::default()
    };

    clear_song(&mut song);
    install_sampler_instruments(&mut song, module);

    let mut phrase_cache: HashMap<[PackedStep; M8_PHRASE_ROWS], u8> = HashMap::new();
    let mut chain_cache: HashMap<[u8; 4], u8> = HashMap::new();
    let mut next_phrase = 0u8;
    let mut next_chain = 0u8;
    let mut current_instruments = vec![EMPTY; module.channel_count];

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
        ..M8Report::default()
    };

    clear_song(&mut song);
    install_s3m_sampler_instruments(&mut song, module, &mut report);

    let mut phrase_cache: HashMap<[PackedStep; M8_PHRASE_ROWS], u8> = HashMap::new();
    let mut chain_cache: HashMap<[u8; 4], u8> = HashMap::new();
    let mut next_phrase = 0u8;
    let mut next_chain = 0u8;
    let mut current_instruments = vec![EMPTY; module.active_channels.len()];

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
                        track,
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
            table_tick: 0,
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
            table_tick: 0,
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
    report: &mut M8Report,
) -> Step {
    if cell.sample_number > 0 {
        *current_instrument = cell.sample_number.saturating_sub(1);
    }

    if should_report_effect(cell) {
        report.unsupported_effects.push(UnsupportedEffect {
            order,
            pattern,
            row,
            channel,
            effect: cell.effect,
            param: cell.effect_param,
        });
    }

    let mut step = empty_step();
    if let Some(note) = period_to_note(cell.period) {
        step.note = Note(note);
        step.instrument = *current_instrument;
        step.velocity = velocity_for_cell(module, cell, *current_instrument);
    } else if cell.effect == 0x0c {
        step.velocity = volume_to_velocity(cell.effect_param.min(64));
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
    report: &mut M8Report,
) -> Step {
    if cell.instrument > 0 {
        *current_instrument = cell.instrument.saturating_sub(1);
    }
    if cell.command != 0 {
        report.unsupported_effects.push(UnsupportedEffect {
            order,
            pattern,
            row,
            channel,
            effect: cell.command,
            param: cell.info,
        });
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
    } else if cell.volume <= 64 {
        step.velocity = volume_to_velocity(cell.volume);
    }

    step
}

fn should_report_effect(cell: Cell) -> bool {
    if cell.effect == 0 && cell.effect_param == 0 {
        return false;
    }

    !matches!(cell.effect, 0x0c)
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
        amp: 0xff,
        limit: LimitType::try_from(0).expect("limit type 0 is valid"),
        mixer_pan: 0x80,
        mixer_dry: 0xff,
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

fn patch_header(song_bytes: &mut [u8], title: &str, bundle_directory: Option<&str>) {
    let directory_offset = 14;
    if let Some(directory) = bundle_directory {
        patch_fixed_ascii(song_bytes, directory_offset, 128, directory);
    }

    let name_offset = 14 + 128 + 1 + 4 + 1;
    patch_fixed_ascii(song_bytes, name_offset, 12, title);
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
