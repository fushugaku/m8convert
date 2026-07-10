use super::*;

pub(super) fn convert_cell(
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
    tremolo_memory: &mut u8,
    pitch_slide_memory: &mut PitchSlideMemory,
    tone_portamento: &mut TonePortamentoState,
    retrigger_memory: &mut RetriggerMemory,
    sample_offset_memory: &mut SampleOffsetMemory,
    waveform_state: &mut TrackerWaveformState,
    current_pan: &mut u8,
    active_table_effect: &mut bool,
    active_pitch_bend: &mut bool,
    report: &mut M8Report,
) -> Step {
    let triggers_note = period_to_note(cell.period).is_some() && !is_mod_tone_portamento(cell);
    if cell.sample_number > 0 {
        *current_instrument = cell.sample_number.saturating_sub(1);
    }

    let mut step = empty_step();
    if let Some(note) = period_to_note(cell.period) {
        if is_mod_tone_portamento(cell) {
            step.note = Note(note);
            tone_portamento.target_note = Some(note);
            tone_portamento.target_period = Some(cell.period);
        } else {
            tone_portamento.current_note = Some(note);
            tone_portamento.target_note = None;
            tone_portamento.current_period = Some(cell.period);
            tone_portamento.target_period = None;
            tone_portamento.speed = 0;
            pitch_slide_memory.up = 0;
            pitch_slide_memory.down = 0;
        }
    }

    if let Some(note) = period_to_note(cell.period).filter(|_| !is_mod_tone_portamento(cell)) {
        step.note = Note(note);
        step.instrument = *current_instrument;
        step.velocity = velocity_for_cell(module, cell, *current_instrument);
        *current_velocity = step.velocity;
    } else if cell.effect == 0x0c {
        step.velocity = volume_to_velocity(cell.effect_param.min(64));
        *current_velocity = step.velocity;
    }

    let mut used_table_effect = false;
    let mut used_pitch_bend = false;
    let mapped = map_mod_effect(
        module,
        cell,
        timing,
        &mut step,
        table_allocator,
        tables,
        *current_instrument,
        current_velocity,
        volume_slide_memory,
        vibrato_memory,
        tremolo_memory,
        pitch_slide_memory,
        tone_portamento,
        retrigger_memory,
        sample_offset_memory,
        waveform_state,
        current_pan,
        &mut used_table_effect,
        &mut used_pitch_bend,
        EffectLocation {
            order,
            pattern,
            row,
            channel,
        },
        report,
    );
    if triggers_note && !mod_cell_sets_pan(cell) {
        push_fx(&mut step, FX_SAMPLER_PAN, *current_pan);
    }
    update_active_pitch_bend(&mut step, used_pitch_bend, active_pitch_bend);
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

pub(super) fn mod_default_channel_pan(channel: usize) -> u8 {
    match channel % 4 {
        0 | 3 => 0x00,
        _ => 0xff,
    }
}

fn mod_cell_sets_pan(cell: Cell) -> bool {
    cell.effect == 0x08 || (cell.effect == 0x0e && cell.effect_param >> 4 == 0x08)
}

pub(super) fn continued_mod_cell(cell: Cell) -> Cell {
    let continuous =
        matches!(cell.effect, 0x01..=0x07 | 0x0a) || (cell.effect == 0 && cell.effect_param != 0);
    Cell {
        period: 0,
        sample_number: 0,
        effect: if continuous { cell.effect } else { 0 },
        effect_param: if continuous { cell.effect_param } else { 0 },
    }
}

pub(super) fn continued_s3m_cell(cell: S3mCell) -> S3mCell {
    let continuous_effect = match cell.command {
        4 | 14 | 23 => !is_s3m_fine_volume_slide(cell.info),
        5 | 6 => !matches!(cell.info >> 4, 0x0e | 0x0f),
        16 => !is_s3m_fine_pan_slide(cell.info),
        7..=12 | 17 | 18 | 21 | 25 => true,
        _ => false,
    };
    let continuous_volume = matches!(
        s3m_volume_column_command(cell.volume),
        S3mVolumeColumnCommand::VolumeUp(_)
            | S3mVolumeColumnCommand::VolumeDown(_)
            | S3mVolumeColumnCommand::PortamentoDown(_)
            | S3mVolumeColumnCommand::PortamentoUp(_)
            | S3mVolumeColumnCommand::TonePortamento(_)
            | S3mVolumeColumnCommand::VibratoDepth(_)
    );
    S3mCell {
        note: 0xff,
        instrument: 0,
        volume: if continuous_volume { cell.volume } else { 0xff },
        command: if continuous_effect { cell.command } else { 0 },
        info: if continuous_effect { cell.info } else { 0 },
    }
}

pub(super) fn convert_hvl_step(
    module: &HvlModule,
    source: HvlStep,
    position: usize,
    track: usize,
    row: usize,
    channel: usize,
    timing: TimingContext,
    hvl_table_ids: &[Option<u8>],
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: &mut u8,
    tone_portamento: &mut TonePortamentoState,
    report: &mut M8Report,
) -> Step {
    let mut step = empty_step();
    let tone_portamento_row = [source.fx, source.fx_b]
        .iter()
        .any(|effect| matches!(effect, 0x03 | 0x05));

    if source.note != 0 {
        let note = source.note.saturating_sub(1).min(0x7f);
        step.note = Note(note);
        if tone_portamento_row {
            tone_portamento.target_note = Some(note);
            tone_portamento.target_period = Some(s3m_period_for_m8_note(note));
        } else {
            tone_portamento.current_note = Some(note);
            tone_portamento.target_note = None;
            tone_portamento.current_period = Some(s3m_period_for_m8_note(note));
            tone_portamento.target_period = None;
            tone_portamento.speed = 0;
            if source.instrument > 0 {
                step.instrument = source.instrument.saturating_sub(1);
                step.velocity = module
                    .instruments
                    .get(step.instrument as usize)
                    .map(|instrument| volume_to_velocity(instrument.volume))
                    .unwrap_or(0xff);
                *current_velocity = step.velocity;
                if let Some(Some(table_id)) = hvl_table_ids.get(step.instrument as usize) {
                    push_fx(&mut step, FX_TBL, *table_id);
                }
            }
        }
    }

    let mut used_table_effect = false;
    let fx_mapped = map_hvl_track_effect(
        source.fx,
        source.fx_param,
        module.speed_multiplier,
        timing,
        &mut step,
        current_velocity,
        tone_portamento,
        table_allocator,
        tables,
        &mut used_table_effect,
    );
    let fx_b_mapped = map_hvl_track_effect(
        source.fx_b,
        source.fx_b_param,
        module.speed_multiplier,
        timing,
        &mut step,
        current_velocity,
        tone_portamento,
        table_allocator,
        tables,
        &mut used_table_effect,
    );
    if source.fx != 0 && !fx_mapped {
        report.unsupported_effects.push(UnsupportedEffect {
            order: position,
            pattern: track as u8,
            row,
            channel,
            effect: source.fx,
            param: source.fx_param,
        });
    }
    if source.fx_b != 0 && !fx_b_mapped {
        report.unsupported_effects.push(UnsupportedEffect {
            order: position,
            pattern: track as u8,
            row,
            channel,
            effect: source.fx_b,
            param: source.fx_b_param,
        });
    }

    step
}

pub(super) fn convert_s3m_cell(
    module: &S3mModule,
    cell: S3mCell,
    current_instrument: &mut u8,
    order: usize,
    pattern: u8,
    row: usize,
    channel: usize,
    timing: TimingContext,
    global_volume: GlobalVolumeContext,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: &mut u8,
    voice_active: &mut bool,
    current_channel_volume: &mut u8,
    channel_volume_slide_memory: &mut u8,
    arpeggio_memory: &mut u8,
    volume_slide_memory: &mut u8,
    vibrato_memory: &mut u8,
    tremor_memory: &mut u8,
    tremolo_memory: &mut u8,
    pitch_slide_memory: &mut PitchSlideMemory,
    tone_portamento: &mut TonePortamentoState,
    retrigger_memory: &mut RetriggerMemory,
    sample_offset_memory: &mut SampleOffsetMemory,
    waveform_state: &mut TrackerWaveformState,
    current_pan: &mut u8,
    pan_slide_memory: &mut u8,
    panbrello_state: &mut PanbrelloState,
    static_channel_pan: Option<u8>,
    instrument_remaps: &HashMap<(u8, u8), u8>,
    active_table_effect: &mut bool,
    active_pitch_bend: &mut bool,
    report: &mut M8Report,
) -> Step {
    if cell.instrument > 0 {
        *current_instrument = cell.instrument.saturating_sub(1);
    }
    let mut step = empty_step();
    let previous_channel_volume = *current_channel_volume;
    update_s3m_channel_volume(
        cell.command,
        cell.info,
        timing.speed,
        current_channel_volume,
        channel_volume_slide_memory,
    );
    if previous_channel_volume != *current_channel_volume && *voice_active {
        *current_velocity = rescale_global_velocity(
            *current_velocity,
            previous_channel_volume,
            *current_channel_volume,
        );
        step.velocity = *current_velocity;
    }
    if global_volume.changed() && *voice_active {
        *current_velocity = rescale_global_velocity(
            *current_velocity,
            global_volume.previous,
            global_volume.current,
        );
        step.velocity = *current_velocity;
    }
    let volume_column = s3m_volume_column_command(cell.volume);
    if let Some(note) = s3m_note_to_m8(cell.note) {
        step.note = Note(note);
        if note == NOTE_OFF {
            *voice_active = false;
            tone_portamento.current_note = None;
            tone_portamento.target_note = None;
            tone_portamento.current_period = None;
            tone_portamento.target_period = None;
            tone_portamento.speed = 0;
            pitch_slide_memory.up = 0;
            pitch_slide_memory.down = 0;
        } else if is_s3m_tone_portamento(cell) {
            tone_portamento.target_note = Some(note);
            tone_portamento.target_period = Some(s3m_period_for_m8_note(note));
        } else {
            tone_portamento.current_note = Some(note);
            tone_portamento.target_note = None;
            tone_portamento.current_period = Some(s3m_period_for_m8_note(note));
            tone_portamento.target_period = None;
            tone_portamento.speed = 0;
            pitch_slide_memory.up = 0;
            pitch_slide_memory.down = 0;
            step.instrument =
                s3m_mapped_instrument(*current_instrument, static_channel_pan, instrument_remaps);
            step.velocity = if let S3mVolumeColumnCommand::Volume(volume) = volume_column {
                volume_to_s3m_velocity_with_channel(
                    volume,
                    global_volume.current,
                    *current_channel_volume,
                )
            } else {
                module
                    .instruments
                    .get(*current_instrument as usize)
                    .map(|sample| {
                        volume_to_s3m_velocity_with_channel(
                            sample.volume,
                            global_volume.current,
                            *current_channel_volume,
                        )
                    })
                    .unwrap_or(0xff)
            };
            *current_velocity = step.velocity;
            *voice_active = true;
        }
    } else if let S3mVolumeColumnCommand::Volume(volume) = volume_column {
        step.velocity = volume_to_s3m_velocity_with_channel(
            volume,
            global_volume.current,
            *current_channel_volume,
        );
        *current_velocity = step.velocity;
    }

    let mut used_table_effect = false;
    let mut used_pitch_bend = false;
    let (volume_column_mapped, wrote_pan) = map_s3m_volume_column_effect(
        volume_column,
        timing,
        &mut step,
        table_allocator,
        tables,
        current_velocity,
        volume_slide_memory,
        vibrato_memory,
        pitch_slide_memory,
        tone_portamento,
        current_pan,
        static_channel_pan,
        &mut used_table_effect,
        &mut used_pitch_bend,
    );
    if !volume_column_mapped {
        EffectLocation {
            order,
            pattern,
            row,
            channel,
        }
        .approximation(
            report,
            "S3M volume column",
            format!("volume column byte {:02X} is not mapped", cell.volume),
        );
    }

    let uses_static_instrument_pan = static_channel_pan.is_some_and(|pan| {
        pan == 0x80 || instrument_remaps.contains_key(&(*current_instrument, pan))
    });
    if !wrote_pan
        && !step.note.is_empty()
        && step.note.0 != NOTE_OFF
        && !uses_static_instrument_pan
        && *current_pan != 0x80
    {
        push_fx(&mut step, FX_SAMPLER_PAN, *current_pan);
    }

    let mapped = map_s3m_effect(
        module,
        cell,
        timing,
        &mut step,
        table_allocator,
        tables,
        *current_instrument,
        current_velocity,
        arpeggio_memory,
        volume_slide_memory,
        vibrato_memory,
        tremor_memory,
        tremolo_memory,
        pitch_slide_memory,
        tone_portamento,
        retrigger_memory,
        sample_offset_memory,
        waveform_state,
        current_pan,
        pan_slide_memory,
        panbrello_state,
        static_channel_pan,
        &mut used_table_effect,
        &mut used_pitch_bend,
        EffectLocation {
            order,
            pattern,
            row,
            channel,
        },
        report,
    );
    update_active_pitch_bend(&mut step, used_pitch_bend, active_pitch_bend);
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

pub(super) fn velocity_for_cell(module: &Module, cell: Cell, instrument: u8) -> u8 {
    if cell.effect == 0x0c {
        return volume_to_velocity(cell.effect_param.min(64));
    }

    module
        .samples
        .get(instrument as usize)
        .map(|sample| volume_to_velocity(sample.volume))
        .unwrap_or(0xff)
}

pub(super) fn volume_to_velocity(volume: u8) -> u8 {
    ((volume.min(64) as u16 * 255) / 64) as u8
}

#[cfg(test)]
pub(super) fn volume_to_s3m_velocity(volume: u8, global_volume: u8) -> u8 {
    volume_to_s3m_velocity_with_channel(volume, global_volume, 64)
}

pub(super) fn volume_to_s3m_velocity_with_channel(
    volume: u8,
    global_volume: u8,
    channel_volume: u8,
) -> u8 {
    let numerator = u32::from(volume.min(64))
        * u32::from(global_volume.min(64))
        * u32::from(channel_volume.min(64))
        * 255;
    (numerator / (64 * 64 * 64)).min(255) as u8
}

pub(super) fn rescale_global_velocity(velocity: u8, previous: u8, current: u8) -> u8 {
    match (previous.min(64), current.min(64)) {
        (previous, current) if previous == current => velocity,
        (0, 0) => 0,
        (0, _) => velocity,
        (previous, current) => {
            ((velocity as u32 * current as u32) / previous as u32).min(255) as u8
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum S3mVolumeColumnCommand {
    None,
    Volume(u8),
    FineVolumeUp(u8),
    FineVolumeDown(u8),
    VolumeUp(u8),
    VolumeDown(u8),
    PortamentoDown(u8),
    PortamentoUp(u8),
    Pan(u8),
    TonePortamento(u8),
    VibratoDepth(u8),
    Unknown(u8),
}

pub(super) fn s3m_volume_column_command(volume: u8) -> S3mVolumeColumnCommand {
    match volume {
        0..=64 => S3mVolumeColumnCommand::Volume(volume),
        65..=74 => S3mVolumeColumnCommand::FineVolumeUp(volume - 65),
        75..=84 => S3mVolumeColumnCommand::FineVolumeDown(volume - 75),
        85..=94 => S3mVolumeColumnCommand::VolumeUp(volume - 85),
        95..=104 => S3mVolumeColumnCommand::VolumeDown(volume - 95),
        105..=114 => S3mVolumeColumnCommand::PortamentoDown(volume - 105),
        115..=124 => S3mVolumeColumnCommand::PortamentoUp(volume - 115),
        0x80..=0xc0 => {
            let pan = volume - 0x80;
            S3mVolumeColumnCommand::Pan(if pan == 32 {
                0x80
            } else {
                ((pan as u16 * 255 + 32) / 64) as u8
            })
        }
        193..=202 => S3mVolumeColumnCommand::TonePortamento(volume - 193),
        203..=212 => S3mVolumeColumnCommand::VibratoDepth(volume - 203),
        0xff => S3mVolumeColumnCommand::None,
        value => S3mVolumeColumnCommand::Unknown(value),
    }
}

pub(super) fn s3m_volume_column_pan(volume: u8) -> Option<u8> {
    match s3m_volume_column_command(volume) {
        S3mVolumeColumnCommand::Pan(pan) => Some(pan),
        _ => None,
    }
}

pub(super) fn map_s3m_volume_column_effect(
    command: S3mVolumeColumnCommand,
    timing: TimingContext,
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: &mut u8,
    volume_slide_memory: &mut u8,
    vibrato_memory: &mut u8,
    pitch_slide_memory: &mut PitchSlideMemory,
    tone_portamento: &mut TonePortamentoState,
    current_pan: &mut u8,
    static_channel_pan: Option<u8>,
    used_table_effect: &mut bool,
    used_pitch_bend: &mut bool,
) -> (bool, bool) {
    match command {
        S3mVolumeColumnCommand::None | S3mVolumeColumnCommand::Volume(_) => (true, false),
        S3mVolumeColumnCommand::Pan(pan) => (
            map_s3m_pan(step, pan, current_pan, static_channel_pan),
            true,
        ),
        S3mVolumeColumnCommand::FineVolumeUp(amount) => (
            map_s3m_volume_column_slide(
                step,
                table_allocator,
                tables,
                current_velocity,
                volume_slide_memory,
                timing,
                if amount == 0 { 0 } else { (amount << 4) | 0x0f },
                used_table_effect,
            ),
            false,
        ),
        S3mVolumeColumnCommand::FineVolumeDown(amount) => (
            map_s3m_volume_column_slide(
                step,
                table_allocator,
                tables,
                current_velocity,
                volume_slide_memory,
                timing,
                if amount == 0 { 0 } else { 0xf0 | amount },
                used_table_effect,
            ),
            false,
        ),
        S3mVolumeColumnCommand::VolumeUp(amount) => (
            map_s3m_volume_column_slide(
                step,
                table_allocator,
                tables,
                current_velocity,
                volume_slide_memory,
                timing,
                amount << 4,
                used_table_effect,
            ),
            false,
        ),
        S3mVolumeColumnCommand::VolumeDown(amount) => (
            map_s3m_volume_column_slide(
                step,
                table_allocator,
                tables,
                current_velocity,
                volume_slide_memory,
                timing,
                amount,
                used_table_effect,
            ),
            false,
        ),
        S3mVolumeColumnCommand::PortamentoDown(amount) => (
            map_s3m_pitch_slide(
                step,
                amount.saturating_mul(4),
                false,
                pitch_slide_memory,
                used_pitch_bend,
            ),
            false,
        ),
        S3mVolumeColumnCommand::PortamentoUp(amount) => (
            map_s3m_pitch_slide(
                step,
                amount.saturating_mul(4),
                true,
                pitch_slide_memory,
                used_pitch_bend,
            ),
            false,
        ),
        S3mVolumeColumnCommand::TonePortamento(amount) => (
            map_s3m_tone_portamento(
                step,
                s3m_volume_column_tone_portamento(amount),
                timing.speed,
                tone_portamento,
            ),
            false,
        ),
        S3mVolumeColumnCommand::VibratoDepth(amount) => (
            map_s3m_volume_column_vibrato(step, amount, vibrato_memory),
            false,
        ),
        S3mVolumeColumnCommand::Unknown(_) => (false, false),
    }
}

fn map_s3m_volume_column_slide(
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: &mut u8,
    volume_slide_memory: &mut u8,
    timing: TimingContext,
    param: u8,
    used_table_effect: &mut bool,
) -> bool {
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
        param,
        timing.speed,
        current_velocity,
        volume_slide_memory,
        Some(s3m_table_tick(timing.speed)),
        used_table_effect,
    )
}

pub(super) fn s3m_volume_column_tone_portamento(amount: u8) -> u8 {
    match amount.min(9) {
        0 => 0,
        1 => 0x01,
        2 => 0x04,
        3 => 0x08,
        4 => 0x10,
        5 => 0x20,
        6 => 0x40,
        7 => 0x60,
        8 => 0x80,
        _ => 0xff,
    }
}

fn map_s3m_volume_column_vibrato(step: &mut Step, amount: u8, vibrato_memory: &mut u8) -> bool {
    if amount == 0 {
        return map_vibrato(step, 0, vibrato_memory);
    }

    let speed = *vibrato_memory & 0xf0;
    map_vibrato(step, speed | amount.min(0x0f), vibrato_memory)
}

pub(super) fn s3m_cell_pan(cell: S3mCell, pan_command_uses_8bit: bool) -> Option<u8> {
    let volume_pan = s3m_volume_column_pan(cell.volume);
    match cell.command {
        19 if cell.info >> 4 == 0x08 => Some(s3m_pan_nibble_to_m8(cell.info & 0x0f)),
        24 => Some(s3m_command_pan_to_m8(cell.info, pan_command_uses_8bit)),
        _ => volume_pan,
    }
}

pub(super) fn s3m_cell_triggers_note(cell: S3mCell) -> bool {
    s3m_note_to_m8(cell.note).is_some_and(|note| note != NOTE_OFF) && !is_s3m_tone_portamento(cell)
}

pub(super) fn s3m_mapped_instrument(
    source_instrument: u8,
    static_channel_pan: Option<u8>,
    instrument_remaps: &HashMap<(u8, u8), u8>,
) -> u8 {
    let Some(pan) = static_channel_pan else {
        return source_instrument;
    };
    instrument_remaps
        .get(&(source_instrument, pan))
        .copied()
        .unwrap_or(source_instrument)
}

pub(super) fn is_mod_tone_portamento(cell: Cell) -> bool {
    matches!(cell.effect, 0x03 | 0x05) && cell.period != 0
}

pub(super) fn is_s3m_tone_portamento(cell: S3mCell) -> bool {
    let has_tone_portamento = matches!(cell.command, 7 | 12)
        || matches!(
            s3m_volume_column_command(cell.volume),
            S3mVolumeColumnCommand::TonePortamento(_)
        );
    has_tone_portamento && s3m_note_to_m8(cell.note).is_some_and(|note| note != 0x80)
}

pub(super) fn s3m_period_for_m8_note(note: u8) -> u16 {
    const PERIODS: [u16; 12] = [
        1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 907,
    ];
    let semitone = (note % 12) as usize;
    let octave = (note / 12).min(10) as u32;
    PERIODS[semitone].checked_shr(octave).unwrap_or(0).max(1)
}

pub(super) fn empty_step() -> Step {
    let mut step = Step::default();
    step.clear();
    step
}

pub(super) fn empty_packed_step() -> PackedStep {
    pack_step(&empty_step())
}

pub(super) fn silent_mod_cell() -> Cell {
    Cell {
        period: 0,
        sample_number: 0,
        effect: 0,
        effect_param: 0,
    }
}

pub(super) fn pack_step(step: &Step) -> PackedStep {
    PackedStep {
        note: step.note.0,
        velocity: step.velocity,
        instrument: step.instrument,
        fx: [pack_fx(step.fx1), pack_fx(step.fx2), pack_fx(step.fx3)],
    }
}

pub(super) fn pack_fx(fx: FX) -> (u8, u8) {
    (fx.command, fx.value)
}

pub(super) fn empty_hvl_step() -> HvlStep {
    HvlStep {
        note: 0,
        instrument: 0,
        fx: 0,
        fx_param: 0,
        fx_b: 0,
        fx_b_param: 0,
    }
}
