use super::{
    DisplaySnapshotCandidate, FirmwareDisplaySnapshot, FirmwareDisplayStats,
    memory::ExecutionMemory,
};

const RENDER_CACHE_CHAR_BASE: u32 = 0x2001_365a;
const RENDER_CACHE_STYLE_BASE: u32 = 0x2001_3a1c;
const RENDER_CACHE_WIDTH: usize = 40;
const RENDER_CACHE_HEIGHT: usize = 24;
const RENDER_CACHE_CELLS: usize = RENDER_CACHE_WIDTH * RENDER_CACHE_HEIGHT;
const RENDER_CACHE_SIZE: u32 = 0x3c0;
const LARGE_RENDER_DIFF_DEFER_CELLS: usize = 64;

const DISPLAY_GRID_BASE: u32 = 0x2001_4a07;
const DISPLAY_GRID_VISIBLE_BASE: u32 = 0x2001_4a57;
const DISPLAY_GRID_ROWS: usize = 16;
const DISPLAY_GRID_COLUMNS: usize = 8;
const DISPLAY_GRID_ROW_STRIDE: u32 = 0x28;
const DISPLAY_GRID_CELL_STRIDE: u32 = 3;
const DISPLAY_CELL_TEXT_OFFSET: u32 = 4;

const ASCII_SCAN_START: u32 = 0x2001_3000;
const ASCII_SCAN_END: u32 = 0x2001_5000;
const ASCII_GRID_WIDTH: usize = 64;
const ASCII_GRID_HEIGHT: usize = 12;
const ASCII_GRID_STRIDE: u32 = 0x40;

#[derive(Debug, Clone, PartialEq, Eq)]
struct M8DisplayCell {
    byte: u8,
    r: u8,
    g: u8,
    b: u8,
}

#[derive(Debug, Clone)]
pub(super) struct M8DisplayFrameCache {
    initialized: bool,
    source: String,
    cells: Vec<Option<M8DisplayCell>>,
    pending_cells: Option<Vec<Option<M8DisplayCell>>>,
    stats: FirmwareDisplayStats,
}

impl Default for M8DisplayFrameCache {
    fn default() -> Self {
        Self {
            initialized: false,
            source: String::new(),
            cells: Vec::new(),
            pending_cells: None,
            stats: FirmwareDisplayStats {
                source: "blank".to_string(),
                ..FirmwareDisplayStats::default()
            },
        }
    }
}

struct M8DisplayFrame {
    source: String,
    cells: Vec<Option<M8DisplayCell>>,
    stats: FirmwareDisplayStats,
}

pub(super) fn build_m8webdisplay_slip_frames(
    memory: &ExecutionMemory,
    cache: &mut M8DisplayFrameCache,
) -> Vec<u8> {
    let mut out = Vec::new();
    let frame = build_m8webdisplay_frame(memory, &cache.source);
    let reset = !cache.initialized
        || cache.source != frame.source
        || cache.cells.len() != frame.cells.len();
    let changed_cells = if reset {
        frame.cells.len()
    } else {
        count_changed_cells(&cache.cells, &frame.cells)
    };

    if cache.initialized
        && !reset
        && changed_cells > LARGE_RENDER_DIFF_DEFER_CELLS
        && cache.pending_cells.as_ref() != Some(&frame.cells)
    {
        cache.pending_cells = Some(frame.cells);
        return Vec::new();
    }

    if reset {
        push_slip_frame(&mut out, &[0xff, 0, 0, 0, 0, 0]);
        push_rect_frame(&mut out, 0, 0, 320, 240, 0, 0, 0);
    }

    for (index, cell) in frame.cells.iter().enumerate() {
        let previous = cache.cells.get(index).cloned().flatten();
        if !reset && previous.as_ref() == cell.as_ref() {
            continue;
        }

        let x = (index % RENDER_CACHE_WIDTH) as u16 * 8;
        let y = (index / RENDER_CACHE_WIDTH) as u16 * 10;
        if !reset && previous.is_some() {
            push_rect_frame(&mut out, x, y, 8, 10, 0, 0, 0);
        }
        if let Some(cell) = cell {
            push_text_frame(&mut out, cell.byte, x, y, cell.r, cell.g, cell.b);
        } else if reset || (cache.initialized && previous.is_some()) {
            push_text_frame(&mut out, b' ', x, y, 0, 0, 0);
        }
    }

    cache.initialized = true;
    cache.source = frame.source;
    cache.cells = frame.cells;
    cache.pending_cells = None;
    cache.stats = frame.stats;
    out
}

pub(super) fn live_display_stats(cache: &M8DisplayFrameCache) -> FirmwareDisplayStats {
    cache.stats.clone()
}

fn build_m8webdisplay_frame(memory: &ExecutionMemory, _previous_source: &str) -> M8DisplayFrame {
    let render_cells = render_cache_cells(memory);
    if render_cells.iter().any(Option::is_some) {
        let non_blank_cells = render_cells.iter().filter(|cell| cell.is_some()).count();
        return M8DisplayFrame {
            source: "render-cache".to_string(),
            cells: render_cells,
            stats: FirmwareDisplayStats {
                source: "render-cache".to_string(),
                preferred: (non_blank_cells > 0).then_some("M8 render cache".to_string()),
                format: (non_blank_cells > 0).then_some("m8-render-cache".to_string()),
                non_blank_cells,
                printable_cells: non_blank_cells,
                score: non_blank_cells * 9,
            },
        };
    }

    M8DisplayFrame {
        source: "blank".to_string(),
        cells: vec![None; RENDER_CACHE_CELLS],
        stats: FirmwareDisplayStats {
            source: "blank".to_string(),
            ..FirmwareDisplayStats::default()
        },
    }
}

fn count_changed_cells(
    previous: &[Option<M8DisplayCell>],
    next: &[Option<M8DisplayCell>],
) -> usize {
    previous
        .iter()
        .zip(next.iter())
        .filter(|(left, right)| left != right)
        .count()
        + previous.len().abs_diff(next.len())
}

fn render_cache_cells(memory: &ExecutionMemory) -> Vec<Option<M8DisplayCell>> {
    let mut cells = Vec::with_capacity(RENDER_CACHE_CELLS);
    for index in 0..RENDER_CACHE_CELLS {
        let byte = memory
            .read_u8(RENDER_CACHE_CHAR_BASE + index as u32)
            .unwrap_or(0);
        let style = memory
            .read_u16(RENDER_CACHE_STYLE_BASE + index as u32 * 2)
            .unwrap_or(0);
        if is_visible_render_cache_cell(byte, style) {
            let (r, g, b) = style_to_rgb888(style);
            cells.push(Some(M8DisplayCell { byte, r, g, b }));
        } else {
            cells.push(None);
        }
    }
    cells
}

pub(super) fn build_display_snapshot(memory: &ExecutionMemory) -> FirmwareDisplaySnapshot {
    let candidates = vec![
        build_render_cache_candidate(memory),
        build_m8_cell_grid_candidate(
            memory,
            "M8 cell grid",
            DISPLAY_GRID_BASE,
            "Observed firmware tracker-cell buffer. Each row is 0x28 bytes; each two-character cell is stored at +4/+5 with a 3-byte cell stride.",
        ),
        build_m8_cell_grid_candidate(
            memory,
            "M8 cell grid, visible rows",
            DISPLAY_GRID_VISIBLE_BASE,
            "Same cell layout, starting at the first rows that became active in the boot traces.",
        ),
        build_ascii_grid_candidate(memory, "ASCII RAM grid", 0x2001_3200),
        build_ascii_grid_candidate(memory, "ASCII render scratch", 0x2001_3360),
        build_best_ascii_grid_candidate(memory),
        build_ascii_runs_candidate(memory),
    ];
    let preferred = candidates
        .iter()
        .find(|candidate| candidate.format == "m8-render-cache" && candidate.non_blank_cells > 0)
        .or_else(|| {
            candidates.iter().find(|candidate| {
                candidate.format == "m8-cell-grid" && candidate.non_blank_cells > 0
            })
        })
        .or_else(|| candidates.iter().find(|candidate| candidate.score > 0))
        .map(|candidate| candidate.name.clone());

    FirmwareDisplaySnapshot {
        preferred,
        candidates,
    }
}

#[cfg(test)]
pub(super) fn build_memory_live_display_stats(memory: &ExecutionMemory) -> FirmwareDisplayStats {
    let render = build_render_cache_candidate(memory);
    if render.non_blank_cells > 0 {
        return display_stats_from_candidate("render-cache", &render);
    }

    FirmwareDisplayStats {
        source: "blank".to_string(),
        ..FirmwareDisplayStats::default()
    }
}

#[cfg(test)]
fn display_stats_from_candidate(
    source: &str,
    candidate: &DisplaySnapshotCandidate,
) -> FirmwareDisplayStats {
    FirmwareDisplayStats {
        source: source.to_string(),
        preferred: Some(candidate.name.clone()),
        format: Some(candidate.format.clone()),
        non_blank_cells: candidate.non_blank_cells,
        printable_cells: candidate.printable_cells,
        score: candidate.score,
    }
}

fn build_render_cache_candidate(memory: &ExecutionMemory) -> DisplaySnapshotCandidate {
    let mut rows = Vec::new();
    let mut styles = Vec::new();
    let mut printable_cells = 0;
    let mut non_blank_cells = 0;

    for row in 0..RENDER_CACHE_HEIGHT {
        let mut text = String::new();
        let mut style_row = Vec::new();
        for column in 0..RENDER_CACHE_WIDTH {
            let index = row * RENDER_CACHE_WIDTH + column;
            let char_address = RENDER_CACHE_CHAR_BASE + index as u32;
            let style_address = RENDER_CACHE_STYLE_BASE + index as u32 * 2;
            let byte = memory.read_u8(char_address).unwrap_or(0);
            let style = memory.read_u16(style_address).unwrap_or(0);
            printable_cells += usize::from(is_printable_render_cache_cell(byte, style));
            non_blank_cells += usize::from(is_visible_render_cache_cell(byte, style));
            text.push(render_cache_display_byte(byte, style));
            style_row.push(style);
        }
        rows.push(text.trim_end().to_string());
        styles.push(style_row);
    }

    DisplaySnapshotCandidate {
        name: "M8 render cache".to_string(),
        format: "m8-render-cache".to_string(),
        base: RENDER_CACHE_CHAR_BASE,
        end_exclusive: RENDER_CACHE_CHAR_BASE + RENDER_CACHE_SIZE,
        style_base: Some(RENDER_CACHE_STYLE_BASE),
        width: RENDER_CACHE_WIDTH,
        height: RENDER_CACHE_HEIGHT,
        row_stride: RENDER_CACHE_WIDTH as u32,
        cell_stride: 1,
        printable_cells,
        non_blank_cells,
        score: printable_cells + non_blank_cells * 8,
        rows,
        styles,
        notes: vec![
            "Primary preview: live 40x24 firmware render cache. Each char is one byte at 0x2001365a + y*40 + x; style is one u16le at 0x20013a1c + (y*40+x)*2.".to_string(),
            "M8WebDisplay renders the same logical display as 40x24 text cells on a 320x240 canvas; this cache is the closest in-emulator source before USB/Serial frame emission is implemented.".to_string(),
        ],
    }
}

fn build_m8_cell_grid_candidate(
    memory: &ExecutionMemory,
    name: &str,
    base: u32,
    note: &str,
) -> DisplaySnapshotCandidate {
    let mut rows = Vec::new();
    let mut printable_cells = 0;
    let mut non_blank_cells = 0;

    for row in 0..DISPLAY_GRID_ROWS {
        let row_base = base + row as u32 * DISPLAY_GRID_ROW_STRIDE;
        let mut cells = Vec::new();
        for column in 0..DISPLAY_GRID_COLUMNS {
            let cell_base =
                row_base + column as u32 * DISPLAY_GRID_CELL_STRIDE + DISPLAY_CELL_TEXT_OFFSET;
            let first = memory.read_u8(cell_base).unwrap_or(0);
            let second = memory.read_u8(cell_base + 1).unwrap_or(0);
            printable_cells += usize::from(is_printable_text_byte(first));
            printable_cells += usize::from(is_printable_text_byte(second));
            non_blank_cells += usize::from(is_visible_text_byte(first));
            non_blank_cells += usize::from(is_visible_text_byte(second));
            cells.push(format!("{}{}", display_byte(first), display_byte(second)));
        }
        rows.push(cells.join(" "));
    }

    let end_exclusive = base
        + (DISPLAY_GRID_ROWS as u32 - 1) * DISPLAY_GRID_ROW_STRIDE
        + DISPLAY_CELL_TEXT_OFFSET
        + DISPLAY_GRID_COLUMNS as u32 * DISPLAY_GRID_CELL_STRIDE;

    DisplaySnapshotCandidate {
        name: name.to_string(),
        format: "m8-cell-grid".to_string(),
        base,
        end_exclusive,
        style_base: None,
        width: DISPLAY_GRID_COLUMNS,
        height: DISPLAY_GRID_ROWS,
        row_stride: DISPLAY_GRID_ROW_STRIDE,
        cell_stride: DISPLAY_GRID_CELL_STRIDE,
        printable_cells,
        non_blank_cells,
        score: printable_cells + non_blank_cells * 4,
        rows,
        styles: Vec::new(),
        notes: vec![note.to_string()],
    }
}

fn build_ascii_grid_candidate(
    memory: &ExecutionMemory,
    name: &str,
    base: u32,
) -> DisplaySnapshotCandidate {
    build_ascii_grid_candidate_at(memory, name, base, vec!["Raw printable-byte view of a RAM window that the firmware repeatedly touches while rendering text.".to_string()])
}

fn build_best_ascii_grid_candidate(memory: &ExecutionMemory) -> DisplaySnapshotCandidate {
    let span = (ASCII_GRID_HEIGHT as u32 - 1) * ASCII_GRID_STRIDE + ASCII_GRID_WIDTH as u32;
    let mut best_base = ASCII_SCAN_START;
    let mut best_score = 0;

    let last_base = ASCII_SCAN_END.saturating_sub(span);
    let mut base = ASCII_SCAN_START;
    while base <= last_base {
        let score = ascii_grid_score(memory, base);
        if score > best_score {
            best_score = score;
            best_base = base;
        }
        base += 8;
    }

    build_ascii_grid_candidate_at(
        memory,
        "Best ASCII RAM window",
        best_base,
        vec![
            "Scanner-chosen RAM window with the highest printable text density between 0x20013000 and 0x20015000.".to_string(),
        ],
    )
}

fn build_ascii_grid_candidate_at(
    memory: &ExecutionMemory,
    name: &str,
    base: u32,
    notes: Vec<String>,
) -> DisplaySnapshotCandidate {
    let mut rows = Vec::new();
    let mut printable_cells = 0;
    let mut non_blank_cells = 0;

    for row in 0..ASCII_GRID_HEIGHT {
        let row_base = base + row as u32 * ASCII_GRID_STRIDE;
        let mut text = String::new();
        for column in 0..ASCII_GRID_WIDTH {
            let byte = memory.read_u8(row_base + column as u32).unwrap_or(0);
            printable_cells += usize::from(is_printable_text_byte(byte));
            non_blank_cells += usize::from(is_visible_text_byte(byte));
            text.push(display_byte(byte));
        }
        rows.push(text.trim_end().to_string());
    }

    DisplaySnapshotCandidate {
        name: name.to_string(),
        format: "ascii-grid".to_string(),
        base,
        end_exclusive: base
            + (ASCII_GRID_HEIGHT as u32 - 1) * ASCII_GRID_STRIDE
            + ASCII_GRID_WIDTH as u32,
        style_base: None,
        width: ASCII_GRID_WIDTH,
        height: ASCII_GRID_HEIGHT,
        row_stride: ASCII_GRID_STRIDE,
        cell_stride: 1,
        printable_cells,
        non_blank_cells,
        score: printable_cells + non_blank_cells * 3,
        rows,
        styles: Vec::new(),
        notes,
    }
}

fn build_ascii_runs_candidate(memory: &ExecutionMemory) -> DisplaySnapshotCandidate {
    let mut rows = Vec::new();
    let mut printable_cells = 0;
    let mut non_blank_cells = 0;
    let mut current_start = ASCII_SCAN_START;
    let mut current = String::new();

    for address in ASCII_SCAN_START..ASCII_SCAN_END {
        let byte = memory.read_u8(address).unwrap_or(0);
        if is_printable_text_byte(byte) && byte != b' ' {
            if current.is_empty() {
                current_start = address;
            }
            current.push(byte as char);
            continue;
        }

        flush_ascii_run(
            &mut rows,
            current_start,
            &mut current,
            &mut printable_cells,
            &mut non_blank_cells,
        );
        if rows.len() >= 24 {
            break;
        }
    }
    flush_ascii_run(
        &mut rows,
        current_start,
        &mut current,
        &mut printable_cells,
        &mut non_blank_cells,
    );

    DisplaySnapshotCandidate {
        name: "ASCII runs".to_string(),
        format: "ascii-runs".to_string(),
        base: ASCII_SCAN_START,
        end_exclusive: ASCII_SCAN_END,
        style_base: None,
        width: 0,
        height: rows.len(),
        row_stride: 0,
        cell_stride: 1,
        printable_cells,
        non_blank_cells,
        score: printable_cells + non_blank_cells * 5,
        rows,
        styles: Vec::new(),
        notes: vec![
            "Contiguous non-space printable RAM strings, useful while the exact framebuffer handoff is still being mapped.".to_string(),
        ],
    }
}

fn flush_ascii_run(
    rows: &mut Vec<String>,
    start: u32,
    current: &mut String,
    printable_cells: &mut usize,
    non_blank_cells: &mut usize,
) {
    if current.len() >= 4 {
        let text = if current.len() > 80 {
            format!("{}...", &current[..80])
        } else {
            current.clone()
        };
        *printable_cells += current.len();
        *non_blank_cells += current.len();
        rows.push(format!("0x{start:08x}: {text}"));
    }
    current.clear();
}

fn ascii_grid_score(memory: &ExecutionMemory, base: u32) -> usize {
    let mut score = 0;
    for row in 0..ASCII_GRID_HEIGHT {
        let row_base = base + row as u32 * ASCII_GRID_STRIDE;
        for column in 0..ASCII_GRID_WIDTH {
            let byte = memory.read_u8(row_base + column as u32).unwrap_or(0);
            if is_visible_text_byte(byte) {
                score += 4;
            } else if is_printable_text_byte(byte) {
                score += 1;
            }
        }
    }
    score
}

fn is_printable_text_byte(byte: u8) -> bool {
    matches!(byte, 0x20..=0x7e)
}

fn is_visible_text_byte(byte: u8) -> bool {
    is_printable_text_byte(byte) && byte != b' '
}

fn is_printable_render_cache_cell(byte: u8, style: u16) -> bool {
    style != 0 && is_printable_text_byte(byte)
}

fn is_visible_render_cache_cell(byte: u8, style: u16) -> bool {
    style != 0 && is_visible_text_byte(byte)
}

fn display_byte(byte: u8) -> char {
    if is_printable_text_byte(byte) {
        byte as char
    } else if matches!(byte, 0 | 0xff) {
        ' '
    } else {
        '.'
    }
}

fn render_cache_display_byte(byte: u8, style: u16) -> char {
    if style == 0 { ' ' } else { display_byte(byte) }
}

fn style_to_rgb888(style: u16) -> (u8, u8, u8) {
    if style == 0 {
        return (0, 0, 0);
    }

    let r = ((style >> 11) & 0x1f) as f32 * 255.0 / 31.0;
    let g = ((style >> 5) & 0x3f) as f32 * 255.0 / 63.0;
    let b = (style & 0x1f) as f32 * 255.0 / 31.0;
    let boost = 3.25;

    (
        (r * boost).round().min(255.0) as u8,
        (g * boost).round().min(255.0) as u8,
        (b * boost).round().min(255.0) as u8,
    )
}

fn push_text_frame(out: &mut Vec<u8>, byte: u8, x: u16, y: u16, r: u8, g: u8, b: u8) {
    push_slip_frame(
        out,
        &[
            0xfd,
            byte,
            (x & 0xff) as u8,
            (x >> 8) as u8,
            (y & 0xff) as u8,
            (y >> 8) as u8,
            r,
            g,
            b,
            0,
            0,
            0,
        ],
    );
}

fn push_rect_frame(out: &mut Vec<u8>, x: u16, y: u16, w: u16, h: u16, r: u8, g: u8, b: u8) {
    push_slip_frame(
        out,
        &[
            0xfe,
            (x & 0xff) as u8,
            (x >> 8) as u8,
            (y & 0xff) as u8,
            (y >> 8) as u8,
            (w & 0xff) as u8,
            (w >> 8) as u8,
            (h & 0xff) as u8,
            (h >> 8) as u8,
            r,
            g,
            b,
        ],
    );
}

fn push_slip_frame(out: &mut Vec<u8>, payload: &[u8]) {
    for byte in payload {
        match *byte {
            0xc0 => out.extend_from_slice(&[0xdb, 0xdc]),
            0xdb => out.extend_from_slice(&[0xdb, 0xdd]),
            value => out.push(value),
        }
    }
    out.push(0xc0);
}
