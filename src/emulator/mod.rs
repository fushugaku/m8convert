use std::collections::{HashMap, VecDeque};

use serde::Serialize;
use thiserror::Error;

const TEENSY_FLASH_BASE: u32 = 0x6000_0000;
const TEENSY_FLASH_SIZE: u32 = 8 * 1024 * 1024;
const ARM_XPSR_THUMB: u32 = 0x0100_0000;
const TRACE_HEAD_LIMIT: usize = 96;
const TRACE_TAIL_LIMIT: usize = 2048;
const VIRTUAL_CYCLES_PER_STEP: u64 = 1;
const SYNTHETIC_STACK_SNAPSHOT_BYTES: usize = 0x1000;
const SYNTHETIC_STACK_SNAPSHOT_BEFORE_SP: u32 = 0x0800;
const M8_USB_SERIAL_READ: u32 = 0x0003_5d78;
const M8_USB_SERIAL_PEEKCHAR: u32 = 0x0003_5e58;
const M8_USB_SERIAL_AVAILABLE: u32 = 0x0003_5e9c;
const M8_USB_SERIAL_FLUSH_INPUT: u32 = 0x0003_5eb4;
const M8_USB_SERIAL_GETCHAR: u32 = 0x0003_5f24;
const M8_USB_SERIAL_CPP_AVAILABLE: u32 = 0x0003_626c;
const M8_USB_SERIAL_CPP_PEEK: u32 = 0x0003_6294;
const M8_USB_SERIAL_CPP_READ: u32 = 0x0003_6310;
const M8_STREAM_RING_AVAILABLE: u32 = 0x0002_a654;
const M8_STREAM_RING_READ: u32 = 0x0002_a660;
const M8_STREAM_RING_PEEK: u32 = 0x0002_a680;
const M8_REMOTE_SERIAL_PARSER: u32 = 0x6004_af28;
const M8_REMOTE_SERIAL_PUMP: u32 = 0x6004_b24c;
const M8_REMOTE_SERIAL_DEFAULT_BASE: u32 = 0x2001_6428;
const M8_REMOTE_SERIAL_BUFFER_OFFSET: u32 = 0x000c;
const M8_REMOTE_SERIAL_LENGTH_OFFSET: u32 = 0x0016;
const M8_REMOTE_SERIAL_CONNECTED_OFFSET: u32 = 0x0017;
const M8_REMOTE_SERIAL_ACTIVE_OFFSET: u32 = 0x0018;
const M8_REMOTE_SERIAL_BUSY_OFFSET: u32 = 0x0019;
const M8_REMOTE_CONTROLLER_STATE_BASE: u32 = 0x2001_4870;
const M8_REMOTE_CONTROLLER_CURRENT_OFFSET: u32 = 0x000a;
const M8_REMOTE_CONTROLLER_PREVIOUS_OFFSET: u32 = 0x000b;
const M8_REMOTE_CONTROLLER_UPDATE: u32 = 0x6005_118c;
const M8_REMOTE_CONTROLLER_POLLER: u32 = 0x6004_b4a8;
const M8_REMOTE_CONTROLLER_CONSUMER: u32 = 0x6004_b580;
const M8_USB_CONFIGURATION: u32 = 0x2003_b50a;
const M8_USB_CDC_LINE_RTS_DTR: u32 = 0x2003_b510;
const M8_USB_SERIAL_RX_AVAILABLE: u32 = 0x2002_dfe0;
const M8_USB_SERIAL_RX_COUNT_BASE: u32 = 0x2002_dff0;
const M8_USB_SERIAL_RX_INDEX_BASE: u32 = 0x2002_e00c;
const M8_USB_SERIAL_RX_LIST_BASE: u32 = 0x2002_e024;
const M8_USB_SERIAL_RX_HEAD: u32 = 0x2003_b501;
const M8_USB_SERIAL_RX_TAIL: u32 = 0x2003_b503;
const M8_USB_SERIAL_RX_BUFFER_BASE: u32 = 0x2027_c660;
const M8_USB_SERIAL_RX_NUM: u8 = 8;
const M8_USB_SERIAL_RX_PACKET_SIZE: u32 = 512;
const M8_APP_CONTROLLER_HANDLER: u32 = 0x0002_80d0;
const M8_APP_EVENT_PARSER: u32 = 0x0002_82c8;
const M8_APP_INPUT_SCAN_START: u32 = 0x2001_3000;
const M8_APP_INPUT_SCAN_END: u32 = 0x2001_7000;
const M8_APP_INPUT_LANE_STRIDE: u32 = 0x0240;
const M8_APP_INPUT_LANE_COUNT: u32 = 8;
const M8_APP_INPUT_CURRENT_STATE_OFFSET: u32 = 0x016c;
const M8_APP_INPUT_PREVIOUS_STATE_OFFSET: u32 = 0x016d;
const M8_APP_INPUT_DIRTY_FLAG_OFFSET: u32 = 0x0224;
#[derive(Debug, Error)]
pub enum EmulatorError {
    #[error("firmware is not valid UTF-8 Intel HEX text")]
    Utf8,
    #[error("line {line}: expected ':' at start of Intel HEX record")]
    MissingRecordStart { line: usize },
    #[error("line {line}: invalid hex byte at offset {offset}")]
    InvalidHex { line: usize, offset: usize },
    #[error("line {line}: record length does not match byte count")]
    LengthMismatch { line: usize },
    #[error("line {line}: Intel HEX checksum mismatch")]
    Checksum { line: usize },
    #[error("line {line}: unsupported Intel HEX record type 0x{record_type:02x}")]
    UnsupportedRecord { line: usize, record_type: u8 },
    #[error("firmware contains no data records")]
    EmptyImage,
    #[error("could not find a plausible ARM vector table in the firmware image")]
    MissingVectorTable,
}

#[derive(Debug, Clone, Serialize)]
pub struct FirmwareAnalysis {
    pub format: &'static str,
    pub raw_segments: Vec<MemorySegment>,
    pub runtime_segments: Vec<MemorySegment>,
    pub byte_count: usize,
    pub min_raw_address: u32,
    pub max_raw_address: u32,
    pub runtime_base_applied: Option<u32>,
    pub start_linear_address: Option<u32>,
    pub vector_table: Option<VectorTableProbe>,
    pub boot_image: Option<TeensyBootImageProbe>,
    pub known_regions: Vec<KnownRegion>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemorySegment {
    pub start: u32,
    pub end_exclusive: u32,
    pub size: usize,
    pub region: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KnownRegion {
    pub name: &'static str,
    pub start: u32,
    pub end_exclusive: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct VectorTableProbe {
    pub address: u32,
    pub initial_sp: u32,
    pub reset_pc: u32,
    pub reset_pc_aligned: u32,
    pub initial_sp_region: Option<&'static str>,
    pub reset_pc_region: Option<&'static str>,
    pub valid_thumb_entry: bool,
    pub plausible_stack_pointer: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeensyBootImageProbe {
    pub ivt_address: u32,
    pub header: u32,
    pub entry: u32,
    pub entry_aligned: u32,
    pub entry_region: Option<&'static str>,
    pub boot_data_address: u32,
    pub self_address: u32,
    pub csf_address: u32,
    pub image_start: u32,
    pub image_size: u32,
    pub plugin_flags: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootProbe {
    pub analysis: FirmwareAnalysis,
    pub cpu: CpuState,
    pub stats: BootProbeStats,
    pub display: FirmwareDisplaySnapshot,
    pub trace: Vec<TraceEvent>,
    pub trace_total: u64,
    pub trace_truncated: bool,
    pub status: String,
}

#[derive(Clone)]
pub struct TeensyEmulatorSession {
    analysis: FirmwareAnalysis,
    cpu: CpuState,
    memory: ExecutionMemory,
    stats: ProbeStatsBuilder,
    display_cache: M8DisplayFrameCache,
    trace_head: Vec<TraceEvent>,
    trace_tail: VecDeque<TraceEvent>,
    trace_total: u64,
    status: String,
    stopped: bool,
    joypad_state: u8,
    last_note_message: Option<(u8, Option<u8>)>,
    host_display_enabled: bool,
    host_input_queue: VecDeque<u8>,
    host_input_packets: u64,
    host_input_bytes: u64,
    host_cdc_rx_next_packet: u8,
    host_cdc_rx_injected_bytes: u64,
    firmware_input_calls: u64,
    firmware_input_delivered_bytes: u64,
    last_firmware_input_endpoint: Option<&'static str>,
    last_firmware_input_callsite: Option<u32>,
    last_host_input_packet: Option<Vec<u8>>,
    observed_m8_controller_base: Option<u32>,
    observed_m8_controller_state: Option<u8>,
    observed_m8_event_parser_base: Option<u32>,
    observed_m8_event_code: Option<u32>,
    observed_m8_event_value: Option<u32>,
    observed_m8_remote_serial_base: Option<u32>,
    observed_m8_remote_serial_length: Option<u8>,
    observed_m8_remote_serial_connected: Option<bool>,
    observed_m8_remote_serial_active: Option<bool>,
    observed_m8_remote_serial_busy: Option<bool>,
    observed_m8_remote_controller_current: Option<u8>,
    observed_m8_remote_controller_previous: Option<u8>,
    remote_controller_poll_attempts: u64,
    remote_controller_poll_changes: u64,
    remote_controller_consume_successes: u64,
    remote_serial_pump_attempts: u64,
    remote_serial_pump_successes: u64,
    last_joypad_input_state: Option<u8>,
    last_joypad_input_endpoint: Option<&'static str>,
    last_joypad_input_applied: bool,
    discovered_m8_input_base: Option<u32>,
    m8_input_base_last_scan_cycle: u64,
    synthetic_m8_controller_call: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FirmwareDisplaySnapshot {
    pub preferred: Option<String>,
    pub candidates: Vec<DisplaySnapshotCandidate>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FirmwareDisplayStats {
    pub source: String,
    pub preferred: Option<String>,
    pub format: Option<String>,
    pub non_blank_cells: usize,
    pub printable_cells: usize,
    pub score: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisplaySnapshotCandidate {
    pub name: String,
    pub format: String,
    pub base: u32,
    pub end_exclusive: u32,
    pub style_base: Option<u32>,
    pub width: usize,
    pub height: usize,
    pub row_stride: u32,
    pub cell_stride: u32,
    pub printable_cells: usize,
    pub non_blank_cells: usize,
    pub score: usize,
    pub rows: Vec<String>,
    pub styles: Vec<Vec<u16>>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmulatorStepSummary {
    pub executed_steps: u32,
    pub stopped: bool,
    pub status: String,
    pub pc: u32,
    pub sp: u32,
    pub lr: u32,
    pub cycles: u64,
    pub trace_total: u64,
    pub trace_retained: usize,
    pub trace_truncated: bool,
    pub last_trace_pc: Option<u32>,
    pub last_trace_opcode16: Option<u16>,
    pub last_trace_opcode32: Option<u32>,
    pub last_trace_note: Option<String>,
    pub stop_event_pc: Option<u32>,
    pub stop_event_opcode16: Option<u16>,
    pub stop_event_opcode32: Option<u32>,
    pub stop_event_note: Option<String>,
    pub joypad_state: u8,
    pub host_input: HostInputStats,
    pub display: FirmwareDisplayStats,
    pub sd_card: VirtualSdCardStats,
    pub usdhc_commands: Vec<UsdhcCommandStats>,
    pub usdhc_controllers: Vec<UsdhcControllerStats>,
    pub usdhc_reads: Vec<AddressCount>,
    pub usdhc_writes: Vec<AddressCount>,
    pub audio: AudioStats,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HostInputStats {
    pub packets: u64,
    pub bytes: u64,
    pub queued_bytes: usize,
    pub last_packet: Vec<u8>,
    pub last_packet_label: Option<String>,
    pub firmware_input_ready: bool,
    pub firmware_input_endpoint: Option<&'static str>,
    pub firmware_input_calls: u64,
    pub firmware_input_delivered_bytes: u64,
    pub last_firmware_input_endpoint: Option<&'static str>,
    pub last_firmware_input_callsite: Option<u32>,
    pub cdc_rx_available_bytes: u32,
    pub cdc_rx_injected_bytes: u64,
    pub observed_m8_controller_base: Option<u32>,
    pub observed_m8_controller_state: Option<u8>,
    pub observed_m8_event_parser_base: Option<u32>,
    pub observed_m8_event_code: Option<u32>,
    pub observed_m8_event_value: Option<u32>,
    pub observed_m8_remote_serial_base: Option<u32>,
    pub observed_m8_remote_serial_length: Option<u8>,
    pub observed_m8_remote_serial_connected: Option<bool>,
    pub observed_m8_remote_serial_active: Option<bool>,
    pub observed_m8_remote_serial_busy: Option<bool>,
    pub observed_m8_remote_controller_current: Option<u8>,
    pub observed_m8_remote_controller_previous: Option<u8>,
    pub remote_controller_poll_attempts: u64,
    pub remote_controller_poll_changes: u64,
    pub remote_controller_consume_successes: u64,
    pub remote_serial_pump_attempts: u64,
    pub remote_serial_pump_successes: u64,
    pub last_joypad_input_state: Option<u8>,
    pub last_joypad_input_endpoint: Option<&'static str>,
    pub last_joypad_input_applied: bool,
    pub discovered_m8_input_base: Option<u32>,
    pub active_m8_input_base: Option<u32>,
    pub input_memory_current_state: Option<u8>,
    pub input_memory_previous_state: Option<u8>,
    pub input_memory_dirty_flag: Option<u8>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VirtualSdCardStats {
    pub present: bool,
    pub bytes: usize,
    pub blocks: u32,
    pub read_blocks: u64,
    pub written_blocks: u64,
    pub last_read_block: Option<u32>,
    pub last_written_block: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AudioStats {
    pub observed: bool,
    pub status: String,
    pub total_reads: u64,
    pub total_writes: u64,
    pub dma_transfers: u64,
    pub captured_pcm_words: u64,
    pub nonzero_pcm_words: u64,
    pub last_pcm_word: Option<u32>,
    pub last_nonzero_pcm_word: Option<u32>,
    pub peak_abs_sample: u16,
    pub top_reads: Vec<AddressCount>,
    pub top_writes: Vec<AddressCount>,
    pub sai_reads: Vec<AddressCount>,
    pub sai_writes: Vec<AddressCount>,
    pub dma_reads: Vec<AddressCount>,
    pub dma_writes: Vec<AddressCount>,
    pub dmamux_reads: Vec<AddressCount>,
    pub dmamux_writes: Vec<AddressCount>,
    pub usb_reads: Vec<AddressCount>,
    pub usb_writes: Vec<AddressCount>,
    pub clock_reads: Vec<AddressCount>,
    pub clock_writes: Vec<AddressCount>,
    pub active_dma_channels: Vec<AudioDmaChannelStats>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AudioDmaChannelStats {
    pub channel: u8,
    pub dmamux_source: u8,
    pub dmamux_enabled: bool,
    pub erq_enabled: bool,
    pub saddr: u32,
    pub daddr: u32,
    pub source_ring_base: u32,
    pub source_ring_bytes: u32,
    pub source_ring_nonzero_words: u32,
    pub source_ring_peak_abs_sample: u16,
    pub source_preview_words: Vec<u32>,
    pub source_label: Option<String>,
    pub nbytes: u32,
    pub citer: u16,
    pub biter: u16,
    pub csr: u16,
    pub destination_label: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BootProbeStats {
    pub external_callback_stubs: u64,
    pub exception_entries: u64,
    pub exception_returns: u64,
    pub exception_vectors: Vec<ExceptionVectorStats>,
    pub callback_targets: Vec<CallbackTargetStats>,
    pub callback_samples: Vec<CallbackSample>,
    pub pointer_sanitizations: Vec<PointerSanitizationStats>,
    pub pointer_sanitization_samples: Vec<PointerSanitizationSample>,
    pub unmapped_reads: Vec<UnmappedReadStats>,
    pub unmapped_read_samples: Vec<UnmappedReadSample>,
    pub unmapped_read_context_samples: Vec<UnmappedReadContextSample>,
    pub hot_pcs: Vec<AddressCount>,
    pub hot_edges: Vec<EdgeCount>,
    pub mmio_reads: Vec<AddressCount>,
    pub mmio_writes: Vec<AddressCount>,
    pub usdhc_reads: Vec<AddressCount>,
    pub usdhc_writes: Vec<AddressCount>,
    pub usdhc_commands: Vec<UsdhcCommandStats>,
    pub usdhc_controllers: Vec<UsdhcControllerStats>,
    pub sd_card: VirtualSdCardStats,
    pub audio: AudioStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExceptionVectorStats {
    pub exception: u16,
    pub irq: Option<u16>,
    pub handler: u32,
    pub count: u64,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallbackTargetStats {
    pub target: u32,
    pub callsite: u32,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallbackSample {
    pub target: u32,
    pub callsite: u32,
    pub registers: [u32; 16],
    pub memory_probes: Vec<CallbackMemoryProbe>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallbackMemoryProbe {
    pub register: usize,
    pub register_value: u32,
    pub offset: u32,
    pub address: u32,
    pub value: Option<u32>,
    pub bytes: Vec<MemoryByteProbe>,
    pub provenance: Option<MemoryWriteProvenance>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MemoryByteProbe {
    pub address: u32,
    pub value: Option<u8>,
    pub provenance: Option<MemoryWriteProvenance>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PointerSanitizationSample {
    pub address: u32,
    pub raw_value: u32,
    pub sanitized_value: u32,
    pub pc: u32,
    pub cycle: u64,
    pub reason: String,
    pub bytes: Vec<MemoryByteProbe>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PointerSanitizationStats {
    pub address: u32,
    pub raw_value: u32,
    pub pc: u32,
    pub count: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MemoryWriteProvenance {
    pub address: u32,
    pub size: u8,
    pub value: u32,
    pub writer_pc: u32,
    pub writer_cycle: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct UnmappedReadSample {
    pub address: u32,
    pub size: u8,
    pub pc: u32,
    pub cycle: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnmappedReadStats {
    pub address: u32,
    pub size: u8,
    pub pc: u32,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnmappedReadContextSample {
    pub address: u32,
    pub callsite: u32,
    pub registers: [u32; 16],
    pub memory_probes: Vec<CallbackMemoryProbe>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddressCount {
    pub address: u32,
    pub count: u64,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EdgeCount {
    pub from: u32,
    pub to: u32,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsdhcCommandStats {
    pub instance: &'static str,
    pub command_index: u32,
    pub app_command: bool,
    pub argument: u32,
    pub data_present: bool,
    pub count: u64,
    pub ds_addr: u32,
    pub block_attribute: u32,
    pub mix_ctrl: u32,
    pub response0: u32,
    pub int_status: u32,
    pub int_status_en: u32,
    pub int_signal_en: u32,
    pub pending_data_status: u32,
    pub data_preview: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsdhcControllerStats {
    pub instance: &'static str,
    pub int_status: u32,
    pub int_status_en: u32,
    pub int_signal_en: u32,
    pub pres_state: u32,
    pub ds_addr: u32,
    pub block_attribute: u32,
    pub mix_ctrl: u32,
    pub pending_data_status: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuState {
    pub registers: [u32; 16],
    pub fpu_s: [u32; 32],
    pub fpscr: u32,
    pub xpsr: u32,
    pub apsr: ApsrFlags,
    pub primask: u32,
    pub basepri: u32,
    pub faultmask: u32,
    pub control: u32,
    pub psp: u32,
    pub it_conditions: [u16; 4],
    pub it_remaining: u8,
    pub it_suppresses_flags: bool,
    pub exception_depth: u32,
    pub active_exception: Option<u16>,
    pub cycles: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ApsrFlags {
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub ge: u8,
}

impl ApsrFlags {
    pub(super) fn from_xpsr_bits(value: u32) -> Self {
        Self {
            n: value & 0x8000_0000 != 0,
            z: value & 0x4000_0000 != 0,
            c: value & 0x2000_0000 != 0,
            v: value & 0x1000_0000 != 0,
            ge: ((value >> 16) & 0x0f) as u8,
        }
    }

    pub(super) fn to_xpsr_bits(self) -> u32 {
        (u32::from(self.n) << 31)
            | (u32::from(self.z) << 30)
            | (u32::from(self.c) << 29)
            | (u32::from(self.v) << 28)
            | ((self.ge as u32 & 0x0f) << 16)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceEvent {
    pub cycle: u64,
    pub pc: u32,
    pub opcode16: Option<u16>,
    pub opcode32: Option<u32>,
    pub note: String,
}

#[derive(Debug, Clone)]
struct HexImage {
    records: Vec<DataRecord>,
    start_linear_address: Option<u32>,
}

#[derive(Debug, Clone)]
struct DataRecord {
    address: u32,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct RuntimeImage {
    records: Vec<DataRecord>,
    base_applied: Option<u32>,
}

mod decode;
mod display;
mod hex;
mod memory;
mod mmio;

#[cfg(test)]
mod tests;

use self::{
    decode::step_thumb_trace,
    display::{
        M8DisplayFrameCache, build_display_snapshot, build_m8webdisplay_slip_frames,
        live_display_stats,
    },
    hex::{
        address_span, known_regions, merge_segments, normalize_for_teensy_runtime, parse_intel_hex,
        probe_teensy_boot_image, probe_vector_table, read_u16,
    },
    memory::ExecutionMemory,
};

pub fn analyze_teensy_hex(input: &[u8]) -> Result<FirmwareAnalysis, EmulatorError> {
    let image = parse_intel_hex(input)?;
    let raw_segments = merge_segments(&image.records, false);
    let runtime = normalize_for_teensy_runtime(&image)?;
    let runtime_segments = merge_segments(&runtime.records, true);
    let byte_count = image.records.iter().map(|record| record.bytes.len()).sum();
    let (min_raw_address, max_raw_address) = address_span(&image.records)?;
    let vector_table = probe_vector_table(&runtime, image.start_linear_address);
    let boot_image = probe_teensy_boot_image(&runtime, image.start_linear_address);
    let mut warnings = Vec::new();

    if runtime.base_applied.is_some() {
        warnings.push(
            "HEX records used low flash offsets; runtime addresses were mirrored to Teensy FlexSPI flash at 0x60000000"
                .to_string(),
        );
    }
    if vector_table.is_none() && boot_image.is_none() {
        warnings.push(
            "No Cortex-M vector table or Teensy i.MX RT boot image was found; CPU reset cannot start yet"
                .to_string(),
        );
    } else if vector_table.is_none() && boot_image.is_some() {
        warnings.push(
            "No Cortex-M vector table was found, but a Teensy i.MX RT boot image was found; boot probe will start at the IVT entry point"
                .to_string(),
        );
    }

    Ok(FirmwareAnalysis {
        format: "intel-hex",
        raw_segments,
        runtime_segments,
        byte_count,
        min_raw_address,
        max_raw_address,
        runtime_base_applied: runtime.base_applied,
        start_linear_address: image.start_linear_address,
        vector_table,
        boot_image,
        known_regions: known_regions(),
        warnings,
    })
}

pub fn probe_teensy_hex_boot(input: &[u8], max_steps: u32) -> Result<BootProbe, EmulatorError> {
    probe_teensy_hex_boot_with_sd(input, max_steps, None)
}

pub fn probe_teensy_hex_boot_with_sd(
    input: &[u8],
    max_steps: u32,
    sd_image: Option<&[u8]>,
) -> Result<BootProbe, EmulatorError> {
    let mut session = TeensyEmulatorSession::new(input)?;
    if let Some(sd_image) = sd_image {
        session.load_sd_image(sd_image);
    }
    session.run_steps(max_steps);
    let status = if session.stopped {
        session.status.clone()
    } else {
        "boot-probe-max-steps-reached".to_string()
    };
    Ok(session.snapshot_with_status(status))
}

impl TeensyEmulatorSession {
    pub fn new(input: &[u8]) -> Result<Self, EmulatorError> {
        let image = parse_intel_hex(input)?;
        let runtime = normalize_for_teensy_runtime(&image)?;
        let analysis = analyze_teensy_hex(input)?;
        let reset_source = reset_source(&analysis).ok_or(EmulatorError::MissingVectorTable)?;

        let mut cpu = reset_cpu_state();
        cpu.registers[13] = reset_source.initial_sp.unwrap_or(0);
        cpu.registers[14] = 0xffff_ffff;
        cpu.registers[15] = reset_source.pc;

        let mut trace_head = Vec::new();
        let mut trace_tail = VecDeque::new();
        let mut trace_total = 0;
        retain_trace_event(
            &mut trace_head,
            &mut trace_tail,
            trace_reset_event(&runtime.records, &cpu, reset_source.note),
        );
        trace_total += 1;

        Ok(Self {
            analysis,
            cpu,
            memory: ExecutionMemory::new(runtime.records),
            stats: ProbeStatsBuilder::default(),
            display_cache: M8DisplayFrameCache::default(),
            trace_head,
            trace_tail,
            trace_total,
            status: "boot-probe-running".to_string(),
            stopped: false,
            joypad_state: 0,
            last_note_message: None,
            host_display_enabled: false,
            host_input_queue: VecDeque::new(),
            host_input_packets: 0,
            host_input_bytes: 0,
            host_cdc_rx_next_packet: 0,
            host_cdc_rx_injected_bytes: 0,
            firmware_input_calls: 0,
            firmware_input_delivered_bytes: 0,
            last_firmware_input_endpoint: None,
            last_firmware_input_callsite: None,
            last_host_input_packet: None,
            observed_m8_controller_base: None,
            observed_m8_controller_state: None,
            observed_m8_event_parser_base: None,
            observed_m8_event_code: None,
            observed_m8_event_value: None,
            observed_m8_remote_serial_base: None,
            observed_m8_remote_serial_length: None,
            observed_m8_remote_serial_connected: None,
            observed_m8_remote_serial_active: None,
            observed_m8_remote_serial_busy: None,
            observed_m8_remote_controller_current: None,
            observed_m8_remote_controller_previous: None,
            remote_controller_poll_attempts: 0,
            remote_controller_poll_changes: 0,
            remote_controller_consume_successes: 0,
            remote_serial_pump_attempts: 0,
            remote_serial_pump_successes: 0,
            last_joypad_input_state: None,
            last_joypad_input_endpoint: None,
            last_joypad_input_applied: false,
            discovered_m8_input_base: None,
            m8_input_base_last_scan_cycle: 0,
            synthetic_m8_controller_call: false,
        })
    }

    pub fn run_steps(&mut self, max_steps: u32) -> u32 {
        if self.stopped {
            return 0;
        }

        let steps = max_steps.clamp(1, 500_000_000);
        let mut executed = 0;
        for _ in 0..steps {
            let event = self.step_thumb_or_host_io();
            update_probe_stats(&mut self.stats, &event, &self.cpu, &self.memory);
            let should_stop = should_stop_event(&event);
            if should_stop {
                self.status = stop_status_for_event(&event).to_string();
                self.stopped = true;
            }
            retain_trace_event(&mut self.trace_head, &mut self.trace_tail, event);
            self.trace_total += 1;
            self.cpu.cycles += 1;
            self.memory.advance_virtual_cycles(VIRTUAL_CYCLES_PER_STEP);
            executed += 1;
            if should_stop {
                break;
            }
            if let Some(event) =
                maybe_enter_exception(&mut self.memory, &mut self.cpu, Some(&mut self.stats))
            {
                retain_trace_event(&mut self.trace_head, &mut self.trace_tail, event);
                self.trace_total += 1;
            }
        }
        self.refresh_host_connection_state();

        executed
    }

    pub fn run_steps_summary(&mut self, max_steps: u32) -> EmulatorStepSummary {
        let executed_steps = self.run_steps(max_steps);
        self.resolve_m8_app_input_base();
        self.step_summary(executed_steps)
    }

    pub fn run_steps_live(&mut self, max_steps: u32) -> u32 {
        if self.stopped {
            return 0;
        }

        let steps = max_steps.clamp(1, 500_000_000);
        let mut executed = 0;
        for _ in 0..steps {
            let event = self.step_thumb_or_host_io();
            let should_stop = should_stop_event(&event);
            if should_stop {
                self.status = stop_status_for_event(&event).to_string();
                self.stopped = true;
                retain_trace_event(&mut self.trace_head, &mut self.trace_tail, event);
            }
            self.trace_total += 1;
            self.cpu.cycles += 1;
            self.memory.advance_virtual_cycles(VIRTUAL_CYCLES_PER_STEP);
            executed += 1;
            if should_stop {
                break;
            }
            if let Some(event) = maybe_enter_exception(&mut self.memory, &mut self.cpu, None) {
                retain_trace_event(&mut self.trace_head, &mut self.trace_tail, event);
                self.trace_total += 1;
            }
        }
        self.refresh_host_connection_state();

        executed
    }

    pub fn run_steps_live_summary(&mut self, max_steps: u32) -> EmulatorStepSummary {
        let executed_steps = self.run_steps_live(max_steps);
        self.resolve_m8_app_input_base();
        self.step_summary(executed_steps)
    }

    pub fn take_display_slip(&mut self) -> Vec<u8> {
        build_m8webdisplay_slip_frames(&self.memory, &mut self.display_cache)
    }

    pub fn display_stats(&self) -> FirmwareDisplayStats {
        live_display_stats(&self.display_cache)
    }

    pub fn send_joypad_state(&mut self, state: u8) -> bool {
        let packet = [0x43, state];
        self.record_host_input_packet(&packet);
        self.dispatch_m8_joypad_packet(&packet, state)
    }

    pub fn load_sd_image(&mut self, bytes: &[u8]) {
        self.memory.load_sd_image(bytes);
    }

    pub fn clear_sd_image(&mut self) {
        self.memory.clear_sd_image();
    }

    pub fn sd_card_stats(&self) -> VirtualSdCardStats {
        self.memory.sd_card_stats()
    }

    pub fn audio_stats(&self) -> AudioStats {
        self.memory.audio_stats()
    }

    pub fn take_audio_pcm_words(&mut self) -> Vec<u32> {
        self.memory.take_audio_pcm_words()
    }

    pub fn sd_image_bytes(&self) -> Vec<u8> {
        self.memory.sd_image_bytes()
    }

    pub fn send_note_on(&mut self, note: u8, velocity: u8) {
        self.last_note_message = Some((note, Some(velocity)));
    }

    pub fn send_note_off(&mut self) {
        self.last_note_message = Some((0xff, None));
    }

    pub fn receive_host_bytes(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.record_host_input_packet(bytes);
            let firmware_visible = !matches!(bytes, [0x44, ..] | [0x45, ..]);
            let controller_packet = matches!(bytes, [0x43, _]);
            let remote_serial_packet = self.can_dispatch_m8_remote_serial_packet(bytes);
            if firmware_visible && !remote_serial_packet && !controller_packet {
                self.host_input_queue.extend(bytes.iter().copied());
            }
            if firmware_visible
                && !remote_serial_packet
                && !controller_packet
                && self.cpu.cycles > 0
            {
                self.inject_host_bytes_into_cdc_rx(bytes);
            }
        }
        match bytes {
            [0x43, state] => {
                self.dispatch_m8_joypad_packet(bytes, *state);
            }
            [0x4b, 0xff, ..] => {
                self.dispatch_m8_remote_serial_packet(bytes);
                self.send_note_off();
            }
            [0x4b, note, velocity, ..] => {
                self.dispatch_m8_remote_serial_packet(bytes);
                self.send_note_on(*note, *velocity);
            }
            [0x44, ..] => {
                self.host_display_enabled = false;
                self.mark_m8_remote_serial_active(false);
                self.display_cache = M8DisplayFrameCache::default();
            }
            [0x45, ..] => {
                self.host_display_enabled = true;
                self.mark_m8_remote_serial_active(true);
                self.display_cache = M8DisplayFrameCache::default();
            }
            _ => {}
        }
    }

    fn dispatch_m8_joypad_packet(&mut self, bytes: &[u8], state: u8) -> bool {
        self.joypad_state = state;
        self.last_joypad_input_state = Some(state);
        self.last_joypad_input_endpoint = None;
        self.last_joypad_input_applied = false;
        if self.cpu.cycles > 0 {
            self.write_m8_remote_controller_state(state);
        }

        let mut applied = false;
        if self.dispatch_m8_remote_serial_packet(bytes) {
            applied = true;
            self.last_joypad_input_endpoint = Some("M8 remote serial parser");
            applied |= self.pump_m8_remote_controller_update();
            applied |= self.ensure_m8_controller_shadow_state(state);
        } else {
            applied |= self.dispatch_m8_controller_state(state);
            if applied {
                self.last_joypad_input_endpoint = self.last_firmware_input_endpoint;
            }
            applied |= self.pump_m8_remote_controller_update();
        }

        self.last_joypad_input_applied = applied;
        applied
    }

    fn record_host_input_packet(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.host_input_packets = self.host_input_packets.saturating_add(1);
        self.host_input_bytes = self.host_input_bytes.saturating_add(bytes.len() as u64);
        self.last_host_input_packet = Some(bytes.to_vec());
        self.mark_usb_serial_connected();
    }

    fn can_dispatch_m8_remote_serial_packet(&self, bytes: &[u8]) -> bool {
        self.cpu.cycles > 0
            && matches!(bytes, [0x43, ..] | [0x4b, ..])
            && !bytes.is_empty()
            && (matches!(bytes, [0x43, _]) || matches!(bytes, [0x4b, ..] if bytes.len() <= 5))
            && self.memory.read_u16(M8_REMOTE_SERIAL_PARSER).is_some()
    }

    fn dispatch_m8_remote_serial_packet(&mut self, bytes: &[u8]) -> bool {
        if !self.can_dispatch_m8_remote_serial_packet(bytes) {
            return false;
        }

        let base = self
            .m8_remote_serial_base()
            .unwrap_or(M8_REMOTE_SERIAL_DEFAULT_BASE);
        self.stage_m8_remote_serial_packet_at(base, bytes);
        self.remote_serial_pump_attempts = self.remote_serial_pump_attempts.saturating_add(1);
        if !self.call_m8_remote_serial_parser_at(base) {
            return false;
        }

        self.remote_serial_pump_successes = self.remote_serial_pump_successes.saturating_add(1);
        self.observed_m8_remote_serial_base = Some(base);
        self.observed_m8_remote_serial_length =
            self.memory.read_u8(base + M8_REMOTE_SERIAL_LENGTH_OFFSET);
        self.observed_m8_remote_serial_connected = Some(true);
        self.observed_m8_remote_serial_active = Some(true);
        self.observed_m8_remote_serial_busy = self
            .memory
            .read_u8(base + M8_REMOTE_SERIAL_BUSY_OFFSET)
            .map(|value| value != 0);
        self.firmware_input_calls = self.firmware_input_calls.saturating_add(1);
        self.last_firmware_input_endpoint = Some("M8 remote serial parser");
        true
    }

    fn stage_m8_remote_serial_packet_at(&mut self, base: u32, bytes: &[u8]) {
        for offset in 0..=4 {
            let value = bytes.get(offset as usize).copied().unwrap_or(0);
            self.memory
                .write_u8(base + M8_REMOTE_SERIAL_BUFFER_OFFSET + offset, value);
        }
        self.memory
            .write_u8(base + M8_REMOTE_SERIAL_LENGTH_OFFSET, bytes.len() as u8);
        self.memory
            .write_u8(base + M8_REMOTE_SERIAL_CONNECTED_OFFSET, 1);
        self.memory
            .write_u8(base + M8_REMOTE_SERIAL_ACTIVE_OFFSET, 1);
        self.memory.write_u8(base + M8_REMOTE_SERIAL_BUSY_OFFSET, 0);
    }

    fn snapshot_synthetic_stack(&self, cpu: &CpuState) -> (u32, Vec<Option<u8>>) {
        let start = cpu.registers[13].saturating_sub(SYNTHETIC_STACK_SNAPSHOT_BEFORE_SP);
        (
            start,
            self.memory
                .snapshot_byte_range(start, SYNTHETIC_STACK_SNAPSHOT_BYTES),
        )
    }

    fn restore_synthetic_stack(&mut self, start: u32, snapshot: &[Option<u8>]) {
        self.memory.restore_byte_range(start, snapshot);
    }

    fn call_m8_remote_serial_parser_at(&mut self, base: u32) -> bool {
        let saved_cpu = self.cpu.clone();
        let (stack_start, stack_snapshot) = self.snapshot_synthetic_stack(&saved_cpu);
        let return_pc = saved_cpu.registers[15];
        self.cpu.registers[0] = base;
        self.cpu.registers[1] = 0;
        self.cpu.registers[14] = return_pc | 1;
        self.cpu.registers[15] = M8_REMOTE_SERIAL_PARSER;

        let mut returned = false;
        for _ in 0..40_000 {
            let event = self.step_thumb_or_host_io();
            self.cpu.cycles = self.cpu.cycles.saturating_add(1);
            self.memory.advance_virtual_cycles(VIRTUAL_CYCLES_PER_STEP);
            if should_stop_event(&event) {
                break;
            }
            if self.cpu.registers[15] == return_pc {
                returned = true;
                break;
            }
        }

        self.cpu = saved_cpu;
        self.restore_synthetic_stack(stack_start, &stack_snapshot);
        returned
    }

    fn call_m8_controller_handler_at(&mut self, base: u32, state: u8) -> bool {
        if self.memory.read_u16(M8_APP_CONTROLLER_HANDLER) != Some(0xe92d)
            || self.memory.read_u16(M8_APP_CONTROLLER_HANDLER + 4) != Some(0xed2d)
        {
            return false;
        }

        let saved_cpu = self.cpu.clone();
        let (stack_start, stack_snapshot) = self.snapshot_synthetic_stack(&saved_cpu);
        let return_pc = saved_cpu.registers[15];
        let saved_synthetic_call = self.synthetic_m8_controller_call;
        self.synthetic_m8_controller_call = true;
        self.cpu.registers[0] = base;
        self.cpu.registers[1] = u32::from(state);
        self.cpu.registers[2] = 0;
        self.cpu.registers[14] = return_pc | 1;
        self.cpu.registers[15] = M8_APP_CONTROLLER_HANDLER;

        let mut returned = false;
        for _ in 0..20_000 {
            let event = self.step_thumb_or_host_io();
            self.cpu.cycles = self.cpu.cycles.saturating_add(1);
            self.memory.advance_virtual_cycles(VIRTUAL_CYCLES_PER_STEP);
            if should_stop_event(&event) {
                break;
            }
            if self.cpu.registers[15] == return_pc {
                returned = true;
                break;
            }
        }

        self.cpu = saved_cpu;
        self.restore_synthetic_stack(stack_start, &stack_snapshot);
        self.synthetic_m8_controller_call = saved_synthetic_call;
        returned
    }

    fn dispatch_m8_controller_state(&mut self, state: u8) -> bool {
        self.joypad_state = state;
        if self.cpu.cycles == 0 {
            return false;
        }
        let Some(base) = self.resolve_m8_app_input_base() else {
            return false;
        };

        let m8_state = !state;
        if self.call_m8_controller_handler_at(base, state) {
            self.observed_m8_controller_base = Some(base);
            self.observed_m8_controller_state = Some(state);
            self.firmware_input_calls = self.firmware_input_calls.saturating_add(1);
            self.last_firmware_input_endpoint = Some("M8 controller handler");
            if self
                .memory
                .read_u8(base + M8_APP_INPUT_CURRENT_STATE_OFFSET)
                != Some(m8_state)
            {
                self.write_m8_input_memory_state_at(base, m8_state);
            }
            return true;
        }

        self.write_m8_input_memory_state_at(base, m8_state);
        self.observed_m8_controller_base = Some(base);
        self.observed_m8_controller_state = Some(m8_state);
        self.firmware_input_calls = self.firmware_input_calls.saturating_add(1);
        self.last_firmware_input_endpoint = Some("M8 input memory");
        true
    }

    fn ensure_m8_controller_shadow_state(&mut self, state: u8) -> bool {
        let Some(base) = self.resolve_m8_app_input_base() else {
            return false;
        };
        let m8_state = !state;
        if self
            .memory
            .read_u8(base + M8_APP_INPUT_CURRENT_STATE_OFFSET)
            != Some(m8_state)
        {
            self.write_m8_input_memory_state_at(base, m8_state);
        }
        self.observed_m8_controller_base = Some(base);
        self.observed_m8_controller_state = Some(state);
        true
    }

    fn write_m8_remote_controller_state(&mut self, state: u8) {
        self.memory.write_u8(
            M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_CURRENT_OFFSET,
            state,
        );
        self.observed_m8_remote_controller_current = Some(state);
        self.observed_m8_remote_controller_previous = self
            .memory
            .read_u8(M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_PREVIOUS_OFFSET);
    }

    fn pump_m8_remote_controller_update(&mut self) -> bool {
        if self.cpu.cycles == 0 || self.memory.read_u16(M8_REMOTE_CONTROLLER_UPDATE).is_none() {
            return false;
        }
        self.call_m8_function1_at(M8_REMOTE_CONTROLLER_UPDATE, M8_REMOTE_CONTROLLER_STATE_BASE)
            .is_some()
    }

    fn call_m8_function1_at(&mut self, address: u32, r0: u32) -> Option<u32> {
        let saved_cpu = self.cpu.clone();
        let (stack_start, stack_snapshot) = self.snapshot_synthetic_stack(&saved_cpu);
        let return_pc = saved_cpu.registers[15];
        self.cpu.registers[0] = r0;
        self.cpu.registers[14] = return_pc | 1;
        self.cpu.registers[15] = address;

        let mut returned = false;
        for _ in 0..40_000 {
            let event = self.step_thumb_or_host_io();
            self.cpu.cycles = self.cpu.cycles.saturating_add(1);
            self.memory.advance_virtual_cycles(VIRTUAL_CYCLES_PER_STEP);
            if should_stop_event(&event) {
                break;
            }
            if self.cpu.registers[15] == return_pc {
                returned = true;
                break;
            }
        }

        let return_value = self.cpu.registers[0];
        self.cpu = saved_cpu;
        self.restore_synthetic_stack(stack_start, &stack_snapshot);
        returned.then_some(return_value)
    }

    fn write_m8_input_memory_state_at(&mut self, base: u32, state: u8) {
        let previous = self
            .memory
            .read_u8(base + M8_APP_INPUT_CURRENT_STATE_OFFSET)
            .unwrap_or(0);
        self.memory
            .write_u8(base + M8_APP_INPUT_PREVIOUS_STATE_OFFSET, previous);
        self.memory
            .write_u8(base + M8_APP_INPUT_CURRENT_STATE_OFFSET, state);
        self.memory
            .write_u8(base + M8_APP_INPUT_DIRTY_FLAG_OFFSET, 1);
    }

    fn call_m8_event_parser_at(
        &mut self,
        base: u32,
        lane_offset: u32,
        event_code: u32,
        value: u32,
    ) -> bool {
        if self.memory.read_u16(M8_APP_EVENT_PARSER) != Some(0xe92d) {
            return false;
        }

        let saved_cpu = self.cpu.clone();
        let (stack_start, stack_snapshot) = self.snapshot_synthetic_stack(&saved_cpu);
        let return_pc = saved_cpu.registers[15];
        self.cpu.registers[0] = base;
        self.cpu.registers[1] = lane_offset;
        self.cpu.registers[2] = event_code;
        self.cpu.registers[3] = value;
        self.cpu.registers[14] = return_pc | 1;
        self.cpu.registers[15] = M8_APP_EVENT_PARSER;

        let mut returned = false;
        for _ in 0..40_000 {
            let event = self.step_thumb_or_host_io();
            self.cpu.cycles = self.cpu.cycles.saturating_add(1);
            self.memory.advance_virtual_cycles(VIRTUAL_CYCLES_PER_STEP);
            if should_stop_event(&event) {
                break;
            }
            if self.cpu.registers[15] == return_pc {
                returned = true;
                break;
            }
        }

        self.cpu = saved_cpu;
        self.restore_synthetic_stack(stack_start, &stack_snapshot);
        returned
    }

    fn m8_app_input_base(&self) -> Option<u32> {
        self.observed_m8_controller_base
            .or(self.observed_m8_event_parser_base)
            .or(self.discovered_m8_input_base)
            .filter(|base| self.is_plausible_m8_input_base(*base))
    }

    fn resolve_m8_app_input_base(&mut self) -> Option<u32> {
        if let Some(base) = self.m8_app_input_base() {
            return Some(base);
        }
        if self.m8_input_base_last_scan_cycle != 0
            && self
                .cpu
                .cycles
                .saturating_sub(self.m8_input_base_last_scan_cycle)
                < 1_000_000
        {
            return None;
        }
        self.m8_input_base_last_scan_cycle = self.cpu.cycles;
        self.discovered_m8_input_base = self.scan_m8_app_input_base();
        self.m8_app_input_base()
    }

    fn scan_m8_app_input_base(&self) -> Option<u32> {
        let scan_limit = M8_APP_INPUT_SCAN_END
            .saturating_sub(M8_APP_INPUT_LANE_STRIDE * (M8_APP_INPUT_LANE_COUNT - 1));
        let mut base = M8_APP_INPUT_SCAN_START;
        while base < scan_limit {
            if self.looks_like_m8_input_lane_series(base) {
                return Some(base);
            }
            base = base.saturating_add(4);
        }
        None
    }

    fn looks_like_m8_input_lane_series(&self, base: u32) -> bool {
        let Some(vtable) = self.memory.read_u32(base) else {
            return false;
        };
        if !(0x2000_0000..0x2028_0000).contains(&vtable) {
            return false;
        }
        for lane in 0..M8_APP_INPUT_LANE_COUNT {
            let lane_base = base + lane * M8_APP_INPUT_LANE_STRIDE;
            if self.memory.read_u32(lane_base) != Some(vtable) {
                return false;
            }
            if self.memory.read_u8(lane_base + 0x0c) != Some(lane as u8) {
                return false;
            }
            if !self.is_plausible_m8_input_base(lane_base) {
                return false;
            }
            let current = self
                .memory
                .read_u8(lane_base + M8_APP_INPUT_CURRENT_STATE_OFFSET)
                .unwrap_or(0);
            let previous = self
                .memory
                .read_u8(lane_base + M8_APP_INPUT_PREVIOUS_STATE_OFFSET)
                .unwrap_or(0);
            let dirty = self
                .memory
                .read_u8(lane_base + M8_APP_INPUT_DIRTY_FLAG_OFFSET)
                .unwrap_or(0);
            let dirty_next = self
                .memory
                .read_u8(lane_base + M8_APP_INPUT_DIRTY_FLAG_OFFSET + 1)
                .unwrap_or(0);
            if dirty > 1 || dirty_next > 1 {
                return false;
            }
            if lane > 0 && (current != 0xff || previous != 0xff) {
                return false;
            }
        }
        true
    }

    fn observe_m8_input_handler_entry(&mut self) {
        if self.synthetic_m8_controller_call {
            return;
        }
        let pc = self.cpu.registers[15];
        match pc {
            M8_APP_CONTROLLER_HANDLER => {
                let base = self.cpu.registers[0];
                if self.is_plausible_m8_input_base(base) {
                    self.observed_m8_controller_base = Some(base);
                    self.observed_m8_controller_state = Some(self.cpu.registers[1] as u8);
                }
            }
            M8_APP_EVENT_PARSER => {
                let base = self.cpu.registers[0];
                if self.is_plausible_m8_input_base(base) {
                    self.observed_m8_event_parser_base = Some(base);
                    self.observed_m8_event_code = Some(self.cpu.registers[2]);
                    self.observed_m8_event_value = Some(self.cpu.registers[3]);
                }
            }
            M8_REMOTE_SERIAL_PUMP | M8_REMOTE_SERIAL_PARSER => {
                self.observe_m8_remote_serial_base(self.cpu.registers[0]);
            }
            M8_REMOTE_CONTROLLER_POLLER => {
                self.remote_controller_poll_attempts =
                    self.remote_controller_poll_attempts.saturating_add(1);
                self.observed_m8_remote_controller_current = self
                    .memory
                    .read_u8(M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_CURRENT_OFFSET);
                self.observed_m8_remote_controller_previous = self.memory.read_u8(
                    M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_PREVIOUS_OFFSET,
                );
            }
            M8_REMOTE_CONTROLLER_CONSUMER => {
                self.remote_controller_consume_successes =
                    self.remote_controller_consume_successes.saturating_add(1);
                let current = self
                    .memory
                    .read_u8(M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_CURRENT_OFFSET);
                let previous = self.memory.read_u8(
                    M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_PREVIOUS_OFFSET,
                );
                if current != previous {
                    self.remote_controller_poll_changes =
                        self.remote_controller_poll_changes.saturating_add(1);
                }
                self.observed_m8_remote_controller_current = current;
                self.observed_m8_remote_controller_previous = previous;
            }
            _ => {}
        }
    }

    fn observe_m8_remote_serial_base(&mut self, base: u32) {
        if !self.is_plausible_m8_remote_serial_base(base) {
            return;
        }
        self.observed_m8_remote_serial_base = Some(base);
        self.observed_m8_remote_serial_length =
            self.memory.read_u8(base + M8_REMOTE_SERIAL_LENGTH_OFFSET);
        self.observed_m8_remote_serial_connected = self
            .memory
            .read_u8(base + M8_REMOTE_SERIAL_CONNECTED_OFFSET)
            .map(|value| value != 0);
        self.observed_m8_remote_serial_active = self
            .memory
            .read_u8(base + M8_REMOTE_SERIAL_ACTIVE_OFFSET)
            .map(|value| value != 0);
        self.observed_m8_remote_serial_busy = self
            .memory
            .read_u8(base + M8_REMOTE_SERIAL_BUSY_OFFSET)
            .map(|value| value != 0);
    }

    fn is_plausible_m8_input_base(&self, base: u32) -> bool {
        (0x2000_0000..0x2028_0000).contains(&base)
            && self
                .memory
                .read_u8(base + M8_APP_INPUT_CURRENT_STATE_OFFSET)
                .is_some()
            && self
                .memory
                .read_u8(base + M8_APP_INPUT_DIRTY_FLAG_OFFSET)
                .is_some()
    }

    fn m8_remote_serial_base(&self) -> Option<u32> {
        self.observed_m8_remote_serial_base
            .filter(|base| self.is_plausible_m8_remote_serial_base(*base))
            .or_else(|| {
                self.is_plausible_m8_remote_serial_base(M8_REMOTE_SERIAL_DEFAULT_BASE)
                    .then_some(M8_REMOTE_SERIAL_DEFAULT_BASE)
            })
    }

    fn is_plausible_m8_remote_serial_base(&self, base: u32) -> bool {
        if !(0x2000_0000..0x2028_0000).contains(&base) {
            return false;
        }
        let Some(length) = self.memory.read_u8(base + M8_REMOTE_SERIAL_LENGTH_OFFSET) else {
            return false;
        };
        if length > 5 {
            return false;
        }
        for offset in 0..=4 {
            if self
                .memory
                .read_u8(base + M8_REMOTE_SERIAL_BUFFER_OFFSET + offset)
                .is_none()
            {
                return false;
            }
        }
        self.memory
            .read_u8(base + M8_REMOTE_SERIAL_CONNECTED_OFFSET)
            .is_some()
            && self
                .memory
                .read_u8(base + M8_REMOTE_SERIAL_ACTIVE_OFFSET)
                .is_some()
            && self
                .memory
                .read_u8(base + M8_REMOTE_SERIAL_BUSY_OFFSET)
                .is_some()
    }

    #[doc(hidden)]
    pub fn debug_poke_u8(&mut self, address: u32, value: u8) {
        self.memory.write_u8(address, value);
    }

    #[doc(hidden)]
    pub fn debug_read_u8(&self, address: u32) -> Option<u8> {
        self.memory.read_u8(address)
    }

    #[doc(hidden)]
    pub fn debug_call_m8_controller_handler_at(&mut self, base: u32, state: u8) -> bool {
        self.call_m8_controller_handler_at(base, state)
    }

    #[doc(hidden)]
    pub fn debug_call_m8_event_parser_at(
        &mut self,
        base: u32,
        lane_offset: u32,
        event_code: u32,
        value: u32,
    ) -> bool {
        self.call_m8_event_parser_at(base, lane_offset, event_code, value)
    }

    pub fn host_input_stats(&self) -> HostInputStats {
        let last_packet = self.last_host_input_packet.clone().unwrap_or_default();
        let remote_serial_base = self.m8_remote_serial_base();
        let active_input_base = self.m8_app_input_base();
        HostInputStats {
            packets: self.host_input_packets,
            bytes: self.host_input_bytes,
            queued_bytes: self.host_input_queue.len(),
            last_packet_label: label_host_input_packet(&last_packet),
            last_packet,
            firmware_input_ready: self.firmware_host_input_supported(),
            firmware_input_endpoint: self
                .firmware_host_input_supported()
                .then_some("Teensy USB CDC Serial"),
            firmware_input_calls: self.firmware_input_calls,
            firmware_input_delivered_bytes: self.firmware_input_delivered_bytes,
            last_firmware_input_endpoint: self.last_firmware_input_endpoint,
            last_firmware_input_callsite: self.last_firmware_input_callsite,
            cdc_rx_available_bytes: self.cdc_rx_available(),
            cdc_rx_injected_bytes: self.host_cdc_rx_injected_bytes,
            observed_m8_controller_base: self.observed_m8_controller_base,
            observed_m8_controller_state: self.observed_m8_controller_state,
            observed_m8_event_parser_base: self.observed_m8_event_parser_base,
            observed_m8_event_code: self.observed_m8_event_code,
            observed_m8_event_value: self.observed_m8_event_value,
            observed_m8_remote_serial_base: remote_serial_base,
            observed_m8_remote_serial_length: remote_serial_base
                .and_then(|base| self.memory.read_u8(base + M8_REMOTE_SERIAL_LENGTH_OFFSET)),
            observed_m8_remote_serial_connected: remote_serial_base.and_then(|base| {
                self.memory
                    .read_u8(base + M8_REMOTE_SERIAL_CONNECTED_OFFSET)
                    .map(|value| value != 0)
            }),
            observed_m8_remote_serial_active: remote_serial_base.and_then(|base| {
                self.memory
                    .read_u8(base + M8_REMOTE_SERIAL_ACTIVE_OFFSET)
                    .map(|value| value != 0)
            }),
            observed_m8_remote_serial_busy: remote_serial_base.and_then(|base| {
                self.memory
                    .read_u8(base + M8_REMOTE_SERIAL_BUSY_OFFSET)
                    .map(|value| value != 0)
            }),
            observed_m8_remote_controller_current: self
                .memory
                .read_u8(M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_CURRENT_OFFSET)
                .or(self.observed_m8_remote_controller_current),
            observed_m8_remote_controller_previous: self
                .memory
                .read_u8(M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_PREVIOUS_OFFSET)
                .or(self.observed_m8_remote_controller_previous),
            remote_controller_poll_attempts: self.remote_controller_poll_attempts,
            remote_controller_poll_changes: self.remote_controller_poll_changes,
            remote_controller_consume_successes: self.remote_controller_consume_successes,
            remote_serial_pump_attempts: self.remote_serial_pump_attempts,
            remote_serial_pump_successes: self.remote_serial_pump_successes,
            last_joypad_input_state: self.last_joypad_input_state,
            last_joypad_input_endpoint: self.last_joypad_input_endpoint,
            last_joypad_input_applied: self.last_joypad_input_applied,
            discovered_m8_input_base: self.discovered_m8_input_base,
            active_m8_input_base: active_input_base,
            input_memory_current_state: active_input_base.and_then(|base| {
                self.memory
                    .read_u8(base + M8_APP_INPUT_CURRENT_STATE_OFFSET)
            }),
            input_memory_previous_state: active_input_base.and_then(|base| {
                self.memory
                    .read_u8(base + M8_APP_INPUT_PREVIOUS_STATE_OFFSET)
            }),
            input_memory_dirty_flag: active_input_base
                .and_then(|base| self.memory.read_u8(base + M8_APP_INPUT_DIRTY_FLAG_OFFSET)),
        }
    }

    pub fn snapshot(&self) -> BootProbe {
        self.snapshot_with_status(self.status.clone())
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    fn step_summary(&self, executed_steps: u32) -> EmulatorStepSummary {
        let trace_retained = self.trace_head.len() + self.trace_tail.len();
        let last_trace = self.trace_tail.back().or_else(|| self.trace_head.last());
        let stop_event = self.stopped.then_some(last_trace).flatten();
        EmulatorStepSummary {
            executed_steps,
            stopped: self.stopped,
            status: self.status.clone(),
            pc: self.cpu.registers[15],
            sp: self.cpu.registers[13],
            lr: self.cpu.registers[14],
            cycles: self.cpu.cycles,
            trace_total: self.trace_total,
            trace_retained,
            trace_truncated: self.trace_total as usize > trace_retained,
            last_trace_pc: last_trace.map(|event| event.pc),
            last_trace_opcode16: last_trace.and_then(|event| event.opcode16),
            last_trace_opcode32: last_trace.and_then(|event| event.opcode32),
            last_trace_note: last_trace.map(|event| event.note.clone()),
            stop_event_pc: stop_event.map(|event| event.pc),
            stop_event_opcode16: stop_event.and_then(|event| event.opcode16),
            stop_event_opcode32: stop_event.and_then(|event| event.opcode32),
            stop_event_note: stop_event.map(|event| event.note.clone()),
            joypad_state: self.joypad_state,
            host_input: self.host_input_stats(),
            display: live_display_stats(&self.display_cache),
            sd_card: self.memory.sd_card_stats(),
            usdhc_commands: self.memory.recent_usdhc_commands(8),
            usdhc_controllers: self.memory.usdhc_controller_stats(),
            usdhc_reads: self.memory.top_usdhc_reads(8),
            usdhc_writes: self.memory.top_usdhc_writes(8),
            audio: self.memory.audio_stats(),
        }
    }

    fn snapshot_with_status(&self, status: String) -> BootProbe {
        let mut trace = self.trace_head.clone();
        trace.extend(self.trace_tail.iter().cloned());
        let trace_truncated = self.trace_total as usize > trace.len();

        BootProbe {
            analysis: self.analysis.clone(),
            cpu: self.cpu.clone(),
            stats: self.stats.snapshot(&self.memory),
            display: build_display_snapshot(&self.memory),
            trace,
            trace_total: self.trace_total,
            trace_truncated,
            status,
        }
    }

    fn step_thumb_or_host_io(&mut self) -> TraceEvent {
        self.observe_m8_input_handler_entry();
        if let Some(event) = self.intercept_m8_usb_serial_call() {
            return event;
        }
        step_thumb_trace(&mut self.memory, &mut self.cpu)
    }

    fn intercept_m8_usb_serial_call(&mut self) -> Option<TraceEvent> {
        let pc = self.cpu.registers[15];
        match pc {
            M8_USB_SERIAL_AVAILABLE => {
                Some(self.return_host_input_available("raw usb_serial_available"))
            }
            M8_USB_SERIAL_READ => {
                let buffer = self.cpu.registers[0];
                let requested = self.cpu.registers[1] as usize;
                let count =
                    self.copy_host_input_to_firmware(buffer, requested, "raw usb_serial_read");
                Some(self.return_from_host_io_call(
                    count as u32,
                    format!(
                        "M8 USB Serial.read shim ; copied {count}/{requested} byte(s) to 0x{buffer:08x}"
                    ),
                ))
            }
            M8_USB_SERIAL_GETCHAR => Some(self.return_host_input_byte("raw usb_serial_getchar")),
            M8_USB_SERIAL_PEEKCHAR => Some(self.return_host_input_peek("raw usb_serial_peek")),
            M8_USB_SERIAL_FLUSH_INPUT => {
                let dropped = self.host_input_queue.len();
                self.host_input_queue.clear();
                Some(self.return_from_host_io_call(
                    0,
                    format!("M8 USB Serial.flush_input shim ; dropped {dropped} byte(s)"),
                ))
            }
            M8_USB_SERIAL_CPP_AVAILABLE => {
                Some(self.return_host_input_available("USBSerial::available"))
            }
            M8_USB_SERIAL_CPP_READ => Some(self.return_host_input_byte("USBSerial::read")),
            M8_USB_SERIAL_CPP_PEEK => Some(self.return_host_input_peek("USBSerial::peek")),
            M8_STREAM_RING_AVAILABLE => {
                Some(self.return_host_input_available("Stream ring available"))
            }
            M8_STREAM_RING_READ => Some(self.return_host_input_byte("Stream ring read")),
            M8_STREAM_RING_PEEK => Some(self.return_host_input_peek("Stream ring peek")),
            _ => None,
        }
    }

    fn return_host_input_available(&mut self, endpoint: &'static str) -> TraceEvent {
        self.sync_host_queue_to_cdc_rx();
        self.firmware_input_calls = self.firmware_input_calls.saturating_add(1);
        self.last_firmware_input_endpoint = Some(endpoint);
        self.last_firmware_input_callsite = Some(self.cpu.registers[14] & !1);
        let available = self
            .host_input_queue
            .len()
            .max(self.cdc_rx_available() as usize)
            .min(i32::MAX as usize) as u32;
        self.return_from_host_io_call(
            available,
            format!("M8 input {endpoint} shim ; {available} byte(s) queued"),
        )
    }

    fn return_host_input_byte(&mut self, endpoint: &'static str) -> TraceEvent {
        self.sync_host_queue_to_cdc_rx();
        self.firmware_input_calls = self.firmware_input_calls.saturating_add(1);
        self.last_firmware_input_endpoint = Some(endpoint);
        self.last_firmware_input_callsite = Some(self.cpu.registers[14] & !1);
        let value = self
            .host_input_queue
            .pop_front()
            .map(|byte| {
                self.pop_cdc_rx_byte();
                self.firmware_input_delivered_bytes =
                    self.firmware_input_delivered_bytes.saturating_add(1);
                u32::from(byte)
            })
            .or_else(|| {
                self.pop_cdc_rx_byte().map(|byte| {
                    self.firmware_input_delivered_bytes =
                        self.firmware_input_delivered_bytes.saturating_add(1);
                    u32::from(byte)
                })
            })
            .unwrap_or(u32::MAX);
        let note = if value == u32::MAX {
            format!("M8 input {endpoint} shim ; no byte queued")
        } else {
            format!("M8 input {endpoint} shim ; returned 0x{value:02x}")
        };
        self.return_from_host_io_call(value, note)
    }

    fn return_host_input_peek(&mut self, endpoint: &'static str) -> TraceEvent {
        self.sync_host_queue_to_cdc_rx();
        self.firmware_input_calls = self.firmware_input_calls.saturating_add(1);
        self.last_firmware_input_endpoint = Some(endpoint);
        self.last_firmware_input_callsite = Some(self.cpu.registers[14] & !1);
        let value = self
            .host_input_queue
            .front()
            .copied()
            .or_else(|| self.peek_cdc_rx_byte())
            .map(u32::from)
            .unwrap_or(u32::MAX);
        let note = if value == u32::MAX {
            format!("M8 input {endpoint} shim ; no byte queued")
        } else {
            format!("M8 input {endpoint} shim ; saw 0x{value:02x}")
        };
        self.return_from_host_io_call(value, note)
    }

    fn copy_host_input_to_firmware(
        &mut self,
        buffer: u32,
        requested: usize,
        endpoint: &'static str,
    ) -> usize {
        self.sync_host_queue_to_cdc_rx();
        self.firmware_input_calls = self.firmware_input_calls.saturating_add(1);
        self.last_firmware_input_endpoint = Some(endpoint);
        self.last_firmware_input_callsite = Some(self.cpu.registers[14] & !1);
        let count = requested.min(
            self.host_input_queue
                .len()
                .max(self.cdc_rx_available() as usize),
        );
        self.memory
            .set_current_instruction(self.cpu.registers[15], self.cpu.cycles);
        for offset in 0..count {
            let byte = self
                .host_input_queue
                .pop_front()
                .inspect(|_| {
                    self.pop_cdc_rx_byte();
                })
                .or_else(|| self.pop_cdc_rx_byte());
            let Some(byte) = byte else { break };
            self.memory
                .write_u8(buffer.wrapping_add(offset as u32), byte);
        }
        self.firmware_input_delivered_bytes = self
            .firmware_input_delivered_bytes
            .saturating_add(count as u64);
        count
    }

    fn return_from_host_io_call(&mut self, return_value: u32, note: String) -> TraceEvent {
        let pc = self.cpu.registers[15];
        self.memory.set_current_instruction(pc, self.cpu.cycles);
        self.cpu.registers[0] = return_value;
        self.cpu.registers[15] = self.cpu.registers[14] & !1;
        TraceEvent {
            cycle: self.cpu.cycles,
            pc,
            opcode16: self.memory.read_u16(pc).or(Some(0xbf00)),
            opcode32: None,
            note,
        }
    }

    fn mark_usb_serial_connected(&mut self) {
        self.memory.write_u8(M8_USB_CONFIGURATION, 1);
        self.memory.write_u8(M8_USB_CDC_LINE_RTS_DTR, 0x03);
        self.refresh_host_connection_state();
    }

    fn refresh_host_connection_state(&mut self) {
        if self.host_display_enabled {
            self.mark_m8_remote_serial_active(true);
        }
    }

    fn mark_m8_remote_serial_active(&mut self, active: bool) {
        let base = self
            .m8_remote_serial_base()
            .unwrap_or(M8_REMOTE_SERIAL_DEFAULT_BASE);
        if self
            .memory
            .read_u8(base + M8_REMOTE_SERIAL_LENGTH_OFFSET)
            .is_none()
        {
            self.memory
                .write_u8(base + M8_REMOTE_SERIAL_LENGTH_OFFSET, 0);
        }
        for offset in 0..=4 {
            if self
                .memory
                .read_u8(base + M8_REMOTE_SERIAL_BUFFER_OFFSET + offset)
                .is_none()
            {
                self.memory
                    .write_u8(base + M8_REMOTE_SERIAL_BUFFER_OFFSET + offset, 0);
            }
        }
        self.memory
            .write_u8(base + M8_REMOTE_SERIAL_CONNECTED_OFFSET, 1);
        self.memory
            .write_u8(base + M8_REMOTE_SERIAL_ACTIVE_OFFSET, u8::from(active));
        self.memory.write_u8(base + M8_REMOTE_SERIAL_BUSY_OFFSET, 0);
    }

    fn inject_host_bytes_into_cdc_rx(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(M8_USB_SERIAL_RX_PACKET_SIZE as usize) {
            self.inject_host_cdc_packet(chunk);
        }
    }

    fn inject_host_cdc_packet(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.memory
            .set_current_instruction(self.cpu.registers[15], self.cpu.cycles);
        let head = self.memory.read_u8(M8_USB_SERIAL_RX_HEAD).unwrap_or(0);
        let tail = self.memory.read_u8(M8_USB_SERIAL_RX_TAIL).unwrap_or(0);
        if head != tail {
            let packet = self
                .memory
                .read_u8(M8_USB_SERIAL_RX_LIST_BASE + u32::from(head))
                .unwrap_or(0)
                % M8_USB_SERIAL_RX_NUM;
            let count_addr = M8_USB_SERIAL_RX_COUNT_BASE + u32::from(packet) * 2;
            let count = self.memory.read_u16(count_addr).unwrap_or(0);
            let free = M8_USB_SERIAL_RX_PACKET_SIZE.saturating_sub(u32::from(count));
            if free >= bytes.len() as u32 {
                let base =
                    M8_USB_SERIAL_RX_BUFFER_BASE + u32::from(packet) * M8_USB_SERIAL_RX_PACKET_SIZE;
                for (offset, byte) in bytes.iter().copied().enumerate() {
                    self.memory
                        .write_u8(base + u32::from(count) + offset as u32, byte);
                }
                self.memory
                    .write_u16(count_addr, count.saturating_add(bytes.len() as u16));
                self.add_cdc_rx_available(bytes.len() as u32);
                self.host_cdc_rx_injected_bytes = self
                    .host_cdc_rx_injected_bytes
                    .saturating_add(bytes.len() as u64);
                return;
            }
        }

        let next_head = next_cdc_ring_slot(head);
        if next_head == tail {
            return;
        }
        let packet = self.host_cdc_rx_next_packet % M8_USB_SERIAL_RX_NUM;
        self.host_cdc_rx_next_packet = (packet + 1) % M8_USB_SERIAL_RX_NUM;
        let base = M8_USB_SERIAL_RX_BUFFER_BASE + u32::from(packet) * M8_USB_SERIAL_RX_PACKET_SIZE;
        for (offset, byte) in bytes.iter().copied().enumerate() {
            self.memory.write_u8(base + offset as u32, byte);
        }
        self.memory.write_u16(
            M8_USB_SERIAL_RX_COUNT_BASE + u32::from(packet) * 2,
            bytes.len() as u16,
        );
        self.memory
            .write_u16(M8_USB_SERIAL_RX_INDEX_BASE + u32::from(packet) * 2, 0);
        self.memory
            .write_u8(M8_USB_SERIAL_RX_LIST_BASE + u32::from(next_head), packet);
        self.memory.write_u8(M8_USB_SERIAL_RX_HEAD, next_head);
        self.add_cdc_rx_available(bytes.len() as u32);
        self.host_cdc_rx_injected_bytes = self
            .host_cdc_rx_injected_bytes
            .saturating_add(bytes.len() as u64);
    }

    fn cdc_rx_available(&self) -> u32 {
        self.memory
            .read_u32(M8_USB_SERIAL_RX_AVAILABLE)
            .unwrap_or(0)
    }

    fn add_cdc_rx_available(&mut self, amount: u32) {
        let available = self.cdc_rx_available().saturating_add(amount);
        self.memory.write_u32(M8_USB_SERIAL_RX_AVAILABLE, available);
    }

    fn subtract_cdc_rx_available(&mut self, amount: u32) {
        let available = self.cdc_rx_available().saturating_sub(amount);
        self.memory.write_u32(M8_USB_SERIAL_RX_AVAILABLE, available);
    }

    fn sync_host_queue_to_cdc_rx(&mut self) {
        if self.host_cdc_rx_injected_bytes == 0 {
            return;
        }
        let available = self.cdc_rx_available() as usize;
        while self.host_input_queue.len() > available {
            self.host_input_queue.pop_front();
        }
    }

    fn peek_cdc_rx_byte(&self) -> Option<u8> {
        let tail = self.memory.read_u8(M8_USB_SERIAL_RX_TAIL).unwrap_or(0);
        let head = self.memory.read_u8(M8_USB_SERIAL_RX_HEAD).unwrap_or(0);
        if tail == head {
            return None;
        }
        let next_tail = next_cdc_ring_slot(tail);
        let packet = self
            .memory
            .read_u8(M8_USB_SERIAL_RX_LIST_BASE + u32::from(next_tail))
            .unwrap_or(0)
            % M8_USB_SERIAL_RX_NUM;
        let index = self
            .memory
            .read_u16(M8_USB_SERIAL_RX_INDEX_BASE + u32::from(packet) * 2)
            .unwrap_or(0);
        let count = self
            .memory
            .read_u16(M8_USB_SERIAL_RX_COUNT_BASE + u32::from(packet) * 2)
            .unwrap_or(0);
        if index >= count {
            return None;
        }
        self.memory.read_u8(
            M8_USB_SERIAL_RX_BUFFER_BASE
                + u32::from(packet) * M8_USB_SERIAL_RX_PACKET_SIZE
                + u32::from(index),
        )
    }

    fn pop_cdc_rx_byte(&mut self) -> Option<u8> {
        let tail = self.memory.read_u8(M8_USB_SERIAL_RX_TAIL).unwrap_or(0);
        let head = self.memory.read_u8(M8_USB_SERIAL_RX_HEAD).unwrap_or(0);
        if tail == head {
            return None;
        }
        let next_tail = next_cdc_ring_slot(tail);
        let packet = self
            .memory
            .read_u8(M8_USB_SERIAL_RX_LIST_BASE + u32::from(next_tail))
            .unwrap_or(0)
            % M8_USB_SERIAL_RX_NUM;
        let index_addr = M8_USB_SERIAL_RX_INDEX_BASE + u32::from(packet) * 2;
        let count_addr = M8_USB_SERIAL_RX_COUNT_BASE + u32::from(packet) * 2;
        let index = self.memory.read_u16(index_addr).unwrap_or(0);
        let count = self.memory.read_u16(count_addr).unwrap_or(0);
        if index >= count {
            self.memory.write_u8(M8_USB_SERIAL_RX_TAIL, next_tail);
            return None;
        }
        let byte = self.memory.read_u8(
            M8_USB_SERIAL_RX_BUFFER_BASE
                + u32::from(packet) * M8_USB_SERIAL_RX_PACKET_SIZE
                + u32::from(index),
        );
        let next_index = index.saturating_add(1);
        if next_index >= count {
            self.memory.write_u16(index_addr, 0);
            self.memory.write_u16(count_addr, 0);
            self.memory.write_u8(M8_USB_SERIAL_RX_TAIL, next_tail);
        } else {
            self.memory.write_u16(index_addr, next_index);
        }
        if byte.is_some() {
            self.subtract_cdc_rx_available(1);
        }
        byte
    }

    fn firmware_host_input_supported(&self) -> bool {
        self.memory.read_u16(M8_USB_SERIAL_READ) == Some(0xe92d)
            && self.memory.read_u16(M8_USB_SERIAL_AVAILABLE) == Some(0x4b04)
            && self.memory.read_u16(M8_USB_SERIAL_GETCHAR) == Some(0xb500)
    }
}

fn next_cdc_ring_slot(value: u8) -> u8 {
    if value >= M8_USB_SERIAL_RX_NUM {
        0
    } else {
        value + 1
    }
}

fn label_host_input_packet(bytes: &[u8]) -> Option<String> {
    match bytes {
        [0x43, state, ..] => Some(format!("controller 0b{state:08b}")),
        [0x4b, 0xff, ..] => Some("keyjazz note off".to_string()),
        [0x4b, note, velocity, ..] => Some(format!("keyjazz note {note} velocity {velocity}")),
        [0x44, ..] => Some("display disconnect".to_string()),
        [0x45, ..] => Some("display enable".to_string()),
        [0x52, ..] => Some("display reset".to_string()),
        _ if bytes.is_empty() => None,
        _ => Some(format!("{} raw bytes", bytes.len())),
    }
}

fn reset_cpu_state() -> CpuState {
    CpuState {
        registers: [0; 16],
        fpu_s: [0; 32],
        fpscr: 0,
        xpsr: ARM_XPSR_THUMB,
        apsr: ApsrFlags::default(),
        primask: 0,
        basepri: 0,
        faultmask: 0,
        control: 0,
        psp: 0,
        it_conditions: [0; 4],
        it_remaining: 0,
        it_suppresses_flags: false,
        exception_depth: 0,
        active_exception: None,
        cycles: 0,
    }
}

#[derive(Clone, Default)]
struct ProbeStatsBuilder {
    external_callback_stubs: u64,
    exception_entries: u64,
    exception_returns: u64,
    exception_vectors: HashMap<(u16, u32), u64>,
    callback_targets: HashMap<(u32, u32), u64>,
    callback_samples: Vec<CallbackSample>,
    unmapped_read_context_samples: Vec<UnmappedReadContextSample>,
    pc_counts: HashMap<u32, u64>,
    edge_counts: HashMap<(u32, u32), u64>,
}

impl ProbeStatsBuilder {
    fn snapshot(&self, memory: &ExecutionMemory) -> BootProbeStats {
        self.clone().finish(memory)
    }

    fn finish(self, memory: &ExecutionMemory) -> BootProbeStats {
        BootProbeStats {
            external_callback_stubs: self.external_callback_stubs,
            exception_entries: self.exception_entries,
            exception_returns: self.exception_returns,
            exception_vectors: top_exception_vectors(self.exception_vectors, 16),
            callback_targets: top_callback_targets(self.callback_targets, 16),
            callback_samples: self.callback_samples,
            pointer_sanitizations: memory.top_pointer_sanitizations(16),
            pointer_sanitization_samples: memory.pointer_sanitization_samples(),
            unmapped_reads: memory.top_unmapped_reads(16),
            unmapped_read_samples: memory.unmapped_read_samples(),
            unmapped_read_context_samples: self.unmapped_read_context_samples,
            hot_pcs: top_address_counts(self.pc_counts, 16),
            hot_edges: top_edge_counts(self.edge_counts, 16),
            mmio_reads: memory.top_mmio_reads(256),
            mmio_writes: memory.top_mmio_writes(256),
            usdhc_reads: memory.top_usdhc_reads(32),
            usdhc_writes: memory.top_usdhc_writes(32),
            usdhc_commands: memory.top_usdhc_commands(32),
            usdhc_controllers: memory.usdhc_controller_stats(),
            sd_card: memory.sd_card_stats(),
            audio: memory.audio_stats(),
        }
    }
}

fn update_probe_stats(
    stats: &mut ProbeStatsBuilder,
    event: &TraceEvent,
    cpu: &CpuState,
    memory: &ExecutionMemory,
) {
    *stats.pc_counts.entry(event.pc).or_default() += 1;
    *stats
        .edge_counts
        .entry((event.pc, cpu.registers[15]))
        .or_default() += 1;

    if event.note.starts_with("Exception return") {
        stats.exception_returns = stats.exception_returns.saturating_add(1);
    }

    if event.note.contains("stubbed external callback") {
        stats.external_callback_stubs = stats.external_callback_stubs.saturating_add(1);
        if let Some(target) = parse_hex_after(&event.note, "target 0x") {
            *stats
                .callback_targets
                .entry((target, event.pc))
                .or_default() += 1;
            if stats.callback_samples.len() < 8 {
                stats.callback_samples.push(CallbackSample {
                    target,
                    callsite: event.pc,
                    registers: cpu.registers,
                    memory_probes: memory.callback_memory_probes(&cpu.registers),
                });
            }
        }
    }

    if event.note.contains("unmapped data read")
        && stats.unmapped_read_context_samples.len() < 8
        && let Some(address) = parse_hex_after(&event.note, "unmapped data read 0x")
    {
        stats
            .unmapped_read_context_samples
            .push(UnmappedReadContextSample {
                address,
                callsite: event.pc,
                registers: cpu.registers,
                memory_probes: memory.callback_memory_probes(&cpu.registers),
            });
    }
}

fn maybe_enter_exception(
    memory: &mut ExecutionMemory,
    cpu: &mut CpuState,
    stats: Option<&mut ProbeStatsBuilder>,
) -> Option<TraceEvent> {
    memory.update_periodic_pending();

    if cpu.exception_depth != 0 || cpu.primask != 0 || cpu.faultmask != 0 {
        return None;
    }

    let pending = memory.next_pending_exception(cpu.basepri)?;
    let handler = memory.exception_handler(pending.exception)?;
    let return_pc = cpu.registers[15];
    let return_lr = cpu.registers[14];
    let return_xpsr = (cpu.xpsr & !0xf00f_0000) | cpu.apsr.to_xpsr_bits();
    let sp = cpu.registers[13].wrapping_sub(32);

    memory.write_u32(sp, cpu.registers[0]);
    memory.write_u32(sp + 4, cpu.registers[1]);
    memory.write_u32(sp + 8, cpu.registers[2]);
    memory.write_u32(sp + 12, cpu.registers[3]);
    memory.write_u32(sp + 16, cpu.registers[12]);
    memory.write_u32(sp + 20, return_lr);
    memory.write_u32(sp + 24, return_pc);
    memory.write_u32(sp + 28, return_xpsr);

    cpu.registers[13] = sp;
    cpu.registers[14] = EXCEPTION_RETURN_THREAD_MSP;
    cpu.registers[15] = handler;
    cpu.xpsr = ARM_XPSR_THUMB | u32::from(pending.exception);
    cpu.it_conditions = [0; 4];
    cpu.it_remaining = 0;
    cpu.it_suppresses_flags = false;
    cpu.exception_depth = 1;
    cpu.active_exception = Some(pending.exception);
    memory.accept_exception(pending.exception);

    if let Some(stats) = stats {
        stats.exception_entries = stats.exception_entries.saturating_add(1);
        *stats
            .exception_vectors
            .entry((pending.exception, handler))
            .or_default() += 1;
    }

    Some(TraceEvent {
        cycle: cpu.cycles,
        pc: return_pc,
        opcode16: Some(0),
        opcode32: None,
        note: format!(
            "Exception entry {} ; handler 0x{handler:08x}, stacked PC=0x{return_pc:08x}, SP=0x{sp:08x}",
            exception_label(pending.exception)
        ),
    })
}

fn parse_hex_after(text: &str, marker: &str) -> Option<u32> {
    let start = text.find(marker)? + marker.len();
    let hex = text.get(start..start + 8)?;
    u32::from_str_radix(hex, 16).ok()
}

fn top_callback_targets(
    callback_targets: HashMap<(u32, u32), u64>,
    limit: usize,
) -> Vec<CallbackTargetStats> {
    let mut items: Vec<_> = callback_targets
        .into_iter()
        .map(|((target, callsite), count)| CallbackTargetStats {
            target,
            callsite,
            count,
        })
        .collect();
    items.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.callsite.cmp(&b.callsite))
            .then_with(|| a.target.cmp(&b.target))
    });
    items.truncate(limit);
    items
}

fn top_exception_vectors(
    exception_vectors: HashMap<(u16, u32), u64>,
    limit: usize,
) -> Vec<ExceptionVectorStats> {
    let mut items: Vec<_> = exception_vectors
        .into_iter()
        .map(|((exception, handler), count)| ExceptionVectorStats {
            exception,
            irq: external_irq_number(exception),
            handler,
            count,
            label: exception_label(exception),
        })
        .collect();
    items.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.exception.cmp(&b.exception))
            .then_with(|| a.handler.cmp(&b.handler))
    });
    items.truncate(limit);
    items
}

fn top_address_counts(counts: HashMap<u32, u64>, limit: usize) -> Vec<AddressCount> {
    let mut items: Vec<_> = counts
        .into_iter()
        .map(|(address, count)| AddressCount {
            address,
            count,
            label: address_label(address),
        })
        .collect();
    items.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.address.cmp(&b.address))
    });
    items.truncate(limit);
    items
}

fn top_edge_counts(counts: HashMap<(u32, u32), u64>, limit: usize) -> Vec<EdgeCount> {
    let mut items: Vec<_> = counts
        .into_iter()
        .map(|((from, to), count)| EdgeCount { from, to, count })
        .collect();
    items.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.from.cmp(&b.from))
            .then_with(|| a.to.cmp(&b.to))
    });
    items.truncate(limit);
    items
}

fn address_label(address: u32) -> Option<String> {
    if let Some(label) = mmio::mmio_label(address) {
        return Some(label);
    }

    known_regions()
        .into_iter()
        .find(|region| address >= region.start && address < region.end_exclusive)
        .map(|region| region.name.to_string())
}

const EXCEPTION_RETURN_THREAD_MSP: u32 = 0xffff_fff9;

#[derive(Debug, Clone, Copy)]
pub(super) struct PendingException {
    exception: u16,
}

fn external_irq_number(exception: u16) -> Option<u16> {
    exception.checked_sub(16)
}

fn exception_label(exception: u16) -> String {
    match exception {
        14 => "PendSV".to_string(),
        15 => "SysTick".to_string(),
        value if value >= 16 => format!("IRQ{}", value - 16),
        value => format!("Exception{value}"),
    }
}

fn should_stop_event(event: &TraceEvent) -> bool {
    event.opcode16.is_none()
        || event.note.starts_with("BKPT")
        || event.note.contains("unmapped data read")
        || event.note.contains("decoded semantics not implemented")
}

fn stop_status_for_event(event: &TraceEvent) -> &'static str {
    if event.opcode16.is_none() {
        "boot-probe-pc-outside-loaded-memory"
    } else if event.note.starts_with("BKPT") {
        "boot-probe-hit-breakpoint"
    } else if event.note.contains("unmapped data read") {
        "boot-probe-hit-unmapped-read"
    } else {
        "boot-probe-hit-unsupported-instruction"
    }
}

fn retain_trace_event(
    trace_head: &mut Vec<TraceEvent>,
    trace_tail: &mut VecDeque<TraceEvent>,
    event: TraceEvent,
) {
    if trace_head.len() < TRACE_HEAD_LIMIT {
        trace_head.push(event);
        return;
    }

    if trace_tail.len() == TRACE_TAIL_LIMIT {
        trace_tail.pop_front();
    }
    trace_tail.push_back(event);
}

fn trace_reset_event(records: &[DataRecord], cpu: &CpuState, reset_note: String) -> TraceEvent {
    TraceEvent {
        cycle: cpu.cycles,
        pc: cpu.registers[15],
        opcode16: read_u16(records, cpu.registers[15]),
        opcode32: None,
        note: reset_note,
    }
}

struct ResetSource {
    initial_sp: Option<u32>,
    pc: u32,
    note: String,
}

fn reset_source(analysis: &FirmwareAnalysis) -> Option<ResetSource> {
    if let Some(vector) = &analysis.vector_table {
        return Some(ResetSource {
            initial_sp: Some(vector.initial_sp),
            pc: vector.reset_pc_aligned,
            note: "Cortex-M reset vector loaded; instruction decode is the next emulator layer"
                .to_string(),
        });
    }

    analysis.boot_image.as_ref().map(|boot| ResetSource {
        initial_sp: None,
        pc: boot.entry_aligned,
        note: "Teensy i.MX RT IVT entry loaded; boot ROM setup is assumed, instruction decode is the next emulator layer"
            .to_string(),
    })
}
