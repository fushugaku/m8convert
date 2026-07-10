use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TimingContext {
    pub(super) speed: u8,
    pub(super) tpo: Option<u8>,
}

impl Default for TimingContext {
    fn default() -> Self {
        Self {
            speed: 6,
            tpo: None,
        }
    }
}

pub(super) fn mod_m8_tempo(module: &Module) -> f32 {
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

    tracker_rows_to_m8_bpm(speed, tempo)
}

pub(super) fn mod_timing_contexts(
    module: &Module,
    playback_blocks: &[PlaybackBlock],
) -> Vec<Vec<Vec<TimingContext>>> {
    let mut out = Vec::with_capacity(playback_blocks.len());
    let mut speed = 6;
    let mut tempo = 125;

    for block in playback_blocks {
        let mut block_contexts = Vec::with_capacity(block.rows.len());
        for playback_row in &block.rows {
            let source_row = module
                .patterns
                .get(playback_row.pattern_id as usize)
                .and_then(|pattern| pattern.rows.get(playback_row.source_row));
            let mut row_contexts = Vec::with_capacity(module.channel_count);
            for channel in 0..module.channel_count {
                let cell = source_row
                    .and_then(|row| row.get(channel))
                    .copied()
                    .filter(|_| playback_row.emit_cells)
                    .unwrap_or_else(silent_mod_cell);
                let mut tpo = None;
                if cell.effect == 0x0f && cell.effect_param != 0 {
                    if cell.effect_param < 0x20 {
                        speed = cell.effect_param;
                    } else {
                        tempo = cell.effect_param;
                    }
                    tpo = Some(tracker_rows_to_m8_tpo(speed, tempo));
                }
                row_contexts.push(TimingContext { speed, tpo });
            }
            block_contexts.push(row_contexts);
        }
        out.push(block_contexts);
    }

    out
}

pub(super) fn s3m_timing_contexts(
    module: &S3mModule,
    playback_blocks: &[PlaybackBlock],
) -> Vec<Vec<Vec<TimingContext>>> {
    let mut out = Vec::with_capacity(playback_blocks.len());
    let mut speed = module.initial_speed;
    let mut tempo = module.initial_tempo;

    for block in playback_blocks {
        let mut block_contexts = Vec::with_capacity(block.rows.len());
        for playback_row in &block.rows {
            let source_row = module
                .patterns
                .get(playback_row.pattern_id as usize)
                .and_then(|pattern| pattern.rows.get(playback_row.source_row));
            let mut row_contexts = Vec::with_capacity(module.active_channels.len());
            for source_channel in &module.active_channels {
                let cell = source_row
                    .and_then(|row| row.get(*source_channel))
                    .copied()
                    .filter(|_| playback_row.emit_cells)
                    .unwrap_or(S3mCell::EMPTY);
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
                row_contexts.push(TimingContext { speed, tpo });
            }
            block_contexts.push(row_contexts);
        }
        out.push(block_contexts);
    }

    out
}

pub(super) fn hvl_timing_contexts(
    module: &HvlModule,
    playback_blocks: &[PlaybackBlock],
) -> Vec<Vec<Vec<TimingContext>>> {
    let mut out = Vec::with_capacity(playback_blocks.len());
    let mut tempo = 6u8;

    for block in playback_blocks {
        let mut block_contexts = Vec::with_capacity(block.rows.len());
        for playback_row in &block.rows {
            let position = module.positions.get(playback_row.order_index);
            let mut row_contexts = Vec::with_capacity(module.channel_count);
            for channel in 0..module.channel_count {
                let step = position
                    .and_then(|position| position.tracks.get(channel))
                    .and_then(|track| module.tracks.get(*track as usize))
                    .and_then(|track| track.rows.get(playback_row.source_row))
                    .copied()
                    .unwrap_or_else(empty_hvl_step);
                let mut tpo = None;
                for (command, param) in [(step.fx, step.fx_param), (step.fx_b, step.fx_b_param)] {
                    if command == 0x0f && param != 0 {
                        tempo = param;
                        tpo = Some(hvl_rows_to_m8_tpo(module.speed_multiplier, tempo));
                    }
                }
                row_contexts.push(TimingContext { speed: tempo, tpo });
            }
            block_contexts.push(row_contexts);
        }
        out.push(block_contexts);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GlobalVolumeContext {
    pub(super) previous: u8,
    pub(super) current: u8,
}

impl GlobalVolumeContext {
    pub(super) fn changed(self) -> bool {
        self.previous != self.current
    }
}

pub(super) fn s3m_global_volume_contexts(
    module: &S3mModule,
    playback_blocks: &[PlaybackBlock],
) -> Vec<Vec<GlobalVolumeContext>> {
    let mut out = Vec::with_capacity(playback_blocks.len());
    let mut global_volume = module.global_volume.min(64);
    let mut slide_memory = 0u8;
    let mut speed = module.initial_speed.max(1);

    for block in playback_blocks {
        let mut block_contexts = Vec::with_capacity(block.rows.len());
        for playback_row in &block.rows {
            let previous = global_volume;
            let source_row = module
                .patterns
                .get(playback_row.pattern_id as usize)
                .and_then(|pattern| pattern.rows.get(playback_row.source_row));
            for cell in source_row
                .into_iter()
                .flat_map(|row| row.iter())
                .filter(|cell| {
                    playback_row.emit_cells
                        || (cell.command == 23 && !is_s3m_fine_volume_slide(cell.info))
                })
            {
                match cell.command {
                    1 if cell.info != 0 => speed = cell.info,
                    22 => global_volume = cell.info.min(64),
                    23 => {
                        let param = if cell.info == 0 {
                            slide_memory
                        } else {
                            slide_memory = cell.info;
                            cell.info
                        };
                        let up = param >> 4;
                        let down = param & 0x0f;
                        if down == 0x0f && up != 0 {
                            global_volume = global_volume.saturating_add(up).min(64);
                        } else if up == 0x0f && down != 0 {
                            global_volume = global_volume.saturating_sub(down);
                        } else if up != 0 && down == 0 {
                            global_volume = global_volume
                                .saturating_add(up.saturating_mul(speed.saturating_sub(1)))
                                .min(64);
                        } else if down != 0 && up == 0 {
                            global_volume = global_volume
                                .saturating_sub(down.saturating_mul(speed.saturating_sub(1)));
                        }
                    }
                    _ => {}
                }
            }
            block_contexts.push(GlobalVolumeContext {
                previous,
                current: global_volume,
            });
        }
        out.push(block_contexts);
    }

    out
}

pub(super) fn tracker_rows_to_m8_bpm(speed: u8, tempo: u8) -> f32 {
    let speed = speed.max(1) as f32;
    let tempo = tempo.max(32) as f32;
    (tempo * 6.0 / speed).clamp(20.0, 999.0)
}

pub(super) fn tracker_rows_to_m8_tpo(speed: u8, tempo: u8) -> u8 {
    tracker_rows_to_m8_bpm(speed, tempo)
        .round()
        .clamp(20.0, 255.0) as u8
}

pub(super) fn hvl_rows_to_m8_bpm(speed_multiplier: u8, tempo: u8) -> f32 {
    let speed_multiplier = speed_multiplier.max(1) as f32;
    let tempo = tempo.max(1) as f32;
    (750.0 * speed_multiplier / tempo).clamp(20.0, 999.0)
}

pub(super) fn hvl_rows_to_m8_tpo(speed_multiplier: u8, tempo: u8) -> u8 {
    hvl_rows_to_m8_bpm(speed_multiplier, tempo)
        .round()
        .clamp(20.0, 255.0) as u8
}
