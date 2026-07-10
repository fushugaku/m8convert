use super::*;

#[derive(Debug, Clone)]
pub(super) struct PlaybackBlock {
    pub(super) rows: Vec<PlaybackRow>,
    pub(super) song_hop: Option<u8>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PlaybackRow {
    pub(super) order_index: usize,
    pub(super) pattern_id: u8,
    pub(super) source_row: usize,
    pub(super) emit_cells: bool,
}

pub(super) fn build_mod_playback_blocks(
    module: &Module,
    report: &mut M8Report,
) -> Vec<PlaybackBlock> {
    let mut blocks: Vec<PlaybackBlock> = Vec::new();
    let mut order_index = 0usize;
    let mut start_row = 0usize;
    let mut visits = 0usize;
    let mut seen_entries: HashMap<(usize, usize), usize> = HashMap::new();
    let mut entry_song_rows: HashMap<(usize, usize), u8> = HashMap::new();
    let mut loop_start = vec![0usize; module.channel_count];
    let mut loop_counts: HashMap<(usize, usize, usize), u8> = HashMap::new();

    while order_index < module.orders.len()
        && can_append_playback_row(&blocks)
        && visits < MAX_FLOW_VISITS
    {
        let entry = (order_index, start_row);
        if let Some(target_song_row) = entry_song_rows.get(&entry).copied()
            && let Some(block) = blocks.last_mut()
        {
            block.song_hop = Some(target_song_row);
        }
        let seen = seen_entries.entry(entry).or_default();
        if *seen > 0 {
            report.warnings.push(format!(
                "MOD playback flow loops back to order {order_index}, row {start_row}; inserted M8 SNG hop"
            ));
            break;
        }
        *seen += 1;
        entry_song_rows
            .entry(entry)
            .or_insert_with(|| next_playback_song_row(&blocks));

        let pattern_id = module.orders[order_index];
        let Some(pattern) = module.patterns.get(pattern_id as usize) else {
            report.warnings.push(format!(
                "order {order_index}: pattern {pattern_id} is outside parsed pattern data"
            ));
            break;
        };

        let mut rows = Vec::new();
        let mut row = start_row.min(pattern.rows.len().saturating_sub(1));
        let mut next_order = order_index + 1;
        let mut next_start_row = 0usize;

        while row < pattern.rows.len() && visits < MAX_FLOW_VISITS {
            visits += 1;
            rows.push(PlaybackRow {
                order_index,
                pattern_id,
                source_row: row,
                emit_cells: true,
            });
            let control = mod_row_flow_control(pattern, row, module.channel_count);
            if control.delay > 0 {
                for _ in 0..control.delay {
                    rows.push(PlaybackRow {
                        order_index,
                        pattern_id,
                        source_row: row,
                        emit_cells: false,
                    });
                }
            }
            if control.stop {
                next_order = module.orders.len();
                break;
            }

            for channel in &control.loop_starts {
                loop_start[*channel] = row;
            }
            if let Some((channel, count)) = control.loop_repeat {
                let key = (order_index, channel, row);
                let remaining = loop_counts.entry(key).or_insert(count);
                if *remaining > 0 {
                    *remaining = remaining.saturating_sub(1);
                    row = loop_start[channel].min(pattern.rows.len().saturating_sub(1));
                    continue;
                }
            }

            if let Some(target_order) = control.position_jump {
                next_order = target_order as usize;
                next_start_row = control.pattern_break.unwrap_or(0).min(63);
                break;
            }
            if let Some(target_row) = control.pattern_break {
                next_order = order_index + 1;
                next_start_row = target_row.min(63);
                break;
            }

            row += 1;
        }

        push_playback_rows(&mut blocks, rows, report, "MOD");
        order_index = next_order;
        start_row = next_start_row;
    }

    if let Some(restart_song_row) = mod_restart_song_row(module, &entry_song_rows)
        && let Some(block) = blocks.last_mut()
        && block.song_hop.is_none()
    {
        block.song_hop = Some(restart_song_row);
        report.warnings.push(format!(
            "MOD restart position {} was mapped to M8 SNG hop row {}",
            module.restart_position, restart_song_row
        ));
    }

    if blocks.len() >= MAX_PLAYBACK_BLOCKS {
        report
            .warnings
            .push("MOD playback flow exceeded M8's 256 song rows and was truncated".to_string());
    }
    blocks
}

pub(super) fn build_s3m_playback_blocks(
    module: &S3mModule,
    report: &mut M8Report,
) -> Vec<PlaybackBlock> {
    let mut blocks: Vec<PlaybackBlock> = Vec::new();
    let mut order_index = 0usize;
    let mut start_row = 0usize;
    let mut visits = 0usize;
    let mut seen_entries: HashMap<(usize, usize), usize> = HashMap::new();
    let mut entry_song_rows: HashMap<(usize, usize), u8> = HashMap::new();
    let mut loop_start = vec![0usize; 32];
    let mut loop_counts: HashMap<(usize, usize, usize), u8> = HashMap::new();

    while order_index < module.orders.len()
        && can_append_playback_row(&blocks)
        && visits < MAX_FLOW_VISITS
    {
        let entry = (order_index, start_row);
        if let Some(target_song_row) = entry_song_rows.get(&entry).copied()
            && let Some(block) = blocks.last_mut()
        {
            block.song_hop = Some(target_song_row);
        }
        let seen = seen_entries.entry(entry).or_default();
        if *seen > 0 {
            report.warnings.push(format!(
                "S3M playback flow loops back to order {order_index}, row {start_row}; inserted M8 SNG hop"
            ));
            break;
        }
        *seen += 1;
        entry_song_rows
            .entry(entry)
            .or_insert_with(|| next_playback_song_row(&blocks));

        let pattern_id = module.orders[order_index];
        let Some(pattern) = module.patterns.get(pattern_id as usize) else {
            report.warnings.push(format!(
                "order {order_index}: S3M pattern {pattern_id} is outside parsed pattern data"
            ));
            break;
        };

        let mut rows = Vec::new();
        let mut row = start_row.min(pattern.rows.len().saturating_sub(1));
        let mut next_order = order_index + 1;
        let mut next_start_row = 0usize;

        while row < pattern.rows.len() && visits < MAX_FLOW_VISITS {
            visits += 1;
            rows.push(PlaybackRow {
                order_index,
                pattern_id,
                source_row: row,
                emit_cells: true,
            });
            let control = s3m_row_flow_control(pattern, row);
            if control.delay > 0 {
                for _ in 0..control.delay {
                    rows.push(PlaybackRow {
                        order_index,
                        pattern_id,
                        source_row: row,
                        emit_cells: false,
                    });
                }
            }

            for channel in &control.loop_starts {
                loop_start[*channel] = row;
            }
            if let Some((channel, count)) = control.loop_repeat {
                let key = (order_index, channel, row);
                let remaining = loop_counts.entry(key).or_insert(count);
                if *remaining > 0 {
                    *remaining = remaining.saturating_sub(1);
                    row = loop_start[channel].min(pattern.rows.len().saturating_sub(1));
                    continue;
                }
            }

            if let Some(target_order) = control.position_jump {
                next_order = target_order as usize;
                next_start_row = control.pattern_break.unwrap_or(0).min(63);
                break;
            }
            if let Some(target_row) = control.pattern_break {
                next_order = order_index + 1;
                next_start_row = target_row.min(63);
                break;
            }

            row += 1;
        }

        push_playback_rows(&mut blocks, rows, report, "S3M");
        order_index = next_order;
        start_row = next_start_row;
    }

    if let Some(restart) = module.restart_position.map(usize::from)
        && restart < module.orders.len()
        && let Some(restart_song_row) = entry_song_rows.get(&(restart, 0)).copied()
        && let Some(block) = blocks.last_mut()
        && block.song_hop.is_none()
    {
        block.song_hop = Some(restart_song_row);
        report.warnings.push(format!(
            "S3M-compatible restart position {restart} was mapped to M8 SNG hop row {restart_song_row}"
        ));
    }

    if blocks.len() >= MAX_PLAYBACK_BLOCKS {
        report
            .warnings
            .push("S3M playback flow exceeded M8's 256 song rows and was truncated".to_string());
    }
    blocks
}

pub(super) fn build_hvl_playback_blocks(
    module: &HvlModule,
    report: &mut M8Report,
) -> Vec<PlaybackBlock> {
    let mut blocks: Vec<PlaybackBlock> = Vec::new();
    let mut position_index = 0usize;
    let mut start_row = 0usize;
    let mut visits = 0usize;
    let mut jump_high = 0usize;
    let mut entry_song_rows = HashMap::<(usize, usize), u8>::new();

    while position_index < module.positions.len()
        && blocks.len() < MAX_PLAYBACK_BLOCKS
        && visits < MAX_FLOW_VISITS
    {
        let entry = (position_index, start_row);
        if let Some(song_row) = entry_song_rows.get(&entry).copied() {
            if let Some(block) = blocks.last_mut() {
                block.song_hop = Some(song_row);
            }
            report.warnings.push(format!(
                "HVL playback flow loops back to position {position_index}, row {start_row}; inserted M8 SNG hop"
            ));
            break;
        }
        entry_song_rows.insert(entry, blocks.len().min(u8::MAX as usize) as u8);

        let mut rows = Vec::new();
        let mut row = start_row.min(module.track_length.saturating_sub(1));
        let mut next_position = position_index + 1;
        let mut next_start_row = 0usize;
        let mut stopped = false;

        while row < module.track_length && visits < MAX_FLOW_VISITS {
            visits += 1;
            rows.push(PlaybackRow {
                order_index: position_index,
                pattern_id: 0,
                source_row: row,
                emit_cells: true,
            });
            let control = hvl_row_flow_control(module, position_index, row, &mut jump_high);
            if control.stop {
                stopped = true;
                break;
            }
            if let Some(target) = control.position_jump {
                next_position = target;
                next_start_row = control.pattern_break.unwrap_or(0);
                break;
            }
            if let Some(target_row) = control.pattern_break {
                next_position = position_index + 1;
                next_start_row = target_row;
                break;
            }
            row += 1;
        }

        blocks.push(PlaybackBlock {
            rows,
            song_hop: None,
        });
        if stopped {
            break;
        }
        if next_position >= module.positions.len() {
            let restart = module.restart_position as usize;
            if restart < module.positions.len()
                && let Some(song_row) = entry_song_rows.get(&(restart, 0)).copied()
                && let Some(block) = blocks.last_mut()
            {
                block.song_hop = Some(song_row);
            }
            break;
        }
        position_index = next_position;
        start_row = next_start_row.min(module.track_length.saturating_sub(1));
    }

    if blocks.len() >= MAX_PLAYBACK_BLOCKS {
        report
            .warnings
            .push("HVL playback flow exceeded M8's 256 song rows and was truncated".to_string());
    }
    blocks
}

#[derive(Default)]
struct HvlFlowControl {
    position_jump: Option<usize>,
    pattern_break: Option<usize>,
    stop: bool,
}

fn hvl_row_flow_control(
    module: &HvlModule,
    position_index: usize,
    row: usize,
    jump_high: &mut usize,
) -> HvlFlowControl {
    let mut control = HvlFlowControl::default();
    let Some(position) = module.positions.get(position_index) else {
        return control;
    };
    for channel in 0..module.channel_count {
        let Some(step) = position
            .tracks
            .get(channel)
            .and_then(|track| module.tracks.get(*track as usize))
            .and_then(|track| track.rows.get(row))
            .copied()
        else {
            continue;
        };
        for (command, param) in [(step.fx, step.fx_param), (step.fx_b, step.fx_b_param)] {
            match command {
                0 if (1..=9).contains(&(param & 0x0f)) => {
                    *jump_high = (param & 0x0f) as usize;
                }
                0x0b => {
                    control.position_jump = Some(*jump_high * 100 + bcd_row(param));
                    control.pattern_break = Some(0);
                }
                0x0d => {
                    control.pattern_break = Some(bcd_row(param).min(module.track_length - 1));
                }
                0x0f if param == 0 => control.stop = true,
                _ => {}
            }
        }
    }
    control
}

#[derive(Default)]
pub(super) struct FlowControl {
    position_jump: Option<u8>,
    pattern_break: Option<usize>,
    delay: u8,
    loop_starts: Vec<usize>,
    loop_repeat: Option<(usize, u8)>,
    stop: bool,
}

pub(super) fn mod_row_flow_control(pattern: &Pattern, row: usize, channels: usize) -> FlowControl {
    let mut control = FlowControl::default();
    for (channel, cell) in pattern.rows[row].iter().take(channels).copied().enumerate() {
        match cell.effect {
            0x0b => control.position_jump = Some(cell.effect_param),
            0x0d => control.pattern_break = Some(bcd_row(cell.effect_param)),
            0x0f if cell.effect_param == 0 => control.stop = true,
            0x0e => match cell.effect_param >> 4 {
                0x06 if cell.effect_param & 0x0f == 0 => control.loop_starts.push(channel),
                0x06 => control.loop_repeat = Some((channel, cell.effect_param & 0x0f)),
                0x0e => control.delay = control.delay.max(cell.effect_param & 0x0f),
                _ => {}
            },
            _ => {}
        }
    }
    control
}

pub(super) fn s3m_row_flow_control(pattern: &S3mPattern, row: usize) -> FlowControl {
    let mut control = FlowControl::default();
    for (channel, cell) in pattern.rows[row].iter().copied().enumerate() {
        match cell.command {
            2 => control.position_jump = Some(cell.info),
            3 => control.pattern_break = Some(cell.info.min(63) as usize),
            19 => match cell.info >> 4 {
                0x0b if cell.info & 0x0f == 0 => control.loop_starts.push(channel),
                0x0b => control.loop_repeat = Some((channel, cell.info & 0x0f)),
                0x0e => control.delay = control.delay.max(cell.info & 0x0f),
                _ => {}
            },
            _ => {}
        }
    }
    control
}

pub(super) fn push_playback_rows(
    blocks: &mut Vec<PlaybackBlock>,
    rows: Vec<PlaybackRow>,
    report: &mut M8Report,
    format: &str,
) {
    for row in rows {
        if blocks.last().is_none_or(|block| block.rows.len() >= 64) {
            if blocks.len() >= MAX_PLAYBACK_BLOCKS {
                report.warnings.push(format!(
                    "{format} playback flow produced more than 256 M8 song rows; remaining rows were dropped"
                ));
                break;
            }
            blocks.push(PlaybackBlock {
                rows: Vec::new(),
                song_hop: None,
            });
        }
        if let Some(block) = blocks.last_mut() {
            block.rows.push(row);
        }
    }
}

pub(super) fn can_append_playback_row(blocks: &[PlaybackBlock]) -> bool {
    blocks.len() < MAX_PLAYBACK_BLOCKS
        || blocks
            .last()
            .is_some_and(|block| block.rows.len() < M8_PHRASE_ROWS * 4)
}

pub(super) fn next_playback_song_row(blocks: &[PlaybackBlock]) -> u8 {
    if blocks.is_empty() {
        return 0;
    }
    if blocks
        .last()
        .is_some_and(|block| block.rows.len() < M8_PHRASE_ROWS * 4)
    {
        (blocks.len() - 1).min(u8::MAX as usize) as u8
    } else {
        blocks.len().min(u8::MAX as usize) as u8
    }
}

pub(super) fn mod_restart_song_row(
    module: &Module,
    entry_song_rows: &HashMap<(usize, usize), u8>,
) -> Option<u8> {
    let restart = module.restart_position as usize;
    if restart == 0 || restart >= module.orders.len() {
        return None;
    }
    entry_song_rows.get(&(restart, 0)).copied()
}

pub(super) fn bcd_row(value: u8) -> usize {
    (((value >> 4) as usize) * 10 + (value & 0x0f) as usize).min(63)
}
