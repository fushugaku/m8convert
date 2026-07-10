use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::converter::s3mfile::{
    S3mAmpEnvelope, S3mAutoVibrato, S3mCell, S3mInstrument, S3mModule, S3mPattern,
};

const XM_MAGIC: &[u8; 17] = b"Extended Module: ";
const XM_FIXED_HEADER_LEN: usize = 60;
const XM_ORDER_TABLE_LEN: usize = 256;
const XM_MAX_CHANNELS: usize = 32;
const XM_MAX_M8_INSTRUMENTS: usize = 128;
const XM_ROWS_FALLBACK: usize = 64;
const XM_MAX_TIMELINE_ROWS: usize = 16_384;

#[derive(Debug, Error)]
pub enum XmError {
    #[error("file is too short to be an XM module")]
    TooShort,
    #[error("missing Extended Module XM signature")]
    MissingSignature,
    #[error("unsupported XM version {0:#06x}")]
    UnsupportedVersion(u16),
    #[error("unsupported XM channel count {0}")]
    UnsupportedChannelCount(u16),
    #[error("XM data is truncated while reading {0}")]
    Truncated(&'static str),
    #[error("invalid XM header size {0}")]
    InvalidHeaderSize(u32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmModule {
    pub title: String,
    pub tracker_name: String,
    pub version: u16,
    pub restart_position: u16,
    pub channel_count: usize,
    pub orders: Vec<u8>,
    pub patterns: Vec<XmPattern>,
    pub instruments: Vec<XmInstrument>,
    pub initial_speed: u8,
    pub initial_tempo: u8,
    pub frequency_table_flags: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmPattern {
    pub rows: Vec<Vec<XmCell>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct XmCell {
    pub note: u8,
    pub instrument: u8,
    pub volume: u8,
    pub effect: u8,
    pub effect_param: u8,
}

impl XmCell {
    pub const EMPTY: Self = Self {
        note: 0,
        instrument: 0,
        volume: 0,
        effect: 0,
        effect_param: 0,
    };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmInstrument {
    pub name: String,
    pub sample_map: Vec<u8>,
    pub volume_envelope: XmEnvelope,
    pub panning_envelope: XmEnvelope,
    pub volume_envelope_type: u8,
    pub panning_envelope_type: u8,
    pub volume_fadeout: u16,
    pub vibrato_type: u8,
    pub vibrato_sweep: u8,
    pub vibrato_depth: u8,
    pub vibrato_rate: u8,
    pub samples: Vec<XmSample>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct XmEnvelope {
    pub points: Vec<XmEnvelopePoint>,
    pub sustain_point: u8,
    pub loop_start_point: u8,
    pub loop_end_point: u8,
    pub flags: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct XmEnvelopePoint {
    pub frame: u16,
    pub value: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmSample {
    pub name: String,
    pub length: u32,
    pub loop_start: u32,
    pub loop_length: u32,
    pub volume: u8,
    pub finetune: i8,
    pub sample_type: u8,
    pub panning: u8,
    pub relative_note: i8,
    pub data: Vec<i16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmS3mAdapter {
    pub module: S3mModule,
    pub warnings: Vec<String>,
    pub unsupported_effects: Vec<XmUnsupportedEffect>,
    pub approximations: Vec<XmApproximation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmUnsupportedEffect {
    pub order: Option<usize>,
    pub pattern: u8,
    pub row: usize,
    pub channel: usize,
    pub effect: String,
    pub param: u8,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmApproximation {
    pub order: usize,
    pub pattern: u8,
    pub row: usize,
    pub channel: usize,
    pub category: String,
    pub detail: String,
}

impl XmModule {
    pub fn to_s3m_module(&self) -> S3mModule {
        self.to_s3m_adapter().module
    }

    pub fn to_s3m_adapter(&self) -> XmS3mAdapter {
        let adapter = XmAdapter::new(self);
        adapter.build()
    }

    pub(crate) fn playback_rows_for_events(&self) -> Vec<(usize, u8, usize, bool, Vec<XmCell>)> {
        let mut out = Vec::new();
        for row in XmAdapter::new(self).build_timeline().rows {
            let delay = row
                .cells
                .iter()
                .filter(|cell| cell.effect == 0x0e && cell.effect_param >> 4 == 0x0e)
                .map(|cell| cell.effect_param & 0x0f)
                .max()
                .unwrap_or(0);
            out.push((row.order, row.pattern, row.row, true, row.cells.clone()));
            for _ in 0..delay {
                out.push((row.order, row.pattern, row.row, false, Vec::new()));
            }
        }
        out
    }
}

struct XmAdapter<'a> {
    module: &'a XmModule,
    instrument_sample_slots: Vec<Vec<Option<u8>>>,
    first_sample_slots: Vec<Option<u8>>,
    current_xm_instruments: Vec<Option<usize>>,
    current_sample_slots: Vec<Option<u8>>,
    current_pans: Vec<u8>,
    pan_slide_memory: Vec<u8>,
    vibrato_speed_memory: Vec<u8>,
    vibrato_depth_memory: Vec<u8>,
    instruments: Vec<S3mInstrument>,
    warnings: Vec<String>,
    unsupported_effects: Vec<XmUnsupportedEffect>,
    approximations: Vec<XmApproximation>,
}

#[derive(Debug, Clone)]
struct XmTimeline {
    rows: Vec<XmTimelineRow>,
    restart_row: Option<usize>,
}

#[derive(Debug, Clone)]
struct XmTimelineRow {
    order: usize,
    pattern: u8,
    row: usize,
    cells: Vec<XmCell>,
}

#[derive(Debug, Default)]
struct XmFlowControl {
    position_jump: Option<u8>,
    pattern_break: Option<usize>,
    loop_starts: Vec<usize>,
    loop_repeat: Option<(usize, u8)>,
    stop: bool,
}

impl<'a> XmAdapter<'a> {
    fn new(module: &'a XmModule) -> Self {
        Self {
            module,
            instrument_sample_slots: vec![Vec::new(); module.instruments.len()],
            first_sample_slots: vec![None; module.instruments.len()],
            current_xm_instruments: vec![None; XM_MAX_CHANNELS],
            current_sample_slots: vec![None; XM_MAX_CHANNELS],
            current_pans: vec![0x80; XM_MAX_CHANNELS],
            pan_slide_memory: vec![0; XM_MAX_CHANNELS],
            vibrato_speed_memory: vec![0; XM_MAX_CHANNELS],
            vibrato_depth_memory: vec![0; XM_MAX_CHANNELS],
            instruments: Vec::new(),
            warnings: Vec::new(),
            unsupported_effects: Vec::new(),
            approximations: Vec::new(),
        }
    }

    fn build(mut self) -> XmS3mAdapter {
        self.build_instruments();
        let timeline = self.build_timeline();
        let (orders, patterns, restart_position) = self.timeline_to_s3m(timeline);

        let module = S3mModule {
            title: self.module.title.clone(),
            orders,
            restart_position,
            active_channels: (0..self.module.channel_count.min(XM_MAX_CHANNELS)).collect(),
            channel_pans: (0..XM_MAX_CHANNELS).map(|_| 0x80).collect(),
            instruments: self.instruments,
            patterns,
            initial_speed: self.module.initial_speed,
            initial_tempo: self.module.initial_tempo,
            global_volume: 64,
            tracker_version: self.module.version,
            ffi: self.module.frequency_table_flags,
            pan_command_uses_8bit: true,
        };

        XmS3mAdapter {
            module,
            warnings: self.warnings,
            unsupported_effects: self.unsupported_effects,
            approximations: self.approximations,
        }
    }

    fn build_timeline(&mut self) -> XmTimeline {
        let mut rows = Vec::new();
        let mut order_index = 0usize;
        let mut start_row = 0usize;
        let mut visits = 0usize;
        let mut seen_entries = std::collections::HashSet::<(usize, usize)>::new();
        let mut entry_rows = std::collections::HashMap::<(usize, usize), usize>::new();
        let mut loop_start = vec![0usize; self.module.channel_count.min(XM_MAX_CHANNELS)];
        let mut loop_counts = std::collections::HashMap::<(usize, usize, usize), u8>::new();
        let mut restart_row = None;

        while order_index < self.module.orders.len()
            && visits < XM_MAX_TIMELINE_ROWS
            && rows.len() < XM_MAX_TIMELINE_ROWS
        {
            let entry = (order_index, start_row);
            if !seen_entries.insert(entry) {
                restart_row = entry_rows.get(&entry).copied();
                self.warnings.push(format!(
                    "XM playback flow loops back to order {order_index}, row {start_row}; mapped to an M8 SNG hop where possible"
                ));
                break;
            }
            entry_rows.entry(entry).or_insert(rows.len());

            let source_pattern_id = self.module.orders[order_index];
            let Some(pattern) = self.module.patterns.get(source_pattern_id as usize) else {
                self.warnings.push(format!(
                    "XM order {order_index}: pattern {source_pattern_id} is outside parsed pattern data"
                ));
                break;
            };
            let mut row = start_row.min(pattern.rows.len().saturating_sub(1));
            let mut next_order = order_index + 1;
            let mut next_start_row = 0usize;

            while row < pattern.rows.len()
                && visits < XM_MAX_TIMELINE_ROWS
                && rows.len() < XM_MAX_TIMELINE_ROWS
            {
                visits += 1;
                rows.push(XmTimelineRow {
                    order: order_index,
                    pattern: source_pattern_id,
                    row,
                    cells: pattern.rows[row].clone(),
                });

                let control = xm_row_flow_control(pattern, row, self.module.channel_count);
                if control.stop {
                    next_order = self.module.orders.len();
                    break;
                }
                for channel in &control.loop_starts {
                    if let Some(start) = loop_start.get_mut(*channel) {
                        *start = row;
                    }
                }
                if let Some((channel, count)) = control.loop_repeat {
                    let key = (order_index, channel, row);
                    let remaining = loop_counts.entry(key).or_insert(count);
                    if *remaining > 0 {
                        *remaining = remaining.saturating_sub(1);
                        row = loop_start
                            .get(channel)
                            .copied()
                            .unwrap_or(0)
                            .min(pattern.rows.len().saturating_sub(1));
                        continue;
                    }
                }

                if let Some(target_order) = control.position_jump {
                    next_order = target_order as usize;
                    next_start_row = control
                        .pattern_break
                        .unwrap_or(0)
                        .min(pattern.rows.len().saturating_sub(1));
                    break;
                }
                if let Some(target_row) = control.pattern_break {
                    next_order = order_index + 1;
                    next_start_row = target_row.min(pattern.rows.len().saturating_sub(1));
                    break;
                }

                row += 1;
            }

            order_index = next_order;
            start_row = next_start_row;
        }

        if rows.len() >= XM_MAX_TIMELINE_ROWS || visits >= XM_MAX_TIMELINE_ROWS {
            self.warnings.push(
                "XM native timeline exceeded the editable conversion visit limit and was truncated"
                    .to_string(),
            );
        }
        if restart_row.is_none() {
            let restart = self.module.restart_position as usize;
            if restart < self.module.orders.len() {
                restart_row = entry_rows.get(&(restart, 0)).copied();
            }
        }

        XmTimeline { rows, restart_row }
    }

    fn timeline_to_s3m(&mut self, timeline: XmTimeline) -> (Vec<u8>, Vec<S3mPattern>, Option<u8>) {
        let mut split_points = std::collections::HashSet::new();
        if let Some(restart_row) = timeline.restart_row {
            split_points.insert(restart_row);
        }

        let mut orders = Vec::new();
        let mut patterns = Vec::new();
        let mut current_rows = Vec::new();
        let mut current_start = 0usize;
        let mut restart_position = None;

        for (timeline_index, timeline_row) in timeline.rows.iter().enumerate() {
            if (!current_rows.is_empty() && split_points.contains(&timeline_index))
                || current_rows.len() >= XM_ROWS_FALLBACK
            {
                self.push_timeline_pattern(
                    &mut patterns,
                    &mut orders,
                    &mut current_rows,
                    current_start,
                    timeline.restart_row,
                    &mut restart_position,
                );
                current_start = timeline_index;
            }
            current_rows.push(timeline_row.clone());
        }
        self.push_timeline_pattern(
            &mut patterns,
            &mut orders,
            &mut current_rows,
            current_start,
            timeline.restart_row,
            &mut restart_position,
        );

        if patterns.is_empty() {
            patterns.push(S3mPattern {
                rows: vec![vec![S3mCell::EMPTY; XM_MAX_CHANNELS]],
            });
            orders.push(0);
        }

        (orders, patterns, restart_position)
    }

    fn push_timeline_pattern(
        &mut self,
        patterns: &mut Vec<S3mPattern>,
        orders: &mut Vec<u8>,
        current_rows: &mut Vec<XmTimelineRow>,
        current_start: usize,
        restart_row: Option<usize>,
        restart_position: &mut Option<u8>,
    ) {
        if current_rows.is_empty() {
            return;
        }
        if patterns.len() > u8::MAX as usize {
            self.warnings.push(
                "XM native timeline exceeded 256 adapted patterns; remaining rows were dropped"
                    .to_string(),
            );
            current_rows.clear();
            return;
        }

        let adapted_pattern_id = patterns.len() as u8;
        if restart_row == Some(current_start) {
            *restart_position = Some(adapted_pattern_id);
        }

        let rows = current_rows
            .iter()
            .map(|timeline_row| self.xm_timeline_row_to_s3m(timeline_row))
            .collect::<Vec<_>>();
        patterns.push(S3mPattern { rows });
        orders.push(adapted_pattern_id);
        current_rows.clear();
    }

    fn build_instruments(&mut self) {
        for (instrument_index, instrument) in self.module.instruments.iter().enumerate() {
            self.warn_instrument_limitations(instrument_index, instrument);
            let mut sample_slots = vec![None; instrument.samples.len()];
            for (sample_index, sample) in instrument.samples.iter().enumerate() {
                if sample.data.is_empty() {
                    continue;
                }
                if self.instruments.len() >= XM_MAX_M8_INSTRUMENTS {
                    self.warnings.push(format!(
                        "XM instrument {}, sample {} was not exported because the adapter reached the 128 M8 instrument slots",
                        instrument_index + 1,
                        sample_index + 1
                    ));
                    continue;
                }
                let slot = self.instruments.len() as u8 + 1;
                self.instruments.push(xm_sample_to_s3m(
                    instrument_index,
                    sample_index,
                    instrument,
                    sample,
                    self.module.frequency_table_flags,
                ));
                sample_slots[sample_index] = Some(slot);
                if self.first_sample_slots[instrument_index].is_none() {
                    self.first_sample_slots[instrument_index] = Some(slot);
                }
            }
            self.instrument_sample_slots[instrument_index] = sample_slots;
        }
    }

    fn warn_instrument_limitations(&mut self, index: usize, instrument: &XmInstrument) {
        if instrument.samples.len() > 1 {
            self.warnings.push(format!(
                "XM instrument {} uses a sample map with {} samples; notes are remapped to separate M8 sampler instruments where possible",
                index + 1,
                instrument.samples.len()
            ));
        }
        if xm_amp_envelope_approx(instrument).is_some() {
            self.warnings.push(format!(
                "XM instrument {} simple volume envelope was approximated with M8 sampler amp modulation",
                index + 1
            ));
        } else if instrument.volume_envelope_type != 0 {
            self.warnings.push(format!(
                "XM instrument {} volume envelope is too complex for editable M8 approximation and was reported only",
                index + 1
            ));
        }
        if instrument.panning_envelope_type != 0 {
            self.warnings.push(format!(
                "XM instrument {} panning envelope is not rendered in editable conversion",
                index + 1
            ));
        }
        if instrument.volume_fadeout != 0 {
            self.warnings.push(format!(
                "XM instrument {} volume fadeout {} was approximated with sampler amp decay where possible",
                index + 1, instrument.volume_fadeout
            ));
        }
        if instrument.vibrato_depth != 0 || instrument.vibrato_rate != 0 {
            self.warnings.push(format!(
                "XM instrument {} auto-vibrato was approximated with sampler LFO metadata where possible",
                index + 1
            ));
        }
        for (sample_index, sample) in instrument.samples.iter().enumerate() {
            match sample.sample_type & 0x03 {
                0x02 => self.warnings.push(format!(
                    "XM instrument {}, sample {} ping-pong loop is preserved with M8 FWD_PP playback",
                    index + 1,
                    sample_index + 1
                )),
                0x03 => self.warnings.push(format!(
                    "XM instrument {}, sample {} has reserved loop type 3; exported as forward loop when loop points are valid",
                    index + 1,
                    sample_index + 1
                )),
                _ => {}
            }
        }
    }

    fn xm_timeline_row_to_s3m(&mut self, timeline_row: &XmTimelineRow) -> Vec<S3mCell> {
        let mut row = vec![S3mCell::EMPTY; XM_MAX_CHANNELS];
        for (channel, cell) in timeline_row
            .cells
            .iter()
            .take(XM_MAX_CHANNELS)
            .copied()
            .enumerate()
        {
            row[channel] = self.xm_cell_to_s3m(
                timeline_row.order,
                timeline_row.pattern,
                timeline_row.row,
                channel,
                strip_consumed_xm_flow(cell),
            );
        }
        row
    }

    fn xm_cell_to_s3m(
        &mut self,
        order: usize,
        pattern: u8,
        row: usize,
        channel: usize,
        cell: XmCell,
    ) -> S3mCell {
        let mapped_instrument = self.mapped_instrument_for_cell(order, pattern, row, channel, cell);
        let mut out = S3mCell {
            note: xm_note_to_s3m(cell.note),
            instrument: mapped_instrument.unwrap_or(cell.instrument),
            volume: xm_volume_to_s3m(cell.volume),
            command: 0,
            info: 0,
        };

        if out.volume == 0xff
            && let Some(instrument) = mapped_instrument
            && let Some(pan) = self.sample_pan_for_flattened_instrument(instrument)
            && pan != 0x80
        {
            out.volume = xm_pan_to_s3m_volume(pan);
        }

        let (command, info, volume_override) =
            self.xm_effect_to_s3m(order, pattern, row, channel, cell.effect, cell.effect_param);
        out.command = command;
        out.info = info;
        if let Some(volume) = volume_override {
            out.volume = volume;
        }
        self.apply_xm_volume_column(order, pattern, row, channel, cell.volume, &mut out);
        out
    }

    fn mapped_instrument_for_cell(
        &mut self,
        order: usize,
        pattern: u8,
        row: usize,
        channel: usize,
        cell: XmCell,
    ) -> Option<u8> {
        let instrument_index = if cell.instrument > 0 {
            let instrument_index = cell.instrument.saturating_sub(1) as usize;
            if channel < self.current_xm_instruments.len() {
                self.current_xm_instruments[channel] = Some(instrument_index);
            }
            instrument_index
        } else if (1..=96).contains(&cell.note) {
            let Some(instrument_index) =
                self.current_xm_instruments.get(channel).copied().flatten()
            else {
                return None;
            };
            instrument_index
        } else {
            return self.current_sample_slots.get(channel).copied().flatten();
        };

        let Some(instrument) = self.module.instruments.get(instrument_index) else {
            return None;
        };
        if instrument.samples.is_empty() {
            return None;
        }

        if cell.note == 0 || cell.note > 96 {
            if let Some(slot) = self.current_sample_slots.get(channel).copied().flatten() {
                return Some(slot);
            }
            if instrument.samples.len() > 1 {
                self.warnings.push(format!(
                    "order {order}, pattern {pattern}, row {row}, channel {channel}: XM instrument {} was set without a note before any sample-map note; falling back to first populated sample",
                    instrument_index + 1
                ));
            }
            let slot = self
                .first_sample_slots
                .get(instrument_index)
                .copied()
                .flatten();
            if let Some(slot) = slot
                && channel < self.current_sample_slots.len()
            {
                self.current_sample_slots[channel] = Some(slot);
            }
            return slot;
        }

        let sample_index = xm_note_sample_index(instrument, cell.note);
        if let Some(slot) = self
            .instrument_sample_slots
            .get(instrument_index)
            .and_then(|slots| slots.get(sample_index))
            .copied()
            .flatten()
        {
            if channel < self.current_sample_slots.len() {
                self.current_sample_slots[channel] = Some(slot);
            }
            return Some(slot);
        }

        self.unsupported_effects.push(XmUnsupportedEffect {
            order: Some(order),
            pattern,
            row,
            channel,
            effect: "sample-map".to_string(),
            param: sample_index.min(0xff) as u8,
            detail: format!(
                "XM instrument {} maps note {} to empty or unavailable sample {}",
                instrument_index + 1,
                cell.note,
                sample_index + 1
            ),
        });
        let slot = self
            .first_sample_slots
            .get(instrument_index)
            .copied()
            .flatten();
        if let Some(slot) = slot
            && channel < self.current_sample_slots.len()
        {
            self.current_sample_slots[channel] = Some(slot);
        }
        slot
    }

    fn sample_pan_for_flattened_instrument(&self, instrument: u8) -> Option<u8> {
        self.instruments
            .get(instrument.checked_sub(1)? as usize)
            .and_then(|sample| sample.default_pan)
    }

    fn xm_effect_to_s3m(
        &mut self,
        order: usize,
        pattern: u8,
        row: usize,
        channel: usize,
        effect: u8,
        param: u8,
    ) -> (u8, u8, Option<u8>) {
        match effect {
            0x00 if param != 0 => (10, param, None),
            0x00 => (0, 0, None),
            0x01 => (6, param, None),
            0x02 => (5, param, None),
            0x03 => (7, param, None),
            0x04 => (8, param, None),
            0x05 => (12, param, None),
            0x06 => (11, param, None),
            0x07 => (18, param, None),
            0x08 => (24, param, None),
            0x09 => (15, param, None),
            0x0a => (4, param, None),
            0x0b => (2, param, None),
            0x0c => (0, 0, Some(param.min(64))),
            0x0d => (3, xm_bcd_row(param) as u8, None),
            0x0e => self.xm_extended_effect_to_s3m(order, pattern, row, channel, param),
            0x0f if param < 0x20 => (1, param, None),
            0x0f => (20, param, None),
            0x10 => (22, param.min(64), None),
            0x11 => (23, param, None),
            0x14 => (19, 0xc0 | param.min(0x0f), None),
            0x19 => self.map_xm_pan_slide(order, pattern, row, channel, "P", param),
            0x1b => (17, param, None),
            0x1d => (9, param, None),
            0x21 => match param >> 4 {
                0x01 => (6, 0xe0 | (param & 0x0f), None),
                0x02 => (5, 0xe0 | (param & 0x0f), None),
                _ => self.unsupported(
                    Some(order),
                    pattern,
                    row,
                    channel,
                    "X",
                    param,
                    "XM extra-fine portamento subcommand is not mapped",
                ),
            },
            _ => self.unsupported(
                Some(order),
                pattern,
                row,
                channel,
                &format!("{effect:X}"),
                param,
                "XM effect is not mapped",
            ),
        }
    }

    fn xm_extended_effect_to_s3m(
        &mut self,
        order: usize,
        pattern: u8,
        row: usize,
        channel: usize,
        param: u8,
    ) -> (u8, u8, Option<u8>) {
        let high = param >> 4;
        let low = param & 0x0f;
        match high {
            0x01 => (6, 0xf0 | low, None),
            0x02 => (5, 0xf0 | low, None),
            0x03 => (19, 0x10 | low, None),
            0x04 => (19, 0x30 | low, None),
            0x05 => (19, 0x20 | low, None),
            0x06 => (19, 0xb0 | low, None),
            0x07 => (19, 0x40 | low, None),
            0x08 => (19, 0x80 | low, None),
            0x09 => (17, low, None),
            0x0a => (4, (low << 4) | 0x0f, None),
            0x0b => (4, 0xf0 | low, None),
            0x0c => (19, 0xc0 | low, None),
            0x0d => (19, 0xd0 | low, None),
            0x0e => (19, 0xe0 | low, None),
            _ => self.unsupported(
                Some(order),
                pattern,
                row,
                channel,
                &format!("E{high:X}"),
                low,
                "XM extended effect is not mapped",
            ),
        }
    }

    fn unsupported(
        &mut self,
        order: Option<usize>,
        pattern: u8,
        row: usize,
        channel: usize,
        effect: &str,
        param: u8,
        detail: &str,
    ) -> (u8, u8, Option<u8>) {
        self.unsupported_effects.push(XmUnsupportedEffect {
            order,
            pattern,
            row,
            channel,
            effect: effect.to_string(),
            param,
            detail: detail.to_string(),
        });
        (25, param, None)
    }

    fn apply_xm_volume_column(
        &mut self,
        order: usize,
        pattern: u8,
        row: usize,
        channel: usize,
        volume: u8,
        out: &mut S3mCell,
    ) {
        match volume {
            0xa0..=0xaf => {
                self.vibrato_speed_memory[channel] = (volume & 0x0f) << 4;
                self.approximation(
                    order,
                    pattern,
                    row,
                    channel,
                    "XM volume column vibrato",
                    "XM volume-column vibrato speed was stored for the next vibrato-depth event",
                );
            }
            0xb0..=0xbf => {
                let depth = volume & 0x0f;
                self.vibrato_depth_memory[channel] = depth;
                let param = self.vibrato_speed_memory[channel] | depth;
                if out.command == 0 {
                    out.command = 8;
                    out.info = param;
                } else {
                    self.unsupported_effects.push(XmUnsupportedEffect {
                        order: Some(order),
                        pattern,
                        row,
                        channel,
                        effect: format!("V{:X}", volume >> 4),
                        param: depth,
                        detail:
                            "XM volume-column vibrato could not be mapped because the effect column is already used"
                                .to_string(),
                    });
                }
            }
            0xc0..=0xcf => {
                let pan = xm_volume_pan_nibble_to_m8(volume & 0x0f);
                self.current_pans[channel] = pan;
            }
            0xd0..=0xdf => {
                let param = volume & 0x0f;
                let (_, info, _) =
                    self.map_xm_pan_slide(order, pattern, row, channel, "vol D", param);
                out.volume = xm_pan_to_s3m_volume(info);
            }
            0xe0..=0xef => {
                let param = (volume & 0x0f) << 4;
                let (_, info, _) =
                    self.map_xm_pan_slide(order, pattern, row, channel, "vol E", param);
                out.volume = xm_pan_to_s3m_volume(info);
            }
            _ => {}
        }
    }

    fn map_xm_pan_slide(
        &mut self,
        order: usize,
        pattern: u8,
        row: usize,
        channel: usize,
        effect: &str,
        param: u8,
    ) -> (u8, u8, Option<u8>) {
        let param = if param == 0 {
            self.pan_slide_memory[channel]
        } else {
            self.pan_slide_memory[channel] = param;
            param
        };
        let right = param >> 4;
        let left = param & 0x0f;
        let delta = if right > 0 {
            i16::from(right) * 4
        } else {
            -(i16::from(left) * 4)
        };
        let current = self.current_pans[channel];
        let pan = (i16::from(current) + delta).clamp(0, 255) as u8;
        self.current_pans[channel] = pan;
        self.approximation(
            order,
            pattern,
            row,
            channel,
            "XM panning slide",
            format!(
                "{effect}{param:02X} was approximated as direct M8 sampler pan {:02X} on this row",
                pan
            ),
        );
        (24, pan, None)
    }

    fn approximation(
        &mut self,
        order: usize,
        pattern: u8,
        row: usize,
        channel: usize,
        category: &str,
        detail: impl Into<String>,
    ) {
        self.approximations.push(XmApproximation {
            order,
            pattern,
            row,
            channel,
            category: category.to_string(),
            detail: detail.into(),
        });
    }
}

fn xm_row_flow_control(pattern: &XmPattern, row: usize, channels: usize) -> XmFlowControl {
    let mut control = XmFlowControl::default();
    for (channel, cell) in pattern.rows[row]
        .iter()
        .take(channels.min(XM_MAX_CHANNELS))
        .copied()
        .enumerate()
    {
        match cell.effect {
            0x0b => control.position_jump = Some(cell.effect_param),
            0x0d => control.pattern_break = Some(xm_bcd_row(cell.effect_param)),
            0x0f if cell.effect_param == 0 => control.stop = true,
            0x0e => match cell.effect_param >> 4 {
                0x06 if cell.effect_param & 0x0f == 0 => control.loop_starts.push(channel),
                0x06 => control.loop_repeat = Some((channel, cell.effect_param & 0x0f)),
                _ => {}
            },
            _ => {}
        }
    }
    control
}

fn strip_consumed_xm_flow(mut cell: XmCell) -> XmCell {
    let consumed = matches!(cell.effect, 0x0b | 0x0d)
        || (cell.effect == 0x0f && cell.effect_param == 0)
        || (cell.effect == 0x0e && cell.effect_param >> 4 == 0x06);
    if consumed {
        cell.effect = 0;
        cell.effect_param = 0;
    }
    cell
}

pub fn is_xm(input: &[u8]) -> bool {
    input.len() >= XM_MAGIC.len() && &input[..XM_MAGIC.len()] == XM_MAGIC
}

pub fn parse_xm(input: &[u8]) -> Result<XmModule, XmError> {
    if input.len() < XM_FIXED_HEADER_LEN {
        return Err(XmError::TooShort);
    }
    if !is_xm(input) {
        return Err(XmError::MissingSignature);
    }
    if input[37] != 0x1a {
        return Err(XmError::MissingSignature);
    }

    let title = decode_fixed_string(&input[17..37]);
    let tracker_name = decode_fixed_string(&input[38..58]);
    let version = read_u16(input, 58)?;
    if version < 0x0104 {
        return Err(XmError::UnsupportedVersion(version));
    }

    let header_size = read_u32(input, 60)?;
    if header_size < 20 || header_size as usize > input.len().saturating_sub(XM_FIXED_HEADER_LEN) {
        return Err(XmError::InvalidHeaderSize(header_size));
    }

    let header_end = XM_FIXED_HEADER_LEN + header_size as usize;
    let song_length = read_u16(input, 64)? as usize;
    let restart_position = read_u16(input, 66)?;
    let channel_count = read_u16(input, 68)?;
    if channel_count == 0 || channel_count as usize > XM_MAX_CHANNELS {
        return Err(XmError::UnsupportedChannelCount(channel_count));
    }
    let pattern_count = read_u16(input, 70)? as usize;
    let instrument_count = read_u16(input, 72)? as usize;
    let frequency_table_flags = read_u16(input, 74)?;
    let initial_speed = read_u16(input, 76)?.clamp(1, u8::MAX as u16) as u8;
    let initial_tempo = read_u16(input, 78)?.clamp(1, u8::MAX as u16) as u8;
    let order_table = input
        .get(80..80 + XM_ORDER_TABLE_LEN)
        .ok_or(XmError::Truncated("order table"))?;
    let orders = order_table[..song_length.min(XM_ORDER_TABLE_LEN)]
        .iter()
        .copied()
        .filter(|order| (*order as usize) < pattern_count)
        .collect::<Vec<_>>();

    let mut pos = header_end;
    let mut patterns = Vec::with_capacity(pattern_count);
    for _ in 0..pattern_count {
        let (pattern, next_pos) = parse_pattern(input, pos, channel_count as usize)?;
        patterns.push(pattern);
        pos = next_pos;
    }

    let mut instruments = Vec::with_capacity(instrument_count);
    for _ in 0..instrument_count {
        let (instrument, next_pos) = parse_instrument(input, pos)?;
        instruments.push(instrument);
        pos = next_pos;
    }

    Ok(XmModule {
        title,
        tracker_name,
        version,
        restart_position,
        channel_count: channel_count as usize,
        orders,
        patterns,
        instruments,
        initial_speed,
        initial_tempo,
        frequency_table_flags,
    })
}

fn parse_pattern(
    input: &[u8],
    offset: usize,
    channel_count: usize,
) -> Result<(XmPattern, usize), XmError> {
    let header = input
        .get(offset..offset + 9)
        .ok_or(XmError::Truncated("pattern header"))?;
    let header_len = read_u32_slice(header, 0) as usize;
    let row_count = read_u16_slice(header, 5) as usize;
    let packed_len = read_u16_slice(header, 7) as usize;
    let body_start = offset + header_len;
    let body_end = body_start + packed_len;
    let body = input
        .get(body_start..body_end)
        .ok_or(XmError::Truncated("pattern data"))?;

    let rows = row_count.max(1);
    let mut pattern = XmPattern {
        rows: vec![vec![XmCell::EMPTY; channel_count]; rows],
    };
    let mut pos = 0usize;

    for row in 0..rows {
        for channel in 0..channel_count {
            if pos >= body.len() {
                if packed_len == 0 {
                    return Ok((pattern, body_end));
                }
                return Err(XmError::Truncated("pattern cell"));
            }
            let flags = body[pos];
            pos += 1;
            let cell = if flags & 0x80 != 0 {
                let mut cell = XmCell::EMPTY;
                if flags & 0x01 != 0 {
                    cell.note = read_pattern_byte(body, &mut pos)?;
                }
                if flags & 0x02 != 0 {
                    cell.instrument = read_pattern_byte(body, &mut pos)?;
                }
                if flags & 0x04 != 0 {
                    cell.volume = read_pattern_byte(body, &mut pos)?;
                }
                if flags & 0x08 != 0 {
                    cell.effect = read_pattern_byte(body, &mut pos)?;
                }
                if flags & 0x10 != 0 {
                    cell.effect_param = read_pattern_byte(body, &mut pos)?;
                }
                cell
            } else {
                XmCell {
                    note: flags,
                    instrument: read_pattern_byte(body, &mut pos)?,
                    volume: read_pattern_byte(body, &mut pos)?,
                    effect: read_pattern_byte(body, &mut pos)?,
                    effect_param: read_pattern_byte(body, &mut pos)?,
                }
            };
            pattern.rows[row][channel] = cell;
        }
    }

    Ok((pattern, body_end))
}

fn parse_instrument(input: &[u8], offset: usize) -> Result<(XmInstrument, usize), XmError> {
    let header_len = read_u32(input, offset)? as usize;
    let header = input
        .get(offset..offset + header_len)
        .ok_or(XmError::Truncated("instrument header"))?;
    if header.len() < 29 {
        return Err(XmError::Truncated("instrument header"));
    }
    let name = decode_fixed_string(
        header
            .get(4..26)
            .ok_or(XmError::Truncated("instrument name"))?,
    );
    let sample_count = read_u16_slice(header, 27) as usize;
    let mut pos = offset + header_len;

    if sample_count == 0 {
        return Ok((
            XmInstrument {
                name,
                sample_map: vec![0; 96],
                volume_envelope: XmEnvelope::default(),
                panning_envelope: XmEnvelope::default(),
                volume_envelope_type: 0,
                panning_envelope_type: 0,
                volume_fadeout: 0,
                vibrato_type: 0,
                vibrato_sweep: 0,
                vibrato_depth: 0,
                vibrato_rate: 0,
                samples: Vec::new(),
            },
            pos,
        ));
    }

    if header.len() < 241 {
        return Err(XmError::Truncated("instrument extended header"));
    }
    let sample_header_size = read_u32_slice(header, 29) as usize;
    let sample_map = header[33..129].to_vec();
    let volume_envelope_type = header[233];
    let panning_envelope_type = header[234];
    let volume_envelope = parse_envelope(
        &header[129..177],
        header[225],
        header[227],
        header[228],
        header[229],
        volume_envelope_type,
    );
    let panning_envelope = parse_envelope(
        &header[177..225],
        header[226],
        header[230],
        header[231],
        header[232],
        panning_envelope_type,
    );
    let vibrato_type = header[235];
    let vibrato_sweep = header[236];
    let vibrato_depth = header[237];
    let vibrato_rate = header[238];
    let volume_fadeout = read_u16_slice(header, 239);
    let mut raw_samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let raw = parse_sample_header(input, pos, sample_header_size)?;
        raw_samples.push(raw);
        pos += sample_header_size;
    }

    let mut samples = Vec::with_capacity(sample_count);
    for raw in raw_samples {
        let data_bytes = input
            .get(pos..pos + raw.length as usize)
            .ok_or(XmError::Truncated("sample data"))?;
        let data = decode_sample_data(data_bytes, raw.is_16bit());
        pos += raw.length as usize;
        let loop_start = raw.loop_start_frames().min(data.len() as u32);
        let loop_length = raw.loop_length_frames().min(data.len() as u32);
        samples.push(XmSample {
            name: raw.name,
            length: data.len() as u32,
            loop_start,
            loop_length,
            volume: raw.volume.min(64),
            finetune: raw.finetune,
            sample_type: raw.sample_type,
            panning: raw.panning,
            relative_note: raw.relative_note,
            data,
        });
    }

    Ok((
        XmInstrument {
            name,
            sample_map,
            volume_envelope,
            panning_envelope,
            volume_envelope_type,
            panning_envelope_type,
            volume_fadeout,
            vibrato_type,
            vibrato_sweep,
            vibrato_depth,
            vibrato_rate,
            samples,
        },
        pos,
    ))
}

#[derive(Debug, Clone)]
struct RawSampleHeader {
    name: String,
    length: u32,
    loop_start: u32,
    loop_length: u32,
    volume: u8,
    finetune: i8,
    sample_type: u8,
    panning: u8,
    relative_note: i8,
}

impl RawSampleHeader {
    fn is_16bit(&self) -> bool {
        self.sample_type & 0x10 != 0
    }

    fn loop_start_frames(&self) -> u32 {
        if self.is_16bit() {
            self.loop_start / 2
        } else {
            self.loop_start
        }
    }

    fn loop_length_frames(&self) -> u32 {
        if self.is_16bit() {
            self.loop_length / 2
        } else {
            self.loop_length
        }
    }
}

fn parse_sample_header(
    input: &[u8],
    offset: usize,
    header_size: usize,
) -> Result<RawSampleHeader, XmError> {
    let header = input
        .get(offset..offset + header_size)
        .ok_or(XmError::Truncated("sample header"))?;
    if header.len() < 40 {
        return Err(XmError::Truncated("sample header"));
    }

    Ok(RawSampleHeader {
        length: read_u32_slice(header, 0),
        loop_start: read_u32_slice(header, 4),
        loop_length: read_u32_slice(header, 8),
        volume: header[12].min(64),
        finetune: header[13] as i8,
        sample_type: header[14],
        panning: header[15],
        relative_note: header[16] as i8,
        name: decode_fixed_string(&header[18..40]),
    })
}

fn parse_envelope(
    bytes: &[u8],
    point_count: u8,
    sustain_point: u8,
    loop_start_point: u8,
    loop_end_point: u8,
    flags: u8,
) -> XmEnvelope {
    let count = point_count.min(12) as usize;
    let mut points = Vec::with_capacity(count);
    for index in 0..count {
        let offset = index * 4;
        points.push(XmEnvelopePoint {
            frame: read_u16_slice(bytes, offset),
            value: read_u16_slice(bytes, offset + 2),
        });
    }
    XmEnvelope {
        points,
        sustain_point,
        loop_start_point,
        loop_end_point,
        flags,
    }
}

fn xm_sample_to_s3m(
    _instrument_index: usize,
    sample_index: usize,
    instrument: &XmInstrument,
    sample: &XmSample,
    frequency_table_flags: u16,
) -> S3mInstrument {
    let RewrittenXmSample {
        data,
        loop_start,
        loop_end,
        looped: is_looped,
        ping_pong,
    } = xm_rewrite_ping_pong_sample(sample);

    S3mInstrument {
        kind: 1,
        name: if sample.name.is_empty() {
            if instrument.samples.len() > 1 {
                format!("{} s{}", instrument.name, sample_index + 1)
            } else {
                instrument.name.clone()
            }
        } else {
            sample.name.clone()
        },
        length: data.len() as u32,
        loop_start,
        loop_end,
        volume: sample.volume.min(64),
        flags: if is_looped { 0x01 } else { 0x00 }
            | if sample.sample_type & 0x10 != 0 {
                0x04
            } else {
                0x00
            },
        c5_speed: xm_c5_speed(sample.relative_note, sample.finetune, frequency_table_flags),
        pack: 0,
        default_pan: Some(sample.panning),
        amp_envelope: xm_amp_envelope_approx(instrument),
        auto_vibrato: xm_auto_vibrato_approx(instrument),
        data,
        right_data: None,
        ping_pong_loop: ping_pong,
    }
}

struct RewrittenXmSample {
    data: Vec<i16>,
    loop_start: u32,
    loop_end: u32,
    looped: bool,
    ping_pong: bool,
}

fn xm_rewrite_ping_pong_sample(sample: &XmSample) -> RewrittenXmSample {
    let loop_start = sample.loop_start.min(sample.data.len() as u32);
    let loop_end = loop_start
        .saturating_add(sample.loop_length)
        .min(sample.data.len() as u32);
    let looped = sample.sample_type & 0x03 != 0 && loop_end > loop_start;
    RewrittenXmSample {
        data: sample.data.clone(),
        loop_start,
        loop_end,
        looped,
        ping_pong: looped && sample.sample_type & 0x03 == 0x02,
    }
}

fn xm_amp_envelope_approx(instrument: &XmInstrument) -> Option<S3mAmpEnvelope> {
    let envelope = &instrument.volume_envelope;
    if envelope.flags & 0x01 == 0 || envelope.points.len() < 2 {
        if instrument.volume_fadeout == 0 {
            return None;
        }
        return Some(S3mAmpEnvelope {
            amount: 0x80,
            attack: 0,
            hold: 0,
            decay: xm_frame_to_m8((instrument.volume_fadeout / 32).max(1)),
        });
    }

    let first = envelope.points[0];
    let peak = envelope
        .points
        .iter()
        .copied()
        .max_by_key(|point| point.value)?;
    let attack = peak.frame.saturating_sub(first.frame);
    let sustain_frame = envelope
        .points
        .get(envelope.sustain_point as usize)
        .map(|point| point.frame)
        .unwrap_or(peak.frame);
    let last_frame = envelope
        .points
        .last()
        .map(|point| point.frame)
        .unwrap_or(peak.frame);
    let release = if instrument.volume_fadeout > 0 {
        (instrument.volume_fadeout / 32).max(1)
    } else {
        last_frame.saturating_sub(sustain_frame).max(1)
    };

    Some(S3mAmpEnvelope {
        amount: ((peak.value.min(64) as u16 * 255) / 64) as u8,
        attack: xm_frame_to_m8(attack),
        hold: xm_frame_to_m8(sustain_frame.saturating_sub(peak.frame)),
        decay: xm_frame_to_m8(release),
    })
}

fn xm_auto_vibrato_approx(instrument: &XmInstrument) -> Option<S3mAutoVibrato> {
    (instrument.vibrato_depth != 0 || instrument.vibrato_rate != 0).then_some(S3mAutoVibrato {
        speed: instrument.vibrato_rate.min(0x0f),
        depth: instrument.vibrato_depth.min(0x0f),
    })
}

fn xm_note_sample_index(instrument: &XmInstrument, note: u8) -> usize {
    let note_index = note.saturating_sub(1).min(95) as usize;
    instrument.sample_map.get(note_index).copied().unwrap_or(0) as usize
}

fn xm_note_to_s3m(note: u8) -> u8 {
    match note {
        0 => 0xff,
        97 => 0xfe,
        1..=96 => {
            let zero_based = note - 1;
            ((zero_based / 12) << 4) | (zero_based % 12)
        }
        _ => 0xff,
    }
}

fn xm_volume_to_s3m(volume: u8) -> u8 {
    match volume {
        0 => 0xff,
        0x10..=0x50 => (volume - 0x10).min(64),
        0x60..=0x6f => 95 + (volume & 0x0f),
        0x70..=0x7f => 85 + (volume & 0x0f),
        0x80..=0x8f => 75 + (volume & 0x0f),
        0x90..=0x9f => 65 + (volume & 0x0f),
        0xc0..=0xcf => 0x80 + ((volume & 0x0f) * 4),
        0xf0..=0xff => 193 + (volume & 0x0f),
        _ => 0xff,
    }
}

fn xm_volume_pan_nibble_to_m8(value: u8) -> u8 {
    ((value.min(0x0f) as u16 * 255 + 7) / 15) as u8
}

fn xm_pan_to_s3m_volume(pan: u8) -> u8 {
    0x80 + ((pan as u16 * 64 + 127) / 255).min(64) as u8
}

fn xm_bcd_row(value: u8) -> usize {
    (((value >> 4) as usize) * 10 + (value & 0x0f) as usize).min(63)
}

fn decode_sample_data(input: &[u8], is_16bit: bool) -> Vec<i16> {
    if is_16bit {
        let mut current = 0i16;
        input
            .chunks_exact(2)
            .map(|bytes| {
                let delta = i16::from_le_bytes([bytes[0], bytes[1]]);
                current = current.wrapping_add(delta);
                current
            })
            .collect()
    } else {
        let mut current = 0i8;
        input
            .iter()
            .map(|byte| {
                current = current.wrapping_add(*byte as i8);
                (current as i16) << 8
            })
            .collect()
    }
}

fn xm_c5_speed(relative_note: i8, finetune: i8, _frequency_table_flags: u16) -> u32 {
    let semitones = relative_note as f64 + finetune as f64 / 128.0;
    (8363.0 * 2f64.powf(semitones / 12.0))
        .round()
        .clamp(1.0, u32::MAX as f64) as u32
}

fn xm_frame_to_m8(frames: u16) -> u8 {
    if frames == 0 {
        0
    } else {
        ((frames as u32 * 255 + 255) / 512).clamp(1, 255) as u8
    }
}

fn read_pattern_byte(input: &[u8], pos: &mut usize) -> Result<u8, XmError> {
    let value = input
        .get(*pos)
        .copied()
        .ok_or(XmError::Truncated("pattern cell"))?;
    *pos = (*pos).saturating_add(1);
    Ok(value)
}

fn read_u16(input: &[u8], offset: usize) -> Result<u16, XmError> {
    let bytes = input
        .get(offset..offset + 2)
        .ok_or(XmError::Truncated("u16"))?;
    Ok(read_u16_slice(bytes, 0))
}

fn read_u32(input: &[u8], offset: usize) -> Result<u32, XmError> {
    let bytes = input
        .get(offset..offset + 4)
        .ok_or(XmError::Truncated("u32"))?;
    Ok(read_u32_slice(bytes, 0))
}

fn read_u16_slice(input: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([input[offset], input[offset + 1]])
}

fn read_u32_slice(input: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        input[offset],
        input[offset + 1],
        input[offset + 2],
        input[offset + 3],
    ])
}

fn decode_fixed_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end])
        .trim_end()
        .to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn parses_minimal_xm() {
        let input = minimal_xm();
        let module = parse_xm(&input).expect("valid xm");
        assert_eq!(module.title, "XMTEST");
        assert_eq!(module.channel_count, 1);
        assert_eq!(module.orders, vec![0]);
        assert_eq!(module.patterns[0].rows[0][0].note, 49);
        assert_eq!(module.instruments[0].samples[0].data.len(), 4);

        let s3m = module.to_s3m_module();
        assert_eq!(s3m.patterns[0].rows[0][0].note, 0x40);
        assert_eq!(s3m.instruments[0].c5_speed, 8363);
    }

    #[test]
    fn maps_xm_speed_20_as_tempo() {
        let mut input = minimal_xm();
        set_first_effect(&mut input, 0x0f, 0x20);
        let module = parse_xm(&input).expect("valid xm");
        let s3m = module.to_s3m_module();
        let cell = s3m.patterns[0].rows[0][0];
        assert_eq!(cell.command, 20);
        assert_eq!(cell.info, 0x20);
    }

    #[test]
    fn xm_f00_stops_the_native_timeline() {
        let mut module = test_xm_module(1, 1, 4);
        module.patterns[0].rows[0][0] = XmCell {
            effect: 0x0f,
            effect_param: 0,
            ..XmCell::EMPTY
        };
        module.patterns[0].rows[1][0] = XmCell {
            note: 49,
            ..XmCell::EMPTY
        };

        let adapter = module.to_s3m_adapter();
        assert_eq!(adapter.module.patterns[0].rows.len(), 1);
        assert_eq!(adapter.module.patterns[0].rows[0][0].command, 0);
    }

    #[test]
    fn maps_xm_fine_and_extra_fine_portamento() {
        let mut input = minimal_xm();
        set_first_effect(&mut input, 0x0e, 0x13);
        let module = parse_xm(&input).expect("valid xm");
        let cell = module.to_s3m_module().patterns[0].rows[0][0];
        assert_eq!((cell.command, cell.info), (6, 0xf3));

        let mut input = minimal_xm();
        set_first_effect(&mut input, 0x21, 0x24);
        let module = parse_xm(&input).expect("valid xm");
        let cell = module.to_s3m_module().patterns[0].rows[0][0];
        assert_eq!((cell.command, cell.info), (5, 0xe4));
    }

    #[test]
    fn maps_xm_fine_volume_slides_and_key_off_tick() {
        let mut input = minimal_xm();
        set_first_effect(&mut input, 0x0e, 0xa3);
        let module = parse_xm(&input).expect("valid xm");
        let cell = module.to_s3m_module().patterns[0].rows[0][0];
        assert_eq!((cell.command, cell.info), (4, 0x3f));

        let mut input = minimal_xm();
        set_first_effect(&mut input, 0x14, 0x05);
        let module = parse_xm(&input).expect("valid xm");
        let cell = module.to_s3m_module().patterns[0].rows[0][0];
        assert_eq!((cell.command, cell.info), (19, 0xc5));
    }

    #[test]
    fn maps_xm_pattern_break_as_decimal_row() {
        let mut module = test_xm_module(1, 2, 16);
        module.orders = vec![0, 1];
        module.patterns[0].rows[0][0] = XmCell {
            effect: 0x0d,
            effect_param: 0x10,
            ..XmCell::EMPTY
        };
        module.patterns[1].rows[10][0] = XmCell {
            note: 49,
            ..XmCell::EMPTY
        };
        let s3m = module.to_s3m_module();
        assert_eq!(s3m.patterns[0].rows[0][0].command, 0);
        assert_eq!(s3m.patterns[0].rows[1][0].note, 0x40);
    }

    #[test]
    fn xm_position_jump_loop_becomes_restart_position() {
        let mut module = test_xm_module(1, 2, 1);
        module.orders = vec![0, 1];
        module.patterns[1].rows[0][0] = XmCell {
            effect: 0x0b,
            effect_param: 0,
            ..XmCell::EMPTY
        };
        let s3m = module.to_s3m_module();
        assert_eq!(s3m.restart_position, Some(0));
        assert_eq!(s3m.patterns[0].rows[1][0].command, 0);
    }

    #[test]
    fn xm_pattern_loop_is_expanded_by_native_timeline() {
        let mut module = test_xm_module(1, 1, 4);
        module.patterns[0].rows[0][0] = XmCell {
            note: 49,
            effect: 0x0e,
            effect_param: 0x60,
            ..XmCell::EMPTY
        };
        module.patterns[0].rows[1][0] = XmCell {
            note: 50,
            ..XmCell::EMPTY
        };
        module.patterns[0].rows[2][0] = XmCell {
            note: 51,
            effect: 0x0e,
            effect_param: 0x62,
            ..XmCell::EMPTY
        };
        let s3m = module.to_s3m_module();
        let notes = s3m.patterns[0].rows[..9]
            .iter()
            .map(|row| row[0].note)
            .collect::<Vec<_>>();
        assert_eq!(
            notes,
            vec![0x40, 0x41, 0x42, 0x40, 0x41, 0x42, 0x40, 0x41, 0x42]
        );
        assert!(
            s3m.patterns[0].rows[..9]
                .iter()
                .all(|row| row[0].command == 0)
        );
    }

    #[test]
    fn xm_pattern_delay_is_preserved_for_m8_emit_delay_rows() {
        let mut module = test_xm_module(1, 1, 1);
        module.patterns[0].rows[0][0] = XmCell {
            effect: 0x0e,
            effect_param: 0xe2,
            ..XmCell::EMPTY
        };
        let s3m = module.to_s3m_module();
        assert_eq!(s3m.patterns[0].rows[0][0].command, 19);
        assert_eq!(s3m.patterns[0].rows[0][0].info, 0xe2);
    }

    #[test]
    fn xm_extended_tick_effects_map_to_m8_compatible_paths() {
        let mut module = test_xm_module(1, 1, 3);
        module.patterns[0].rows[0][0] = XmCell {
            effect: 0x0e,
            effect_param: 0x93,
            ..XmCell::EMPTY
        };
        module.patterns[0].rows[1][0] = XmCell {
            effect: 0x0e,
            effect_param: 0xc4,
            ..XmCell::EMPTY
        };
        module.patterns[0].rows[2][0] = XmCell {
            effect: 0x0e,
            effect_param: 0xd5,
            ..XmCell::EMPTY
        };
        let s3m = module.to_s3m_module();
        assert_eq!(s3m.patterns[0].rows[0][0].command, 17);
        assert_eq!(s3m.patterns[0].rows[0][0].info, 0x03);
        assert_eq!(s3m.patterns[0].rows[1][0].command, 19);
        assert_eq!(s3m.patterns[0].rows[1][0].info, 0xc4);
        assert_eq!(s3m.patterns[0].rows[2][0].command, 19);
        assert_eq!(s3m.patterns[0].rows[2][0].info, 0xd5);
    }

    #[test]
    fn reports_xm_panning_slide_without_faking_direct_pan() {
        let mut input = minimal_xm();
        set_first_effect(&mut input, 0x19, 0x34);
        let module = parse_xm(&input).expect("valid xm");
        let adapter = module.to_s3m_adapter();
        let cell = adapter.module.patterns[0].rows[0][0];
        assert_eq!(cell.command, 24);
        assert_eq!(cell.info, 0x8c);
        assert_eq!(adapter.approximations[0].category, "XM panning slide");
        assert!(adapter.unsupported_effects.is_empty());
    }

    #[test]
    fn maps_xm_tremor_to_s3m_tremor_path() {
        let mut input = minimal_xm();
        set_first_effect(&mut input, 0x1d, 0x43);
        let module = parse_xm(&input).expect("valid xm");
        let s3m = module.to_s3m_module();
        let cell = s3m.patterns[0].rows[0][0];
        assert_eq!(cell.command, 9);
        assert_eq!(cell.info, 0x43);
    }

    #[test]
    fn reports_original_xm_extended_unsupported_effect() {
        let mut input = minimal_xm();
        set_first_effect(&mut input, 0x0e, 0xf1);
        let module = parse_xm(&input).expect("valid xm");
        let adapter = module.to_s3m_adapter();
        assert_eq!(adapter.unsupported_effects[0].order, Some(0));
        assert_eq!(adapter.unsupported_effects[0].effect, "EF");
        assert_eq!(adapter.unsupported_effects[0].param, 0x01);
    }

    #[test]
    fn rejects_truncated_packed_pattern_cells() {
        let mut input = minimal_xm();
        let pattern_offset = 60 + 276;
        input[pattern_offset + 7..pattern_offset + 9].copy_from_slice(&5u16.to_le_bytes());
        assert!(matches!(
            parse_xm(&input),
            Err(XmError::Truncated("pattern cell"))
        ));
    }

    #[test]
    fn rejects_short_instrument_extended_header() {
        let mut input = minimal_xm();
        let instrument_offset = 60 + 276 + 9 + 68;
        input[instrument_offset..instrument_offset + 4].copy_from_slice(&30u32.to_le_bytes());
        assert!(matches!(
            parse_xm(&input),
            Err(XmError::Truncated("instrument extended header"))
        ));
    }

    #[test]
    fn sample_map_remaps_note_to_second_sample_and_applies_pan() {
        let input = minimal_xm_with_two_samples();
        let module = parse_xm(&input).expect("valid xm");
        let adapter = module.to_s3m_adapter();
        assert_eq!(adapter.module.instruments.len(), 2);
        let cell = adapter.module.patterns[0].rows[0][0];
        assert_eq!(cell.instrument, 2);
        assert_eq!(cell.volume, 0xc0);
    }

    #[test]
    fn sample_map_reuses_current_xm_instrument_when_note_has_no_instrument() {
        let mut input = minimal_xm_with_two_samples();
        replace_packed_empty_row_with_cell(&mut input, 1, &[49, 0, 0, 0, 0]);
        let module = parse_xm(&input).expect("valid xm");
        let adapter = module.to_s3m_adapter();
        assert_eq!(adapter.module.patterns[0].rows[1][0].instrument, 2);
    }

    #[test]
    fn xm_volume_column_vibrato_depth_uses_effect_slot() {
        let mut input = minimal_xm();
        set_first_volume(&mut input, 0xb3);
        let module = parse_xm(&input).expect("valid xm");
        let adapter = module.to_s3m_adapter();
        let cell = adapter.module.patterns[0].rows[0][0];
        assert_eq!(cell.command, 8);
        assert_eq!(cell.info, 0x03);
    }

    #[test]
    fn xm_volume_column_vibrato_speed_is_remembered_for_depth() {
        let mut input = minimal_xm();
        set_first_volume(&mut input, 0xa4);
        replace_packed_empty_row_with_cell(&mut input, 1, &[0, 0, 0xb3, 0, 0]);
        let module = parse_xm(&input).expect("valid xm");
        let adapter = module.to_s3m_adapter();
        let cell = adapter.module.patterns[0].rows[1][0];
        assert_eq!(cell.command, 8);
        assert_eq!(cell.info, 0x43);
    }

    #[test]
    fn xm_volume_column_pan_slide_updates_pan() {
        let mut input = minimal_xm();
        set_first_volume(&mut input, 0xe2);
        let module = parse_xm(&input).expect("valid xm");
        let adapter = module.to_s3m_adapter();
        let cell = adapter.module.patterns[0].rows[0][0];
        assert_eq!(cell.volume, xm_pan_to_s3m_volume(0x88));
        assert_eq!(adapter.approximations[0].category, "XM panning slide");
    }

    #[test]
    fn ping_pong_loop_is_preserved_without_rewriting_pcm() {
        let mut input = minimal_xm();
        let sample_header = minimal_xm_sample_header_offset();
        input[sample_header + 4..sample_header + 8].copy_from_slice(&1u32.to_le_bytes());
        input[sample_header + 8..sample_header + 12].copy_from_slice(&2u32.to_le_bytes());
        input[sample_header + 14] = 0x02;
        let module = parse_xm(&input).expect("valid xm");
        let adapter = module.to_s3m_adapter();
        let sample = &adapter.module.instruments[0];
        assert_eq!(sample.data.len(), 4);
        assert_eq!(sample.loop_start, 1);
        assert_eq!(sample.loop_end, 3);
        assert!(sample.is_looped());
        assert!(sample.ping_pong_loop);
    }

    #[test]
    fn xm_finetune_scale_is_independent_of_frequency_table_mode() {
        let amiga = xm_c5_speed(0, 64, 0);
        let linear = xm_c5_speed(0, 64, 1);
        assert_eq!(amiga, linear);
        assert!(amiga > 8363);
    }

    #[test]
    fn flattened_samples_stop_at_the_m8_instrument_limit() {
        let instrument = parse_xm(&minimal_xm()).unwrap().instruments[0].clone();
        let mut module = test_xm_module(1, 1, 1);
        module.instruments = vec![instrument; 129];

        let adapter = module.to_s3m_adapter();

        assert_eq!(adapter.module.instruments.len(), 128);
        assert!(
            adapter
                .warnings
                .iter()
                .any(|warning| warning.contains("128 M8 instrument slots"))
        );
    }

    #[test]
    fn simple_volume_envelope_becomes_amp_envelope_metadata() {
        let mut input = minimal_xm();
        let instrument = minimal_xm_instrument_offset();
        input[instrument + 129..instrument + 131].copy_from_slice(&0u16.to_le_bytes());
        input[instrument + 131..instrument + 133].copy_from_slice(&0u16.to_le_bytes());
        input[instrument + 133..instrument + 135].copy_from_slice(&8u16.to_le_bytes());
        input[instrument + 135..instrument + 137].copy_from_slice(&64u16.to_le_bytes());
        input[instrument + 225] = 2;
        input[instrument + 233] = 1;
        let module = parse_xm(&input).expect("valid xm");
        let adapter = module.to_s3m_adapter();
        let envelope = adapter.module.instruments[0]
            .amp_envelope
            .expect("amp envelope");
        assert!(envelope.attack > 0);
        assert_eq!(envelope.amount, 255);
    }

    pub fn minimal_xm() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(XM_MAGIC);
        push_fixed(&mut out, "XMTEST", 20);
        out.push(0x1a);
        push_fixed(&mut out, "m8convert", 20);
        out.extend_from_slice(&0x0104u16.to_le_bytes());
        out.extend_from_slice(&276u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&6u16.to_le_bytes());
        out.extend_from_slice(&125u16.to_le_bytes());
        out.push(0);
        out.resize(60 + 276, 0);

        out.extend_from_slice(&9u32.to_le_bytes());
        out.push(0);
        out.extend_from_slice(&64u16.to_le_bytes());
        out.extend_from_slice(&68u16.to_le_bytes());
        out.extend_from_slice(&[49, 1, 0x40, 0, 0]);
        out.extend(std::iter::repeat(0x80).take(63));

        out.extend_from_slice(&263u32.to_le_bytes());
        push_fixed(&mut out, "Lead", 22);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.resize(out.len() + 263 - 33, 0);

        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.push(64);
        out.push(0);
        out.push(0);
        out.push(128);
        out.push(0);
        out.push(0);
        push_fixed(&mut out, "tone", 22);
        out.extend_from_slice(&[0, 20, 216, 20]);
        out
    }

    fn minimal_xm_with_two_samples() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(XM_MAGIC);
        push_fixed(&mut out, "XMTEST", 20);
        out.push(0x1a);
        push_fixed(&mut out, "m8convert", 20);
        out.extend_from_slice(&0x0104u16.to_le_bytes());
        out.extend_from_slice(&276u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&6u16.to_le_bytes());
        out.extend_from_slice(&125u16.to_le_bytes());
        out.push(0);
        out.resize(60 + 276, 0);

        out.extend_from_slice(&9u32.to_le_bytes());
        out.push(0);
        out.extend_from_slice(&64u16.to_le_bytes());
        out.extend_from_slice(&68u16.to_le_bytes());
        out.extend_from_slice(&[49, 1, 0, 0, 0]);
        out.extend(std::iter::repeat(0x80).take(63));

        let instrument_offset = out.len();
        out.extend_from_slice(&263u32.to_le_bytes());
        push_fixed(&mut out, "Lead", 22);
        out.push(0);
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.resize(instrument_offset + 263, 0);
        out[instrument_offset + 33 + 48] = 1;

        push_sample_header(&mut out, 4, 64, 128, "tone a");
        push_sample_header(&mut out, 4, 64, 255, "tone b");
        out.extend_from_slice(&[0, 20, 216, 20]);
        out.extend_from_slice(&[0, 30, 196, 30]);
        out
    }

    fn test_xm_module(channels: usize, pattern_count: usize, rows: usize) -> XmModule {
        XmModule {
            title: "test".to_string(),
            tracker_name: "m8convert-test".to_string(),
            version: 0x0104,
            restart_position: 0,
            channel_count: channels,
            orders: (0..pattern_count).map(|pattern| pattern as u8).collect(),
            patterns: vec![
                XmPattern {
                    rows: vec![vec![XmCell::EMPTY; channels]; rows]
                };
                pattern_count
            ],
            instruments: Vec::new(),
            initial_speed: 6,
            initial_tempo: 125,
            frequency_table_flags: 1,
        }
    }

    fn push_sample_header(out: &mut Vec<u8>, len: u32, volume: u8, panning: u8, name: &str) {
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.push(volume);
        out.push(0);
        out.push(0);
        out.push(panning);
        out.push(0);
        out.push(0);
        push_fixed(out, name, 22);
    }

    fn set_first_effect(input: &mut [u8], effect: u8, param: u8) {
        let first_cell = 60 + 276 + 9;
        input[first_cell + 3] = effect;
        input[first_cell + 4] = param;
    }

    fn set_first_volume(input: &mut [u8], volume: u8) {
        let first_cell = 60 + 276 + 9;
        input[first_cell + 2] = volume;
    }

    fn replace_packed_empty_row_with_cell(input: &mut Vec<u8>, row: usize, cell: &[u8; 5]) {
        let pattern_offset = 60 + 276;
        let body_start = pattern_offset + 9;
        let packed_len = u16::from_le_bytes(
            input[pattern_offset + 7..pattern_offset + 9]
                .try_into()
                .unwrap(),
        );
        let offset = body_start + 5 + row.saturating_sub(1);
        input.splice(offset..offset + 1, cell.iter().copied());
        input[pattern_offset + 7..pattern_offset + 9]
            .copy_from_slice(&(packed_len + 4).to_le_bytes());
    }

    fn minimal_xm_instrument_offset() -> usize {
        60 + 276 + 9 + 68
    }

    fn minimal_xm_sample_header_offset() -> usize {
        minimal_xm_instrument_offset() + 263
    }

    fn push_fixed(out: &mut Vec<u8>, value: &str, len: usize) {
        let start = out.len();
        out.extend_from_slice(value.as_bytes());
        out.resize(start + len, 0);
    }
}
