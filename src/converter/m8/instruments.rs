use super::*;

#[derive(Debug, Clone)]
pub(super) struct S3mPanPlan {
    pub(super) channel_static_pans: Vec<Option<u8>>,
    pub(super) instrument_remaps: HashMap<(u8, u8), u8>,
}

pub(super) fn clear_song(song: &mut Song) {
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

pub(super) fn install_sampler_instruments(song: &mut Song, module: &Module) {
    for (index, sample) in module.samples.iter().enumerate().take(Song::N_INSTRUMENTS) {
        if sample.length_bytes == 0 {
            continue;
        }

        let loop_start = scaled_sample_position(sample.loop_start_bytes, sample.length_bytes);
        let length = if sample.has_loop() {
            scaled_sample_position(
                sample
                    .loop_start_bytes
                    .saturating_add(sample.loop_length_bytes)
                    .min(sample.length_bytes),
                sample.length_bytes,
            )
        } else {
            0xff
        };
        let mut synth_params = sampler_params(sample.volume);
        synth_params.fine_tune = mod_finetune_to_m8(sample.finetune);

        song.instruments[index] = Instrument::Sampler(Sampler {
            number: index as u8,
            name: truncate_ascii(&fallback_sample_name(index, &sample.name), 12),
            transpose: true,
            table_tick: TABLE_TICK_PER_TRACKER_TICK,
            synth_params,
            sample_path: format!("Samples/{}", sample_filename(index, &sample.name)),
            play_mode: if sample.has_loop() {
                SamplePlayMode::FWDLOOP
            } else {
                SamplePlayMode::FWD
            },
            slice: 0,
            start: 0,
            loop_start,
            length,
            degrade: 0,
        });
    }
}

pub(super) fn install_hvl_wavsynth_instruments(song: &mut Song, module: &HvlModule) {
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

pub(super) fn install_hvl_performance_tables(
    song: &mut Song,
    module: &HvlModule,
) -> Vec<Option<u8>> {
    let mut table_ids = vec![None; module.instruments.len().min(Song::N_TABLES)];
    for (index, instrument) in module.instruments.iter().enumerate().take(Song::N_TABLES) {
        if instrument.plist.is_empty() {
            continue;
        }
        let mut table = song.tables[index].clone();
        table.clear();
        for (row, entry) in instrument.plist.iter().take(16).enumerate() {
            let step = &mut table.steps[row];
            step.fx1 = FX {
                command: FX_WAVSYNTH_OSC,
                value: hvl_plist_waveform(entry.waveform),
            };
            if entry.note != 0 {
                step.transpose = entry.note;
            }
            for fx_index in 0..2 {
                if entry.fx[fx_index] == 0x0c {
                    step.velocity = volume_to_velocity(entry.fx_param[fx_index].min(64));
                    continue;
                }
                if let Some(fx) = hvl_plist_fx(entry.fx[fx_index], entry.fx_param[fx_index]) {
                    if step.fx2.is_empty() {
                        step.fx2 = fx;
                    } else if step.fx3.is_empty() {
                        step.fx3 = fx;
                    }
                }
            }
        }
        if !table.is_empty() {
            song.tables[index] = table;
            table_ids[index] = Some(index as u8);
        }
    }
    table_ids
}

pub(super) fn hvl_plist_fx(command: u8, param: u8) -> Option<FX> {
    let fx = match command {
        0x01 => FX {
            command: FX_PBN,
            value: pitch_bend_value(param, true),
        },
        0x02 => FX {
            command: FX_PBN,
            value: pitch_bend_value(param, false),
        },
        0x04 => FX {
            command: FX_WAVSYNTH_CUT,
            value: param,
        },
        0x0f => FX {
            command: FX_WAVSYNTH_SIZ,
            value: param.max(1),
        },
        _ => return None,
    };
    Some(fx)
}

pub(super) fn hvl_plist_waveform(waveform: u8) -> u8 {
    match waveform & 0x07 {
        0 => WavShape::TRIANGLE.into(),
        1 => WavShape::SAW.into(),
        2 => WavShape::PULSE50.into(),
        3 => WavShape::NOISE_PITCHED.into(),
        4 => WavShape::SINE.into(),
        _ => WavShape::PULSE25.into(),
    }
}

pub(super) fn install_s3m_sampler_instruments(
    song: &mut Song,
    module: &S3mModule,
    report: &mut M8Report,
) {
    let table_tick = s3m_table_tick(module.initial_speed);
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
                "S3M instrument {} stereo channels were preserved in the exported WAV",
                index + 1
            ));
        }

        let sample_len = sample.data.len();
        let loop_start = scaled_sample_position(sample.loop_start as usize, sample_len);
        let length = if sample.is_looped() {
            scaled_sample_position(sample.loop_end.min(sample_len as u32) as usize, sample_len)
        } else {
            0xff
        };

        let mut synth_params = sampler_params(sample.volume);
        if let Some(envelope) = sample.amp_envelope {
            synth_params.mods[0] = AHDEnv {
                dest: 1,
                amount: envelope.amount,
                attack: envelope.attack,
                hold: envelope.hold,
                decay: envelope.decay,
            }
            .to_mod();
        }
        if let Some(vibrato) = sample.auto_vibrato
            && vibrato.depth != 0
            && vibrato.speed != 0
        {
            synth_params.mods[1] = LFO {
                shape: LfoShape::SIN,
                dest: 2,
                trigger_mode: LfoTriggerMode::RETRIG,
                freq: vibrato.speed,
                amount: vibrato.depth.saturating_mul(12),
                retrigger: 0,
            }
            .to_mod();
        }

        song.instruments[index] = Instrument::Sampler(Sampler {
            number: index as u8,
            name: truncate_ascii(&fallback_sample_name(index, &sample.name), 12),
            transpose: true,
            table_tick,
            synth_params,
            sample_path: format!("Samples/{}", s3m_sample_filename(index, &sample.name)),
            play_mode: if sample.ping_pong_loop {
                SamplePlayMode::FWD_PP
            } else if sample.is_looped() {
                SamplePlayMode::FWDLOOP
            } else {
                SamplePlayMode::FWD
            },
            slice: 0,
            start: 0,
            loop_start,
            length,
            degrade: 0,
        });
    }
}

pub(super) fn install_s3m_static_pan_instruments(
    song: &mut Song,
    module: &S3mModule,
    playback_blocks: &[PlaybackBlock],
    report: &mut M8Report,
) -> S3mPanPlan {
    let channel_static_pans = s3m_static_channel_pans(module, playback_blocks);
    let (static_uses, neutral_uses) =
        s3m_static_pan_instrument_uses(module, playback_blocks, &channel_static_pans);
    let mut instrument_remaps = HashMap::new();
    let mut next_slot = first_free_instrument_slot(song, 0);

    let mut source_instruments = static_uses.keys().copied().collect::<Vec<_>>();
    source_instruments.sort_unstable();

    for source_instrument in source_instruments {
        if source_instrument >= Song::N_INSTRUMENTS {
            continue;
        }
        if !matches!(song.instruments[source_instrument], Instrument::Sampler(_)) {
            continue;
        }

        let mut pans = static_uses
            .get(&source_instrument)
            .map(|values| values.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        pans.sort_unstable();

        let mut can_reuse_original = !neutral_uses.contains(&source_instrument);
        for pan in pans {
            if pan == 0x80 {
                continue;
            }

            if can_reuse_original {
                if let Instrument::Sampler(sampler) = &mut song.instruments[source_instrument] {
                    sampler.synth_params.mixer_pan = pan;
                }
                instrument_remaps.insert((source_instrument as u8, pan), source_instrument as u8);
                can_reuse_original = false;
                continue;
            }

            let Some(slot) = next_slot else {
                report.warnings.push(format!(
                    "S3M static pan for instrument {} could not be moved to an M8 instrument because all instrument slots are used",
                    source_instrument + 1
                ));
                continue;
            };

            let Instrument::Sampler(mut sampler) = song.instruments[source_instrument].clone()
            else {
                continue;
            };
            sampler.number = slot as u8;
            sampler.name = truncate_ascii(&format!("{} P{:02X}", sampler.name, pan), 12);
            sampler.synth_params.mixer_pan = pan;
            song.instruments[slot] = Instrument::Sampler(sampler);
            instrument_remaps.insert((source_instrument as u8, pan), slot as u8);
            next_slot = first_free_instrument_slot(song, slot + 1);
        }
    }

    S3mPanPlan {
        channel_static_pans,
        instrument_remaps,
    }
}

pub(super) fn s3m_static_channel_pans(
    module: &S3mModule,
    playback_blocks: &[PlaybackBlock],
) -> Vec<Option<u8>> {
    let mut channel_static_pans = vec![None; module.channel_pans.len()];
    for source_channel in module.active_channels.iter().take(M8_TRACKS) {
        channel_static_pans[*source_channel] =
            s3m_channel_static_note_pan(module, playback_blocks, *source_channel);
    }
    channel_static_pans
}

pub(super) fn s3m_channel_static_note_pan(
    module: &S3mModule,
    playback_blocks: &[PlaybackBlock],
    source_channel: usize,
) -> Option<u8> {
    let mut current_pan = module
        .channel_pans
        .get(source_channel)
        .copied()
        .unwrap_or(0x80);
    let mut note_pan = None;

    for block in playback_blocks {
        for playback_row in &block.rows {
            if !playback_row.emit_cells {
                continue;
            }
            let cell = module
                .patterns
                .get(playback_row.pattern_id as usize)
                .and_then(|pattern| pattern.rows.get(playback_row.source_row))
                .and_then(|row| row.get(source_channel))
                .copied()
                .unwrap_or(S3mCell::EMPTY);
            let row_pan = s3m_cell_pan(cell, module.pan_command_uses_8bit).unwrap_or(current_pan);
            if s3m_cell_triggers_note(cell) {
                match note_pan {
                    Some(existing) if existing != row_pan => return None,
                    None => note_pan = Some(row_pan),
                    _ => {}
                }
            }
            current_pan = row_pan;
        }
    }

    Some(note_pan.unwrap_or(current_pan))
}

pub(super) fn s3m_static_pan_instrument_uses(
    module: &S3mModule,
    playback_blocks: &[PlaybackBlock],
    channel_static_pans: &[Option<u8>],
) -> (HashMap<usize, HashSet<u8>>, HashSet<usize>) {
    let mut static_uses = HashMap::<usize, HashSet<u8>>::new();
    let mut neutral_uses = HashSet::new();
    let mut current_instruments = vec![EMPTY; module.channel_pans.len()];

    for block in playback_blocks {
        for source_channel in module.active_channels.iter().take(M8_TRACKS) {
            for playback_row in &block.rows {
                if !playback_row.emit_cells {
                    continue;
                }
                let cell = module
                    .patterns
                    .get(playback_row.pattern_id as usize)
                    .and_then(|pattern| pattern.rows.get(playback_row.source_row))
                    .and_then(|row| row.get(*source_channel))
                    .copied()
                    .unwrap_or(S3mCell::EMPTY);
                if cell.instrument > 0 {
                    current_instruments[*source_channel] = cell.instrument.saturating_sub(1);
                }
                if !s3m_cell_triggers_note(cell) {
                    continue;
                }
                let instrument = current_instruments[*source_channel];
                if instrument == EMPTY {
                    continue;
                }
                let instrument = instrument as usize;
                match channel_static_pans.get(*source_channel).copied().flatten() {
                    Some(0x80) | None => {
                        neutral_uses.insert(instrument);
                    }
                    Some(pan) => {
                        static_uses.entry(instrument).or_default().insert(pan);
                    }
                }
            }
        }
    }

    (static_uses, neutral_uses)
}

pub(super) fn first_free_instrument_slot(song: &Song, start: usize) -> Option<usize> {
    song.instruments
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, instrument)| matches!(instrument, Instrument::None).then_some(index))
}

pub(super) fn s3m_table_tick(speed: u8) -> u8 {
    let speed = speed.max(1);
    ((6 + speed / 2) / speed).clamp(1, 6)
}

pub(super) fn scaled_sample_position(position: usize, sample_len: usize) -> u8 {
    if sample_len == 0 {
        return 0;
    }
    if position >= sample_len {
        return 0xff;
    }
    ((position as u64 * 255 + sample_len as u64 / 2) / sample_len as u64).min(255) as u8
}

pub(super) fn mod_finetune_to_m8(finetune: i8) -> u8 {
    (0x80i16 + finetune.clamp(-8, 7) as i16 * 16).clamp(0, 255) as u8
}

pub(super) fn mod_finetune_nibble_to_signed(value: u8) -> i8 {
    let value = value & 0x0f;
    if value <= 7 {
        value as i8
    } else {
        value as i8 - 16
    }
}

pub(super) fn sampler_params(volume: u8) -> SynthParams {
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

pub(super) fn hvl_synth_params(instrument: &HvlInstrument) -> SynthParams {
    let mut params = sampler_params(instrument.volume);
    params.filter_cutoff = instrument.filter_upper_limit.saturating_mul(4).max(1);
    params.filter_res = instrument.filter_speed.saturating_mul(4);
    if hvl_has_envelope(instrument) {
        params.mods[0] = AHDEnv {
            dest: 1,
            amount: hvl_envelope_amount(instrument),
            attack: hvl_frame_to_m8(instrument.envelope.attack_frames),
            hold: hvl_frame_to_m8(instrument.envelope.sustain_frames),
            decay: hvl_frame_to_m8(
                instrument
                    .envelope
                    .decay_frames
                    .saturating_add(instrument.envelope.release_frames),
            ),
        }
        .to_mod();
    }
    if instrument.vibrato_depth != 0 && instrument.vibrato_speed != 0 {
        params.mods[1] = LFO {
            shape: LfoShape::SIN,
            dest: 2,
            trigger_mode: LfoTriggerMode::RETRIG,
            freq: instrument.vibrato_speed,
            amount: instrument.vibrato_depth.saturating_mul(12),
            retrigger: instrument.vibrato_delay,
        }
        .to_mod();
    }
    params
}

pub(super) fn hvl_has_envelope(instrument: &HvlInstrument) -> bool {
    let envelope = &instrument.envelope;
    envelope.attack_frames != 0
        || envelope.decay_frames != 0
        || envelope.sustain_frames != 0
        || envelope.release_frames != 0
}

pub(super) fn hvl_envelope_amount(instrument: &HvlInstrument) -> u8 {
    let peak = instrument
        .envelope
        .attack_volume
        .max(instrument.envelope.decay_volume)
        .max(instrument.envelope.release_volume)
        .min(64);
    volume_to_velocity(peak)
}

pub(super) fn hvl_frame_to_m8(frames: u8) -> u8 {
    frames.saturating_mul(4).max(frames.min(1))
}

pub(super) fn hvl_wave_shape(instrument: &HvlInstrument) -> WavShape {
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
