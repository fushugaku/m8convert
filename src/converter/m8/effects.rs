use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum TableSpec {
    Empty,
    VolumeSlide {
        start: u8,
        delta: i8,
        rows: u8,
    },
    Tremor {
        velocity: u8,
        on: u8,
        off: u8,
        rows: u8,
    },
    Tremolo {
        center: u8,
        depth: u8,
        speed: u8,
        waveform: u8,
        rows: u8,
    },
    Panbrello {
        center: u8,
        depth: u8,
        speed: u8,
        waveform: u8,
        phase: u8,
        rows: u8,
    },
}

pub(super) struct TableAllocator {
    pub(super) next_table: usize,
    cache: HashMap<TableSpec, u8>,
    pub(super) dropped: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PitchSlideMemory {
    pub(super) up: u8,
    pub(super) down: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TonePortamentoState {
    pub(super) current_note: Option<u8>,
    pub(super) target_note: Option<u8>,
    pub(super) current_period: Option<u16>,
    pub(super) target_period: Option<u16>,
    pub(super) speed: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct RetriggerMemory {
    pub(super) mod_interval: u8,
    pub(super) s3m_param: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct SampleOffsetMemory {
    pub(super) mod_high_byte: u8,
    pub(super) s3m_high_byte: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TrackerWaveformState {
    pub(super) glissando: u8,
    pub(super) vibrato: u8,
    pub(super) tremolo: u8,
    pub(super) panbrello: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PanbrelloState {
    pub(super) param: u8,
    pub(super) phase: u8,
}

impl TableAllocator {
    pub(super) fn new() -> Self {
        Self {
            next_table: 0,
            cache: HashMap::new(),
            dropped: 0,
        }
    }

    pub(super) fn starting_at(next_table: usize) -> Self {
        Self {
            next_table,
            cache: HashMap::new(),
            dropped: 0,
        }
    }

    pub(super) fn allocate(&mut self, tables: &mut [Table], spec: TableSpec) -> Option<u8> {
        if let Some(table_id) = self.cache.get(&spec) {
            return Some(*table_id);
        }
        if self.next_table >= tables.len() || self.next_table > u8::MAX as usize {
            self.dropped += 1;
            return None;
        }

        let table_id = self.next_table as u8;
        self.next_table += 1;

        let mut table = tables[table_id as usize].clone();
        table.clear();
        build_table(&mut table, spec);
        tables[table_id as usize] = table;
        self.cache.insert(spec, table_id);
        Some(table_id)
    }

    pub(super) fn empty_table(&mut self, tables: &mut [Table]) -> Option<u8> {
        self.allocate(tables, TableSpec::Empty)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn map_hvl_track_effect(
    command: u8,
    param: u8,
    speed_multiplier: u8,
    timing: TimingContext,
    step: &mut Step,
    current_velocity: &mut u8,
    tone_portamento: &mut TonePortamentoState,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    used_table_effect: &mut bool,
) -> bool {
    match command {
        0 => true,
        0x01 => push_fx(step, FX_PBN, pitch_bend_value(param, true)),
        0x02 => push_fx(step, FX_PBN, pitch_bend_value(param, false)),
        0x03 => map_tone_portamento(step, param, timing.speed, tone_portamento),
        0x04 => map_hvl_filter_override(step, param),
        0x05 => {
            let tone = map_tone_portamento(step, 0, timing.speed, tone_portamento);
            let volume = map_hvl_volume_slide(
                step,
                param,
                timing.speed,
                current_velocity,
                table_allocator,
                tables,
                used_table_effect,
            );
            tone && volume
        }
        0x07 => push_fx(step, FX_SAMPLER_PAN, param.wrapping_add(128)),
        0x09 => push_fx(step, FX_WAVSYNTH_SIZ, param.max(1)),
        0x0a => map_hvl_volume_slide(
            step,
            param,
            timing.speed,
            current_velocity,
            table_allocator,
            tables,
            used_table_effect,
        ),
        0x0b | 0x0d => true,
        0x0c => {
            let volume = match param {
                0x00..=0x40 => Some(param),
                0x50..=0x90 => Some(param - 0x50),
                0xa0..=0xe0 => Some(param - 0xa0),
                _ => None,
            };
            if let Some(volume) = volume {
                step.velocity = volume_to_velocity(volume);
                *current_velocity = step.velocity;
            }
            true
        }
        0x0e => match param >> 4 {
            0x01 => push_fx(step, FX_PBN, pitch_bend_value(param & 0x0f, true)),
            0x02 => push_fx(step, FX_PBN, pitch_bend_value(param & 0x0f, false)),
            0x04 => push_fx(step, FX_PVB, param & 0x0f),
            0x0a => map_fine_volume_slide(step, param & 0x0f, current_velocity, true),
            0x0b => map_fine_volume_slide(step, param & 0x0f, current_velocity, false),
            0x0c => push_fx(step, FX_KIL, param & 0x0f),
            0x0f => true,
            _ => false,
        },
        0x0f if param != 0 => push_fx(
            step,
            FX_TPO,
            timing
                .tpo
                .unwrap_or_else(|| hvl_rows_to_m8_tpo(speed_multiplier, param)),
        ),
        0x0f => true,
        _ => false,
    }
}

fn map_hvl_filter_override(step: &mut Step, param: u8) -> bool {
    let position = match param {
        0x01..=0x3f => param,
        0x41..=0x7f => param - 0x40,
        _ => return true,
    };
    let cutoff = ((u16::from(position - 1) * 255 + 31) / 62) as u8;
    push_fx(step, FX_WAVSYNTH_CUT, cutoff)
}

#[allow(clippy::too_many_arguments)]
fn map_hvl_volume_slide(
    step: &mut Step,
    param: u8,
    tempo: u8,
    current_velocity: &mut u8,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    used_table_effect: &mut bool,
) -> bool {
    if param == 0 || *current_velocity == EMPTY {
        return true;
    }
    let up = i16::from(param >> 4);
    let down = i16::from(param & 0x0f);
    let delta = ((up - down) * 4).clamp(i16::from(i8::MIN), i16::from(i8::MAX)) as i8;
    if delta == 0 {
        return true;
    }
    let rows = tempo.clamp(1, 16);
    let start = *current_velocity;
    *current_velocity = stepped_value(start, delta, rows as usize - 1);
    map_table(
        step,
        table_allocator,
        tables,
        TableSpec::VolumeSlide { start, delta, rows },
        Some(TABLE_TICK_PER_TRACKER_TICK),
        used_table_effect,
    )
}

pub(super) fn should_report_effect(cell: Cell) -> bool {
    if cell.effect == 0 && cell.effect_param == 0 {
        return false;
    }

    !matches!(cell.effect, 0x0c)
}

pub(super) fn update_active_table_effect(
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

    if let Some(table) = table_allocator.empty_table(tables)
        && push_fx(step, FX_TBL, table)
    {
        *active_table_effect = false;
    }
}

pub(super) fn update_active_pitch_bend(
    step: &mut Step,
    used_pitch_bend: bool,
    active_pitch_bend: &mut bool,
) {
    if used_pitch_bend {
        *active_pitch_bend = true;
        return;
    }

    if !*active_pitch_bend {
        return;
    }

    if push_fx(step, FX_PBN, 0) {
        *active_pitch_bend = false;
    }
}

pub(super) fn map_mod_effect(
    module: &Module,
    cell: Cell,
    timing: TimingContext,
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_instrument: u8,
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
    used_table_effect: &mut bool,
    used_pitch_bend: &mut bool,
    location: EffectLocation,
    report: &mut M8Report,
) -> bool {
    match cell.effect {
        0x00 if cell.effect_param != 0 => push_fx(step, FX_ARP, cell.effect_param),
        0x01 => map_pitch_slide(
            step,
            cell.effect_param,
            true,
            pitch_slide_memory,
            used_pitch_bend,
        ),
        0x02 => map_pitch_slide(
            step,
            cell.effect_param,
            false,
            pitch_slide_memory,
            used_pitch_bend,
        ),
        0x03 => map_tone_portamento(step, cell.effect_param, timing.speed, tone_portamento),
        0x04 => map_vibrato(step, cell.effect_param, vibrato_memory),
        0x05 => {
            let tone_mapped = map_tone_portamento(step, 0, timing.speed, tone_portamento);
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
                None,
                used_table_effect,
            );
            tone_mapped && volume_mapped
        }
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
                None,
                used_table_effect,
            );
            vibrato_mapped && volume_mapped
        }
        0x07 => map_tremolo(
            step,
            table_allocator,
            tables,
            *current_velocity,
            remembered_effect_param(cell.effect_param, tremolo_memory),
            timing.speed,
            waveform_state.tremolo,
            None,
            used_table_effect,
        ),
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
                None,
                used_table_effect,
            )
        }
        0x08 => {
            *current_pan = cell.effect_param;
            push_fx(step, FX_SAMPLER_PAN, *current_pan)
        }
        0x09 => map_mod_sample_offset(
            step,
            module,
            current_instrument,
            cell.effect_param,
            sample_offset_memory,
            location,
            report,
        ),
        0x0b | 0x0d => true,
        0x0c => true,
        0x0f => timing
            .tpo
            .map(|tempo| push_fx(step, FX_TPO, tempo))
            .unwrap_or(false),
        0x0e => match cell.effect_param >> 4 {
            0x01 => push_pitch_bend(step, cell.effect_param & 0x0f, true, used_pitch_bend),
            0x02 => push_pitch_bend(step, cell.effect_param & 0x0f, false, used_pitch_bend),
            0x03 => map_glissando_control(
                waveform_state,
                cell.effect_param & 0x0f,
                location,
                report,
                "MOD",
            ),
            0x04 => map_waveform_control(
                &mut waveform_state.vibrato,
                cell.effect_param & 0x0f,
                location,
                report,
                "MOD vibrato waveform",
            ),
            0x06 | 0x0e => true,
            0x07 => map_waveform_control(
                &mut waveform_state.tremolo,
                cell.effect_param & 0x0f,
                location,
                report,
                "MOD tremolo waveform",
            ),
            0x05 => push_fx(
                step,
                FX_SAMPLER_FIN,
                mod_finetune_to_m8(mod_finetune_nibble_to_signed(cell.effect_param & 0x0f)),
            ),
            0x08 => {
                *current_pan = s3m_pan_nibble_to_m8(cell.effect_param & 0x0f);
                push_fx(step, FX_SAMPLER_PAN, *current_pan)
            }
            0x09 => map_mod_retrigger(step, cell.effect_param & 0x0f, retrigger_memory),
            0x0a => map_fine_volume_slide(step, cell.effect_param & 0x0f, current_velocity, true),
            0x0b => map_fine_volume_slide(step, cell.effect_param & 0x0f, current_velocity, false),
            0x0c => push_fx(step, FX_KIL, cell.effect_param & 0x0f),
            0x0d => push_fx(step, FX_DEL, cell.effect_param & 0x0f),
            _ => false,
        },
        _ => false,
    }
}

pub(super) fn map_s3m_effect(
    module: &S3mModule,
    cell: S3mCell,
    timing: TimingContext,
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_instrument: u8,
    current_velocity: &mut u8,
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
    used_table_effect: &mut bool,
    used_pitch_bend: &mut bool,
    location: EffectLocation,
    report: &mut M8Report,
) -> bool {
    match cell.command {
        0 => true,
        2 | 3 => true,
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
                Some(s3m_table_tick(timing.speed)),
                used_table_effect,
            )
        }
        5 => map_s3m_pitch_slide(step, cell.info, false, pitch_slide_memory, used_pitch_bend),
        6 => map_s3m_pitch_slide(step, cell.info, true, pitch_slide_memory, used_pitch_bend),
        7 => map_s3m_tone_portamento(step, cell.info, timing.speed, tone_portamento),
        8 | 21 => map_vibrato(step, cell.info, vibrato_memory),
        9 => map_tremor(
            step,
            table_allocator,
            tables,
            *current_velocity,
            remembered_effect_param(cell.info, tremor_memory),
            timing.speed,
            Some(s3m_table_tick(timing.speed)),
            used_table_effect,
        ),
        10 => {
            let param = remembered_effect_param(cell.info, arpeggio_memory);
            if param == 0 {
                true
            } else {
                push_fx(step, FX_ARP, param)
            }
        }
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
                Some(s3m_table_tick(timing.speed)),
                used_table_effect,
            );
            vibrato_mapped && volume_mapped
        }
        12 => {
            let tone_mapped = map_s3m_tone_portamento(step, 0, timing.speed, tone_portamento);
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
                Some(s3m_table_tick(timing.speed)),
                used_table_effect,
            );
            tone_mapped && volume_mapped
        }
        13 | 14 => true,
        15 => map_s3m_sample_offset(
            step,
            module,
            current_instrument,
            cell.info,
            sample_offset_memory,
            location,
            report,
        ),
        16 => map_s3m_pan_slide(
            step,
            cell.info,
            timing.speed,
            pan_slide_memory,
            current_pan,
            static_channel_pan,
        ),
        17 => map_s3m_retrigger(step, cell.info, retrigger_memory, location, report),
        18 => map_tremolo(
            step,
            table_allocator,
            tables,
            *current_velocity,
            remembered_effect_param(cell.info, tremolo_memory),
            timing.speed,
            waveform_state.tremolo,
            Some(s3m_table_tick(timing.speed)),
            used_table_effect,
        ),
        19 => match cell.info >> 4 {
            0x01 => {
                map_glissando_control(waveform_state, cell.info & 0x0f, location, report, "S3M")
            }
            0x02 => {
                location.approximation(
                    report,
                    "sample tuning",
                    format!(
                        "S3M S2{:X} dynamic finetune was mapped to M8 sampler FIN; per-instrument C5 speed is still preserved in the WAV sample rate",
                        cell.info & 0x0f
                    ),
                );
                push_fx(
                    step,
                    FX_SAMPLER_FIN,
                    mod_finetune_to_m8(mod_finetune_nibble_to_signed(cell.info & 0x0f)),
                )
            }
            0x03 => map_waveform_control(
                &mut waveform_state.vibrato,
                cell.info & 0x0f,
                location,
                report,
                "S3M vibrato waveform",
            ),
            0x04 => map_waveform_control(
                &mut waveform_state.tremolo,
                cell.info & 0x0f,
                location,
                report,
                "S3M tremolo waveform",
            ),
            0x05 => map_waveform_control(
                &mut waveform_state.panbrello,
                cell.info & 0x0f,
                location,
                report,
                "S3M panbrello waveform",
            ),
            0x08 => map_s3m_pan(
                step,
                s3m_pan_nibble_to_m8(cell.info & 0x0f),
                current_pan,
                static_channel_pan,
            ),
            0x0b | 0x0e => true,
            0x0c => push_fx(step, FX_KIL, cell.info & 0x0f),
            0x0d => push_fx(step, FX_DEL, cell.info & 0x0f),
            _ => false,
        },
        22 | 23 => true,
        24 => map_s3m_pan(
            step,
            s3m_command_pan_to_m8(cell.info, module.pan_command_uses_8bit),
            current_pan,
            static_channel_pan,
        ),
        25 => map_s3m_panbrello(
            step,
            cell.info,
            timing.speed,
            panbrello_state,
            waveform_state.panbrello,
            current_pan,
            table_allocator,
            tables,
            used_table_effect,
        ),
        _ => false,
    }
}

pub(super) fn remembered_effect_param(param: u8, memory: &mut u8) -> u8 {
    if param == 0 {
        *memory
    } else {
        *memory = param;
        param
    }
}

pub(super) fn map_mod_retrigger(
    step: &mut Step,
    interval: u8,
    memory: &mut RetriggerMemory,
) -> bool {
    let interval = if interval == 0 {
        memory.mod_interval
    } else {
        memory.mod_interval = interval;
        interval
    };
    if interval == 0 {
        return true;
    }

    push_fx(step, FX_RET, 0x80 | interval.min(0x0f))
}

pub(super) fn map_s3m_retrigger(
    step: &mut Step,
    param: u8,
    memory: &mut RetriggerMemory,
    location: EffectLocation,
    report: &mut M8Report,
) -> bool {
    let param = if param == 0 {
        memory.s3m_param
    } else {
        memory.s3m_param = param;
        param
    };
    if param == 0 {
        return true;
    }

    let interval = param & 0x0f;
    if interval == 0 {
        return true;
    }

    let change = param >> 4;
    let m8_change = s3m_retrigger_volume_to_m8(change);
    if matches!(change, 0x05 | 0x06 | 0x07 | 0x0d | 0x0e | 0x0f) {
        location.approximation(
            report,
            "retrigger",
            format!(
                "S3M Q{:X}{:X} retrigger volume curve was mapped to nearest M8 RET volume step",
                change, interval
            ),
        );
    }

    push_fx(step, FX_RET, (m8_change << 4) | interval)
}

pub(super) fn s3m_retrigger_volume_to_m8(change: u8) -> u8 {
    match change & 0x0f {
        0x00 | 0x08 => 0x08,
        0x01 => 0x07,
        0x02 => 0x06,
        0x03 => 0x04,
        0x04 | 0x05 => 0x00,
        0x06 => 0x05,
        0x07 => 0x04,
        0x09 => 0x09,
        0x0a => 0x0a,
        0x0b => 0x0c,
        0x0c | 0x0d => 0x0f,
        0x0e => 0x0c,
        _ => 0x0f,
    }
}

pub(super) fn map_mod_sample_offset(
    step: &mut Step,
    module: &Module,
    current_instrument: u8,
    high_byte: u8,
    memory: &mut SampleOffsetMemory,
    location: EffectLocation,
    report: &mut M8Report,
) -> bool {
    let high_byte = if high_byte == 0 {
        memory.mod_high_byte
    } else {
        memory.mod_high_byte = high_byte;
        high_byte
    };
    if high_byte == 0 {
        return true;
    }

    let sample_len = module
        .samples
        .get(current_instrument as usize)
        .map(|sample| sample.length_bytes)
        .unwrap_or(0);
    map_sample_offset(step, "MOD", high_byte, sample_len, location, report)
}

pub(super) fn map_s3m_sample_offset(
    step: &mut Step,
    module: &S3mModule,
    current_instrument: u8,
    high_byte: u8,
    memory: &mut SampleOffsetMemory,
    location: EffectLocation,
    report: &mut M8Report,
) -> bool {
    let high_byte = if high_byte == 0 {
        memory.s3m_high_byte
    } else {
        memory.s3m_high_byte = high_byte;
        high_byte
    };
    if high_byte == 0 {
        return true;
    }

    let sample_len = module
        .instruments
        .get(current_instrument as usize)
        .map(|sample| sample.data.len())
        .unwrap_or(0);
    map_sample_offset(step, "S3M", high_byte, sample_len, location, report)
}

fn map_sample_offset(
    step: &mut Step,
    format: &str,
    high_byte: u8,
    sample_len: usize,
    location: EffectLocation,
    report: &mut M8Report,
) -> bool {
    if step.note.is_empty() || step.note.0 == NOTE_OFF {
        return true;
    }

    if sample_len == 0 {
        location.approximation(
            report,
            "sample offset",
            format!(
                "{format} sample offset {:02X} had no active sample length to scale against",
                high_byte
            ),
        );
        return push_fx(step, FX_SAMPLER_STA, high_byte);
    }

    let byte_offset = high_byte as usize * 256;
    if byte_offset >= sample_len {
        location.approximation(
            report,
            "sample offset",
            format!(
                "{format} sample offset {:02X} points past sample length {}; clamped to M8 STA FF",
                high_byte, sample_len
            ),
        );
    }
    push_fx(
        step,
        FX_SAMPLER_STA,
        scaled_sample_position(byte_offset, sample_len),
    )
}

pub(super) fn map_glissando_control(
    state: &mut TrackerWaveformState,
    value: u8,
    location: EffectLocation,
    report: &mut M8Report,
    format: &str,
) -> bool {
    state.glissando = value & 0x0f;
    if state.glissando != 0 {
        location.approximation(
            report,
            "glissando",
            format!("{format} glissando control is tracked but M8 PSL is continuous"),
        );
    }
    true
}

pub(super) fn map_waveform_control(
    state: &mut u8,
    value: u8,
    location: EffectLocation,
    report: &mut M8Report,
    category: &str,
) -> bool {
    *state = value & 0x0f;
    if *state != 0 {
        let mapping_note = if category.contains("tremolo") {
            "M8 table tremolo approximates that shape"
        } else {
            "M8 PVB/table modulation uses its own shape"
        };
        location.approximation(
            report,
            "waveform",
            format!("{category} {value:X} is tracked, but {mapping_note}"),
        );
    }
    true
}

pub(super) fn map_volume_slide(
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    start_velocity: u8,
    param: u8,
    ticks: u8,
    current_velocity: &mut u8,
    volume_slide_memory: &mut u8,
    table_tick: Option<u8>,
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
        table_tick,
        used_table_effect,
    )
}

pub(super) fn map_tremor(
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: u8,
    param: u8,
    speed: u8,
    table_tick: Option<u8>,
    used_table_effect: &mut bool,
) -> bool {
    let on = param >> 4;
    let off = param & 0x0f;
    if current_velocity == EMPTY || (on == 0 && off == 0) {
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
        TableSpec::Tremor {
            velocity: current_velocity,
            on,
            off,
            rows,
        },
        table_tick,
        used_table_effect,
    )
}

pub(super) fn map_tremolo(
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    current_velocity: u8,
    param: u8,
    speed: u8,
    waveform: u8,
    table_tick: Option<u8>,
    used_table_effect: &mut bool,
) -> bool {
    let tremolo_speed = param >> 4;
    let depth = (param & 0x0f).saturating_mul(4);
    if current_velocity == EMPTY || tremolo_speed == 0 || depth == 0 {
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
        TableSpec::Tremolo {
            center: current_velocity,
            depth,
            speed: tremolo_speed,
            waveform,
            rows,
        },
        table_tick,
        used_table_effect,
    )
}

pub(super) fn map_pitch_slide(
    step: &mut Step,
    amount: u8,
    upward: bool,
    memory: &mut PitchSlideMemory,
    used_pitch_bend: &mut bool,
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

    push_pitch_bend(step, amount, upward, used_pitch_bend)
}

pub(super) fn map_s3m_pitch_slide(
    step: &mut Step,
    amount: u8,
    upward: bool,
    memory: &mut PitchSlideMemory,
    used_pitch_bend: &mut bool,
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

    let Some(scaled_amount) = s3m_pitch_bend_amount(amount) else {
        return true;
    };
    if scaled_amount == 0 {
        return true;
    }

    push_pitch_bend(step, scaled_amount, upward, used_pitch_bend)
}

pub(super) fn s3m_pitch_bend_amount(amount: u8) -> Option<u8> {
    let high = amount >> 4;
    let low = amount & 0x0f;
    match high {
        0x0e if low != 0 => Some(low),
        0x0f if low != 0 => Some(low.saturating_mul(4)),
        _ => Some(amount.saturating_mul(4)),
    }
}

pub(super) fn map_tone_portamento(
    step: &mut Step,
    amount: u8,
    speed: u8,
    state: &mut TonePortamentoState,
) -> bool {
    if amount != 0 {
        state.speed = amount;
    }
    let Some(target_note) = state.target_note else {
        return true;
    };
    if state.speed == 0 {
        return true;
    }

    let ticks = if let (Some(current_period), Some(target_period)) =
        (state.current_period, state.target_period)
    {
        if current_period == target_period {
            state.current_note = Some(target_note);
            state.current_period = Some(target_period);
            return true;
        }
        period_portamento_ticks(current_period, target_period, state.speed)
    } else {
        let Some(current_note) = state.current_note else {
            return true;
        };
        if current_note == target_note {
            return true;
        }
        note_portamento_ticks(current_note, target_note, state.speed, speed)
    };

    if ticks == 0 {
        return true;
    }

    let mapped = push_fx(step, FX_PSL, ticks);
    if mapped {
        state.current_note = Some(target_note);
        state.current_period = state.target_period;
    }
    mapped
}

pub(super) fn map_s3m_tone_portamento(
    step: &mut Step,
    amount: u8,
    speed: u8,
    state: &mut TonePortamentoState,
) -> bool {
    let scaled_amount = amount.saturating_mul(4);
    map_tone_portamento(step, scaled_amount, speed, state)
}

pub(super) fn map_s3m_pan(
    step: &mut Step,
    pan: u8,
    current_pan: &mut u8,
    static_channel_pan: Option<u8>,
) -> bool {
    *current_pan = pan;
    if static_channel_pan == Some(pan) {
        true
    } else {
        push_fx(step, FX_SAMPLER_PAN, pan)
    }
}

pub(super) fn update_s3m_channel_volume(
    command: u8,
    param: u8,
    speed: u8,
    current: &mut u8,
    slide_memory: &mut u8,
) {
    if command == 13 {
        *current = param.min(64);
        return;
    }
    if command != 14 {
        return;
    }

    let param = remembered_effect_param(param, slide_memory);
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

pub(super) fn map_s3m_pan_slide(
    step: &mut Step,
    param: u8,
    speed: u8,
    memory: &mut u8,
    current_pan: &mut u8,
    static_channel_pan: Option<u8>,
) -> bool {
    let param = remembered_effect_param(param, memory);
    if param == 0 {
        return true;
    }
    let left = param >> 4;
    let right = param & 0x0f;
    let delta = if right == 0x0f && left != 0 {
        -i16::from(left) * 4
    } else if left == 0x0f && right != 0 {
        i16::from(right) * 4
    } else if right != 0 {
        i16::from(right) * 4 * i16::from(speed.saturating_sub(1))
    } else {
        -i16::from(left) * 4 * i16::from(speed.saturating_sub(1))
    };
    let pan = (i16::from(*current_pan) + delta).clamp(0, 255) as u8;
    map_s3m_pan(step, pan, current_pan, static_channel_pan)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn map_s3m_panbrello(
    step: &mut Step,
    param: u8,
    row_speed: u8,
    state: &mut PanbrelloState,
    waveform: u8,
    current_pan: &mut u8,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    used_table_effect: &mut bool,
) -> bool {
    if param & 0xf0 != 0 {
        state.param = (state.param & 0x0f) | (param & 0xf0);
    }
    if param & 0x0f != 0 {
        state.param = (state.param & 0xf0) | (param & 0x0f);
    }
    let speed = state.param >> 4;
    let depth = state.param & 0x0f;
    let rows = tracker_effect_rows(row_speed);
    if speed == 0 || depth == 0 || rows == 0 {
        return true;
    }

    let spec = TableSpec::Panbrello {
        center: *current_pan,
        depth,
        speed,
        waveform,
        phase: state.phase,
        rows,
    };
    let final_phase = state
        .phase
        .wrapping_add(speed.wrapping_mul(rows.saturating_sub(1)))
        & 0x0f;
    let final_delta = tremolo_waveform_value(waveform, final_phase) * i16::from(depth);
    let final_pan = (i16::from(*current_pan) + final_delta).clamp(0, 255) as u8;
    let mapped = map_table(
        step,
        table_allocator,
        tables,
        spec,
        Some(s3m_table_tick(row_speed)),
        used_table_effect,
    );
    if mapped {
        *current_pan = final_pan;
        state.phase = state.phase.wrapping_add(speed.wrapping_mul(rows)) & 0x0f;
    }
    mapped
}

pub(super) fn map_table(
    step: &mut Step,
    table_allocator: &mut TableAllocator,
    tables: &mut [Table],
    spec: TableSpec,
    table_tick: Option<u8>,
    used_table_effect: &mut bool,
) -> bool {
    let needed_slots = 1 + usize::from(table_tick.is_some());
    if available_fx_slots(step) < needed_slots {
        return false;
    }

    let Some(table) = table_allocator.allocate(tables, spec) else {
        return false;
    };
    if let Some(tick) = table_tick {
        push_fx(step, FX_TIC, tick);
    }
    let mapped = push_fx(step, FX_TBL, table);
    if mapped {
        *used_table_effect = true;
    }
    mapped
}

pub(super) fn push_pitch_bend(
    step: &mut Step,
    amount: u8,
    upward: bool,
    used_pitch_bend: &mut bool,
) -> bool {
    let mapped = push_fx(step, FX_PBN, pitch_bend_value(amount, upward));
    if mapped {
        *used_pitch_bend = true;
    }
    mapped
}

pub(super) fn available_fx_slots(step: &Step) -> usize {
    [step.fx1, step.fx2, step.fx3]
        .iter()
        .filter(|fx| fx.is_empty())
        .count()
}

pub(super) fn pitch_bend_value(amount: u8, upward: bool) -> u8 {
    let amount = amount.min(0x7f);
    if upward {
        amount
    } else {
        0u8.wrapping_sub(amount)
    }
}

pub(super) fn period_portamento_ticks(current_period: u16, target_period: u16, speed: u8) -> u8 {
    let distance = current_period.abs_diff(target_period).max(1);
    let speed = speed.max(1) as u16;
    distance.div_ceil(speed).clamp(1, 0xff) as u8
}

pub(super) fn note_portamento_ticks(
    current_note: u8,
    target_note: u8,
    speed: u8,
    _row_speed: u8,
) -> u8 {
    let distance = current_note.abs_diff(target_note).max(1) as u16;
    let speed = speed.max(1) as u16;
    let ticks = (distance * 16).div_ceil(speed);
    ticks.clamp(1, 0xff) as u8
}

pub(super) fn positive_delta(value: u8) -> i8 {
    value.min(63) as i8
}

pub(super) fn negative_delta(value: u8) -> i8 {
    -(value.min(63) as i8)
}

pub(super) fn map_vibrato(step: &mut Step, param: u8, vibrato_memory: &mut u8) -> bool {
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

pub(super) fn map_fine_volume_slide(
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

pub(super) fn is_s3m_fine_volume_slide(param: u8) -> bool {
    let up = param >> 4;
    let down = param & 0x0f;
    (down == 0x0f && up != 0) || (up == 0x0f && down != 0)
}

pub(super) fn is_s3m_fine_pan_slide(param: u8) -> bool {
    is_s3m_fine_volume_slide(param)
}

pub(super) fn build_table(table: &mut Table, spec: TableSpec) {
    match spec {
        TableSpec::Empty => {}
        TableSpec::VolumeSlide { start, delta, rows } => {
            for (index, step) in table.steps.iter_mut().take(rows as usize).enumerate() {
                step.velocity = stepped_value(start, delta, index);
            }
        }
        TableSpec::Tremor {
            velocity,
            on,
            off,
            rows,
        } => {
            let cycle = on.saturating_add(off).max(1);
            for (index, step) in table.steps.iter_mut().take(rows as usize).enumerate() {
                let phase = index as u8 % cycle;
                step.velocity = if phase < on { velocity } else { 0 };
            }
        }
        TableSpec::Tremolo {
            center,
            depth,
            speed,
            waveform,
            rows,
        } => {
            for (index, step) in table.steps.iter_mut().take(rows as usize).enumerate() {
                let phase = ((index as u8).wrapping_mul(speed.max(1))) & 0x0f;
                let delta = tremolo_waveform_value(waveform, phase) * depth as i16 / 8;
                step.velocity = (center as i16 + delta).clamp(0, 255) as u8;
            }
        }
        TableSpec::Panbrello {
            center,
            depth,
            speed,
            waveform,
            phase,
            rows,
        } => {
            for (index, step) in table.steps.iter_mut().take(rows as usize).enumerate() {
                let phase = phase.wrapping_add((index as u8).wrapping_mul(speed)) & 0x0f;
                let delta = tremolo_waveform_value(waveform, phase) * i16::from(depth);
                step.fx1 = FX {
                    command: FX_SAMPLER_PAN,
                    value: (i16::from(center) + delta).clamp(0, 255) as u8,
                };
            }
        }
    }
}

pub(super) fn tremolo_waveform_value(waveform: u8, phase: u8) -> i16 {
    let phase = (phase & 0x0f) as i16;
    match waveform & 0x03 {
        0 => {
            let triangle = if phase < 8 { phase } else { 16 - phase };
            triangle * 2 - 8
        }
        1 => 8 - phase,
        2 => {
            if phase < 8 {
                8
            } else {
                -8
            }
        }
        _ => {
            let value = ((phase as u8).wrapping_mul(73).wrapping_add(41)) & 0x0f;
            value as i16 - 8
        }
    }
}

pub(super) fn tracker_effect_rows(speed: u8) -> u8 {
    speed.saturating_sub(1).min(16)
}

pub(super) fn stepped_value(start: u8, delta: i8, index: usize) -> u8 {
    let next = start as i16 + delta as i16 * (index as i16 + 1);
    next.clamp(0, 255) as u8
}

pub(super) fn push_fx(step: &mut Step, command: u8, value: u8) -> bool {
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

pub(super) fn push_song_hop(
    step: &mut Step,
    target_song_row: u8,
    report: &mut M8Report,
    format: &str,
) {
    if !push_fx(step, FX_SNG, target_song_row) {
        report.warnings.push(format!(
            "{format} playback loop could not be written because the final M8 step has no free FX slot"
        ));
    }
}
