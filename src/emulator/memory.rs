use std::collections::{HashMap, VecDeque};

use super::mmio::*;
use super::*;

pub(super) const AUDIO_DMA_SERVICE_PERIOD: u64 = 2_048;
const AUDIO_PCM_QUEUE_LIMIT: usize = 16_384;
const SYSTICK_EXTERNAL_CLOCK_DIVISOR: u64 = 100;
const DMAMUX_SOURCE_SAI1_TX: u8 = 20;
const DMAMUX_SOURCE_SAI2_TX: u8 = 22;
const DMAMUX_SOURCE_SAI3_TX: u8 = 84;
const DMA_CSR_INTMAJOR: u16 = 1 << 1;
const DMA_CSR_DREQ: u16 = 1 << 3;

#[derive(Debug, Clone)]
pub(super) struct ExecutionMemory {
    rom: HashMap<u32, u8>,
    rom_dense_base: u32,
    rom_dense_bytes: Vec<u8>,
    rom_dense_present: Vec<bool>,
    writes: HashMap<u32, u8>,
    peripheral_read_counts: HashMap<u32, u32>,
    mmio_reads: HashMap<u32, u64>,
    mmio_writes: HashMap<u32, u64>,
    usdhc_commands: HashMap<(u32, u32, bool, u32, bool), u64>,
    usdhc_recent_commands: VecDeque<UsdhcCommandStats>,
    usdhc_data_words: HashMap<u32, VecDeque<u32>>,
    usdhc_pending_data_status: HashMap<u32, u32>,
    usdhc_write_transfers: HashMap<u32, UsdhcWriteTransfer>,
    usdhc_app_command_pending: HashMap<u32, bool>,
    sd_switch_group1_function: u8,
    sd_card: VirtualSdCard,
    audio_dma_transfers: u64,
    audio_pcm_words: u64,
    audio_nonzero_pcm_words: u64,
    audio_last_pcm_word: Option<u32>,
    audio_last_nonzero_pcm_word: Option<u32>,
    audio_peak_abs_sample: u16,
    audio_pcm_queue: VecDeque<u32>,
    next_audio_dma_cycle: u64,
    virtual_cycles: u64,
    dwt_base_value: u32,
    dwt_base_cycle: u64,
    systick_epoch_cycle: u64,
    systick_wrap_count: u64,
    systick_countflag: bool,
    current_writer_pc: u32,
    current_writer_cycle: u64,
    write_provenance: HashMap<u32, MemoryWriteProvenance>,
    pointer_sanitizations: HashMap<(u32, u32, u32), u64>,
    pointer_sanitization_samples: Vec<PointerSanitizationSample>,
    unmapped_reads: HashMap<(u32, u8, u32), u64>,
    unmapped_read_samples: Vec<UnmappedReadSample>,
    pending_unmapped_read: Option<UnmappedReadSample>,
}

impl ExecutionMemory {
    pub(super) fn new(records: Vec<DataRecord>) -> Self {
        let mut rom = HashMap::new();
        let mut min_address = u32::MAX;
        let mut max_address = 0;
        for record in &records {
            min_address = min_address.min(record.address);
            max_address = max_address.max(record.address.saturating_add(record.bytes.len() as u32));
        }
        let dense_size = max_address.saturating_sub(min_address) as usize;
        let use_dense = !records.is_empty() && dense_size <= 16 * 1024 * 1024;
        let mut rom_dense_bytes = if use_dense {
            vec![0; dense_size]
        } else {
            Vec::new()
        };
        let mut rom_dense_present = if use_dense {
            vec![false; dense_size]
        } else {
            Vec::new()
        };

        for record in records {
            let record_address = record.address;
            for (offset, byte) in record.bytes.into_iter().enumerate() {
                let address = record_address + offset as u32;
                rom.insert(record.address + offset as u32, byte);
                if use_dense {
                    let dense_index = (address - min_address) as usize;
                    rom_dense_bytes[dense_index] = byte;
                    rom_dense_present[dense_index] = true;
                }
            }
        }
        Self {
            rom,
            rom_dense_base: if use_dense { min_address } else { 0 },
            rom_dense_bytes,
            rom_dense_present,
            writes: HashMap::new(),
            peripheral_read_counts: HashMap::new(),
            mmio_reads: HashMap::new(),
            mmio_writes: HashMap::new(),
            usdhc_commands: HashMap::new(),
            usdhc_recent_commands: VecDeque::new(),
            usdhc_data_words: HashMap::new(),
            usdhc_pending_data_status: HashMap::new(),
            usdhc_write_transfers: HashMap::new(),
            usdhc_app_command_pending: HashMap::new(),
            sd_switch_group1_function: 0,
            sd_card: VirtualSdCard::default(),
            audio_dma_transfers: 0,
            audio_pcm_words: 0,
            audio_nonzero_pcm_words: 0,
            audio_last_pcm_word: None,
            audio_last_nonzero_pcm_word: None,
            audio_peak_abs_sample: 0,
            audio_pcm_queue: VecDeque::new(),
            next_audio_dma_cycle: AUDIO_DMA_SERVICE_PERIOD,
            virtual_cycles: 0,
            dwt_base_value: 0,
            dwt_base_cycle: 0,
            systick_epoch_cycle: 0,
            systick_wrap_count: 0,
            systick_countflag: false,
            current_writer_pc: 0,
            current_writer_cycle: 0,
            write_provenance: HashMap::new(),
            pointer_sanitizations: HashMap::new(),
            pointer_sanitization_samples: Vec::new(),
            unmapped_reads: HashMap::new(),
            unmapped_read_samples: Vec::new(),
            pending_unmapped_read: None,
        }
    }

    pub(super) fn set_current_instruction(&mut self, pc: u32, cycle: u64) {
        self.current_writer_pc = pc;
        self.current_writer_cycle = cycle;
    }

    pub(super) fn read_u8(&self, address: u32) -> Option<u8> {
        self.read_dense_u8(address).or_else(|| {
            self.writes
                .get(&address)
                .copied()
                .or_else(|| self.rom.get(&address).copied())
        })
    }

    pub(super) fn read_u8_or_zero(&self, address: u32) -> u8 {
        self.read_u8(address).unwrap_or(0)
    }

    pub(super) fn read_u16(&self, address: u32) -> Option<u16> {
        Some(u16::from_le_bytes([
            self.read_u8(address)?,
            self.read_u8(address + 1)?,
        ]))
    }

    pub(super) fn has_executable_halfword(&self, address: u32) -> bool {
        is_executable_region(address) && self.read_u16(address).is_some()
    }

    pub(super) fn is_executable_address(&self, address: u32) -> bool {
        is_executable_region(address)
    }

    pub(super) fn snapshot_byte_range(&self, start: u32, byte_count: usize) -> Vec<Option<u8>> {
        (0..byte_count)
            .map(|offset| self.read_u8(start.wrapping_add(offset as u32)))
            .collect()
    }

    pub(super) fn restore_byte_range(&mut self, start: u32, snapshot: &[Option<u8>]) {
        for (offset, value) in snapshot.iter().copied().enumerate() {
            let address = start.wrapping_add(offset as u32);
            match value {
                Some(byte) => self.write_byte_raw(address, byte),
                None => {
                    self.writes.remove(&address);
                    self.write_provenance.remove(&address);
                }
            }
        }
    }

    pub(super) fn read_u16_or_zero(&self, address: u32) -> u16 {
        self.read_u16(address)
            .unwrap_or_else(|| peripheral_default_value(address) as u16)
    }

    pub(super) fn read_u32(&self, address: u32) -> Option<u32> {
        Some(u32::from_le_bytes([
            self.read_u8(address)?,
            self.read_u8(address + 1)?,
            self.read_u8(address + 2)?,
            self.read_u8(address + 3)?,
        ]))
    }

    pub(super) fn read_u32_or_zero(&mut self, address: u32) -> u32 {
        self.record_mmio_read(address);

        if let Some(source) = nvic_enable_mirror_source(address) {
            return self.read_u32(source).unwrap_or(0);
        }
        if let Some(source) = nvic_pending_mirror_source(address) {
            return self.read_u32(source).unwrap_or(0);
        }
        if address == SCB_ICSR {
            return self.current_icsr_value();
        }
        if is_nvic_active_register(address) {
            return self.read_u32(address).unwrap_or(0);
        }
        if address == SYSTICK_CTRL {
            self.update_systick_from_clock();
            let current = self.read_u32(address).unwrap_or(0) & 0x0000_0007;
            let countflag = if self.systick_countflag { 1 << 16 } else { 0 };
            self.systick_countflag = false;
            return current | countflag;
        }
        if address == SYSTICK_VALUE {
            self.update_systick_from_clock();
            return self.systick_value();
        }
        if address == DWT_CYCCNT {
            return self.dwt_cycle_counter();
        }
        if let Some(channel) = pit_channel_register(address, 0x04) {
            let load = self
                .read_u32(pit_channel_address(channel, 0x00))
                .unwrap_or(0xffff_ffff);
            let tctrl = self
                .read_u32(pit_channel_address(channel, 0x08))
                .unwrap_or(0);
            if tctrl & 1 == 0 {
                return load;
            }
            let reads = self.peripheral_read_counts.entry(address).or_default();
            *reads = reads.wrapping_add(1);
            return load.saturating_sub(*reads % load.saturating_add(1).max(1));
        }
        if let Some(channel) = pit_channel_register(address, 0x0c) {
            let current = self.read_u32(address).unwrap_or(0) & 1;
            let tctrl = self
                .read_u32(pit_channel_address(channel, 0x08))
                .unwrap_or(0);
            if tctrl & 1 != 0 {
                let reads = self.peripheral_read_counts.entry(address).or_default();
                *reads = reads.wrapping_add(1);
                return current | u32::from(*reads > 1);
            }
            return current;
        }
        if address == CCM_CDHIPR {
            return 0;
        }
        if flexspi_register(address, FLEXSPI_INTR).is_some() {
            return self.read_u32(address).unwrap_or(0) | 1;
        }
        if flexspi_register(address, FLEXSPI_IPRXFSTS).is_some() {
            return self.read_u32(address).unwrap_or(0) | 1;
        }
        if usdhc_register(address, USDHC_PRES_STATE).is_some() {
            return self.read_u32(address).unwrap_or(0) | USDHC_PRES_READY;
        }
        if let Some(base) = usdhc_base(address)
            && address - base == USDHC_DATA_BUFF_ACC_PORT
        {
            return self.read_usdhc_data_word(base);
        }
        if usdhc_register(address, USDHC_HOST_CTRL_CAP).is_some() {
            return self.read_u32(address).unwrap_or(USDHC_HOST_CAP_READY) | USDHC_HOST_CAP_READY;
        }

        if address == M8_BOOT_TICK_COUNTER {
            let current = self.read_u32(address).unwrap_or(0);
            if current <= 0x13 {
                let reads = self.peripheral_read_counts.entry(address).or_default();
                *reads = reads.wrapping_add(1);
                return current.max((*reads / 3).min(0x1000));
            }
        }

        if address == 0x4008_410c {
            if self.read_u32(address) == Some(0) {
                let reads = self.peripheral_read_counts.entry(address).or_default();
                let value = if *reads == 0 { 0 } else { 1 };
                *reads = reads.wrapping_add(1);
                return value;
            }
        }

        if let Some(value) = self.read_u32(address) {
            value
        } else {
            self.record_unmapped_read(address, 4);
            peripheral_default_value(address)
        }
    }

    pub(super) fn write_u32(&mut self, address: u32, value: u32) {
        self.record_mmio_write(address);

        if address == SYSTICK_CTRL {
            self.write_u32_raw(address, value & 0x0000_0007);
            self.reset_systick_epoch();
            return;
        }
        if address == SYSTICK_LOAD {
            self.write_u32_raw(address, value & 0x00ff_ffff);
            self.reset_systick_epoch();
            return;
        }
        if address == SYSTICK_VALUE {
            self.systick_countflag = false;
            self.reset_systick_epoch();
            self.write_u32_raw(address, 0);
            return;
        }
        if address == DWT_CYCCNT {
            self.dwt_base_value = value;
            self.dwt_base_cycle = self.virtual_cycles;
            self.write_u32_raw(address, value);
            return;
        }
        if let Some(target) = nvic_set_enable_target(address) {
            let next = self.read_u32(target).unwrap_or(0) | value;
            self.write_u32_raw(target, next);
            return;
        }
        if let Some(target) = nvic_clear_enable_target(address) {
            let next = self.read_u32(target).unwrap_or(0) & !value;
            self.write_u32_raw(target, next);
            return;
        }
        if let Some(target) = nvic_set_pending_target(address) {
            let next = self.read_u32(target).unwrap_or(0) | value;
            self.write_u32_raw(target, next);
            return;
        }
        if let Some(target) = nvic_clear_pending_target(address) {
            let next = self.read_u32(target).unwrap_or(0) & !value;
            self.write_u32_raw(target, next);
            return;
        }
        if address == NVIC_STIR {
            let irq = value & 0x01ff;
            if irq < 240 {
                let target = NVIC_ISPR_BASE + (irq / 32) * 4;
                let next = self.read_u32(target).unwrap_or(0) | (1 << (irq % 32));
                self.write_u32_raw(target, next);
            }
            return;
        }
        if address == SCB_ICSR {
            let mut next = self.read_u32(SCB_ICSR).unwrap_or(0)
                & (SCB_ICSR_VECTACTIVE_MASK | SCB_ICSR_PENDING_MASK);
            if value & SCB_ICSR_PENDSTSET != 0 {
                next |= SCB_ICSR_PENDSTSET;
            }
            if value & SCB_ICSR_PENDSTCLR != 0 {
                next &= !SCB_ICSR_PENDSTSET;
            }
            if value & SCB_ICSR_PENDSVSET != 0 {
                next |= SCB_ICSR_PENDSVSET;
            }
            if value & SCB_ICSR_PENDSVCLR != 0 {
                next &= !SCB_ICSR_PENDSVSET;
            }
            self.write_u32_raw(SCB_ICSR, next);
            return;
        }
        if address == SCB_AIRCR {
            self.write_u32_raw(address, value & !0x0000_0004);
            return;
        }
        if matches!(address, SCB_CFSR | SCB_HFSR | SCB_DFSR) {
            let next = self.read_u32(address).unwrap_or(0) & !value;
            self.write_u32_raw(address, next);
            return;
        }
        if let Some(channel) = pit_channel_register(address, 0x00) {
            self.peripheral_read_counts
                .insert(pit_channel_address(channel, 0x04), 0);
            self.write_u32_raw(address, value);
            return;
        }
        if let Some(channel) = pit_channel_register(address, 0x04) {
            self.peripheral_read_counts.insert(address, 0);
            self.write_u32_raw(address, value);
            self.write_u32_raw(pit_channel_address(channel, 0x0c), 0);
            return;
        }
        if let Some(channel) = pit_channel_register(address, 0x08) {
            self.peripheral_read_counts
                .insert(pit_channel_address(channel, 0x04), 0);
            self.peripheral_read_counts
                .insert(pit_channel_address(channel, 0x0c), 0);
            self.write_u32_raw(address, value & 0x7);
            return;
        }
        if pit_channel_register(address, 0x0c).is_some() {
            let next = self.read_u32(address).unwrap_or(0) & !(value & 1);
            self.peripheral_read_counts.insert(address, 0);
            self.write_u32_raw(address, next);
            return;
        }
        if address == CCM_CDHIPR {
            self.write_u32_raw(address, 0);
            return;
        }
        if flexspi_register(address, FLEXSPI_MCR0).is_some() {
            self.write_u32_raw(address, value & !1);
            return;
        }
        if flexspi_register(address, FLEXSPI_INTR).is_some() {
            let next = (self.read_u32(address).unwrap_or(1) & !value) | 1;
            self.write_u32_raw(address, next);
            return;
        }
        if flexspi_register(address, FLEXSPI_IPCMD).is_some() {
            self.write_u32_raw(address, value);
            if let Some(base) = flexspi_base(address) {
                self.write_u32_raw(
                    base + FLEXSPI_INTR,
                    self.read_u32(base + FLEXSPI_INTR).unwrap_or(0) | 1,
                );
                self.write_u32_raw(
                    base + FLEXSPI_IPRXFSTS,
                    self.read_u32(base + FLEXSPI_IPRXFSTS).unwrap_or(0) | 1,
                );
            }
            return;
        }
        if flexspi_register(address, FLEXSPI_LUTKEY).is_some()
            || flexspi_register(address, FLEXSPI_LUTCR).is_some()
            || flexspi_register(address, FLEXSPI_IPCR0).is_some()
            || flexspi_register(address, FLEXSPI_IPCR1).is_some()
            || flexspi_lut_address(address)
        {
            self.write_u32_raw(address, value);
            return;
        }

        if let Some(base) = usdhc_base(address) {
            let offset = address - base;
            match offset {
                USDHC_SYS_CTRL => {
                    if value & USDHC_SYS_CTRL_RESET_MASK != 0 {
                        self.reset_usdhc_state(base);
                    }
                    self.write_u32_raw(address, value & !USDHC_SYS_CTRL_RESET_MASK);
                    return;
                }
                USDHC_INT_STATUS => {
                    let current = self.read_u32(address).unwrap_or(0);
                    self.write_u32_raw(address, current & !value);
                    if value & USDHC_INT_CC != 0 {
                        self.commit_pending_usdhc_data_status(base);
                    }
                    self.refresh_usdhc_irq(base);
                    return;
                }
                USDHC_INT_STATUS_EN | USDHC_INT_SIGNAL_EN => {
                    self.write_u32_raw(address, value);
                    self.refresh_usdhc_irq(base);
                    return;
                }
                USDHC_CMD_XFR_TYP => {
                    self.write_u32_raw(address, value);
                    self.complete_usdhc_command(base, value);
                    return;
                }
                USDHC_DATA_BUFF_ACC_PORT => {
                    self.write_usdhc_data_word(base, value);
                    self.set_usdhc_interrupt(base, USDHC_INT_TC | USDHC_INT_BWR | USDHC_INT_BRR);
                    return;
                }
                USDHC_PRES_STATE => {
                    self.write_u32_raw(address, value | USDHC_PRES_READY);
                    return;
                }
                _ => {
                    self.write_u32_raw(address, value);
                    return;
                }
            }
        }

        if let Some((actual, alias)) = peripheral_alias(address) {
            let current = self.read_u32_or_zero(actual);
            let next = match alias {
                PeripheralAlias::Set => current | value,
                PeripheralAlias::Clear => current & !value,
                PeripheralAlias::Toggle => current ^ value,
            };
            self.write_u32_raw(actual, peripheral_ready_value(actual, next));
            return;
        }

        self.write_u32_raw(address, peripheral_ready_value(address, value));
    }

    fn write_u32_raw(&mut self, address: u32, value: u32) {
        if address == 0x4008_410c {
            self.peripheral_read_counts.insert(address, 0);
        }
        self.record_write_provenance(address, 4, value);
        for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write_byte_raw(address + offset as u32, byte);
        }
    }

    fn write_u16_raw(&mut self, address: u32, value: u16) {
        for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write_byte_raw(address + offset as u32, byte);
        }
    }

    pub(super) fn write_u8(&mut self, address: u32, value: u8) {
        self.record_mmio_write(address);
        if self.handle_dma_command_u8(address, value) {
            return;
        }
        self.record_write_provenance(address, 1, u32::from(value));
        self.write_byte_raw(address, value);
    }

    pub(super) fn write_u16(&mut self, address: u32, value: u16) {
        self.record_mmio_write(address);
        self.record_write_provenance(address, 2, u32::from(value));
        for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write_byte_raw(address + offset as u32, byte);
        }
    }

    fn write_byte_raw(&mut self, address: u32, value: u8) {
        if self.is_runtime_direct_flash_write(address) {
            return;
        }
        if self.write_dense_u8(address, value) {
            return;
        }
        self.writes.insert(address, value);
    }

    fn is_runtime_direct_flash_write(&self, address: u32) -> bool {
        is_flexspi_flash_region(address)
            && self.current_writer_pc != 0
            && is_executable_region(self.current_writer_pc)
    }

    fn read_dense_u8(&self, address: u32) -> Option<u8> {
        if self.rom_dense_bytes.is_empty() || address < self.rom_dense_base {
            return None;
        }
        let index = (address - self.rom_dense_base) as usize;
        if index >= self.rom_dense_bytes.len() || !self.rom_dense_present[index] {
            return None;
        }
        Some(self.rom_dense_bytes[index])
    }

    fn write_dense_u8(&mut self, address: u32, value: u8) -> bool {
        if self.rom_dense_bytes.is_empty() || address < self.rom_dense_base {
            return false;
        }
        let index = (address - self.rom_dense_base) as usize;
        if index >= self.rom_dense_bytes.len() {
            return false;
        }
        self.rom_dense_bytes[index] = value;
        self.rom_dense_present[index] = true;
        true
    }

    pub(super) fn callback_memory_probes(&self, registers: &[u32; 16]) -> Vec<CallbackMemoryProbe> {
        const OFFSETS: [u32; 4] = [0, 4, 8, 0x13c];
        let mut probes = Vec::new();

        for (register, register_value) in registers.iter().copied().enumerate() {
            if !is_provenance_probe_base(register_value) {
                continue;
            }
            for offset in OFFSETS {
                let address = register_value.wrapping_add(offset);
                if !is_provenance_region(address) {
                    continue;
                }
                let provenance = self.write_provenance.get(&address).copied();
                let value = self.read_u32(address);
                if provenance.is_none() && value.is_none() {
                    continue;
                }
                probes.push(CallbackMemoryProbe {
                    register,
                    register_value,
                    offset,
                    address,
                    value,
                    bytes: self.word_byte_probes(address),
                    provenance,
                });
            }
        }

        probes
    }

    pub(super) fn sanitize_optional_pointer_load(
        &mut self,
        address: u32,
        raw_value: u32,
    ) -> Option<u32> {
        self.sanitize_pointer_load(
            address,
            raw_value,
            "optional pointer field contained display-cell bytes",
        )
    }

    pub(super) fn sanitize_virtual_dispatch_receiver(
        &mut self,
        address: u32,
        raw_value: u32,
    ) -> Option<u32> {
        let looks_like_display_cell = self.looks_like_display_cell_collision(address, raw_value);
        if raw_value == 0
            || is_plausible_pointer_value(raw_value)
            || !is_ram_region(address)
            || (!looks_like_display_cell && !is_m8_runtime_pc(self.current_writer_pc))
        {
            return None;
        }

        let reason = if looks_like_display_cell {
            "virtual dispatch receiver contained display-cell bytes"
        } else {
            "virtual dispatch receiver contained non-pointer UI/runtime bytes"
        };
        self.record_pointer_sanitization(address, raw_value, reason)
    }

    pub(super) fn sanitize_virtual_dispatch_receiver_base(
        &mut self,
        effective_address: u32,
        base_value: u32,
    ) -> Option<u32> {
        if base_value == 0
            || is_plausible_pointer_value(base_value)
            || !is_faulting_unmapped_read(effective_address)
            || !is_m8_runtime_pc(self.current_writer_pc)
        {
            return None;
        }

        self.record_pointer_sanitization(
            effective_address,
            base_value,
            "virtual dispatch receiver base contained non-pointer runtime bytes",
        )
    }

    pub(super) fn sanitize_display_cell_effective_address(
        &mut self,
        base_value: u32,
        offset: u32,
        effective_address: u32,
    ) -> Option<u32> {
        if base_value == 0
            || offset > 0x100
            || is_plausible_pointer_value(base_value)
            || !is_faulting_unmapped_read(effective_address)
            || !looks_like_corrupt_ui_data_pointer_value(base_value, self.current_writer_pc)
        {
            return None;
        }

        let sanitized_value = 0;
        let sample = PointerSanitizationSample {
            address: effective_address,
            raw_value: base_value,
            sanitized_value,
            pc: self.current_writer_pc,
            cycle: self.current_writer_cycle,
            reason: "effective address base contained display-cell bytes".to_string(),
            bytes: self.word_byte_probes(effective_address),
        };
        *self
            .pointer_sanitizations
            .entry((effective_address, base_value, self.current_writer_pc))
            .or_default() += 1;
        if self.pointer_sanitization_samples.len() < 16 {
            self.pointer_sanitization_samples.push(sample);
        }
        Some(sanitized_value)
    }

    fn sanitize_pointer_load(
        &mut self,
        address: u32,
        raw_value: u32,
        reason: &'static str,
    ) -> Option<u32> {
        if raw_value == 0
            || is_plausible_pointer_value(raw_value)
            || !is_ram_region(address)
            || !self.looks_like_display_cell_collision(address, raw_value)
        {
            return None;
        }

        self.record_pointer_sanitization(address, raw_value, reason)
    }

    fn record_pointer_sanitization(
        &mut self,
        address: u32,
        raw_value: u32,
        reason: &'static str,
    ) -> Option<u32> {
        let sanitized_value = 0;
        let sample = PointerSanitizationSample {
            address,
            raw_value,
            sanitized_value,
            pc: self.current_writer_pc,
            cycle: self.current_writer_cycle,
            reason: reason.to_string(),
            bytes: self.word_byte_probes(address),
        };
        *self
            .pointer_sanitizations
            .entry((address, raw_value, self.current_writer_pc))
            .or_default() += 1;
        if self.pointer_sanitization_samples.len() < 16 {
            self.pointer_sanitization_samples.push(sample);
        }
        Some(sanitized_value)
    }

    pub(super) fn top_mmio_reads(&self, limit: usize) -> Vec<AddressCount> {
        top_counts(&self.mmio_reads, limit)
    }

    pub(super) fn top_mmio_writes(&self, limit: usize) -> Vec<AddressCount> {
        top_counts(&self.mmio_writes, limit)
    }

    pub(super) fn top_usdhc_reads(&self, limit: usize) -> Vec<AddressCount> {
        top_usdhc_counts(&self.mmio_reads, limit)
    }

    pub(super) fn top_usdhc_writes(&self, limit: usize) -> Vec<AddressCount> {
        top_usdhc_counts(&self.mmio_writes, limit)
    }

    pub(super) fn audio_stats(&self) -> AudioStats {
        let total_reads = total_audio_counts(&self.mmio_reads);
        let total_writes = total_audio_counts(&self.mmio_writes);
        let observed = total_reads > 0 || total_writes > 0;
        let status = if observed {
            format!(
                "observed {} reads and {} writes across SAI/DMA/DMAMUX/USB-audio ranges",
                total_reads, total_writes
            )
        } else {
            "no SAI/DMA/DMAMUX/USB-audio MMIO observed yet".to_string()
        };

        AudioStats {
            observed,
            status,
            total_reads,
            total_writes,
            dma_transfers: self.audio_dma_transfers,
            captured_pcm_words: self.audio_pcm_words,
            nonzero_pcm_words: self.audio_nonzero_pcm_words,
            last_pcm_word: self.audio_last_pcm_word,
            last_nonzero_pcm_word: self.audio_last_nonzero_pcm_word,
            peak_abs_sample: self.audio_peak_abs_sample,
            top_reads: top_audio_counts(&self.mmio_reads, None, 8),
            top_writes: top_audio_counts(&self.mmio_writes, None, 8),
            sai_reads: top_audio_counts(&self.mmio_reads, Some(AudioMmioKind::Sai), 8),
            sai_writes: top_audio_counts(&self.mmio_writes, Some(AudioMmioKind::Sai), 8),
            dma_reads: top_audio_counts(&self.mmio_reads, Some(AudioMmioKind::Dma), 8),
            dma_writes: top_audio_counts(&self.mmio_writes, Some(AudioMmioKind::Dma), 8),
            dmamux_reads: top_audio_counts(&self.mmio_reads, Some(AudioMmioKind::Dmamux), 8),
            dmamux_writes: top_audio_counts(&self.mmio_writes, Some(AudioMmioKind::Dmamux), 8),
            usb_reads: top_audio_counts(&self.mmio_reads, Some(AudioMmioKind::Usb), 8),
            usb_writes: top_audio_counts(&self.mmio_writes, Some(AudioMmioKind::Usb), 8),
            clock_reads: top_audio_counts(&self.mmio_reads, Some(AudioMmioKind::Clock), 8),
            clock_writes: top_audio_counts(&self.mmio_writes, Some(AudioMmioKind::Clock), 8),
            active_dma_channels: self.audio_dma_channel_stats(),
        }
    }

    pub(super) fn take_audio_pcm_words(&mut self) -> Vec<u32> {
        self.audio_pcm_queue.drain(..).collect()
    }

    pub(super) fn top_usdhc_commands(&self, limit: usize) -> Vec<UsdhcCommandStats> {
        let mut items: Vec<_> = self
            .usdhc_commands
            .iter()
            .map(
                |((base, command_index, app_command, argument, data_present), count)| {
                    UsdhcCommandStats {
                        instance: if *base == USDHC1_BASE {
                            "USDHC1"
                        } else {
                            "USDHC2"
                        },
                        command_index: *command_index,
                        app_command: *app_command,
                        argument: *argument,
                        data_present: *data_present,
                        count: *count,
                        ds_addr: 0,
                        block_attribute: 0,
                        mix_ctrl: 0,
                        response0: 0,
                        int_status: 0,
                        int_status_en: 0,
                        int_signal_en: 0,
                        pending_data_status: 0,
                        data_preview: Vec::new(),
                    }
                },
            )
            .collect();
        items.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.instance.cmp(b.instance))
                .then_with(|| a.command_index.cmp(&b.command_index))
                .then_with(|| a.argument.cmp(&b.argument))
                .then_with(|| a.data_present.cmp(&b.data_present))
        });
        items.truncate(limit);
        items
    }

    pub(super) fn recent_usdhc_commands(&self, limit: usize) -> Vec<UsdhcCommandStats> {
        self.usdhc_recent_commands
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub(super) fn usdhc_controller_stats(&self) -> Vec<UsdhcControllerStats> {
        [USDHC1_BASE, USDHC2_BASE]
            .into_iter()
            .map(|base| UsdhcControllerStats {
                instance: if base == USDHC1_BASE {
                    "USDHC1"
                } else {
                    "USDHC2"
                },
                int_status: self.read_u32(base + USDHC_INT_STATUS).unwrap_or(0),
                int_status_en: self.read_u32(base + USDHC_INT_STATUS_EN).unwrap_or(0),
                int_signal_en: self.read_u32(base + USDHC_INT_SIGNAL_EN).unwrap_or(0),
                pres_state: self.read_u32(base + USDHC_PRES_STATE).unwrap_or(0),
                ds_addr: self.read_u32(base + USDHC_DS_ADDR).unwrap_or(0),
                block_attribute: self.read_u32(base + USDHC_BLK_ATT).unwrap_or(0),
                mix_ctrl: self.read_u32(base + USDHC_MIX_CTRL).unwrap_or(0),
                pending_data_status: self
                    .usdhc_pending_data_status
                    .get(&base)
                    .copied()
                    .unwrap_or(0),
            })
            .collect()
    }

    pub(super) fn load_sd_image(&mut self, bytes: &[u8]) {
        self.sd_card = VirtualSdCard::new(bytes.to_vec());
        self.usdhc_data_words.clear();
        self.usdhc_pending_data_status.clear();
        self.usdhc_write_transfers.clear();
        self.usdhc_app_command_pending.clear();
        self.sd_switch_group1_function = 0;
        self.write_u32_raw(USDHC1_BASE + USDHC_PRES_STATE, USDHC_PRES_READY);
        self.write_u32_raw(USDHC2_BASE + USDHC_PRES_STATE, USDHC_PRES_READY);
    }

    pub(super) fn clear_sd_image(&mut self) {
        self.sd_card = VirtualSdCard::default();
        self.usdhc_data_words.clear();
        self.usdhc_pending_data_status.clear();
        self.usdhc_write_transfers.clear();
        self.usdhc_app_command_pending.clear();
        self.sd_switch_group1_function = 0;
    }

    pub(super) fn sd_card_stats(&self) -> VirtualSdCardStats {
        self.sd_card.stats()
    }

    pub(super) fn sd_image_bytes(&self) -> Vec<u8> {
        self.sd_card.bytes.clone()
    }

    pub(super) fn top_unmapped_reads(&self, limit: usize) -> Vec<UnmappedReadStats> {
        let mut items: Vec<_> = self
            .unmapped_reads
            .iter()
            .map(|((address, size, pc), count)| UnmappedReadStats {
                address: *address,
                size: *size,
                pc: *pc,
                count: *count,
            })
            .collect();
        items.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.address.cmp(&b.address))
                .then_with(|| a.pc.cmp(&b.pc))
                .then_with(|| a.size.cmp(&b.size))
        });
        items.truncate(limit);
        items
    }

    pub(super) fn top_pointer_sanitizations(&self, limit: usize) -> Vec<PointerSanitizationStats> {
        let mut items: Vec<_> = self
            .pointer_sanitizations
            .iter()
            .map(
                |((address, raw_value, pc), count)| PointerSanitizationStats {
                    address: *address,
                    raw_value: *raw_value,
                    pc: *pc,
                    count: *count,
                },
            )
            .collect();
        items.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.address.cmp(&b.address))
                .then_with(|| a.pc.cmp(&b.pc))
                .then_with(|| a.raw_value.cmp(&b.raw_value))
        });
        items.truncate(limit);
        items
    }

    pub(super) fn pointer_sanitization_samples(&self) -> Vec<PointerSanitizationSample> {
        self.pointer_sanitization_samples.clone()
    }

    pub(super) fn unmapped_read_samples(&self) -> Vec<UnmappedReadSample> {
        self.unmapped_read_samples.clone()
    }

    pub(super) fn take_pending_unmapped_read(&mut self) -> Option<UnmappedReadSample> {
        self.pending_unmapped_read.take()
    }

    pub(super) fn advance_virtual_cycles(&mut self, delta: u64) {
        self.virtual_cycles = self.virtual_cycles.wrapping_add(delta);
        self.update_systick_from_clock();
        self.service_audio_dma();
    }

    pub(super) fn update_periodic_pending(&mut self) {
        self.update_systick_from_clock();
    }

    fn service_audio_dma(&mut self) {
        if self.virtual_cycles < self.next_audio_dma_cycle {
            return;
        }
        self.next_audio_dma_cycle = self.virtual_cycles.saturating_add(AUDIO_DMA_SERVICE_PERIOD);

        for channel in 0..DMA0_TCD_COUNT {
            if self.service_audio_dma_channel(channel as u8) {
                break;
            }
        }
    }

    fn service_audio_dma_channel(&mut self, channel: u8) -> bool {
        if !self.dma_erq_enabled(channel) {
            return false;
        }
        let dmamux = self.dmamux_channel_config(channel);
        if !dmamux.enabled || !is_sai_tx_dmamux_source(dmamux.source) {
            return false;
        }

        let tcd = self.read_dma_tcd(channel);
        if tcd.nbytes == 0 || !is_sai_tx_address(tcd.daddr) {
            return false;
        }

        let bytes = tcd.nbytes.clamp(1, 4);
        let mut word_bytes = [0; 4];
        for offset in 0..bytes {
            word_bytes[offset as usize] = self.read_u8_or_zero(tcd.saddr.wrapping_add(offset));
        }
        let word = u32::from_le_bytes(word_bytes);
        self.write_u32_raw(tcd.daddr, word);

        self.audio_dma_transfers = self.audio_dma_transfers.saturating_add(1);
        self.audio_pcm_words = self.audio_pcm_words.saturating_add(1);
        self.audio_last_pcm_word = Some(word);
        if word != 0 {
            self.audio_nonzero_pcm_words = self.audio_nonzero_pcm_words.saturating_add(1);
            self.audio_last_nonzero_pcm_word = Some(word);
        }
        self.audio_peak_abs_sample = self
            .audio_peak_abs_sample
            .max(pcm_word_peak_abs_sample(word));
        if self.audio_pcm_queue.len() >= AUDIO_PCM_QUEUE_LIMIT {
            self.audio_pcm_queue.pop_front();
        }
        self.audio_pcm_queue.push_back(word);

        let next_saddr = tcd.saddr.wrapping_add_signed(i32::from(tcd.soff));
        let next_daddr = tcd.daddr.wrapping_add_signed(i32::from(tcd.doff));
        self.write_u32_raw(dma_tcd_address(channel, 0x00), next_saddr);
        self.write_u32_raw(dma_tcd_address(channel, 0x10), next_daddr);

        let citer = tcd.citer & 0x7fff;
        if citer > 1 {
            self.write_u16_raw(dma_tcd_address(channel, 0x16), citer - 1);
            return true;
        }

        self.write_u32_raw(
            dma_tcd_address(channel, 0x00),
            next_saddr.wrapping_add_signed(tcd.slast),
        );
        self.write_u32_raw(
            dma_tcd_address(channel, 0x10),
            next_daddr.wrapping_add_signed(tcd.dlast_sga),
        );
        self.write_u16_raw(dma_tcd_address(channel, 0x16), tcd.biter);
        let next_int = self.read_u32(DMA0_BASE + DMA_INT).unwrap_or(0) | (1 << channel);
        self.write_u32_raw(DMA0_BASE + DMA_INT, next_int);

        if tcd.csr & DMA_CSR_INTMAJOR != 0 {
            self.set_pending_irq(u16::from(channel));
        }
        if tcd.csr & DMA_CSR_DREQ != 0 {
            self.set_dma_erq(channel, false);
        }

        true
    }

    fn read_dma_tcd(&self, channel: u8) -> DmaTcdState {
        let base = dma_tcd_address(channel, 0);
        DmaTcdState {
            saddr: self.read_u32(base).unwrap_or(0),
            soff: self.read_u16(base + 0x04).unwrap_or(0) as i16,
            nbytes: self.read_u32(base + 0x08).unwrap_or(0),
            slast: self.read_u32(base + 0x0c).unwrap_or(0) as i32,
            daddr: self.read_u32(base + 0x10).unwrap_or(0),
            doff: self.read_u16(base + 0x14).unwrap_or(0) as i16,
            citer: self.read_u16(base + 0x16).unwrap_or(0),
            dlast_sga: self.read_u32(base + 0x18).unwrap_or(0) as i32,
            csr: self.read_u16(base + 0x1c).unwrap_or(0),
            biter: self.read_u16(base + 0x1e).unwrap_or(0),
        }
    }

    fn audio_dma_channel_stats(&self) -> Vec<AudioDmaChannelStats> {
        let mut items = Vec::new();
        for channel in 0..DMA0_TCD_COUNT as u8 {
            let dmamux = self.dmamux_channel_config(channel);
            let tcd = self.read_dma_tcd(channel);
            let erq_enabled = self.dma_erq_enabled(channel);
            if !erq_enabled
                && !(dmamux.enabled && is_audio_dmamux_source(dmamux.source))
                && !is_sai_tx_address(tcd.daddr)
            {
                continue;
            }
            items.push(AudioDmaChannelStats {
                channel,
                dmamux_source: dmamux.source,
                dmamux_enabled: dmamux.enabled,
                erq_enabled,
                saddr: tcd.saddr,
                daddr: tcd.daddr,
                source_ring_base: tcd.source_ring_base(),
                source_ring_bytes: tcd.source_ring_bytes(),
                source_ring_nonzero_words: self.source_ring_nonzero_words(&tcd),
                source_ring_peak_abs_sample: self.source_ring_peak_abs_sample(&tcd),
                source_preview_words: self.source_preview_words(tcd.saddr, tcd.nbytes),
                source_label: address_label(tcd.saddr),
                nbytes: tcd.nbytes,
                citer: tcd.citer,
                biter: tcd.biter,
                csr: tcd.csr,
                destination_label: address_label(tcd.daddr),
            });
        }
        items
    }

    fn source_preview_words(&self, start: u32, nbytes: u32) -> Vec<u32> {
        let step = nbytes.clamp(1, 4);
        (0..8)
            .map(|index| self.read_pcm_word(start.wrapping_add(index * step), step))
            .collect()
    }

    fn source_ring_nonzero_words(&self, tcd: &DmaTcdState) -> u32 {
        let word_count = u32::from(tcd.biter & 0x7fff).min(4096);
        let step = tcd.nbytes.clamp(1, 4);
        let base = tcd.source_ring_base();
        (0..word_count)
            .filter(|index| self.read_pcm_word(base.wrapping_add(index * step), step) != 0)
            .count() as u32
    }

    fn source_ring_peak_abs_sample(&self, tcd: &DmaTcdState) -> u16 {
        let word_count = u32::from(tcd.biter & 0x7fff).min(4096);
        let step = tcd.nbytes.clamp(1, 4);
        let base = tcd.source_ring_base();
        (0..word_count)
            .map(|index| {
                pcm_word_peak_abs_sample(self.read_pcm_word(base.wrapping_add(index * step), step))
            })
            .max()
            .unwrap_or(0)
    }

    fn read_pcm_word(&self, address: u32, bytes: u32) -> u32 {
        let mut word_bytes = [0; 4];
        for offset in 0..bytes.clamp(1, 4) {
            word_bytes[offset as usize] = self.read_u8_or_zero(address.wrapping_add(offset));
        }
        u32::from_le_bytes(word_bytes)
    }

    fn dmamux_channel_config(&self, channel: u8) -> DmamuxChannelConfig {
        let value = self
            .read_u32(DMAMUX_BASE + u32::from(channel) * 4)
            .unwrap_or(0);
        DmamuxChannelConfig {
            source: (value & 0x7f) as u8,
            enabled: value & (1 << 31) != 0,
        }
    }

    fn dma_erq_enabled(&self, channel: u8) -> bool {
        self.read_u32(DMA0_BASE + DMA_ERQ).unwrap_or(0) & (1 << channel) != 0
    }

    fn set_dma_erq(&mut self, channel: u8, enabled: bool) {
        let mask = 1 << channel;
        let current = self.read_u32(DMA0_BASE + DMA_ERQ).unwrap_or(0);
        let next = if enabled {
            current | mask
        } else {
            current & !mask
        };
        self.write_u32_raw(DMA0_BASE + DMA_ERQ, next);
    }

    fn handle_dma_command_u8(&mut self, address: u32, value: u8) -> bool {
        let offset = address.wrapping_sub(DMA0_BASE);
        match offset {
            DMA_CERQ => {
                self.apply_dma_channel_command(value, |memory, channel| {
                    memory.set_dma_erq(channel, false);
                });
                true
            }
            DMA_SERQ => {
                self.apply_dma_channel_command(value, |memory, channel| {
                    memory.set_dma_erq(channel, true);
                });
                true
            }
            DMA_CINT => {
                self.apply_dma_channel_command(value, |memory, channel| {
                    let mask = !(1 << channel);
                    let next = memory.read_u32(DMA0_BASE + DMA_INT).unwrap_or(0) & mask;
                    memory.write_u32_raw(DMA0_BASE + DMA_INT, next);
                    memory.clear_pending_irq(u16::from(channel));
                });
                true
            }
            _ => false,
        }
    }

    fn apply_dma_channel_command(&mut self, value: u8, mut apply: impl FnMut(&mut Self, u8)) {
        if value & 0x80 != 0 {
            return;
        }
        if value & 0x40 != 0 {
            for channel in 0..DMA0_TCD_COUNT as u8 {
                apply(self, channel);
            }
            return;
        }
        let channel = value & 0x1f;
        if u32::from(channel) < DMA0_TCD_COUNT {
            apply(self, channel);
        }
    }

    pub(super) fn next_pending_exception(&self, basepri: u32) -> Option<PendingException> {
        self.highest_pending_exception(basepri)
            .map(|exception| PendingException { exception })
    }

    pub(super) fn exception_handler(&self, exception: u16) -> Option<u32> {
        let vtor = self.read_u32(SCB_VTOR).unwrap_or(0) & !0x7f;
        let raw = self.read_u32(vtor + u32::from(exception) * 4)?;
        let handler = raw & !1;
        (raw & 1 != 0 && self.has_executable_halfword(handler)).then_some(handler)
    }

    pub(super) fn accept_exception(&mut self, exception: u16) {
        if exception == 14 {
            let next = self.read_u32(SCB_ICSR).unwrap_or(0) & !SCB_ICSR_PENDSVSET;
            self.write_u32_raw(SCB_ICSR, next);
        } else if exception == 15 {
            let next = self.read_u32(SCB_ICSR).unwrap_or(0) & !SCB_ICSR_PENDSTSET;
            self.write_u32_raw(SCB_ICSR, next);
            self.peripheral_read_counts.insert(SYSTICK_CTRL, 0);
        }
        if let Some(irq) = exception.checked_sub(16) {
            self.clear_pending_irq(irq);
            self.set_active_irq(irq);
        }
        self.set_active_exception_number(exception);
    }

    pub(super) fn finish_exception(&mut self, exception: u16) {
        if let Some(irq) = exception.checked_sub(16) {
            self.clear_active_irq(irq);
        }
        let next = self.read_u32(SCB_ICSR).unwrap_or(0) & !SCB_ICSR_VECTACTIVE_MASK;
        self.write_u32_raw(SCB_ICSR, next);
    }

    fn record_mmio_read(&mut self, address: u32) {
        if is_mmio_region(address) {
            *self.mmio_reads.entry(address).or_default() += 1;
        }
    }

    fn record_mmio_write(&mut self, address: u32) {
        if is_mmio_region(address) {
            *self.mmio_writes.entry(address).or_default() += 1;
        }
    }

    fn record_write_provenance(&mut self, address: u32, size: u8, value: u32) {
        if !is_provenance_region(address) {
            return;
        }
        let provenance = MemoryWriteProvenance {
            address,
            size,
            value,
            writer_pc: self.current_writer_pc,
            writer_cycle: self.current_writer_cycle,
        };
        for offset in 0..u32::from(size) {
            self.write_provenance
                .insert(address.wrapping_add(offset), provenance);
        }
    }

    fn record_unmapped_read(&mut self, address: u32, size: u8) {
        if !is_faulting_unmapped_read(address) {
            return;
        }
        let sample = UnmappedReadSample {
            address,
            size,
            pc: self.current_writer_pc,
            cycle: self.current_writer_cycle,
        };
        *self
            .unmapped_reads
            .entry((address, size, self.current_writer_pc))
            .or_default() += 1;
        if self.unmapped_read_samples.len() < 16 {
            self.unmapped_read_samples.push(sample);
        }
        self.pending_unmapped_read.get_or_insert(sample);
    }

    fn word_byte_probes(&self, address: u32) -> Vec<MemoryByteProbe> {
        (0..4)
            .map(|offset| {
                let byte_address = address.wrapping_add(offset);
                MemoryByteProbe {
                    address: byte_address,
                    value: self.read_u8(byte_address),
                    provenance: self.write_provenance.get(&byte_address).copied(),
                }
            })
            .collect()
    }

    fn looks_like_display_cell_collision(&self, address: u32, raw_value: u32) -> bool {
        let bytes = raw_value.to_le_bytes();
        let display_writer = self.word_byte_probes(address).into_iter().any(|probe| {
            probe
                .provenance
                .is_some_and(|provenance| is_m8_display_writer_pc(provenance.writer_pc))
        });
        let displayish_bytes = displayish_byte_count(bytes);

        display_writer && displayish_bytes >= 3
    }

    fn dwt_cycle_counter(&self) -> u32 {
        self.dwt_base_value
            .wrapping_add(self.virtual_cycles.wrapping_sub(self.dwt_base_cycle) as u32)
    }

    fn reset_systick_epoch(&mut self) {
        self.systick_epoch_cycle = self.virtual_cycles;
        self.systick_wrap_count = 0;
    }

    fn systick_period(&self) -> Option<u64> {
        let ctrl = self.read_u32(SYSTICK_CTRL).unwrap_or(0);
        let load = self.read_u32(SYSTICK_LOAD).unwrap_or(0) & 0x00ff_ffff;
        if ctrl & 1 == 0 || load == 0 {
            return None;
        }
        let ticks = u64::from(load) + 1;
        let scale = if ctrl & 0x4 == 0 {
            SYSTICK_EXTERNAL_CLOCK_DIVISOR
        } else {
            1
        };
        Some(ticks.saturating_mul(scale))
    }

    fn systick_value(&self) -> u32 {
        let ctrl = self.read_u32(SYSTICK_CTRL).unwrap_or(0);
        let load = self.read_u32(SYSTICK_LOAD).unwrap_or(0) & 0x00ff_ffff;
        let Some(period) = self.systick_period() else {
            return load;
        };
        let scale = if ctrl & 0x4 == 0 {
            SYSTICK_EXTERNAL_CLOCK_DIVISOR
        } else {
            1
        };
        let position =
            (self.virtual_cycles.wrapping_sub(self.systick_epoch_cycle) % period) / scale;
        load.saturating_sub(position as u32)
    }

    fn update_systick_from_clock(&mut self) {
        let Some(period) = self.systick_period() else {
            return;
        };
        let elapsed = self.virtual_cycles.wrapping_sub(self.systick_epoch_cycle);
        let wrap_count = elapsed / period;
        if wrap_count > self.systick_wrap_count {
            self.systick_wrap_count = wrap_count;
            self.systick_countflag = true;
            let ctrl = self.read_u32(SYSTICK_CTRL).unwrap_or(0);
            if ctrl & 0x2 != 0 {
                let next = self.read_u32(SCB_ICSR).unwrap_or(0) | SCB_ICSR_PENDSTSET;
                self.write_u32_raw(SCB_ICSR, next);
            }
        }
    }

    fn highest_pending_exception(&self, basepri: u32) -> Option<u16> {
        let mut best = None;

        if self.read_u32(SCB_ICSR).unwrap_or(0) & SCB_ICSR_PENDSVSET != 0 {
            self.consider_pending_exception(&mut best, 14, basepri);
        }
        if self.read_u32(SCB_ICSR).unwrap_or(0) & SCB_ICSR_PENDSTSET != 0 {
            self.consider_pending_exception(&mut best, 15, basepri);
        }

        for word in 0..8 {
            let enabled = self.read_u32(NVIC_ISER_BASE + word * 4).unwrap_or(0);
            let pending = self.read_u32(NVIC_ISPR_BASE + word * 4).unwrap_or(0);
            let mut ready = enabled & pending;
            while ready != 0 {
                let bit = ready.trailing_zeros();
                let irq = word * 32 + bit;
                if irq < 240 {
                    self.consider_pending_exception(&mut best, 16 + irq as u16, basepri);
                }
                ready &= !(1 << bit);
            }
        }

        best.map(|(exception, _)| exception)
    }

    fn consider_pending_exception(
        &self,
        best: &mut Option<(u16, u8)>,
        exception: u16,
        basepri: u32,
    ) {
        let priority = self.exception_priority(exception);
        if basepri != 0 && u32::from(priority) >= (basepri & 0xff) {
            return;
        }
        match *best {
            Some((best_exception, best_priority))
                if priority > best_priority
                    || (priority == best_priority && exception >= best_exception) => {}
            _ => *best = Some((exception, priority)),
        }
    }

    fn exception_priority(&self, exception: u16) -> u8 {
        if let Some(irq) = exception.checked_sub(16) {
            return self.read_u8(NVIC_IPR_BASE + u32::from(irq)).unwrap_or(0);
        }
        if (4..=15).contains(&exception) {
            let index = u32::from(exception - 4);
            return self.read_u8(SCB_SHPR1 + index).unwrap_or(0);
        }
        0
    }

    fn current_icsr_value(&self) -> u32 {
        let stored = self.read_u32(SCB_ICSR).unwrap_or(0);
        let active = stored & SCB_ICSR_VECTACTIVE_MASK;
        let pending = stored & SCB_ICSR_PENDING_MASK;
        let mut value = active | pending;
        if active != 0 {
            value |= SCB_ICSR_RETTOBASE;
        }
        if let Some(exception) = self.highest_pending_exception(0) {
            value |= SCB_ICSR_ISRPENDING | (u32::from(exception) << SCB_ICSR_VECTPENDING_SHIFT);
        }
        value
    }

    fn clear_pending_irq(&mut self, irq: u16) {
        let address = NVIC_ISPR_BASE + (u32::from(irq) / 32) * 4;
        let mask = 1 << (u32::from(irq) % 32);
        let next = self.read_u32(address).unwrap_or(0) & !mask;
        self.write_u32_raw(address, next);
    }

    fn set_pending_irq(&mut self, irq: u16) {
        let address = NVIC_ISPR_BASE + (u32::from(irq) / 32) * 4;
        let mask = 1 << (u32::from(irq) % 32);
        let next = self.read_u32(address).unwrap_or(0) | mask;
        self.write_u32_raw(address, next);
    }

    fn set_active_irq(&mut self, irq: u16) {
        let address = NVIC_IABR_BASE + (u32::from(irq) / 32) * 4;
        let mask = 1 << (u32::from(irq) % 32);
        let next = self.read_u32(address).unwrap_or(0) | mask;
        self.write_u32_raw(address, next);
    }

    fn clear_active_irq(&mut self, irq: u16) {
        let address = NVIC_IABR_BASE + (u32::from(irq) / 32) * 4;
        let mask = 1 << (u32::from(irq) % 32);
        let next = self.read_u32(address).unwrap_or(0) & !mask;
        self.write_u32_raw(address, next);
    }

    fn set_active_exception_number(&mut self, exception: u16) {
        let next = (self.read_u32(SCB_ICSR).unwrap_or(0) & !SCB_ICSR_VECTACTIVE_MASK)
            | u32::from(exception);
        self.write_u32_raw(SCB_ICSR, next);
    }

    fn complete_usdhc_command(&mut self, base: u32, command: u32) {
        let command_index = (command >> 24) & 0x3f;
        let argument = self.read_u32(base + USDHC_CMD_ARG).unwrap_or(0);
        let data_present = command & (1 << 21) != 0;
        let app_command = self
            .usdhc_app_command_pending
            .remove(&base)
            .unwrap_or(false);
        let count = self
            .usdhc_commands
            .entry((base, command_index, app_command, argument, data_present))
            .or_default();
        *count += 1;
        self.usdhc_recent_commands.push_back(UsdhcCommandStats {
            instance: if base == USDHC1_BASE {
                "USDHC1"
            } else {
                "USDHC2"
            },
            command_index,
            app_command,
            argument,
            data_present,
            count: *count,
            ds_addr: self.read_u32(base + USDHC_DS_ADDR).unwrap_or(0),
            block_attribute: self.read_u32(base + USDHC_BLK_ATT).unwrap_or(0),
            mix_ctrl: self.read_u32(base + USDHC_MIX_CTRL).unwrap_or(0),
            response0: 0,
            int_status: 0,
            int_status_en: 0,
            int_signal_en: 0,
            pending_data_status: 0,
            data_preview: Vec::new(),
        });
        while self.usdhc_recent_commands.len() > 64 {
            self.usdhc_recent_commands.pop_front();
        }
        let read_transfer = matches!(command_index, 17 | 18)
            || (!app_command && command_index == 6)
            || (app_command && command_index == 51);
        let write_transfer = matches!(command_index, 24 | 25);

        let response =
            usdhc_command_response(command_index, app_command, argument, self.sd_card.present());
        self.write_u32_raw(base + USDHC_CMD_RSP0, response[0]);
        self.write_u32_raw(base + USDHC_CMD_RSP1, response[1]);
        self.write_u32_raw(base + USDHC_CMD_RSP2, response[2]);
        self.write_u32_raw(base + USDHC_CMD_RSP3, response[3]);
        self.write_u32_raw(base + USDHC_PRES_STATE, USDHC_PRES_READY);
        let mut data_preview = Vec::new();
        if data_present && read_transfer {
            let words = match command_index {
                17 => self.sd_card.read_block_words(argument),
                18 => self
                    .sd_card
                    .read_multi_block_words(argument, self.transfer_block_count(base)),
                _ => {
                    let words = usdhc_command_data(
                        command_index,
                        app_command,
                        argument,
                        self.sd_switch_group1_function,
                    );
                    if !app_command && command_index == 6 {
                        self.sd_switch_group1_function = sd_switch_next_group1_function(
                            argument,
                            self.sd_switch_group1_function,
                        );
                    }
                    words
                }
            };
            data_preview = words
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .take(32)
                .collect();
            self.copy_usdhc_read_words_to_dma_buffer(base, &words);
            self.usdhc_data_words.insert(base, VecDeque::from(words));
        }
        if data_present && write_transfer {
            let block_count = if command_index == 24 {
                1
            } else {
                self.transfer_block_count(base)
            };
            self.usdhc_write_transfers
                .insert(base, UsdhcWriteTransfer::new(argument, block_count));
        }

        let mut data_status = 0;
        if data_present {
            data_status |= USDHC_INT_TC;
            if self.usdhc_dma_enabled(base) {
                data_status |= USDHC_INT_DINT;
            }
            if read_transfer {
                data_status |= USDHC_INT_BRR;
            }
            if write_transfer {
                data_status |= USDHC_INT_BWR;
            }
        }
        if data_status != 0 {
            self.usdhc_pending_data_status.insert(base, data_status);
        } else {
            self.usdhc_pending_data_status.remove(&base);
        }
        self.set_usdhc_interrupt(base, USDHC_INT_CC);
        let int_status = self.read_u32(base + USDHC_INT_STATUS).unwrap_or(0);
        let int_status_en = self.read_u32(base + USDHC_INT_STATUS_EN).unwrap_or(0);
        let int_signal_en = self.read_u32(base + USDHC_INT_SIGNAL_EN).unwrap_or(0);
        let pending_data_status = self
            .usdhc_pending_data_status
            .get(&base)
            .copied()
            .unwrap_or(0);
        if let Some(recent) = self.usdhc_recent_commands.back_mut() {
            recent.response0 = response[0];
            recent.int_status = int_status;
            recent.int_status_en = int_status_en;
            recent.int_signal_en = int_signal_en;
            recent.pending_data_status = pending_data_status;
            recent.data_preview = data_preview;
        }
        if command_index == 55 {
            self.usdhc_app_command_pending.insert(base, true);
        }
    }

    fn set_usdhc_interrupt(&mut self, base: u32, bits: u32) {
        let status_address = base + USDHC_INT_STATUS;
        let next = self.read_u32(status_address).unwrap_or(0) | bits;
        self.write_u32_raw(status_address, next);
        self.refresh_usdhc_irq(base);
    }

    fn commit_pending_usdhc_data_status(&mut self, base: u32) {
        let Some(bits) = self.usdhc_pending_data_status.remove(&base) else {
            return;
        };
        self.set_usdhc_interrupt(base, bits);
    }

    fn refresh_usdhc_irq(&mut self, base: u32) {
        let status = self.read_u32(base + USDHC_INT_STATUS).unwrap_or(0);
        let status_enabled = self
            .read_u32(base + USDHC_INT_STATUS_EN)
            .unwrap_or(u32::MAX);
        let signal_enabled = self.read_u32(base + USDHC_INT_SIGNAL_EN).unwrap_or(0);
        let irq = if base == USDHC1_BASE {
            USDHC_IRQ1
        } else {
            USDHC_IRQ2
        };
        if status & status_enabled & signal_enabled != 0 {
            self.set_pending_irq(irq);
        } else {
            self.clear_pending_irq(irq);
        }
    }

    fn clear_usdhc_irq(&mut self, base: u32) {
        let irq = if base == USDHC1_BASE {
            USDHC_IRQ1
        } else {
            USDHC_IRQ2
        };
        self.clear_pending_irq(irq);
    }

    fn copy_usdhc_read_words_to_dma_buffer(&mut self, base: u32, words: &[u32]) {
        let address = self.read_u32(base + USDHC_DS_ADDR).unwrap_or(0);
        if address == 0 || !is_ram_region(address) {
            return;
        }
        let byte_len = self.usdhc_transfer_byte_len(base, words.len());
        for (index, byte) in words
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .take(byte_len)
            .enumerate()
        {
            self.record_write_provenance(address + index as u32, 1, u32::from(byte));
            self.write_byte_raw(address + index as u32, byte);
        }
    }

    fn usdhc_transfer_byte_len(&self, base: u32, fallback_words: usize) -> usize {
        let block_attribute = self.read_u32(base + USDHC_BLK_ATT).unwrap_or(0);
        let fallback_bytes = fallback_words.saturating_mul(4);
        let raw_block_size = (block_attribute & 0x1fff) as usize;
        let block_size = if raw_block_size == 0 {
            fallback_bytes
        } else {
            raw_block_size
        };
        let block_count = ((block_attribute >> 16) & 0xffff).max(1) as usize;
        block_size.saturating_mul(block_count).min(fallback_bytes)
    }

    fn reset_usdhc_state(&mut self, base: u32) {
        for offset in [
            USDHC_CMD_RSP0,
            USDHC_CMD_RSP1,
            USDHC_CMD_RSP2,
            USDHC_CMD_RSP3,
            USDHC_DATA_BUFF_ACC_PORT,
            USDHC_INT_STATUS,
            USDHC_AUTOCMD12_ERR_STATUS,
        ] {
            self.write_u32_raw(base + offset, 0);
        }
        self.write_u32_raw(base + USDHC_PRES_STATE, USDHC_PRES_READY);
        self.usdhc_data_words.remove(&base);
        self.usdhc_pending_data_status.remove(&base);
        self.usdhc_write_transfers.remove(&base);
        self.usdhc_app_command_pending.remove(&base);
        self.clear_usdhc_irq(base);
    }

    fn read_usdhc_data_word(&mut self, base: u32) -> u32 {
        self.usdhc_data_words
            .get_mut(&base)
            .and_then(VecDeque::pop_front)
            .unwrap_or(0)
    }

    fn write_usdhc_data_word(&mut self, base: u32, value: u32) {
        if let Some(transfer) = self.usdhc_write_transfers.get_mut(&base) {
            if let Some((block, words)) = transfer.push_word(value) {
                self.sd_card.write_block_words(block, &words);
            }
            if transfer.is_complete() {
                self.usdhc_write_transfers.remove(&base);
            }
        }
        self.write_u32_raw(base + USDHC_DATA_BUFF_ACC_PORT, value);
    }

    fn transfer_block_count(&self, base: u32) -> u32 {
        let block_attribute = self.read_u32(base + USDHC_BLK_ATT).unwrap_or(0);
        let count = (block_attribute >> 16) & 0xffff;
        count.max(1)
    }

    fn usdhc_dma_enabled(&self, base: u32) -> bool {
        self.read_u32(base + USDHC_DS_ADDR).unwrap_or(0) != 0
            || self.read_u32(base + USDHC_MIX_CTRL).unwrap_or(0) & 1 != 0
    }
}

#[derive(Debug, Clone)]
struct UsdhcWriteTransfer {
    next_block: u32,
    remaining_blocks: u32,
    words: Vec<u32>,
}

impl UsdhcWriteTransfer {
    fn new(first_block: u32, block_count: u32) -> Self {
        Self {
            next_block: first_block,
            remaining_blocks: block_count.max(1),
            words: Vec::with_capacity(128),
        }
    }

    fn push_word(&mut self, value: u32) -> Option<(u32, Vec<u32>)> {
        self.words.push(value);
        if self.words.len() < 128 || self.remaining_blocks == 0 {
            return None;
        }

        let block = self.next_block;
        let words = std::mem::take(&mut self.words);
        self.next_block = self.next_block.wrapping_add(1);
        self.remaining_blocks = self.remaining_blocks.saturating_sub(1);
        Some((block, words))
    }

    fn is_complete(&self) -> bool {
        self.remaining_blocks == 0
    }
}

#[derive(Debug, Clone, Default)]
struct VirtualSdCard {
    bytes: Vec<u8>,
    read_blocks: u64,
    written_blocks: u64,
    last_read_block: Option<u32>,
    last_written_block: Option<u32>,
}

impl VirtualSdCard {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            ..Self::default()
        }
    }

    fn present(&self) -> bool {
        !self.bytes.is_empty()
    }

    fn stats(&self) -> VirtualSdCardStats {
        VirtualSdCardStats {
            present: self.present(),
            bytes: self.bytes.len(),
            blocks: self.block_count(),
            read_blocks: self.read_blocks,
            written_blocks: self.written_blocks,
            last_read_block: self.last_read_block,
            last_written_block: self.last_written_block,
        }
    }

    fn block_count(&self) -> u32 {
        (self.bytes.len() / 512).min(u32::MAX as usize) as u32
    }

    fn read_block_words(&mut self, block: u32) -> Vec<u32> {
        self.read_blocks = self.read_blocks.saturating_add(1);
        self.last_read_block = Some(block);
        self.read_block_bytes(block)
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    fn read_multi_block_words(&mut self, first_block: u32, block_count: u32) -> Vec<u32> {
        let mut words = Vec::new();
        for offset in 0..block_count.max(1) {
            words.extend(self.read_block_words(first_block.wrapping_add(offset)));
        }
        words
    }

    fn read_block_bytes(&self, block: u32) -> [u8; 512] {
        let mut out = [0; 512];
        let start = block as usize * 512;
        if start >= self.bytes.len() {
            return out;
        }
        let end = (start + 512).min(self.bytes.len());
        out[..end - start].copy_from_slice(&self.bytes[start..end]);
        out
    }

    fn write_block_words(&mut self, block: u32, words: &[u32]) {
        self.written_blocks = self.written_blocks.saturating_add(1);
        self.last_written_block = Some(block);
        let start = block as usize * 512;
        let end = start.saturating_add(512);
        if end > self.bytes.len() {
            self.bytes.resize(end, 0);
        }
        for (index, word) in words.iter().take(128).enumerate() {
            let offset = start + index * 4;
            self.bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
}

fn usdhc_command_response(
    command_index: u32,
    app_command: bool,
    argument: u32,
    sd_present: bool,
) -> [u32; 4] {
    if app_command {
        return match command_index {
            6 | 51 => [0x0000_0900, 0, 0, 0],
            41 => [if sd_present { 0xc0ff_8000 } else { 0x80ff_8000 }, 0, 0, 0],
            _ => [0x0000_0900, 0, 0, 0],
        };
    }

    match command_index {
        2 => [0x1b53_4d30, 0x3030_3030, 0x100f_0032, 0x5b59_0000],
        3 => [0x0001_0000, 0, 0, 0],
        8 => [argument & 0x0000_0fff, 0, 0, 0],
        9 => [0x400e_0032, 0x5b59_0000, 0x0000_0000, 0x0000_0000],
        10 => [0x1b53_4d30, 0x3030_3030, 0x100f_0032, 0x5b59_0000],
        6 | 7 | 13 | 51 => [0x0000_0900, 0, 0, 0],
        17 | 18 | 24 | 25 => [0x0000_0900, 0, 0, 0],
        41 => [if sd_present { 0xc0ff_8000 } else { 0x80ff_8000 }, 0, 0, 0],
        55 => [0x0000_0120, 0, 0, 0],
        _ => [0, 0, 0, 0],
    }
}

fn usdhc_command_data(
    command_index: u32,
    app_command: bool,
    argument: u32,
    current_group1_function: u8,
) -> Vec<u32> {
    if !app_command && command_index == 6 {
        return sd_switch_status_words(argument, current_group1_function);
    }
    if app_command && command_index == 51 {
        return sd_scr_words();
    }
    vec![0; 128]
}

fn sd_scr_words() -> Vec<u32> {
    let mut bytes = [0u8; 8];
    bytes[0] = 0x02;
    bytes[1] = 0x05;
    bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn sd_switch_status_words(argument: u32, current_group1_function: u8) -> Vec<u32> {
    let mut bytes = [0u8; 64];
    bytes[0] = 0x00;
    bytes[1] = 0x32;
    for group in 2..=6 {
        write_be_u16(&mut bytes, sd_switch_support_offset(group), 0x8001);
    }
    write_be_u16(&mut bytes, sd_switch_support_offset(1), 0x8003);

    let group1 = sd_switch_group1_function(argument, current_group1_function);
    let group2 = sd_switch_selected_function(argument, 2, 0x8001);
    let group3 = sd_switch_selected_function(argument, 3, 0x8001);
    let group4 = sd_switch_selected_function(argument, 4, 0x8001);
    let group5 = sd_switch_selected_function(argument, 5, 0x8001);
    let group6 = sd_switch_selected_function(argument, 6, 0x8001);
    bytes[14] = (group6 << 4) | group5;
    bytes[15] = (group4 << 4) | group3;
    bytes[16] = (group2 << 4) | group1;
    bytes[17] = 0x01;
    bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn write_be_u16(bytes: &mut [u8; 64], offset: usize, value: u16) {
    bytes[offset] = (value >> 8) as u8;
    bytes[offset + 1] = value as u8;
}

fn sd_switch_support_offset(group: u8) -> usize {
    2 + (6 - group as usize) * 2
}

fn sd_switch_requested_function(argument: u32, group: u8) -> u8 {
    ((argument >> ((group - 1) * 4)) & 0x0f) as u8
}

fn sd_switch_group1_function(argument: u32, current_group1_function: u8) -> u8 {
    let requested = sd_switch_requested_function(argument, 1);
    if requested == 0x0f {
        return current_group1_function.min(1);
    }
    if requested <= 1 { requested } else { 0x0f }
}

fn sd_switch_next_group1_function(argument: u32, current_group1_function: u8) -> u8 {
    if argument & 0x8000_0000 == 0 {
        return current_group1_function;
    }
    let selected = sd_switch_group1_function(argument, current_group1_function);
    if selected <= 1 {
        selected
    } else {
        current_group1_function
    }
}

fn sd_switch_selected_function(argument: u32, group: u8, supported_mask: u16) -> u8 {
    let requested = sd_switch_requested_function(argument, group);
    if requested == 0x0f {
        return 0;
    }
    if requested < 0x0f && (supported_mask & (1 << requested)) != 0 {
        requested
    } else {
        0x0f
    }
}

fn is_executable_region(address: u32) -> bool {
    (0x0000_0000..0x0008_0000).contains(&address)
        || (0x2020_0000..0x2030_0000).contains(&address)
        || (TEENSY_FLASH_BASE..TEENSY_FLASH_BASE + TEENSY_FLASH_SIZE).contains(&address)
}

fn is_flexspi_flash_region(address: u32) -> bool {
    (TEENSY_FLASH_BASE..TEENSY_FLASH_BASE + TEENSY_FLASH_SIZE).contains(&address)
}

fn is_provenance_probe_base(address: u32) -> bool {
    is_ram_region(address)
}

fn is_provenance_region(address: u32) -> bool {
    (0x0000_0000..0x0008_0000).contains(&address) || is_ram_region(address)
}

fn is_ram_region(address: u32) -> bool {
    (0x2000_0000..0x2008_0000).contains(&address) || (0x2020_0000..0x2030_0000).contains(&address)
}

fn is_mmio_region(address: u32) -> bool {
    (0x4000_0000..0x6000_0000).contains(&address) || (0xe000_0000..0xe010_0000).contains(&address)
}

fn is_faulting_unmapped_read(address: u32) -> bool {
    !is_executable_region(address) && !is_ram_region(address) && !is_mmio_region(address)
}

fn is_plausible_pointer_value(value: u32) -> bool {
    let aligned = value & !1;
    is_executable_region(aligned) || is_ram_region(aligned) || is_mmio_region(aligned)
}

fn looks_like_display_cell_pointer_address(value: u32) -> bool {
    value >> 24 == 0x20 && displayish_byte_count(value.to_le_bytes()) >= 3
}

fn looks_like_corrupt_ui_data_pointer_value(value: u32, pc: u32) -> bool {
    if looks_like_display_cell_pointer_address(value) {
        return true;
    }
    if !is_m8_runtime_pc(pc) {
        return false;
    }
    let bytes = value.to_le_bytes();
    displayish_byte_count(bytes) >= 2
}

fn is_m8_runtime_pc(address: u32) -> bool {
    (0x0000_0000..0x0008_0000).contains(&address) || (0x6001_0000..0x6020_0000).contains(&address)
}

fn displayish_byte_count(bytes: [u8; 4]) -> usize {
    bytes
        .iter()
        .filter(|byte| matches!(**byte, 0x00 | 0x20 | 0x30..=0x39 | 0x41..=0x5a | 0x61..=0x7a))
        .count()
}

fn is_m8_display_writer_pc(address: u32) -> bool {
    (0x6004_4a00..0x6004_4c20).contains(&address)
}

#[derive(Debug, Clone, Copy, Default)]
struct DmamuxChannelConfig {
    source: u8,
    enabled: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct DmaTcdState {
    saddr: u32,
    soff: i16,
    nbytes: u32,
    slast: i32,
    daddr: u32,
    doff: i16,
    citer: u16,
    dlast_sga: i32,
    csr: u16,
    biter: u16,
}

impl DmaTcdState {
    fn source_ring_base(&self) -> u32 {
        let biter = u32::from(self.biter & 0x7fff);
        let citer = u32::from(self.citer & 0x7fff).min(biter);
        let consumed = biter.saturating_sub(citer);
        self.saddr
            .wrapping_sub(consumed.saturating_mul(self.nbytes.clamp(1, 4)))
    }

    fn source_ring_bytes(&self) -> u32 {
        u32::from(self.biter & 0x7fff).saturating_mul(self.nbytes.clamp(1, 4))
    }
}

fn dma_tcd_address(channel: u8, offset: u32) -> u32 {
    DMA0_TCD_BASE + u32::from(channel) * DMA0_TCD_STRIDE + offset
}

fn is_audio_dmamux_source(source: u8) -> bool {
    matches!(
        source,
        DMAMUX_SOURCE_SAI1_TX | DMAMUX_SOURCE_SAI2_TX | DMAMUX_SOURCE_SAI3_TX | 19 | 21 | 83
    )
}

fn is_sai_tx_dmamux_source(source: u8) -> bool {
    matches!(
        source,
        DMAMUX_SOURCE_SAI1_TX | DMAMUX_SOURCE_SAI2_TX | DMAMUX_SOURCE_SAI3_TX
    )
}

fn is_sai_tx_address(address: u32) -> bool {
    sai_base(address)
        .map(|base| (base + SAI_TDR0..base + SAI_TDR0 + 0x10).contains(&address))
        .unwrap_or(false)
}

fn pcm_word_peak_abs_sample(word: u32) -> u16 {
    let left = (word as u16) as i16;
    let right = ((word >> 16) as u16) as i16;
    left.unsigned_abs().max(right.unsigned_abs())
}

fn top_counts(counts: &HashMap<u32, u64>, limit: usize) -> Vec<AddressCount> {
    let mut items: Vec<_> = counts
        .iter()
        .map(|(address, count)| AddressCount {
            address: *address,
            count: *count,
            label: address_label(*address),
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

fn top_audio_counts(
    counts: &HashMap<u32, u64>,
    kind: Option<AudioMmioKind>,
    limit: usize,
) -> Vec<AddressCount> {
    let mut items: Vec<_> = counts
        .iter()
        .filter_map(|(address, count)| {
            let address_kind = audio_mmio_kind(*address)?;
            if let Some(kind) = kind
                && address_kind != kind
            {
                return None;
            }
            Some(AddressCount {
                address: *address,
                count: *count,
                label: address_label(*address),
            })
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

fn top_usdhc_counts(counts: &HashMap<u32, u64>, limit: usize) -> Vec<AddressCount> {
    let mut items: Vec<_> = counts
        .iter()
        .filter_map(|(address, count)| {
            usdhc_base(*address)?;
            Some(AddressCount {
                address: *address,
                count: *count,
                label: address_label(*address),
            })
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

fn total_audio_counts(counts: &HashMap<u32, u64>) -> u64 {
    counts
        .iter()
        .filter(|(address, _)| audio_mmio_kind(**address).is_some())
        .map(|(_, count)| *count)
        .sum()
}
