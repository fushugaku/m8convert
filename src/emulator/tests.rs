use super::decode::*;
use super::display::*;
use super::memory::*;
use super::mmio::*;
use super::*;

#[test]
fn parses_and_normalizes_low_address_hex() {
    let hex = b":08000000000002200100006075\n:00000001FF\n";
    let analysis = analyze_teensy_hex(hex).expect("hex should parse");

    assert_eq!(analysis.byte_count, 8);
    assert_eq!(analysis.runtime_base_applied, Some(TEENSY_FLASH_BASE));
    assert_eq!(analysis.vector_table.unwrap().reset_pc_aligned, 0x6000_0000);
}

#[test]
fn finds_vector_table_at_start_linear_address_with_stack_at_ram_end() {
    let mut hex = String::new();
    hex.push_str(&hex_record(0, 0x04, &[0x60, 0x00]));
    hex.push_str(&hex_record(
        0x1000,
        0x00,
        &[0x00, 0x00, 0x08, 0x20, 0x01, 0x20, 0x00, 0x60],
    ));
    hex.push_str(&hex_record(0, 0x05, &[0x60, 0x00, 0x10, 0x00]));
    hex.push_str(":00000001FF\n");

    let analysis = analyze_teensy_hex(hex.as_bytes()).expect("hex should parse");
    let vector = analysis.vector_table.expect("vector table");
    assert_eq!(vector.address, 0x6000_1000);
    assert_eq!(vector.initial_sp, 0x2008_0000);
    assert_eq!(vector.initial_sp_region, Some("DTCM"));
    assert_eq!(vector.reset_pc_aligned, 0x6000_2000);
}

#[test]
fn finds_teensy_imxrt_boot_image_ivt() {
    let hex = minimal_teensy_ivt_hex();

    let analysis = analyze_teensy_hex(hex.as_bytes()).expect("hex should parse");
    let boot = analysis.boot_image.expect("boot image");
    assert_eq!(boot.ivt_address, 0x6000_1000);
    assert_eq!(boot.entry, 0x6000_5c25);
    assert_eq!(boot.entry_aligned, 0x6000_5c24);
    assert_eq!(boot.image_start, 0x6000_0000);
    assert_eq!(boot.image_size, 0x001c_b000);
    assert!(analysis.vector_table.is_none());
}

#[test]
fn boot_probe_uses_ivt_entry_when_vector_table_is_absent() {
    let hex = minimal_teensy_ivt_hex();
    let probe = probe_teensy_hex_boot(hex.as_bytes(), 1).expect("boot probe");

    assert_eq!(probe.cpu.registers[15], 0x6000_5c24);
    assert_eq!(probe.cpu.registers[13], 0);
    assert!(probe.trace[0].note.contains("IVT entry"));
}

#[test]
fn boot_probe_decodes_thumb_literal_load() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(
        0x5c24,
        0x00,
        &[0x00, 0x4b, 0x00, 0xbf, 0xef, 0xbe, 0xad, 0xde],
    ));
    hex.push_str(":00000001FF\n");

    let probe = probe_teensy_hex_boot(hex.as_bytes(), 2).expect("boot probe");
    assert_eq!(probe.cpu.registers[3], 0xdead_beef);
    assert!(probe.trace[1].note.starts_with("LDR r3"));
}

#[test]
fn boot_probe_counts_external_callback_stubs() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(0x5c24, 0x00, &[0x98, 0x47, 0x00, 0xbf]));
    hex.push_str(":00000001FF\n");

    let probe = probe_teensy_hex_boot(hex.as_bytes(), 2).expect("boot probe");

    assert_eq!(probe.stats.external_callback_stubs, 1);
    assert_eq!(probe.stats.callback_targets[0].target, 0);
    assert_eq!(probe.stats.callback_targets[0].callsite, 0x6000_5c24);
    assert_eq!(probe.stats.callback_targets[0].count, 1);
    assert_eq!(probe.stats.callback_samples[0].target, 0);
    assert_eq!(probe.stats.callback_samples[0].callsite, 0x6000_5c24);
    assert!(probe.trace[1].note.contains("stubbed external callback"));
}

#[test]
fn boot_probe_stops_on_unmapped_data_read() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(
        0x5c24,
        0x00,
        &[
            0x01, 0x48, // LDR r0, [PC, #0x4]
            0x01, 0x68, // LDR r1, [r0, #0x0]
            0x00, 0xbf, // NOP
            0x00, 0xbf, // NOP
            0x30, 0x00, 0x30, 0x30, // 0x30300030
        ],
    ));
    hex.push_str(":00000001FF\n");

    let probe = probe_teensy_hex_boot(hex.as_bytes(), 4).expect("boot probe");

    assert_eq!(probe.status, "boot-probe-hit-unmapped-read");
    assert_eq!(probe.stats.unmapped_reads[0].address, 0x3030_0030);
    assert_eq!(probe.stats.unmapped_reads[0].size, 4);
    assert_eq!(probe.stats.unmapped_reads[0].pc, 0x6000_5c26);
    assert_eq!(probe.stats.unmapped_read_samples[0].address, 0x3030_0030);
    assert_eq!(
        probe.stats.unmapped_read_context_samples[0].address,
        0x3030_0030
    );
    assert_eq!(
        probe.stats.unmapped_read_context_samples[0].callsite,
        0x6000_5c26
    );
    assert!(
        probe
            .trace
            .iter()
            .any(|event| event.note.contains("unmapped data read 0x30300030"))
    );
}

#[test]
fn live_step_summary_exposes_structured_stop_event_opcode() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(0x5c24, 0x00, &[0x00, 0xbe]));
    hex.push_str(":00000001FF\n");

    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    let summary = session.run_steps_summary(1);

    assert!(summary.stopped);
    assert_eq!(summary.status, "boot-probe-hit-breakpoint");
    assert_eq!(summary.last_trace_pc, Some(0x6000_5c24));
    assert_eq!(summary.last_trace_opcode16, Some(0xbe00));
    assert_eq!(summary.stop_event_pc, Some(0x6000_5c24));
    assert_eq!(summary.stop_event_opcode16, Some(0xbe00));
    assert!(
        summary
            .stop_event_note
            .as_deref()
            .is_some_and(|note| note.contains("BKPT"))
    );
}

#[test]
fn boot_probe_reports_hot_pcs() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(
        0x5c24,
        0x00,
        &[
            0x00, 0xbf, // NOP
            0xfd, 0xe7, // B back to the NOP
        ],
    ));
    hex.push_str(":00000001FF\n");

    let probe = probe_teensy_hex_boot(hex.as_bytes(), 8).expect("boot probe");

    assert_eq!(probe.stats.hot_pcs[0].address, 0x6000_5c24);
    assert_eq!(probe.stats.hot_pcs[0].count, 4);
    assert_eq!(probe.stats.hot_pcs[1].address, 0x6000_5c26);
    assert_eq!(probe.stats.hot_pcs[1].count, 4);
    assert_eq!(probe.stats.hot_edges[0].from, 0x6000_5c24);
    assert_eq!(probe.stats.hot_edges[0].to, 0x6000_5c26);
    assert_eq!(probe.stats.hot_edges[0].count, 4);
}

#[test]
fn boot_probe_can_start_with_loaded_sd_image() {
    let probe = probe_teensy_hex_boot_with_sd(
        minimal_teensy_ivt_hex().as_bytes(),
        1,
        Some(&vec![0u8; 1024]),
    )
    .expect("boot probe");

    assert!(probe.stats.sd_card.present);
    assert_eq!(probe.stats.sd_card.blocks, 2);
}

#[test]
fn execution_memory_reports_mmio_hotspots() {
    let mut memory = test_memory(vec![]);

    assert_eq!(memory.read_u32_or_zero(SCB_CPUID), 0x411f_c271);
    assert_eq!(memory.read_u32_or_zero(SCB_CPUID), 0x411f_c271);
    memory.write_u32(SCB_CCR, 0x200);

    assert_eq!(memory.top_mmio_reads(1)[0].address, SCB_CPUID);
    assert_eq!(memory.top_mmio_reads(1)[0].count, 2);
    assert_eq!(
        memory.top_mmio_reads(1)[0].label.as_deref(),
        Some("SCB CPUID")
    );
    assert_eq!(memory.top_mmio_writes(1)[0].address, SCB_CCR);
    assert_eq!(memory.top_mmio_writes(1)[0].count, 1);
    assert_eq!(
        memory.top_mmio_writes(1)[0].label.as_deref(),
        Some("SCB CCR")
    );
}

#[test]
fn execution_memory_reports_usdhc_mmio_hotspots() {
    let mut memory = test_memory(vec![]);

    assert_eq!(memory.read_u32_or_zero(USDHC1_BASE + USDHC_INT_STATUS), 0);
    assert_eq!(memory.read_u32_or_zero(USDHC1_BASE + USDHC_INT_STATUS), 0);
    memory.write_u32(USDHC1_BASE + USDHC_INT_STATUS_EN, USDHC_INT_CC);

    let reads = memory.top_usdhc_reads(1);
    assert_eq!(reads[0].address, USDHC1_BASE + USDHC_INT_STATUS);
    assert_eq!(reads[0].count, 2);
    assert_eq!(reads[0].label.as_deref(), Some("USDHC1 INT_STATUS"));

    let writes = memory.top_usdhc_writes(1);
    assert_eq!(writes[0].address, USDHC1_BASE + USDHC_INT_STATUS_EN);
    assert_eq!(writes[0].count, 1);
    assert_eq!(writes[0].label.as_deref(), Some("USDHC1 INT_STATUS_EN"));
}

#[test]
fn execution_memory_reports_audio_mmio_hotspots() {
    let mut memory = test_memory(vec![]);

    assert_eq!(memory.read_u32_or_zero(SAI1_BASE + 0x08), 0);
    memory.write_u32(DMA0_TCD_BASE, 0x2020_0000);
    memory.write_u32(DMAMUX_BASE, 0x8000_0014);
    memory.write_u32(CCM_ANALOG_PLL_AUDIO, 0x8000_3000);

    let stats = memory.audio_stats();
    assert!(stats.observed);
    assert_eq!(stats.total_reads, 1);
    assert_eq!(stats.total_writes, 3);
    assert_eq!(stats.sai_reads[0].label.as_deref(), Some("SAI1 TCSR"));
    assert_eq!(
        stats.dma_writes[0].label.as_deref(),
        Some("DMA0 TCD0 SADDR+0x0")
    );
    assert_eq!(
        stats.dmamux_writes[0].label.as_deref(),
        Some("DMAMUX CHCFG0")
    );
    assert_eq!(
        stats.clock_writes[0].label.as_deref(),
        Some("CCM_ANALOG PLL_AUDIO")
    );
}

#[test]
fn audio_dma_service_moves_sai_tx_words_and_pends_dma_irq() {
    let mut memory = test_memory(vec![]);
    let sample_buffer = 0x2020_1000;
    memory.write_u32(sample_buffer, 0x1122_3344);
    memory.write_u32(DMA0_TCD_BASE, sample_buffer);
    memory.write_u16(DMA0_TCD_BASE + 0x04, 4);
    memory.write_u32(DMA0_TCD_BASE + 0x08, 4);
    memory.write_u32(DMA0_TCD_BASE + 0x0c, (-4_i32) as u32);
    memory.write_u32(DMA0_TCD_BASE + 0x10, SAI1_BASE + SAI_TDR0);
    memory.write_u16(DMA0_TCD_BASE + 0x16, 1);
    memory.write_u16(DMA0_TCD_BASE + 0x1c, 1 << 1);
    memory.write_u16(DMA0_TCD_BASE + 0x1e, 1);
    memory.write_u32(DMAMUX_BASE, 0x8000_0014);
    memory.write_u32(NVIC_ISER_BASE, 1);

    memory.write_u8(DMA0_BASE + DMA_SERQ, 0);
    memory.advance_virtual_cycles(AUDIO_DMA_SERVICE_PERIOD);

    assert_eq!(memory.read_u32(SAI1_BASE + SAI_TDR0), Some(0x1122_3344));
    let stats = memory.audio_stats();
    assert_eq!(stats.dma_transfers, 1);
    assert_eq!(stats.captured_pcm_words, 1);
    assert_eq!(stats.nonzero_pcm_words, 1);
    assert_eq!(stats.last_pcm_word, Some(0x1122_3344));
    assert_eq!(stats.last_nonzero_pcm_word, Some(0x1122_3344));
    assert_eq!(stats.peak_abs_sample, 0x3344);
    assert_eq!(memory.take_audio_pcm_words(), vec![0x1122_3344]);
    assert!(memory.take_audio_pcm_words().is_empty());
    assert!(stats.active_dma_channels[0].erq_enabled);
    assert_eq!(stats.active_dma_channels[0].dmamux_source, 20);
    assert_eq!(stats.active_dma_channels[0].source_ring_base, sample_buffer);
    assert_eq!(stats.active_dma_channels[0].source_ring_bytes, 4);
    assert_eq!(stats.active_dma_channels[0].source_ring_nonzero_words, 1);
    assert_eq!(
        stats.active_dma_channels[0].source_ring_peak_abs_sample,
        0x3344
    );
    assert_eq!(
        stats.active_dma_channels[0].source_preview_words[0],
        0x1122_3344
    );
    assert_eq!(
        memory
            .next_pending_exception(0)
            .map(|pending| pending.exception),
        Some(16)
    );
}

#[test]
fn execution_memory_reports_unmapped_read_hotspots() {
    let mut memory = test_memory(vec![]);
    memory.set_current_instruction(0x0002_a5a0, 44);

    assert_eq!(memory.read_u32_or_zero(0x3030_0030), 0);

    let top = memory.top_unmapped_reads(1);
    assert_eq!(top[0].address, 0x3030_0030);
    assert_eq!(top[0].size, 4);
    assert_eq!(top[0].pc, 0x0002_a5a0);
    assert_eq!(top[0].count, 1);
    let pending = memory.take_pending_unmapped_read().expect("pending read");
    assert_eq!(pending.address, 0x3030_0030);
    assert_eq!(pending.cycle, 44);
}

#[test]
fn it_block_implicit_add_does_not_clobber_flags() {
    let records = vec![DataRecord {
        address: 0x6000_0000,
        bytes: vec![
            0x08, 0xbf, // IT EQ
            0x02, 0x30, // ADDS r0, #0x2
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[0] = 3;
    cpu.registers[15] = 0x6000_0000;
    cpu.apsr.z = true;

    let it = step_thumb_trace(&mut memory, &mut cpu);
    let add = step_thumb_trace(&mut memory, &mut cpu);

    assert!(it.note.starts_with("IT EQ"));
    assert!(add.note.starts_with("ADDS r0, #0x2"));
    assert_eq!(cpu.registers[0], 5);
    assert!(cpu.apsr.z);
    assert!(!cpu.it_suppresses_flags);
}

#[test]
fn it_block_cmp_still_updates_flags() {
    let records = vec![DataRecord {
        address: 0x6000_0000,
        bytes: vec![
            0x08, 0xbf, // IT EQ
            0x04, 0x28, // CMP r0, #0x4
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[0] = 5;
    cpu.registers[15] = 0x6000_0000;
    cpu.apsr.z = true;

    step_thumb_trace(&mut memory, &mut cpu);
    let cmp = step_thumb_trace(&mut memory, &mut cpu);

    assert!(cmp.note.starts_with("CMP r0, #0x4"));
    assert!(!cpu.apsr.z);
    assert!(cpu.apsr.c);
}

#[test]
fn callback_memory_probes_report_last_ram_writer() {
    let mut memory = test_memory(vec![]);
    let mut registers = [0; 16];
    registers[10] = 0x2001_4b44;

    memory.set_current_instruction(0x6004_4b7e, 12_345);
    memory.write_u8(0x2001_4b4c, 0x30);

    let probes = memory.callback_memory_probes(&registers);
    let probe = probes
        .iter()
        .find(|probe| probe.register == 10 && probe.offset == 8)
        .expect("r10 + 8 provenance");

    assert_eq!(probe.address, 0x2001_4b4c);
    assert_eq!(probe.value, None);
    let provenance = probe.provenance.expect("write provenance");
    assert_eq!(provenance.writer_pc, 0x6004_4b7e);
    assert_eq!(provenance.writer_cycle, 12_345);
    assert_eq!(provenance.size, 1);
    assert_eq!(provenance.value, 0x30);
    assert_eq!(probe.bytes.len(), 4);
    assert_eq!(probe.bytes[0].value, Some(0x30));
}

#[test]
fn display_snapshot_decodes_m8_cell_grid_rows() {
    let mut memory = test_memory(vec![]);
    let row_base = 0x2001_4a07 + 2 * 0x28;

    memory.write_u8(row_base + 4, b'M');
    memory.write_u8(row_base + 5, b'8');
    memory.write_u8(row_base + 7, b'O');
    memory.write_u8(row_base + 8, b'K');

    let snapshot = build_display_snapshot(&memory);
    let candidate = snapshot
        .candidates
        .iter()
        .find(|candidate| candidate.name == "M8 cell grid")
        .expect("M8 grid candidate");

    assert!(candidate.rows[2].starts_with("M8 OK"));
    assert_eq!(candidate.non_blank_cells, 4);
    assert_eq!(snapshot.preferred.as_deref(), Some("M8 cell grid"));
}

#[test]
fn display_snapshot_prefers_full_render_cache_when_present() {
    let mut memory = test_memory(vec![]);
    for (offset, byte) in b"M8 HEADLESS".iter().copied().enumerate() {
        memory.write_u8(0x2001_365a + offset as u32, byte);
        memory.write_u16(0x2001_3a1c + offset as u32 * 2, 0x4a8c);
    }

    let snapshot = build_display_snapshot(&memory);
    let candidate = snapshot
        .candidates
        .iter()
        .find(|candidate| candidate.name == "M8 render cache")
        .expect("render cache candidate");

    assert!(candidate.rows[0].starts_with("M8 HEADLESS"));
    assert_eq!(candidate.styles[0][0], 0x4a8c);
    assert_eq!(candidate.width, 40);
    assert_eq!(candidate.height, 24);
    assert_eq!(snapshot.preferred.as_deref(), Some("M8 render cache"));
}

#[test]
fn live_display_stats_prefer_render_cache_when_present() {
    let mut memory = test_memory(vec![]);
    for (offset, byte) in b"M8".iter().copied().enumerate() {
        memory.write_u8(0x2001_365a + offset as u32, byte);
        memory.write_u16(0x2001_3a1c + offset as u32 * 2, 0x4a8c);
    }

    let stats = build_memory_live_display_stats(&memory);

    assert_eq!(stats.source, "render-cache");
    assert_eq!(stats.preferred.as_deref(), Some("M8 render cache"));
    assert_eq!(stats.format.as_deref(), Some("m8-render-cache"));
    assert_eq!(stats.non_blank_cells, 2);
}

#[test]
fn live_display_stats_do_not_fall_back_to_m8_cell_grid() {
    let mut memory = test_memory(vec![]);
    let row_base = 0x2001_4a07 + 2 * 0x28;
    memory.write_u8(row_base + 4, b'M');
    memory.write_u8(row_base + 5, b'8');

    let stats = build_memory_live_display_stats(&memory);

    assert_eq!(stats.source, "blank");
    assert!(stats.preferred.is_none());
    assert!(stats.format.is_none());
    assert_eq!(stats.non_blank_cells, 0);
}

#[test]
fn live_display_stats_ignore_ascii_scratch_until_real_display_buffer_is_mapped() {
    let mut memory = test_memory(vec![]);
    for (offset, byte) in b"M8 HEADLESS".iter().copied().enumerate() {
        memory.write_u8(0x2001_3200 + offset as u32, byte);
    }

    let stats = build_memory_live_display_stats(&memory);

    assert_eq!(stats.source, "blank");
    assert_eq!(stats.non_blank_cells, 0);
    assert!(stats.preferred.is_none());
}

#[test]
fn display_slip_frames_follow_m8webdisplay_text_protocol() {
    let mut memory = test_memory(vec![]);
    let mut cache = M8DisplayFrameCache::default();

    memory.write_u8(0x2001_365a, b'A');
    memory.write_u16(0x2001_3a1c, 0xf800);

    let frames = build_m8webdisplay_slip_frames(&memory, &mut cache);
    let decoded = decode_slip_frames(&frames);

    assert_eq!(decoded[0], vec![0xff, 0, 0, 0, 0, 0]);
    assert_eq!(
        decoded[1],
        vec![0xfe, 0, 0, 0, 0, 0x40, 0x01, 0xf0, 0, 0, 0, 0]
    );
    assert_eq!(&decoded[2][0..6], &[0xfd, b'A', 0, 0, 0, 0]);
    assert_eq!(&decoded[2][6..9], &[255, 0, 0]);
    assert_eq!(decoded.len(), 40 * 24 + 2);
    assert!(
        decoded[3..]
            .iter()
            .all(|frame| frame.starts_with(&[0xfd, b' '])),
        "reset frames must clear all unused M8WebDisplay text nodes"
    );
    let stats = live_display_stats(&cache);
    assert_eq!(stats.source, "render-cache");
    assert_eq!(stats.non_blank_cells, 1);

    let next = build_m8webdisplay_slip_frames(&memory, &mut cache);
    let next_decoded = decode_slip_frames(&next);
    assert!(next_decoded.is_empty());
}

#[test]
fn display_slip_frames_ignore_zero_style_render_cache_garbage() {
    let mut memory = test_memory(vec![]);
    let mut cache = M8DisplayFrameCache::default();

    memory.write_u8(0x2001_365a, b'0');
    memory.write_u8(0x2001_365b, b'S');
    memory.write_u16(0x2001_3a1e, 0x07e0);

    let frames = build_m8webdisplay_slip_frames(&memory, &mut cache);
    let decoded = decode_slip_frames(&frames);

    assert!(
        !decoded
            .iter()
            .any(|frame| frame.starts_with(&[0xfd, b'0', 0, 0, 0, 0])),
        "zero-style stale cache byte must not be rendered as visible text"
    );
    assert!(
        decoded
            .iter()
            .any(|frame| frame.starts_with(&[0xfd, b'S', 8, 0, 0, 0]))
    );
    let stats = live_display_stats(&cache);
    assert_eq!(stats.source, "render-cache");
    assert_eq!(stats.non_blank_cells, 1);
    assert_eq!(stats.printable_cells, 1);
}

#[test]
fn display_slip_frames_ignore_m8_cell_grid_when_render_cache_is_empty() {
    let mut memory = test_memory(vec![]);
    let mut cache = M8DisplayFrameCache::default();
    let row_base = 0x2001_4a07 + 2 * 0x28;

    memory.write_u8(row_base + 4, b'0');
    memory.write_u8(row_base + 5, b'1');
    memory.write_u8(row_base + 7, b'2');

    let frames = build_m8webdisplay_slip_frames(&memory, &mut cache);
    let decoded = decode_slip_frames(&frames);

    assert_eq!(decoded[0], vec![0xff, 0, 0, 0, 0, 0]);
    assert_eq!(
        decoded[1],
        vec![0xfe, 0, 0, 0, 0, 0x40, 0x01, 0xf0, 0, 0, 0, 0]
    );
    assert!(
        !decoded.iter().any(|frame| {
            frame.starts_with(&[0xfd, b'0'])
                || frame.starts_with(&[0xfd, b'1'])
                || frame.starts_with(&[0xfd, b'2'])
        }),
        "diagnostic cell-grid bytes must not be emitted as visible M8WebDisplay text"
    );
    assert_eq!(decoded.len(), 40 * 24 + 2);
    let stats = live_display_stats(&cache);
    assert_eq!(stats.source, "blank");
    assert_eq!(stats.non_blank_cells, 0);
}

#[test]
fn display_slip_frames_clear_canvas_before_redraw() {
    let mut memory = test_memory(vec![]);
    let mut cache = M8DisplayFrameCache::default();

    memory.write_u8(0x2001_365a, b'S');
    memory.write_u16(0x2001_3a1c, 0x07e0);
    let initial = build_m8webdisplay_slip_frames(&memory, &mut cache);
    assert!(!initial.is_empty());

    memory.write_u8(0x2001_365a, b' ');
    let blank_redraw = build_m8webdisplay_slip_frames(&memory, &mut cache);
    let decoded = decode_slip_frames(&blank_redraw);
    assert_eq!(decoded.len(), 40 * 24 + 2);
    assert_eq!(decoded[0], vec![0xff, 0, 0, 0, 0, 0]);
    assert_eq!(
        decoded[1],
        vec![0xfe, 0, 0, 0, 0, 0x40, 0x01, 0xf0, 0, 0, 0, 0]
    );
    assert_eq!(&decoded[2][0..6], &[0xfd, b' ', 0, 0, 0, 0]);
    assert_eq!(&decoded[2][6..9], &[0, 0, 0]);
    assert!(
        decoded[2..]
            .iter()
            .all(|frame| frame.starts_with(&[0xfd, b' '])),
        "blank reset must clear the full SVG text layer"
    );
}

#[test]
fn display_slip_frames_redraw_only_changed_cells() {
    let mut memory = test_memory(vec![]);
    let mut cache = M8DisplayFrameCache::default();

    memory.write_u8(0x2001_365a, b'A');
    memory.write_u16(0x2001_3a1c, 0xf800);
    let initial = build_m8webdisplay_slip_frames(&memory, &mut cache);
    assert_eq!(decode_slip_frames(&initial).len(), 40 * 24 + 2);

    memory.write_u8(0x2001_365b, b'B');
    memory.write_u16(0x2001_3a1e, 0x07e0);
    let changed = build_m8webdisplay_slip_frames(&memory, &mut cache);
    let decoded = decode_slip_frames(&changed);

    assert_eq!(decoded.len(), 1);
    assert_eq!(&decoded[0][0..6], &[0xfd, b'B', 8, 0, 0, 0]);
}

#[test]
fn display_slip_frames_clear_text_nodes_for_cells_that_disappear() {
    let mut memory = test_memory(vec![]);
    let mut cache = M8DisplayFrameCache::default();

    memory.write_u8(0x2001_365a, b'A');
    memory.write_u16(0x2001_3a1c, 0xf800);
    memory.write_u8(0x2001_365b, b'B');
    memory.write_u16(0x2001_3a1e, 0x07e0);
    let initial = build_m8webdisplay_slip_frames(&memory, &mut cache);
    assert_eq!(decode_slip_frames(&initial).len(), 40 * 24 + 2);

    memory.write_u16(0x2001_3a1e, 0);
    let changed = build_m8webdisplay_slip_frames(&memory, &mut cache);
    let decoded = decode_slip_frames(&changed);

    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0], vec![0xfe, 8, 0, 0, 0, 8, 0, 10, 0, 0, 0, 0]);
    assert_eq!(&decoded[1][0..6], &[0xfd, b' ', 8, 0, 0, 0]);
    assert_eq!(&decoded[1][6..9], &[0, 0, 0]);
}

#[test]
fn display_slip_frames_defer_one_off_large_render_cache_changes() {
    let mut memory = test_memory(vec![]);
    let mut cache = M8DisplayFrameCache::default();

    for index in 0..80 {
        memory.write_u8(0x2001_365a + index, b'A');
        memory.write_u16(0x2001_3a1c + index * 2, 0x07e0);
    }
    let initial = build_m8webdisplay_slip_frames(&memory, &mut cache);
    assert!(!initial.is_empty());

    for index in 0..80 {
        memory.write_u8(0x2001_365a + index, b'B');
    }
    let first_changed = build_m8webdisplay_slip_frames(&memory, &mut cache);
    assert!(
        first_changed.is_empty(),
        "large same-source jumps should wait for one matching snapshot"
    );

    let repeated = build_m8webdisplay_slip_frames(&memory, &mut cache);
    let decoded = decode_slip_frames(&repeated);
    assert!(!decoded.is_empty());
    assert!(decoded.iter().any(|frame| frame.starts_with(&[0xfd, b'B'])));
}

#[test]
fn execution_memory_reads_dense_rom_and_preserves_write_overlay() {
    let mut memory = test_memory(vec![DataRecord {
        address: 0x6000_0000,
        bytes: vec![0x11, 0x22, 0x33, 0x44],
    }]);

    assert_eq!(memory.read_u32(0x6000_0000), Some(0x4433_2211));

    memory.write_u8(0x6000_0001, 0xaa);
    assert_eq!(memory.read_u32(0x6000_0000), Some(0x4433_aa11));

    memory.write_u16(0x6000_0002, 0xbbcc);
    assert_eq!(memory.read_u32(0x6000_0000), Some(0xbbcc_aa11));
}

#[test]
fn runtime_writes_do_not_mutate_flexspi_flash_instructions() {
    let mut memory = test_memory(vec![DataRecord {
        address: 0x6004_a638,
        bytes: vec![0x94, 0xf8, 0xe4, 0x32],
    }]);

    memory.set_current_instruction(0x6004_aa64, 123);
    memory.write_u8(0x6004_a639, 0xf7);

    assert_eq!(memory.read_u32(0x6004_a638), Some(0x32e4_f894));
}

#[test]
fn teensy_session_runs_incrementally_and_keeps_state() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(
        0x5c24,
        0x00,
        &[0x00, 0x4b, 0x00, 0xbf, 0xef, 0xbe, 0xad, 0xde],
    ));
    hex.push_str(":00000001FF\n");

    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    let first = session.run_steps_summary(1);
    assert_eq!(first.executed_steps, 1);
    assert_eq!(first.pc, 0x6000_5c26);
    assert_eq!(first.status, "boot-probe-running");

    session.send_joypad_state(0b0100_0000);
    let second = session.run_steps_summary(1);
    assert_eq!(second.executed_steps, 1);
    assert_eq!(second.joypad_state, 0b0100_0000);
    assert_eq!(session.snapshot().cpu.registers[3], 0xdead_beef);
}

#[test]
fn teensy_session_tracks_m8webdisplay_host_input_packets() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");

    session.receive_host_bytes(&[0x43, 0b1000_0000]);
    let stats = session.host_input_stats();

    assert_eq!(session.run_steps_summary(1).joypad_state, 0b1000_0000);
    assert_eq!(stats.packets, 1);
    assert_eq!(stats.bytes, 2);
    assert_eq!(stats.queued_bytes, 0);
    assert_eq!(stats.cdc_rx_available_bytes, 0);
    assert_eq!(stats.last_packet, vec![0x43, 0b1000_0000]);
    assert_eq!(
        stats.last_packet_label.as_deref(),
        Some("controller 0b10000000")
    );
}

#[test]
fn m8_display_enable_marks_remote_serial_active() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");

    session.receive_host_bytes(&[0x45, 0x52]);
    let enabled = session.host_input_stats();
    assert_eq!(enabled.queued_bytes, 0);
    assert_eq!(enabled.observed_m8_remote_serial_connected, Some(true));
    assert_eq!(enabled.observed_m8_remote_serial_active, Some(true));
    assert_eq!(enabled.observed_m8_remote_serial_busy, Some(false));

    session.receive_host_bytes(&[0x44]);
    let disabled = session.host_input_stats();
    assert_eq!(disabled.observed_m8_remote_serial_connected, Some(true));
    assert_eq!(disabled.observed_m8_remote_serial_active, Some(false));
}

#[test]
fn m8_usb_serial_getchar_shim_consumes_host_input() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.cpu.registers[14] = 0x6000_5c27;
    session.cpu.registers[15] = M8_USB_SERIAL_GETCHAR;
    session.receive_host_bytes(&[0x52, 0x53]);

    let summary = session.run_steps_summary(1);

    assert_eq!(summary.executed_steps, 1);
    assert_eq!(summary.pc, 0x6000_5c26);
    assert_eq!(session.snapshot().cpu.registers[0], 0x52);
    assert_eq!(session.host_input_stats().queued_bytes, 1);
}

#[test]
fn m8_usb_serial_read_shim_copies_host_input_to_firmware_memory() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.cpu.registers[0] = 0x2000_0100;
    session.cpu.registers[1] = 4;
    session.cpu.registers[14] = 0x6000_5c27;
    session.cpu.registers[15] = M8_USB_SERIAL_READ;
    session.receive_host_bytes(&[0x43, 0xa5, 0x44]);

    let summary = session.run_steps_summary(1);

    assert_eq!(summary.executed_steps, 1);
    assert_eq!(summary.pc, 0x6000_5c26);
    assert_eq!(session.snapshot().cpu.registers[0], 3);
    assert_eq!(session.debug_read_u8(0x2000_0100), Some(0x43));
    assert_eq!(session.debug_read_u8(0x2000_0101), Some(0xa5));
    assert_eq!(session.debug_read_u8(0x2000_0102), Some(0x44));
    assert_eq!(session.host_input_stats().queued_bytes, 0);
}

#[test]
fn m8_cpp_usbserial_read_shim_consumes_host_input() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.cpu.registers[14] = 0x6000_5c27;
    session.cpu.registers[15] = M8_USB_SERIAL_CPP_READ;
    session.receive_host_bytes(&[0x52, 0x20]);

    let summary = session.run_steps_summary(1);
    let stats = session.host_input_stats();

    assert_eq!(summary.executed_steps, 1);
    assert_eq!(summary.pc, 0x6000_5c26);
    assert_eq!(session.snapshot().cpu.registers[0], 0x52);
    assert_eq!(stats.queued_bytes, 1);
    assert_eq!(stats.firmware_input_delivered_bytes, 1);
    assert_eq!(stats.last_firmware_input_endpoint, Some("USBSerial::read"));
}

#[test]
fn m8_stream_ring_available_and_peek_report_host_input_without_consuming() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.receive_host_bytes(&[0x52]);

    session.cpu.registers[14] = 0x6000_5c27;
    session.cpu.registers[15] = M8_STREAM_RING_AVAILABLE;
    let available = session.run_steps_summary(1);
    assert_eq!(available.pc, 0x6000_5c26);
    assert_eq!(session.snapshot().cpu.registers[0], 1);
    assert_eq!(session.host_input_stats().queued_bytes, 1);

    session.cpu.registers[14] = 0x6000_5c27;
    session.cpu.registers[15] = M8_STREAM_RING_PEEK;
    let peek = session.run_steps_summary(1);
    let stats = session.host_input_stats();

    assert_eq!(peek.pc, 0x6000_5c26);
    assert_eq!(session.snapshot().cpu.registers[0], 0x52);
    assert_eq!(stats.queued_bytes, 1);
    assert_eq!(stats.firmware_input_delivered_bytes, 0);
    assert_eq!(stats.last_firmware_input_endpoint, Some("Stream ring peek"));
}

#[test]
fn m8_controller_input_base_scanner_detects_firmware_lane_series() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.cpu.cycles = 1;
    let input_base = 0x2001_4b44;
    seed_m8_input_lane_series(&mut session, input_base);
    session.debug_poke_u8(input_base + M8_APP_INPUT_CURRENT_STATE_OFFSET, 0x10);

    session.run_steps_summary(1);
    let stats = session.host_input_stats();

    assert_eq!(stats.queued_bytes, 0);
    assert_eq!(stats.discovered_m8_input_base, Some(input_base));
    assert_eq!(stats.active_m8_input_base, Some(input_base));
    assert_eq!(stats.last_firmware_input_endpoint, None);
}

#[test]
fn m8webdisplay_controller_packet_updates_m8_input_memory() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    let input_base = 0x2001_4b44;
    session.cpu.cycles = 1;
    seed_m8_input_lane_series(&mut session, input_base);
    session.debug_poke_u8(input_base + M8_APP_INPUT_CURRENT_STATE_OFFSET, 0xff);
    session.debug_poke_u8(input_base + M8_APP_INPUT_PREVIOUS_STATE_OFFSET, 0xff);

    session.receive_host_bytes(&[0x43, 0x50]);
    let stats = session.host_input_stats();

    assert_eq!(stats.observed_m8_controller_base, Some(input_base));
    assert_eq!(stats.observed_m8_controller_state, Some(!0x50u8));
    assert_eq!(stats.queued_bytes, 0);
    assert_eq!(stats.cdc_rx_available_bytes, 0);
    assert_eq!(stats.input_memory_previous_state, Some(0xff));
    assert_eq!(stats.input_memory_current_state, Some(!0x50u8));
    assert_eq!(stats.input_memory_dirty_flag, Some(1));
    assert_eq!(
        session.debug_read_u8(input_base + M8_APP_INPUT_PREVIOUS_STATE_OFFSET),
        Some(0xff)
    );
    assert_eq!(
        session.debug_read_u8(input_base + M8_APP_INPUT_CURRENT_STATE_OFFSET),
        Some(!0x50u8)
    );
    assert_eq!(
        session.debug_read_u8(input_base + M8_APP_INPUT_DIRTY_FLAG_OFFSET),
        Some(1)
    );
    assert_eq!(stats.last_firmware_input_endpoint, Some("M8 input memory"));
}

#[test]
fn m8webdisplay_controller_packet_before_boot_does_not_touch_input_memory() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    let input_base = 0x2001_4b44;
    seed_m8_input_lane_series(&mut session, input_base);

    session.receive_host_bytes(&[0x43, 0x50]);
    let stats = session.host_input_stats();

    assert_eq!(session.run_steps_summary(0).joypad_state, 0x50);
    assert_eq!(stats.queued_bytes, 0);
    assert_eq!(stats.cdc_rx_available_bytes, 0);
    assert_eq!(stats.active_m8_input_base, None);
    assert_eq!(stats.input_memory_current_state, None);
    assert_eq!(stats.observed_m8_remote_controller_current, None);
    assert_eq!(stats.last_firmware_input_endpoint, None);
    assert_eq!(
        session
            .debug_read_u8(M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_CURRENT_OFFSET),
        None
    );
    assert_eq!(
        session.debug_read_u8(input_base + M8_APP_INPUT_CURRENT_STATE_OFFSET),
        Some(0xff)
    );
}

#[test]
fn m8webdisplay_controller_packets_do_not_leave_stale_serial_queue() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    let input_base = 0x2001_4b44;
    session.cpu.cycles = 1;
    seed_m8_input_lane_series(&mut session, input_base);
    session.debug_poke_u8(input_base + M8_APP_INPUT_CURRENT_STATE_OFFSET, 0xff);
    session.debug_poke_u8(input_base + M8_APP_INPUT_PREVIOUS_STATE_OFFSET, 0xff);

    for state in [0x20, 0x00, 0x40, 0x00, 0x10, 0x00] {
        session.receive_host_bytes(&[0x43, state]);
    }
    let stats = session.host_input_stats();

    assert_eq!(stats.queued_bytes, 0);
    assert_eq!(stats.cdc_rx_available_bytes, 0);
    assert_eq!(stats.input_memory_current_state, Some(0xff));
    assert_eq!(stats.input_memory_previous_state, Some(0xef));
    assert_eq!(stats.input_memory_dirty_flag, Some(1));
    assert_eq!(stats.last_firmware_input_endpoint, Some("M8 input memory"));
}

#[test]
fn m8webdisplay_controller_packet_is_not_returned_by_usbserial_read() {
    let hex = minimal_teensy_ivt_hex();
    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.cpu.cycles = 1;
    session.receive_host_bytes(&[0x43, 0x20]);

    session.cpu.registers[14] = 0x6000_5c27;
    session.cpu.registers[15] = M8_USB_SERIAL_CPP_READ;
    let summary = session.run_steps_summary(1);
    let stats = session.host_input_stats();

    assert_eq!(summary.executed_steps, 1);
    assert_eq!(summary.pc, 0x6000_5c26);
    assert_eq!(session.snapshot().cpu.registers[0], u32::MAX);
    assert_eq!(stats.queued_bytes, 0);
    assert_eq!(stats.cdc_rx_available_bytes, 0);
}

#[test]
fn m8webdisplay_controller_packet_calls_controller_handler_when_available() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(0, 0x04, &[0x00, 0x02]));
    hex.push_str(&hex_record(
        0x80d0,
        0x00,
        &[
            0x2d, 0xe9, 0x10, 0x40, // push.w {r4, lr}
            0x2d, 0xed, 0x01, 0x8a, // vpush {s16}
            0x80, 0xf8, 0x6c, 0x11, // strb.w r1, [r0, #0x16c]
            0xbd, 0xec, 0x01, 0x8a, // vpop {s16}
            0xbd, 0xe8, 0x10, 0x80, // pop.w {r4, pc}
        ],
    ));
    hex.push_str(":00000001FF\n");

    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    let input_base = 0x2001_4b44;
    session.cpu.cycles = 1;
    session.cpu.registers[13] = 0x2004_0000;
    session.cpu.registers[15] = 0x6000_5c24;
    seed_m8_input_lane_series(&mut session, input_base);

    session.receive_host_bytes(&[0x43, 0x20]);
    let stats = session.host_input_stats();

    assert_eq!(session.snapshot().cpu.registers[15], 0x6000_5c24);
    assert_eq!(
        session.debug_read_u8(input_base + M8_APP_INPUT_CURRENT_STATE_OFFSET),
        Some(!0x20u8)
    );
    assert_eq!(
        session
            .debug_read_u8(M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_CURRENT_OFFSET),
        Some(0x20)
    );
    assert_eq!(stats.observed_m8_controller_state, Some(0x20));
    assert_eq!(
        stats.last_firmware_input_endpoint,
        Some("M8 controller handler")
    );
}

#[test]
fn m8webdisplay_controller_packet_uses_remote_serial_parser_when_available() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(0, 0x04, &[0x60, 0x04]));
    hex.push_str(&hex_record(
        0xaf28,
        0x00,
        &[
            0x70, 0x47, // bx lr
        ],
    ));
    hex.push_str(":00000001FF\n");

    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.cpu.cycles = 1;
    session.cpu.registers[15] = 0x6000_5c24;

    session.receive_host_bytes(&[0x43, 0x20]);
    let stats = session.host_input_stats();

    assert_eq!(session.snapshot().cpu.registers[15], 0x6000_5c24);
    assert_eq!(
        session
            .debug_read_u8(M8_REMOTE_CONTROLLER_STATE_BASE + M8_REMOTE_CONTROLLER_CURRENT_OFFSET),
        Some(0x20)
    );
    assert_eq!(
        session.debug_read_u8(M8_REMOTE_SERIAL_DEFAULT_BASE + 0x0c),
        Some(0x43)
    );
    assert_eq!(
        session.debug_read_u8(M8_REMOTE_SERIAL_DEFAULT_BASE + 0x0d),
        Some(0x20)
    );
    assert_eq!(
        session.debug_read_u8(M8_REMOTE_SERIAL_DEFAULT_BASE + 0x16),
        Some(2)
    );
    assert_eq!(
        stats.last_firmware_input_endpoint,
        Some("M8 remote serial parser")
    );
    assert_eq!(stats.queued_bytes, 0);
    assert_eq!(stats.cdc_rx_available_bytes, 0);
    assert_eq!(stats.observed_m8_remote_controller_current, Some(0x20));
    assert_eq!(stats.remote_serial_pump_attempts, 1);
    assert_eq!(stats.remote_serial_pump_successes, 1);
    assert_eq!(stats.last_joypad_input_state, Some(0x20));
    assert_eq!(
        stats.last_joypad_input_endpoint,
        Some("M8 remote serial parser")
    );
    assert!(stats.last_joypad_input_applied);
}

#[test]
fn send_joypad_state_uses_remote_serial_parser_when_available() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(0, 0x04, &[0x60, 0x04]));
    hex.push_str(&hex_record(
        0xaf28,
        0x00,
        &[
            0x70, 0x47, // bx lr
        ],
    ));
    hex.push_str(":00000001FF\n");

    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.cpu.cycles = 1;
    session.cpu.registers[15] = 0x6000_5c24;

    assert!(session.send_joypad_state(0x20));
    let stats = session.host_input_stats();

    assert_eq!(session.snapshot().cpu.registers[15], 0x6000_5c24);
    assert_eq!(stats.last_packet, vec![0x43, 0x20]);
    assert_eq!(stats.queued_bytes, 0);
    assert_eq!(stats.cdc_rx_available_bytes, 0);
    assert_eq!(
        stats.last_firmware_input_endpoint,
        Some("M8 remote serial parser")
    );
    assert_eq!(stats.remote_serial_pump_attempts, 1);
    assert_eq!(stats.remote_serial_pump_successes, 1);
    assert_eq!(stats.last_joypad_input_state, Some(0x20));
    assert_eq!(
        stats.last_joypad_input_endpoint,
        Some("M8 remote serial parser")
    );
    assert!(stats.last_joypad_input_applied);
}

#[test]
fn send_joypad_state_release_keeps_remote_high_and_input_memory_low_polarities() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(0, 0x04, &[0x60, 0x04]));
    hex.push_str(&hex_record(
        0xaf28,
        0x00,
        &[
            0x70, 0x47, // bx lr
        ],
    ));
    hex.push_str(":00000001FF\n");

    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.cpu.cycles = 1;
    session.cpu.registers[15] = 0x6000_5c24;
    let input_base = 0x2001_4b44;
    seed_m8_input_lane_series(&mut session, input_base);

    assert!(session.send_joypad_state(0x20));
    assert!(session.send_joypad_state(0x00));
    let stats = session.host_input_stats();

    assert_eq!(stats.last_packet, vec![0x43, 0x00]);
    assert_eq!(stats.observed_m8_remote_controller_current, Some(0x00));
    assert_eq!(
        session.debug_read_u8(input_base + M8_APP_INPUT_CURRENT_STATE_OFFSET),
        Some(0xff)
    );
    assert_eq!(
        session.debug_read_u8(input_base + M8_APP_INPUT_PREVIOUS_STATE_OFFSET),
        Some(0xdf)
    );
    assert_eq!(
        session.debug_read_u8(input_base + M8_APP_INPUT_DIRTY_FLAG_OFFSET),
        Some(1)
    );
    assert_eq!(stats.last_joypad_input_state, Some(0x00));
    assert_eq!(
        stats.last_joypad_input_endpoint,
        Some("M8 remote serial parser")
    );
    assert!(stats.last_joypad_input_applied);
}

#[test]
fn synthetic_firmware_call_restores_stack_writes() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(0, 0x04, &[0x60, 0x05]));
    hex.push_str(&hex_record(
        0x118c,
        0x00,
        &[
            0x01, 0x22, // MOVS r2, #1
            0x01, 0x92, // STR r2, [SP, #0x4]
            0x70, 0x47, // BX LR
        ],
    ));
    hex.push_str(":00000001FF\n");

    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    session.cpu.cycles = 1;
    session.cpu.registers[13] = 0x2004_0000;
    session.cpu.registers[15] = 0x6000_5c24;
    session.debug_poke_u8(0x2004_0004, 0xaa);

    assert!(
        session
            .call_m8_function1_at(M8_REMOTE_CONTROLLER_UPDATE, M8_REMOTE_CONTROLLER_STATE_BASE)
            .is_some()
    );

    assert_eq!(session.snapshot().cpu.registers[15], 0x6000_5c24);
    assert_eq!(session.debug_read_u8(0x2004_0004), Some(0xaa));
}

#[test]
fn send_joypad_state_calls_discovered_m8_controller_handler() {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(&hex_record(0, 0x04, &[0x00, 0x02]));
    hex.push_str(&hex_record(
        0x80d0,
        0x00,
        &[
            0x2d, 0xe9, 0x10, 0x40, // push.w {r4, lr}
            0x2d, 0xed, 0x01, 0x8a, // vpush {s16}
            0x80, 0xf8, 0x6c, 0x11, // strb.w r1, [r0, #0x16c]
            0xbd, 0xec, 0x01, 0x8a, // vpop {s16}
            0xbd, 0xe8, 0x10, 0x80, // pop.w {r4, pc}
        ],
    ));
    hex.push_str(":00000001FF\n");

    let mut session = TeensyEmulatorSession::new(hex.as_bytes()).expect("session");
    let input_base = 0x2001_4b44;
    session.cpu.cycles = 1;
    session.cpu.registers[13] = 0x2004_0000;
    seed_m8_input_lane_series(&mut session, input_base);

    assert!(session.send_joypad_state(0x14));
    let stats = session.host_input_stats();

    assert_eq!(
        session.debug_read_u8(input_base + M8_APP_INPUT_CURRENT_STATE_OFFSET),
        Some(!0x14u8)
    );
    assert_eq!(stats.observed_m8_controller_base, Some(input_base));
    assert_eq!(stats.observed_m8_controller_state, Some(0x14));
    assert_eq!(stats.last_packet, vec![0x43, 0x14]);
    assert_eq!(
        stats.last_packet_label.as_deref(),
        Some("controller 0b00010100")
    );
    assert_eq!(stats.queued_bytes, 0);
    assert_eq!(stats.cdc_rx_available_bytes, 0);
    assert_eq!(
        stats.last_firmware_input_endpoint,
        Some("M8 controller handler")
    );
}

#[test]
fn display_snapshot_collects_ascii_runs_from_render_ram() {
    let mut memory = test_memory(vec![]);
    for (offset, byte) in b"VSCPIT".iter().copied().enumerate() {
        memory.write_u8(0x2001_321a + offset as u32, byte);
    }

    let snapshot = build_display_snapshot(&memory);
    let candidate = snapshot
        .candidates
        .iter()
        .find(|candidate| candidate.name == "ASCII runs")
        .expect("ASCII runs candidate");

    assert!(candidate.rows.iter().any(|row| row.contains("VSCPIT")));
    assert!(candidate.score > 0);
}

#[test]
fn optional_pointer_load_sanitizes_display_cell_garbage_before_null_check() {
    let records = vec![DataRecord {
        address: 0x6000_0000,
        bytes: vec![
            0x02, 0x68, // LDR r2, [r0, #0x0]
            0x02, 0xb1, // CBZ r2, +0x4
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[0] = 0x2001_4b4c;
    cpu.registers[15] = 0x6000_0000;

    memory.set_current_instruction(0x6004_4b7e, 12_345);
    for (offset, byte) in [0x30, 0x00, 0x30, 0x30].into_iter().enumerate() {
        memory.write_u8(0x2001_4b4c + offset as u32, byte);
    }

    let ldr = step_thumb_trace(&mut memory, &mut cpu);

    assert!(
        ldr.note
            .contains("optional pointer sanitized from 0x30300030")
    );
    assert_eq!(cpu.registers[2], 0);
    let samples = memory.pointer_sanitization_samples();
    assert_eq!(samples[0].address, 0x2001_4b4c);
    assert_eq!(samples[0].raw_value, 0x3030_0030);
    assert_eq!(memory.top_pointer_sanitizations(1)[0].count, 1);
}

#[test]
fn optional_pointer_load_keeps_valid_ram_pointer_before_null_check() {
    let records = vec![DataRecord {
        address: 0x6000_0000,
        bytes: vec![
            0x02, 0x68, // LDR r2, [r0, #0x0]
            0x02, 0xb1, // CBZ r2, +0x4
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[0] = 0x2001_4b4c;
    cpu.registers[15] = 0x6000_0000;

    memory.write_u32(0x2001_4b4c, 0x2000_1000);

    let ldr = step_thumb_trace(&mut memory, &mut cpu);

    assert!(!ldr.note.contains("optional pointer sanitized"));
    assert_eq!(cpu.registers[2], 0x2000_1000);
    assert!(memory.pointer_sanitization_samples().is_empty());
}

#[test]
fn virtual_dispatch_stubs_display_cell_receiver_and_returns_to_lr() {
    let records = vec![DataRecord {
        address: 0x6000_0000,
        bytes: vec![
            0x03, 0x68, // LDR r3, [r0, #0x0]
            0x1b, 0x6a, // LDR r3, [r3, #0x20]
            0x18, 0x47, // BX r3
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[0] = 0x2001_4ae4;
    cpu.registers[14] = 0x6000_1235;
    cpu.registers[15] = 0x6000_0000;

    memory.set_current_instruction(0x6004_4b7c, 77);
    for (offset, byte) in [0x44, 0x30, 0x30, 0x20].into_iter().enumerate() {
        memory.write_u8(0x2001_4ae4 + offset as u32, byte);
    }

    let event = step_thumb_trace(&mut memory, &mut cpu);

    assert!(event.note.contains("virtual dispatch receiver sanitized"));
    assert_eq!(cpu.registers[3], 0);
    assert_eq!(cpu.registers[15], 0x6000_1234);
    assert_eq!(
        memory.pointer_sanitization_samples()[0].reason,
        "virtual dispatch receiver contained display-cell bytes"
    );
}

#[test]
fn virtual_dispatch_stubs_runtime_byte_receiver_and_returns_to_lr() {
    let records = vec![DataRecord {
        address: 0x0002_45dc,
        bytes: vec![
            0x03, 0x68, // LDR r3, [r0, #0x0]
            0x5b, 0x68, // LDR r3, [r3, #0x4]
            0x98, 0x47, // BLX r3
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[0] = 0x2001_0000;
    cpu.registers[14] = 0x6000_1235;
    cpu.registers[15] = 0x0002_45dc;

    memory.write_u32(0x2001_0000, 0x3290_f8c4);

    let event = step_thumb_trace(&mut memory, &mut cpu);

    assert!(event.note.contains("virtual dispatch receiver sanitized"));
    assert_eq!(cpu.registers[3], 0);
    assert_eq!(cpu.registers[15], 0x6000_1234);
    assert!(memory.top_unmapped_reads(1).is_empty());
    assert_eq!(
        memory.pointer_sanitization_samples()[0].reason,
        "virtual dispatch receiver contained non-pointer UI/runtime bytes"
    );
}

#[test]
fn virtual_dispatch_stubs_unmapped_runtime_receiver_base_and_returns_to_lr() {
    let records = vec![DataRecord {
        address: 0x0002_45dc,
        bytes: vec![
            0x03, 0x68, // LDR r3, [r0, #0x0]
            0x5b, 0x68, // LDR r3, [r3, #0x4]
            0x98, 0x47, // BLX r3
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[0] = 0x0200_f004;
    cpu.registers[14] = 0x6000_1235;
    cpu.registers[15] = 0x0002_45dc;

    let event = step_thumb_trace(&mut memory, &mut cpu);

    assert!(
        event
            .note
            .contains("virtual dispatch receiver base sanitized")
    );
    assert_eq!(cpu.registers[3], 0);
    assert_eq!(cpu.registers[15], 0x6000_1234);
    assert!(memory.top_unmapped_reads(1).is_empty());
    assert_eq!(
        memory.pointer_sanitization_samples()[0].reason,
        "virtual dispatch receiver base contained non-pointer runtime bytes"
    );
}

#[test]
fn immediate_ldr_sanitizes_display_cell_base_before_unmapped_read() {
    let records = vec![DataRecord {
        address: 0x6000_0000,
        bytes: vec![
            0xdb, 0x69, // LDR r3, [r3, #0x1c]
            0x00, 0xbf, // NOP
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[3] = 0x2030_3044;
    cpu.registers[15] = 0x6000_0000;

    let ldr = step_thumb_trace(&mut memory, &mut cpu);

    assert!(ldr.note.contains("effective address base sanitized"));
    assert_eq!(cpu.registers[3], 0);
    assert!(memory.top_unmapped_reads(1).is_empty());
    let samples = memory.pointer_sanitization_samples();
    assert_eq!(samples[0].address, 0x2030_3060);
    assert_eq!(samples[0].raw_value, 0x2030_3044);
    assert_eq!(
        samples[0].reason,
        "effective address base contained display-cell bytes"
    );
}

#[test]
fn register_offset_ldr_sanitizes_display_cell_base_before_unmapped_read() {
    let records = vec![DataRecord {
        address: 0x6000_0000,
        bytes: vec![
            0x41, 0x58, // LDR r1, [r0, r1]
            0x00, 0xbf, // NOP
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[0] = 0x2030_3060;
    cpu.registers[1] = 0;
    cpu.registers[15] = 0x6000_0000;

    let ldr = step_thumb_trace(&mut memory, &mut cpu);

    assert!(ldr.note.contains("effective address base sanitized"));
    assert_eq!(cpu.registers[1], 0);
    assert!(memory.top_unmapped_reads(1).is_empty());
    assert_eq!(
        memory.pointer_sanitization_samples()[0].address,
        0x2030_3060
    );
}

#[test]
fn immediate_ldr_sanitizes_low_ui_data_base_before_unmapped_read() {
    let records = vec![DataRecord {
        address: 0x0002_45de,
        bytes: vec![
            0x5b, 0x68, // LDR r3, [r3, #0x4]
            0x00, 0xbf, // NOP
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[3] = 0x056a_00ee;
    cpu.registers[15] = 0x0002_45de;

    let ldr = step_thumb_trace(&mut memory, &mut cpu);

    assert!(ldr.note.contains("effective address base sanitized"));
    assert_eq!(cpu.registers[3], 0);
    assert!(memory.top_unmapped_reads(1).is_empty());
    let samples = memory.pointer_sanitization_samples();
    assert_eq!(samples[0].address, 0x056a_00f2);
    assert_eq!(samples[0].raw_value, 0x056a_00ee);
}

#[test]
fn immediate_ldr_sanitizes_ascii_like_runtime_data_base_before_unmapped_read() {
    let records = vec![DataRecord {
        address: 0x0002_45dc,
        bytes: vec![
            0x03, 0x68, // LDR r3, [r0, #0x0]
            0x00, 0xbf, // NOP
        ],
    }];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    cpu.registers[0] = 0x327d_5504;
    cpu.registers[15] = 0x0002_45dc;

    let ldr = step_thumb_trace(&mut memory, &mut cpu);

    assert!(ldr.note.contains("effective address base sanitized"));
    assert_eq!(cpu.registers[3], 0);
    assert!(memory.top_unmapped_reads(1).is_empty());
    let samples = memory.pointer_sanitization_samples();
    assert_eq!(samples[0].address, 0x327d_5504);
    assert_eq!(samples[0].raw_value, 0x327d_5504);
}

#[test]
fn enters_pending_enabled_external_irq_with_valid_vector() {
    let records = vec![
        DataRecord {
            address: 0x40,
            bytes: vec![0x01, 0x01, 0x00, 0x00],
        },
        DataRecord {
            address: 0x100,
            bytes: vec![0x00, 0xbf],
        },
    ];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    let mut stats = ProbeStatsBuilder::default();
    cpu.registers[0] = 0x10;
    cpu.registers[1] = 0x11;
    cpu.registers[13] = 0x2004_0000;
    cpu.registers[14] = 0x6000_1235;
    cpu.registers[15] = 0x6000_2000;
    cpu.apsr.z = true;
    cpu.apsr.c = true;
    cpu.it_conditions = [0, 1, 14, 15];
    cpu.it_remaining = 4;
    cpu.it_suppresses_flags = true;
    memory.write_u32(NVIC_ISER_BASE, 1);
    memory.write_u32(NVIC_ISPR_BASE, 1);

    let event =
        maybe_enter_exception(&mut memory, &mut cpu, Some(&mut stats)).expect("exception entry");

    assert_eq!(
        event.note,
        "Exception entry IRQ0 ; handler 0x00000100, stacked PC=0x60002000, SP=0x2003ffe0"
    );
    assert_eq!(cpu.registers[13], 0x2003_ffe0);
    assert_eq!(cpu.registers[14], 0xffff_fff9);
    assert_eq!(cpu.registers[15], 0x0000_0100);
    assert_eq!(cpu.xpsr, ARM_XPSR_THUMB | 16);
    assert_eq!(cpu.it_conditions, [0; 4]);
    assert_eq!(cpu.it_remaining, 0);
    assert!(!cpu.it_suppresses_flags);
    assert_eq!(cpu.exception_depth, 1);
    assert_eq!(cpu.active_exception, Some(16));
    assert_eq!(memory.read_u32(0x2003_ffe0), Some(0x10));
    assert_eq!(memory.read_u32(0x2003_ffe4), Some(0x11));
    assert_eq!(memory.read_u32(0x2003_fff8), Some(0x6000_2000));
    assert_eq!(
        memory.read_u32(0x2003_fffc).unwrap() & 0x6000_0000,
        0x6000_0000
    );
    assert_eq!(memory.read_u32_or_zero(NVIC_ISPR_BASE), 0);
    assert_eq!(memory.read_u32_or_zero(NVIC_IABR_BASE), 1);
    assert_eq!(
        memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_VECTACTIVE_MASK,
        16
    );
    assert_eq!(stats.exception_entries, 1);
}

#[test]
fn returns_from_exception_frame_on_bx_lr() {
    let mut memory = test_memory(vec![]);
    let mut cpu = test_cpu();
    cpu.exception_depth = 1;
    cpu.active_exception = Some(16);
    cpu.registers[13] = 0x2003_ffe0;
    cpu.registers[14] = 0xffff_fff9;
    memory.write_u32(0x2003_ffe0, 0x10);
    memory.write_u32(0x2003_ffe4, 0x11);
    memory.write_u32(0x2003_ffe8, 0x12);
    memory.write_u32(0x2003_ffec, 0x13);
    memory.write_u32(0x2003_fff0, 0x1c);
    memory.write_u32(0x2003_fff4, 0x6000_1235);
    memory.write_u32(0x2003_fff8, 0x6000_2001);
    memory.write_u32(0x2003_fffc, ARM_XPSR_THUMB | 0x6000_0000);
    memory.write_u32(NVIC_IABR_BASE, 1);
    cpu.apsr.n = true;

    let decoded = decode_thumb(&mut memory, 0x0000_0100, 0x4770, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_2000);
    assert_eq!(cpu.registers[0], 0x10);
    assert_eq!(cpu.registers[1], 0x11);
    assert_eq!(cpu.registers[12], 0x1c);
    assert_eq!(cpu.registers[13], 0x2004_0000);
    assert_eq!(cpu.registers[14], 0x6000_1235);
    assert!(!cpu.apsr.n);
    assert!(cpu.apsr.z);
    assert!(cpu.apsr.c);
    assert_eq!(cpu.exception_depth, 0);
    assert_eq!(cpu.active_exception, None);
    assert_eq!(memory.read_u32_or_zero(NVIC_IABR_BASE), 0);
    assert_eq!(
        memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_VECTACTIVE_MASK,
        0
    );
    assert!(decoded.note.starts_with("Exception return 0xfffffff9"));
}

#[test]
fn returns_from_exception_frame_on_pop_pc() {
    let mut memory = test_memory(vec![]);
    let mut cpu = test_cpu();
    cpu.exception_depth = 1;
    cpu.active_exception = Some(16);
    cpu.registers[13] = 0x2003_ffdc;
    memory.write_u32(0x2003_ffdc, 0xffff_fff9);
    memory.write_u32(0x2003_ffe0, 0x10);
    memory.write_u32(0x2003_ffe4, 0x11);
    memory.write_u32(0x2003_ffe8, 0x12);
    memory.write_u32(0x2003_ffec, 0x13);
    memory.write_u32(0x2003_fff0, 0x1c);
    memory.write_u32(0x2003_fff4, 0x6000_1235);
    memory.write_u32(0x2003_fff8, 0x6000_2001);
    memory.write_u32(0x2003_fffc, ARM_XPSR_THUMB);
    memory.write_u32(NVIC_IABR_BASE, 1);

    let decoded = decode_thumb(&mut memory, 0x0000_0100, 0xbd00, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_2000);
    assert_eq!(cpu.registers[13], 0x2004_0000);
    assert_eq!(cpu.registers[14], 0x6000_1235);
    assert_eq!(cpu.exception_depth, 0);
    assert_eq!(memory.read_u32_or_zero(NVIC_IABR_BASE), 0);
    assert!(decoded.note.starts_with("Exception return 0xfffffff9"));
}

#[test]
fn scb_icsr_pendstset_enters_systick_vector_and_pendstclr_clears_it() {
    let records = vec![
        DataRecord {
            address: 15 * 4,
            bytes: vec![0x01, 0x01, 0x00, 0x00],
        },
        DataRecord {
            address: 0x100,
            bytes: vec![0x00, 0xbf],
        },
    ];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    let mut stats = ProbeStatsBuilder::default();
    cpu.registers[13] = 0x2004_0000;
    cpu.registers[15] = 0x6000_2000;

    memory.write_u32(SCB_ICSR, SCB_ICSR_PENDSTSET);
    assert_eq!(
        memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSTSET,
        SCB_ICSR_PENDSTSET
    );
    let event =
        maybe_enter_exception(&mut memory, &mut cpu, Some(&mut stats)).expect("systick entry");

    assert!(event.note.starts_with("Exception entry SysTick"));
    assert_eq!(cpu.active_exception, Some(15));
    assert_eq!(cpu.registers[15], 0x100);
    assert_eq!(memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSTSET, 0);
    assert_eq!(
        memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_VECTACTIVE_MASK,
        15
    );

    memory.write_u32(SCB_ICSR, SCB_ICSR_PENDSTSET);
    memory.write_u32(SCB_ICSR, SCB_ICSR_PENDSTCLR);
    assert_eq!(memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSTSET, 0);
}

#[test]
fn scb_icsr_pendsvset_enters_pendsv_vector_and_pendsvclr_clears_it() {
    let records = vec![
        DataRecord {
            address: 14 * 4,
            bytes: vec![0x01, 0x01, 0x00, 0x00],
        },
        DataRecord {
            address: 0x100,
            bytes: vec![0x00, 0xbf],
        },
    ];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    let mut stats = ProbeStatsBuilder::default();
    cpu.registers[13] = 0x2004_0000;
    cpu.registers[15] = 0x6000_2000;

    memory.write_u32(SCB_ICSR, SCB_ICSR_PENDSVSET);
    assert_eq!(
        memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSVSET,
        SCB_ICSR_PENDSVSET
    );
    assert_eq!(
        (memory.read_u32_or_zero(SCB_ICSR) >> SCB_ICSR_VECTPENDING_SHIFT)
            & SCB_ICSR_VECTACTIVE_MASK,
        14
    );

    let event =
        maybe_enter_exception(&mut memory, &mut cpu, Some(&mut stats)).expect("pendsv entry");

    assert!(event.note.starts_with("Exception entry PendSV"));
    assert_eq!(cpu.active_exception, Some(14));
    assert_eq!(cpu.registers[15], 0x100);
    assert_eq!(memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSVSET, 0);
    assert_eq!(
        memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_VECTACTIVE_MASK,
        14
    );
    assert_ne!(memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_RETTOBASE, 0);

    memory.write_u32(SCB_ICSR, SCB_ICSR_PENDSVSET);
    memory.write_u32(SCB_ICSR, SCB_ICSR_PENDSVCLR);
    assert_eq!(memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSVSET, 0);
}

#[test]
fn basepri_masks_lower_priority_pending_exceptions() {
    let records = vec![
        DataRecord {
            address: 14 * 4,
            bytes: vec![0x01, 0x01, 0x00, 0x00],
        },
        DataRecord {
            address: 15 * 4,
            bytes: vec![0x21, 0x01, 0x00, 0x00],
        },
        DataRecord {
            address: 0x100,
            bytes: vec![0x00, 0xbf],
        },
        DataRecord {
            address: 0x120,
            bytes: vec![0x00, 0xbf],
        },
    ];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    let mut stats = ProbeStatsBuilder::default();
    cpu.registers[13] = 0x2004_0000;
    cpu.registers[15] = 0x6000_2000;
    cpu.basepri = 0x80;
    memory.write_u32(SCB_SHPR3, 0x20_e0_00_00);
    memory.write_u32(SCB_ICSR, SCB_ICSR_PENDSVSET | SCB_ICSR_PENDSTSET);

    let event = maybe_enter_exception(&mut memory, &mut cpu, Some(&mut stats))
        .expect("higher priority systick entry");

    assert!(event.note.starts_with("Exception entry SysTick"));
    assert_eq!(cpu.active_exception, Some(15));
    assert_eq!(cpu.registers[15], 0x120);
    assert_eq!(
        memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSVSET,
        SCB_ICSR_PENDSVSET
    );
}

#[test]
fn nvic_priority_selects_highest_pending_irq() {
    let records = vec![
        DataRecord {
            address: 16 * 4,
            bytes: vec![0x01, 0x01, 0x00, 0x00],
        },
        DataRecord {
            address: 17 * 4,
            bytes: vec![0x21, 0x01, 0x00, 0x00],
        },
        DataRecord {
            address: 0x100,
            bytes: vec![0x00, 0xbf],
        },
        DataRecord {
            address: 0x120,
            bytes: vec![0x00, 0xbf],
        },
    ];
    let mut memory = test_memory(records);
    let mut cpu = test_cpu();
    let mut stats = ProbeStatsBuilder::default();
    cpu.registers[13] = 0x2004_0000;
    cpu.registers[15] = 0x6000_2000;
    memory.write_u32(NVIC_ISER_BASE, 0x3);
    memory.write_u32(NVIC_ISPR_BASE, 0x3);
    memory.write_u8(NVIC_IPR_BASE, 0x80);
    memory.write_u8(NVIC_IPR_BASE + 1, 0x20);

    let event = maybe_enter_exception(&mut memory, &mut cpu, Some(&mut stats)).expect("irq entry");

    assert!(event.note.starts_with("Exception entry IRQ1"));
    assert_eq!(cpu.active_exception, Some(17));
    assert_eq!(cpu.registers[15], 0x120);
    assert_eq!(memory.read_u32_or_zero(NVIC_ISPR_BASE), 0x1);
    assert_eq!(memory.read_u32_or_zero(NVIC_IABR_BASE), 0x2);
}

#[test]
fn dwt_cycle_counter_advances_between_reads() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(DWT_CYCCNT, 1_000);
    assert_eq!(memory.read_u32_or_zero(DWT_CYCCNT), 1_000);
    assert_eq!(memory.read_u32_or_zero(DWT_CYCCNT), 1_000);

    memory.advance_virtual_cycles(2_048);
    assert_eq!(memory.read_u32_or_zero(DWT_CYCCNT), 3_048);

    memory.write_u32(DWT_CYCCNT, 25);
    assert_eq!(memory.read_u32_or_zero(DWT_CYCCNT), 25);
    memory.advance_virtual_cycles(5);
    assert_eq!(memory.read_u32_or_zero(DWT_CYCCNT), 30);
}

#[test]
fn systick_virtual_tick_is_fast_enough_for_firmware_delays() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(SYSTICK_LOAD, 600_000 - 1);
    memory.write_u32(SYSTICK_CTRL, 0x7);
    memory.advance_virtual_cycles(599_999);
    assert_eq!(memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSTSET, 0);

    memory.advance_virtual_cycles(1);
    assert_eq!(
        memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSTSET,
        SCB_ICSR_PENDSTSET
    );
}

#[test]
fn decode_thumb_nop_advances_to_next_halfword() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);

    let decoded = decode_thumb(&mut memory, 0x6000_5a10, 0xbf00, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_5a12);
    assert_eq!(decoded.opcode32, None);
    assert_eq!(decoded.note, "NOP");
}

#[test]
fn decode_thumb_add_rd_sp_imm_builds_stack_address() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[13] = 0x2003_ffd0;

    let decoded = decode_thumb(&mut memory, 0x0003_4ebc, 0xa901, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_4ebe);
    assert_eq!(cpu.registers[1], 0x2003_ffd4);
    assert!(decoded.note.starts_with("ADD r1, sp"));
}

#[test]
fn decode_thumb_strb_register_offset_writes_byte() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[1] = 0x2003_ffd4;
    cpu.registers[2] = 0;
    cpu.registers[3] = 1;

    let decoded = decode_thumb(&mut memory, 0x0003_284a, 0x54ca, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_284c);
    assert_eq!(memory.read_u8(0x2003_ffd5), Some(0));
    assert!(decoded.note.starts_with("STRB r2, [r1, r3]"));
}

#[test]
fn decode_thumb_rev_reverses_word_bytes() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[2] = 0x1234_5678;

    let decoded = decode_thumb(&mut memory, 0x6000_565e, 0xba12, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_5660);
    assert_eq!(cpu.registers[2], 0x7856_3412);
    assert!(decoded.note.starts_with("REV r2, r2"));
}

#[test]
fn decode_thumb_rsbs_negates_low_register() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[1] = 0x55;

    let decoded = decode_thumb(&mut memory, 0x0003_a4c4, 0x4249, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_a4c6);
    assert_eq!(cpu.registers[1], 0xffff_ffab);
    assert!(cpu.apsr.n);
    assert!(!cpu.apsr.z);
    assert!(!cpu.apsr.c);
    assert!(decoded.note.starts_with("RSBS r1, r1, #0"));
}

#[test]
fn decode_thumb_ldmia_loads_register_list_and_writes_back() {
    let records = vec![DataRecord {
        address: 0x2000_78c0,
        bytes: vec![
            0x2f, 0x53, 0x6f, 0x6e, 0x67, 0x73, 0x00, 0x00, 0xd0, 0x78, 0x00, 0x20,
        ],
    }];
    let mut cpu = test_cpu();
    let mut memory = test_memory(records);
    cpu.registers[4] = 0x2000_78c0;

    let decoded = decode_thumb(&mut memory, 0x6002_0c8a, 0xcc07, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6002_0c8c);
    assert_eq!(cpu.registers[0], 0x6e6f_532f);
    assert_eq!(cpu.registers[1], 0x0000_7367);
    assert_eq!(cpu.registers[2], 0x2000_78d0);
    assert_eq!(cpu.registers[4], 0x2000_78cc);
    assert!(decoded.note.starts_with("LDMIA r4!"));
}

#[test]
fn decode_thumb_str_sp_relative_stores_word() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[2] = 0x0002_15dd;
    cpu.registers[13] = 0x2003_ff98;

    let decoded = decode_thumb(&mut memory, 0x0002_1a30, 0x9202, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0002_1a32);
    assert_eq!(memory.read_u32(0x2003_ffa0), Some(0x0002_15dd));
    assert!(decoded.note.starts_with("STR r2, [sp, #0x8]"));
}

#[test]
fn decode_thumb_returns_on_bx_lr() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[14] = 0x6000_1235;

    let decoded = decode_thumb(&mut memory, 0x6000_5a10, 0x4770, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_1234);
    assert_eq!(decoded.opcode32, None);
    assert!(decoded.note.starts_with("BX lr"));
}

#[test]
fn decode_thumb_blx_stubs_unloaded_callback_targets() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[3] = 0xb386_4605;

    let decoded = decode_thumb(&mut memory, 0x0002_a5dc, 0x4798, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0002_a5de);
    assert_eq!(cpu.registers[14], 0x0002_a5df);
    assert!(decoded.note.contains("stubbed external callback"));
}

#[test]
fn decode_thumb_blx_stubs_loaded_data_targets() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    memory.write_u16(0x2000_1000, 0xbf00);
    cpu.registers[3] = 0x2000_1001;

    let decoded = decode_thumb(&mut memory, 0x0002_a5dc, 0x4798, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0002_a5de);
    assert_eq!(cpu.registers[14], 0x0002_a5df);
    assert!(decoded.note.contains("stubbed external callback"));
}

#[test]
fn decode_thumb_bx_stubs_data_targets_and_returns_to_lr() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    memory.write_u16(0x2000_1000, 0xff00);
    memory.write_u16(0x6000_1234, 0xbf00);
    cpu.registers[3] = 0x2000_1001;
    cpu.registers[14] = 0x6000_1235;

    let decoded = decode_thumb(&mut memory, 0x0002_a5dc, 0x4718, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_1234);
    assert!(decoded.note.contains("stubbed external callback"));
    assert!(decoded.note.contains("returning to LR"));
}

#[test]
fn step_thumb_trace_stubs_pc_that_falls_into_display_style_ram() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    memory.write_u16(0x2001_3b0e, 0xf908);
    memory.write_u16(0x0003_58ba, 0xbf00);
    cpu.registers[15] = 0x2001_3b0e;
    cpu.registers[14] = 0x0003_58bb;

    let event = step_thumb_trace(&mut memory, &mut cpu);

    assert_eq!(cpu.registers[15], 0x0003_58ba);
    assert_eq!(event.opcode16, Some(0xf908));
    assert!(event.note.contains("non-executable RAM/data"));
    assert!(event.note.contains("stubbed external callback"));
}

#[test]
fn decode_thumb_blx_branches_to_loaded_code() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    memory.write_u16(0x6000_1234, 0xbf00);
    cpu.registers[3] = 0x6000_1235;

    let decoded = decode_thumb(&mut memory, 0x0002_a5dc, 0x4798, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_1234);
    assert_eq!(cpu.registers[14], 0x0002_a5df);
    assert!(decoded.note.starts_with("BLX r3"));
}

#[test]
fn decode_thumb_blx_branches_to_loaded_itcm_code() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    memory.write_u16(0x0002_1234, 0xbf00);
    cpu.registers[3] = 0x0002_1235;

    let decoded = decode_thumb(&mut memory, 0x0002_a5dc, 0x4798, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0002_1234);
    assert_eq!(cpu.registers[14], 0x0002_a5df);
    assert!(decoded.note.starts_with("BLX r3"));
}

#[test]
fn decode_thumb32_barrier_advances_without_side_effects() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);

    let decoded = decode_thumb32(&mut memory, 0x6000_5a0c, 0xf3bf, 0x8f4f, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_5a10);
    assert_eq!(decoded.opcode32, Some(0xf3bf_8f4f));
    assert_eq!(decoded.note, "DSB SY");
}

#[test]
fn decode_thumb32_wide_hints_advance_without_stopping() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);

    let nop = decode_thumb32(&mut memory, 0x6005_f2a4, 0xf3af, 0x8000, &mut cpu);
    let wfi = decode_thumb32(&mut memory, 0x6005_f2a8, 0xf3af, 0x8003, &mut cpu);

    assert_eq!(nop.next_pc, 0x6005_f2a8);
    assert_eq!(nop.note, "NOP.W");
    assert_eq!(wfi.next_pc, 0x6005_f2ac);
    assert_eq!(wfi.note, "WFI.W");
}

#[test]
fn decode_thumb_cps_updates_interrupt_masks() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);

    let disabled = decode_thumb(&mut memory, 0x6000_1000, 0xb672, &mut cpu);
    assert_eq!(disabled.next_pc, 0x6000_1002);
    assert_eq!(cpu.primask, 1);
    assert!(disabled.note.contains("CPSID I"));

    let enabled = decode_thumb(&mut memory, 0x6000_1002, 0xb662, &mut cpu);
    assert_eq!(enabled.next_pc, 0x6000_1004);
    assert_eq!(cpu.primask, 0);
    assert!(enabled.note.contains("CPSIE I"));
}

#[test]
fn decode_thumb32_mrs_reads_core_system_registers() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.apsr.n = true;
    cpu.apsr.ge = 0b1010;
    cpu.primask = 1;
    cpu.registers[13] = 0x2003_ffc0;

    let apsr = decode_thumb32(&mut memory, 0x6000_2000, 0xf3ef, 0x8200, &mut cpu);
    let primask = decode_thumb32(&mut memory, 0x6000_2004, 0xf3ef, 0x8010, &mut cpu);
    let msp = decode_thumb32(&mut memory, 0x6000_2008, 0xf3ef, 0x8108, &mut cpu);

    assert_eq!(apsr.next_pc, 0x6000_2004);
    assert_eq!(cpu.registers[2], 0x800a_0000);
    assert_eq!(cpu.registers[0], 1);
    assert_eq!(cpu.registers[1], 0x2003_ffc0);
    assert!(primask.note.starts_with("MRS r0, PRIMASK"));
    assert!(msp.note.starts_with("MRS r1, MSP"));
}

#[test]
fn decode_thumb32_msr_writes_core_system_registers() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[0] = 1;
    cpu.registers[1] = 0x80;
    cpu.registers[2] = 0x40;
    cpu.registers[3] = 0x2002_0000;

    let primask = decode_thumb32(&mut memory, 0x6000_2010, 0xf380, 0x8810, &mut cpu);
    let basepri = decode_thumb32(&mut memory, 0x6000_2014, 0xf381, 0x8811, &mut cpu);
    let basepri_max = decode_thumb32(&mut memory, 0x6000_2018, 0xf382, 0x8812, &mut cpu);
    let psp = decode_thumb32(&mut memory, 0x6000_201c, 0xf383, 0x8809, &mut cpu);

    assert_eq!(primask.next_pc, 0x6000_2014);
    assert_eq!(cpu.primask, 1);
    assert_eq!(cpu.basepri, 0x40);
    assert_eq!(cpu.psp, 0x2002_0000);
    assert!(basepri.note.starts_with("MSR BASEPRI, r1"));
    assert!(basepri_max.note.starts_with("MSR BASEPRI_MAX, r2"));
    assert!(psp.note.starts_with("MSR PSP, r3"));
}

#[test]
fn decode_thumb32_dmb_ish_does_not_become_branch() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);

    let decoded = decode_thumb32(&mut memory, 0x0002_1c48, 0xf3bf, 0x8f5b, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0002_1c4c);
    assert_eq!(decoded.opcode32, Some(0xf3bf_8f5b));
    assert_eq!(decoded.note, "DMB ISH");
}

#[test]
fn decode_thumb32_str_w_reports_memory_mapped_target() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[3] = 0xe000_e000;

    let decoded = decode_thumb32(&mut memory, 0x6005_f2c0, 0xf8c3, 0x1d94, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6005_f2c4);
    assert!(decoded.note.starts_with("STR.W r1, [r3, #0xd94]"));
    assert!(decoded.note.contains("0xe000ed94"));
}

#[test]
fn decode_thumb32_ldmia_w_loads_registers_and_writes_back() {
    let records = vec![DataRecord {
        address: 0x2003_ff54,
        bytes: vec![
            0x11, 0x11, 0x11, 0x11, 0x22, 0x22, 0x22, 0x22, 0x33, 0x33, 0x33, 0x33, 0x44, 0x44,
            0x44, 0x44,
        ],
    }];
    let mut cpu = test_cpu();
    let mut memory = test_memory(records);
    cpu.registers[12] = 0x2003_ff54;

    let decoded = decode_thumb32(&mut memory, 0x0000_d9a2, 0xe8bc, 0x000f, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_d9a6);
    assert_eq!(cpu.registers[0], 0x1111_1111);
    assert_eq!(cpu.registers[1], 0x2222_2222);
    assert_eq!(cpu.registers[2], 0x3333_3333);
    assert_eq!(cpu.registers[3], 0x4444_4444);
    assert_eq!(cpu.registers[12], 0x2003_ff64);
    assert!(decoded.note.starts_with("LDM.W r12!"));
}

#[test]
fn decode_thumb32_stmia_w_stores_registers_and_writes_back() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[0] = 0x1111_1111;
    cpu.registers[1] = 0x2222_2222;
    cpu.registers[12] = 0x2003_ff54;

    let decoded = decode_thumb32(&mut memory, 0x0000_d9a2, 0xe8ac, 0x0003, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_d9a6);
    assert_eq!(memory.read_u32_or_zero(0x2003_ff54), 0x1111_1111);
    assert_eq!(memory.read_u32_or_zero(0x2003_ff58), 0x2222_2222);
    assert_eq!(cpu.registers[12], 0x2003_ff5c);
    assert!(decoded.note.starts_with("STM.W r12!"));
}

#[test]
fn decode_thumb32_ldr_w_reads_loaded_memory_when_available() {
    let records = vec![DataRecord {
        address: 0x2000_0000,
        bytes: vec![0x78, 0x56, 0x34, 0x12],
    }];
    let mut cpu = test_cpu();
    let mut memory = test_memory(records);
    cpu.registers[3] = 0x1fff_fff0;

    let decoded = decode_thumb32(&mut memory, 0x6005_f2c0, 0xf8d3, 0x1010, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6005_f2c4);
    assert_eq!(cpu.registers[1], 0x1234_5678);
    assert!(decoded.note.starts_with("LDR.W r1, [r3, #0x10]"));
}

#[test]
fn decode_thumb32_ldr_w_sanitizes_low_ui_data_base_before_unmapped_read() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[3] = 0x056a_00ee;

    let decoded = decode_thumb32(&mut memory, 0x6001_ea2a, 0xf8d3, 0x30a0, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_ea2e);
    assert_eq!(cpu.registers[3], 0);
    assert!(decoded.note.contains("effective address base sanitized"));
    assert!(memory.top_unmapped_reads(1).is_empty());
    let samples = memory.pointer_sanitization_samples();
    assert_eq!(samples[0].address, 0x056a_018e);
    assert_eq!(samples[0].raw_value, 0x056a_00ee);
}

#[test]
fn decode_thumb32_ldr_w_literal_uses_aligned_thumb_pc_base() {
    let records = vec![DataRecord {
        address: 0x0000_6564,
        bytes: vec![0xe0, 0x25, 0x00, 0x20, 0x60, 0xfb, 0x03, 0x20],
    }];
    let mut cpu = test_cpu();
    let mut memory = test_memory(records);

    let decoded = decode_thumb32(&mut memory, 0x0000_63f0, 0xf8df, 0x9174, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_63f4);
    assert_eq!(cpu.registers[9], 0x2003_fb60);
    assert!(decoded.note.contains("[0x00006568]"));
}

#[test]
fn decode_thumb32_tbb_uses_pc_relative_table_offset() {
    let records = vec![DataRecord {
        address: 0x6000_e4a0,
        bytes: vec![0x33, 0x30, 0x2c, 0x29],
    }];
    let mut cpu = test_cpu();
    let mut memory = test_memory(records);
    cpu.registers[3] = 0;

    let decoded = decode_thumb32(&mut memory, 0x6000_e49c, 0xe8df, 0xf003, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_e506);
    assert!(decoded.note.starts_with("TBB [pc, r3]"));
    assert!(decoded.note.contains("[0x6000e4a0] = 0x33"));
}

#[test]
fn decode_thumb32_tbb_uses_unaligned_pc_plus_four_base() {
    let records = vec![
        DataRecord {
            address: 0x0000_1005,
            bytes: vec![0x7f],
        },
        DataRecord {
            address: 0x0000_1007,
            bytes: vec![0x02],
        },
    ];
    let mut cpu = test_cpu();
    let mut memory = test_memory(records);
    cpu.registers[3] = 1;

    let decoded = decode_thumb32(&mut memory, 0x0000_1002, 0xe8df, 0xf003, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_100a);
    assert!(decoded.note.contains("[0x00001007] = 0x2"));
}

#[test]
fn decode_thumb32_ldrex_reads_word_and_advances() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[3] = 0x2000_1000;
    memory.write_u32(0x2000_1000, 0xdead_beef);

    let decoded = decode_thumb32(&mut memory, 0x0003_20b8, 0xe853, 0x2f00, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_20bc);
    assert_eq!(cpu.registers[2], 0xdead_beef);
    assert!(decoded.note.starts_with("LDREX r2, [r3"));
}

#[test]
fn decode_thumb32_strex_stores_word_and_reports_success() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[1] = 0xffff_ffff;
    cpu.registers[2] = 0x1234_5678;
    cpu.registers[3] = 0x2000_1000;

    let decoded = decode_thumb32(&mut memory, 0x0003_20bc, 0xe843, 0x2100, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_20c0);
    assert_eq!(memory.read_u32_or_zero(0x2000_1000), 0x1234_5678);
    assert_eq!(cpu.registers[1], 0);
    assert!(decoded.note.starts_with("STREX r1, r2, [r3"));
}

#[test]
fn decode_thumb32_ldr_pc_post_index_branches_to_loaded_address() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[13] = 0x2003_ff8c;
    memory.write_u32(0x2003_ff8c, 0x6005_bb51);

    let decoded = decode_thumb32(&mut memory, 0x0002_5e5c, 0xf85d, 0xfb04, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6005_bb50);
    assert_eq!(cpu.registers[13], 0x2003_ff90);
    assert!(decoded.note.starts_with("LDR.W pc, [sp], #0x4"));
}

#[test]
fn decode_thumb32_ldrd_post_index_loads_pair_and_writes_back() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[13] = 0x2003_ffb0;
    memory.write_u32(0x2003_ffb0, 0x2025_f684);
    memory.write_u32(0x2003_ffb4, 0x2001_6630);

    let decoded = decode_thumb32(&mut memory, 0x6000_5664, 0xe8fd, 0x4502, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_5668);
    assert_eq!(cpu.registers[4], 0x2025_f684);
    assert_eq!(cpu.registers[5], 0x2001_6630);
    assert_eq!(cpu.registers[13], 0x2003_ffb8);
    assert!(decoded.note.starts_with("LDRD r4, r5, [sp], #0x8"));
}

#[test]
fn decode_thumb32_strd_pre_index_stores_pair_and_writes_back() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[4] = 0x1111_2222;
    cpu.registers[5] = 0x3333_4444;
    cpu.registers[13] = 0x2003_ffb8;

    let decoded = decode_thumb32(&mut memory, 0x6000_5a08, 0xe96d, 0x4502, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_5a0c);
    assert_eq!(memory.read_u32_or_zero(0x2003_ffb0), 0x1111_2222);
    assert_eq!(memory.read_u32_or_zero(0x2003_ffb4), 0x3333_4444);
    assert_eq!(cpu.registers[13], 0x2003_ffb0);
    assert!(decoded.note.starts_with("STRD r4, r5, [sp, #-0x8]!"));
}

#[test]
fn decode_thumb32_ands_w_shifted_register_sets_flags() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[12] = 1;
    cpu.registers[14] = 0;

    let decoded = decode_thumb32(&mut memory, 0x6000_d0b0, 0xea1e, 0x030c, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_d0b4);
    assert_eq!(cpu.registers[3], 0);
    assert!(cpu.apsr.z);
    assert!(!cpu.apsr.n);
    assert!(decoded.note.starts_with("ANDS.W r3, lr, r12"));
}

#[test]
fn decode_thumb32_orn_w_shifted_register_ors_with_inverted_operand() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[2] = 0x0000_00f0;
    cpu.registers[5] = 0xffff_0f0f;

    let decoded = decode_thumb32(&mut memory, 0x6000_568a, 0xea62, 0x0205, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_568e);
    assert_eq!(cpu.registers[2], 0x0000_f0f0);
    assert!(decoded.note.starts_with("ORN.W r2, r2, r5"));
}

#[test]
fn decode_thumb32_uadd8_sets_parallel_result_and_ge_flags() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[2] = 0x00ff_1080;
    cpu.registers[12] = 0x0101_0280;

    let decoded = decode_thumb32(&mut memory, 0x6000_55f0, 0xfa82, 0xf24c, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_55f4);
    assert_eq!(cpu.registers[2], 0x0100_1200);
    assert_eq!(cpu.apsr.ge, 0b0101);
    assert!(decoded.note.starts_with("UADD8 r2, r2, r12"));
}

#[test]
fn decode_thumb32_sel_picks_bytes_from_ge_flags() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[4] = 0x1111_1111;
    cpu.registers[12] = 0xeeee_eeee;
    cpu.apsr.ge = 0b0101;

    let decoded = decode_thumb32(&mut memory, 0x6000_55f4, 0xfaa4, 0xf28c, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_55f8);
    assert_eq!(cpu.registers[2], 0xee11_ee11);
    assert!(decoded.note.starts_with("SEL r2, r4, r12"));
}

#[test]
fn decode_thumb32_clz_counts_leading_zero_bits() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[2] = 0x0000_00ff;

    let decoded = decode_thumb32(&mut memory, 0x6000_5660, 0xfab2, 0xf282, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_5664);
    assert_eq!(cpu.registers[2], 24);
    assert!(decoded.note.starts_with("CLZ r2, r2"));
}

#[test]
fn decode_thumb32_rbit_reverses_register_bits() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[5] = 0x0123_4567;

    let decoded = decode_thumb32(&mut memory, 0x0000_0034, 0xfa95, 0xf4a5, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_0038);
    assert_eq!(cpu.registers[4], 0xe6a2_c480);
    assert!(decoded.note.starts_with("RBIT r4, r5"));
}

#[test]
fn decode_thumb32_ldrsh_w_immediate_sign_extends_halfword() {
    let records = vec![DataRecord {
        address: 0x2003_6210,
        bytes: vec![0x01, 0x80],
    }];
    let mut cpu = test_cpu();
    let mut memory = test_memory(records);
    cpu.registers[0] = 0x2003_6008;

    let decoded = decode_thumb32(&mut memory, 0x0001_2f58, 0xf9b0, 0x3208, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0001_2f5c);
    assert_eq!(cpu.registers[3], 0xffff_8001);
    assert!(decoded.note.starts_with("LDRSH.W r3, [r0, #0x208]"));
}

#[test]
fn decode_thumb32_ldrsh_w_preindexed_sign_extends_and_writes_back() {
    let records = vec![DataRecord {
        address: 0x2002_0910,
        bytes: vec![0x34, 0x80],
    }];
    let mut cpu = test_cpu();
    let mut memory = test_memory(records);
    cpu.registers[2] = 0x2002_090e;

    let decoded = decode_thumb32(&mut memory, 0x0000_b850, 0xf932, 0x3f02, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_b854);
    assert_eq!(cpu.registers[2], 0x2002_0910);
    assert_eq!(cpu.registers[3], 0xffff_8034);
    assert!(decoded.note.starts_with("LDRSH.W r3, [r2, #0x2]!"));
}

#[test]
fn decode_thumb32_mvn_w_expands_modified_immediate() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);

    let decoded = decode_thumb32(&mut memory, 0x0000_037c, 0xf06f, 0x0163, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_0380);
    assert_eq!(decoded.opcode32, Some(0xf06f_0163));
    assert_eq!(cpu.registers[1], 0xffff_ff9c);
    assert!(decoded.note.starts_with("MVN.W r1, pc"));
}

#[test]
fn decode_thumb32_rsb_w_immediate_subtracts_register_from_immediate() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[2] = 0x20;

    let decoded = decode_thumb32(&mut memory, 0x6005_c318, 0xf1c2, 0x0241, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6005_c31c);
    assert_eq!(cpu.registers[2], 0x21);
    assert!(decoded.note.starts_with("RSB.W r2, r2, #0x00000041"));
}

#[test]
fn decode_thumb32_sbcs_w_immediate_uses_carry_as_not_borrow() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[1] = 4;
    cpu.apsr.c = false;

    let decoded = decode_thumb32(&mut memory, 0x6000_311c, 0xf171, 0x0300, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_3120);
    assert_eq!(cpu.registers[3], 3);
    assert!(cpu.apsr.c);
    assert!(decoded.note.starts_with("SBCS.W r3, r1, #0x00000000"));
}

#[test]
fn decode_thumb32_bl_wins_over_data_processing_overlap() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[9] = 0x8000_8080;

    let decoded = decode_thumb32(&mut memory, 0x6000_5baa, 0xf059, 0xfb81, &mut cpu);

    assert_eq!(decoded.opcode32, Some(0xf059_fb81));
    assert_eq!(cpu.registers[14], 0x6000_5baf);
    assert_eq!(cpu.registers[11], 0);
    assert!(decoded.note.starts_with("BL "));
}

#[test]
fn decode_thumb32_conditional_branch_uses_flags() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.apsr.z = true;

    let taken = decode_thumb32(&mut memory, 0x0003_1daa, 0xf000, 0x80ea, &mut cpu);
    assert_eq!(taken.next_pc, 0x0003_1f82);
    assert!(taken.note.starts_with("BEQ.W"));
    assert!(taken.note.ends_with("taken"));

    cpu.apsr.z = false;
    let skipped = decode_thumb32(&mut memory, 0x0003_1daa, 0xf000, 0x80ea, &mut cpu);
    assert_eq!(skipped.next_pc, 0x0003_1dae);
    assert!(skipped.note.ends_with("not taken"));
}

#[test]
fn decode_thumb32_mls_computes_remainder_pattern() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[0] = 37;
    cpu.registers[2] = 10;
    cpu.registers[5] = 3;

    let decoded = decode_thumb32(&mut memory, 0x0003_2822, 0xfb02, 0x0315, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_2826);
    assert_eq!(cpu.registers[3], 7);
    assert!(decoded.note.starts_with("MLS r3, r2, r5, r0"));
}

#[test]
fn decode_thumb32_mla_with_pc_accumulator_is_mul_alias() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[3] = 0x178;
    cpu.registers[4] = 2;
    cpu.registers[15] = 0x6000_e444;

    let decoded = decode_thumb32(&mut memory, 0x6000_e444, 0xfb03, 0xf204, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_e448);
    assert_eq!(cpu.registers[2], 0x2f0);
    assert!(decoded.note.starts_with("MUL.W r2, r3, r4"));
}

#[test]
fn decode_thumb32_smulbb_multiplies_signed_low_halfwords() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[1] = 0xffff;
    cpu.registers[3] = 0x0002;

    let decoded = decode_thumb32(&mut memory, 0x6005_1e20, 0xfb11, 0xf403, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6005_1e24);
    assert_eq!(cpu.registers[4], 0xffff_fffe);
    assert!(decoded.note.starts_with("SMULBB r4, r1, r3"));
}

#[test]
fn decode_thumb32_uxtb_w_extends_low_byte() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[14] = 0xffff_ff37;

    let decoded = decode_thumb32(&mut memory, 0x0003_2830, 0xfa5f, 0xfe8e, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_2834);
    assert_eq!(cpu.registers[14], 0x0000_0037);
    assert!(decoded.note.starts_with("UXTB.W lr, lr"));
}

#[test]
fn decode_thumb32_uxtah_adds_zero_extended_halfword() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[4] = 0x2025_e65c;
    cpu.registers[5] = 0xffff_0007;

    let decoded = decode_thumb32(&mut memory, 0x6001_dee4, 0xfa14, 0xf385, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_dee8);
    assert_eq!(cpu.registers[3], 0x2025_e663);
    assert!(decoded.note.starts_with("UXTAH r3, r4, r5"));
}

#[test]
fn decode_thumb32_strb_pre_index_writes_back() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[6] = 0x2003_ffd3;
    cpu.registers[14] = 0x0000_0030;

    let decoded = decode_thumb32(&mut memory, 0x0003_283e, 0xf806, 0xef01, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_2842);
    assert_eq!(cpu.registers[6], 0x2003_ffd4);
    assert_eq!(memory.read_u8(0x2003_ffd4), Some(0x30));
    assert!(decoded.note.starts_with("STRB.W lr, [r6, #0x1]!"));
}

#[test]
fn decode_thumb32_rsb_shifted_register_subtracts_from_operand() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[1] = 5;

    let decoded = decode_thumb32(&mut memory, 0x6001_0a3e, 0xebc1, 0x03c1, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_0a42);
    assert_eq!(cpu.registers[3], 35);
    assert!(decoded.note.starts_with("RSB.W r3, r1, r1, LSL #3"));
}

#[test]
fn decode_thumb32_adc_w_uses_apsr_carry() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[0] = 0xffff_ffff;
    cpu.registers[2] = 1;
    cpu.apsr.c = true;

    let decoded = decode_thumb32(&mut memory, 0x0003_a5f8, 0xeb40, 0x0002, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_a5fc);
    assert_eq!(cpu.registers[0], 1);
    assert!(decoded.note.starts_with("ADC.W r0, r0, r2"));
}

#[test]
fn decode_thumb32_cmp_w_shifted_register_sets_sub_flags() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[12] = 0x607c_1000;
    cpu.registers[2] = 0x0006_07c0;

    let decoded = decode_thumb32(&mut memory, 0x0003_22b2, 0xebbc, 0x3f02, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_22b6);
    assert!(!cpu.apsr.z);
    assert!(cpu.apsr.c);
    assert!(decoded.note.starts_with("CMP.W r12, r2, LSL #12"));
}

#[test]
fn decode_thumb32_subs_w_shifted_register_writes_result_and_flags() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[9] = 0x10;
    cpu.registers[4] = 0x20;

    let decoded = decode_thumb32(&mut memory, 0x6000_27ba, 0xebb9, 0x0504, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6000_27be);
    assert_eq!(cpu.registers[5], 0xffff_fff0);
    assert!(cpu.apsr.n);
    assert!(!cpu.apsr.c);
    assert!(decoded.note.starts_with("SUBS.W r5, r9, r4"));
}

#[test]
fn decode_thumb32_smull_writes_signed_product_pair() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[0] = 3;
    cpu.registers[8] = 0xffff_fffe;

    let decoded = decode_thumb32(&mut memory, 0x6001_1ffc, 0xfb88, 0x3200, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_2000);
    assert_eq!(cpu.registers[3], 0xffff_fffa);
    assert_eq!(cpu.registers[2], 0xffff_ffff);
    assert!(decoded.note.starts_with("SMULL r3, r2, r8, r0"));
}

#[test]
fn decode_thumb32_sdiv_uses_cortex_m7_divide_encoding() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[7] = (-21i32) as u32;
    cpu.registers[12] = 4;

    let decoded = decode_thumb32(&mut memory, 0x6004_a4b2, 0xfb97, 0xf3fc, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6004_a4b6);
    assert_eq!(cpu.registers[3], (-5i32) as u32);
    assert!(decoded.note.starts_with("SDIV r3, r7, r12"));
}

#[test]
fn decode_thumb32_ea4f_lsl_alias_does_not_use_pc_as_rn() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[1] = 7;

    let decoded = decode_thumb32(&mut memory, 0x6001_0a68, 0xea4f, 0x02c1, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_0a6c);
    assert_eq!(cpu.registers[2], 56);
    assert!(decoded.note.starts_with("LSL.W r2, r1, #3"));
}

#[test]
fn decode_thumb32_asr_register_uses_register_shift_amount() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[1] = 0xffff_ff00;
    cpu.registers[3] = 4;

    let decoded = decode_thumb32(&mut memory, 0x0003_1756, 0xfa41, 0xf203, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_175a);
    assert_eq!(cpu.registers[2], 0xffff_fff0);
    assert!(decoded.note.starts_with("ASR.W r2, r1, r3"));
}

#[test]
fn decode_thumb32_vmul_f32_multiplies_single_registers() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[14] = 2.0f32.to_bits();
    cpu.fpu_s[15] = 0.5f32.to_bits();

    let decoded = decode_thumb32(&mut memory, 0x0003_39ca, 0xee27, 0x7a27, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_39ce);
    assert_eq!(f32::from_bits(cpu.fpu_s[14]), 1.0);
    assert!(decoded.note.starts_with("VMUL.F32 s14, s14, s15"));
}

#[test]
fn decode_thumb32_vstmia_single_stores_and_writes_back() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[4] = 0x2001_92d8;
    cpu.fpu_s[15] = 0x3586_37bd;

    let decoded = decode_thumb32(&mut memory, 0x0002_1bba, 0xece4, 0x7a01, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0002_1bbe);
    assert_eq!(cpu.registers[4], 0x2001_92dc);
    assert_eq!(memory.read_u32_or_zero(0x2001_92d8), 0x3586_37bd);
    assert!(decoded.note.starts_with("VSTMIA r4!, {s15}"));
}

#[test]
fn decode_thumb32_vpush_double_stores_range_and_writes_back_sp() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[13] = 0x2003_ff80;
    cpu.fpu_s[16] = 0x1111_1111;
    cpu.fpu_s[17] = 0x2222_2222;
    cpu.fpu_s[18] = 0x3333_3333;
    cpu.fpu_s[19] = 0x4444_4444;

    let decoded = decode_thumb32(&mut memory, 0x0002_1b92, 0xed2d, 0x8b04, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0002_1b96);
    assert_eq!(cpu.registers[13], 0x2003_ff70);
    assert_eq!(memory.read_u32_or_zero(0x2003_ff70), 0x1111_1111);
    assert_eq!(memory.read_u32_or_zero(0x2003_ff74), 0x2222_2222);
    assert_eq!(memory.read_u32_or_zero(0x2003_ff78), 0x3333_3333);
    assert_eq!(memory.read_u32_or_zero(0x2003_ff7c), 0x4444_4444);
    assert!(decoded.note.starts_with("VPUSH sp!, {d8-d9}"));
}

#[test]
fn decode_thumb32_vfma_f64_accumulates_double_registers() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    set_fpu_d(&mut cpu, 0, 1.0f64.to_bits());
    set_fpu_d(&mut cpu, 3, 2.0f64.to_bits());
    set_fpu_d(&mut cpu, 4, 0.25f64.to_bits());

    let decoded = decode_thumb32(&mut memory, 0x0003_7b1c, 0xeea3, 0x0b04, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_7b20);
    assert_eq!(f64::from_bits(get_fpu_d(&cpu, 0)), 1.5);
    assert!(decoded.note.starts_with("VFMA.F64 d0, d3, d4"));
}

#[test]
fn decode_thumb32_vfms_f32_subtracts_product_from_accumulator() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[0] = 5.0f32.to_bits();
    cpu.fpu_s[6] = 2.0f32.to_bits();
    cpu.fpu_s[8] = 0.25f32.to_bits();

    let decoded = decode_thumb32(&mut memory, 0x0003_7b1c, 0xeea3, 0x0a44, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_7b20);
    assert_eq!(f32::from_bits(cpu.fpu_s[0]), 4.5);
    assert!(decoded.note.starts_with("VFMS.F32 s0, s6, s8"));
}

#[test]
fn decode_thumb32_vfnms_f64_subtracts_accumulator_from_product() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    set_fpu_d(&mut cpu, 7, 5.0f64.to_bits());
    set_fpu_d(&mut cpu, 2, 3.0f64.to_bits());
    set_fpu_d(&mut cpu, 3, 4.0f64.to_bits());

    let decoded = decode_thumb32(&mut memory, 0x0003_758c, 0xee92, 0x7b03, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_7590);
    assert_eq!(f64::from_bits(get_fpu_d(&cpu, 7)), 7.0);
    assert!(decoded.note.starts_with("VFNMS.F64 d7, d2, d3"));
}

#[test]
fn decode_thumb32_vfnma_f64_negates_product_and_accumulator() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    set_fpu_d(&mut cpu, 7, 5.0f64.to_bits());
    set_fpu_d(&mut cpu, 2, 3.0f64.to_bits());
    set_fpu_d(&mut cpu, 3, 4.0f64.to_bits());

    let decoded = decode_thumb32(&mut memory, 0x0003_758c, 0xee92, 0x7b43, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_7590);
    assert_eq!(f64::from_bits(get_fpu_d(&cpu, 7)), -17.0);
    assert!(decoded.note.starts_with("VFNMA.F64 d7, d2, d3"));
}

#[test]
fn decode_thumb32_vmaxnm_f32_keeps_larger_number() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[1] = (-36.0f32).to_bits();
    cpu.fpu_s[13] = (-18.0f32).to_bits();

    let decoded = decode_thumb32(&mut memory, 0x6001_0a52, 0xfec0, 0x0aa6, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_0a56);
    assert_eq!(f32::from_bits(cpu.fpu_s[1]), -18.0);
    assert!(decoded.note.starts_with("VMAXNM.F32 s1, s1, s13"));
}

#[test]
fn decode_thumb32_vmaxnm_f32_handles_nonzero_vn() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[2] = (-4.0f32).to_bits();
    cpu.fpu_s[14] = 0.0f32.to_bits();

    let decoded = decode_thumb32(&mut memory, 0x6001_0a56, 0xfe81, 0x1a07, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_0a5a);
    assert_eq!(f32::from_bits(cpu.fpu_s[2]), 0.0);
    assert!(decoded.note.starts_with("VMAXNM.F32 s2, s2, s14"));
}

#[test]
fn decode_thumb32_vminnm_f32_keeps_smaller_number() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[2] = (-4.0f32).to_bits();
    cpu.fpu_s[14] = 0.0f32.to_bits();

    let decoded = decode_thumb32(&mut memory, 0x6001_0a56, 0xfe81, 0x1a47, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_0a5a);
    assert_eq!(f32::from_bits(cpu.fpu_s[2]), -4.0);
    assert!(decoded.note.starts_with("VMINNM.F32 s2, s2, s14"));
}

#[test]
fn decode_thumb32_vseleq_f32_uses_apsr_z_flag() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.apsr.z = true;
    cpu.fpu_s[16] = 3.0f32.to_bits();
    cpu.fpu_s[20] = 4.0f32.to_bits();

    let decoded = decode_thumb32(&mut memory, 0x6001_0ad6, 0xfe08, 0x8a0a, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_0ada);
    assert_eq!(f32::from_bits(cpu.fpu_s[16]), 3.0);
    assert!(decoded.note.starts_with("VSELEQ.F32 s16, s16, s20"));

    cpu.apsr.z = false;
    let decoded = decode_thumb32(&mut memory, 0x6001_0ad6, 0xfe08, 0x8a0a, &mut cpu);

    assert_eq!(f32::from_bits(cpu.fpu_s[16]), 4.0);
    assert!(decoded.note.starts_with("VSELEQ.F32 s16, s16, s20"));
}

#[test]
fn decode_thumb32_vcvt_f64_f32_converts_single_to_double() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[8] = 1.25f32.to_bits();

    let decoded = decode_thumb32(&mut memory, 0x0003_7b10, 0xeeb7, 0x4ac4, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_7b14);
    assert_eq!(f64::from_bits(get_fpu_d(&cpu, 4)), 1.25);
    assert!(decoded.note.starts_with("VCVT.F64.F32 d4, s8"));
}

#[test]
fn decode_thumb32_vcvt_f32_f64_converts_double_to_single() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    set_fpu_d(&mut cpu, 0, 1.5f64.to_bits());

    let decoded = decode_thumb32(&mut memory, 0x0003_7b10, 0xeeb7, 0x0bc0, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_7b14);
    assert_eq!(f32::from_bits(cpu.fpu_s[0]), 1.5);
    assert!(decoded.note.starts_with("VCVT.F32.F64 s0, d0"));
}

#[test]
fn decode_thumb32_vcvt_f32_u32_converts_unsigned_integer_bits() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[14] = 42;

    let decoded = decode_thumb32(&mut memory, 0x0003_39bc, 0xeeb8, 0x7a47, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_39c0);
    assert_eq!(f32::from_bits(cpu.fpu_s[14]), 42.0);
    assert!(decoded.note.starts_with("VCVT.F32.U32 s14, s14"));
}

#[test]
fn decode_thumb32_vcvt_u32_f32_converts_source_s0() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[0] = 255.0f32.to_bits();

    let decoded = decode_thumb32(&mut memory, 0x0000_23c2, 0xeefc, 0x7ac0, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_23c6);
    assert_eq!(cpu.fpu_s[15], 255);
    assert!(decoded.note.starts_with("VCVT.U32.F32 s15, s0"));
}

#[test]
fn decode_thumb32_vcvt_f64_s32_converts_signed_integer_bits() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[14] = (-7i32) as u32;

    let decoded = decode_thumb32(&mut memory, 0x0003_39bc, 0xeeb8, 0x7bc7, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_39c0);
    assert_eq!(f64::from_bits(get_fpu_d(&cpu, 7)), -7.0);
    assert!(decoded.note.starts_with("VCVT.F64.S32 d7, s14"));
}

#[test]
fn decode_thumb32_vneg_f32_negates_single_register() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[14] = 1.0f32.to_bits();

    let decoded = decode_thumb32(&mut memory, 0x0000_11d4, 0xeeb1, 0x7a47, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_11d8);
    assert_eq!(f32::from_bits(cpu.fpu_s[14]), -1.0);
    assert!(decoded.note.starts_with("VNEG.F32 s14, s14"));
}

#[test]
fn decode_thumb32_vabs_f32_uses_first_half_d_bit() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[13] = (-0.25f32).to_bits();

    let decoded = decode_thumb32(&mut memory, 0x0000_06d2, 0xeef0, 0x5ae6, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0000_06d6);
    assert_eq!(f32::from_bits(cpu.fpu_s[11]), 0.25);
    assert!(decoded.note.starts_with("VABS.F32 s11, s13"));
}

#[test]
fn decode_thumb32_vcmpe_f32_sets_fpscr_and_apsr_flags() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpu_s[0] = 1.0f32.to_bits();
    cpu.fpu_s[3] = 2.0f32.to_bits();

    let decoded = decode_thumb32(&mut memory, 0x6001_0a2e, 0xeeb4, 0x0ae1, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_0a32);
    assert!(cpu.apsr.n);
    assert!(!cpu.apsr.z);
    assert!(!cpu.apsr.c);
    assert!(!cpu.apsr.v);
    assert_eq!(cpu.fpscr & 0xf000_0000, 0x8000_0000);
    assert!(decoded.note.starts_with("VCMPE.F32 s0, s3"));
}

#[test]
fn decode_thumb32_vmrs_transfers_fpscr_flags_to_apsr() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.fpscr = 0x3000_0000;

    let decoded = decode_thumb32(&mut memory, 0x6001_0a32, 0xeef1, 0xfa10, &mut cpu);

    assert_eq!(decoded.next_pc, 0x6001_0a36);
    assert!(!cpu.apsr.n);
    assert!(!cpu.apsr.z);
    assert!(cpu.apsr.c);
    assert!(cpu.apsr.v);
    assert_eq!(decoded.note, "VMRS APSR_nzcv, FPSCR");
}

#[test]
fn decode_thumb32_vmov_f32_immediate_uses_first_half_d_bit() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);

    let decoded = decode_thumb32(&mut memory, 0x0002_1be0, 0xeef6, 0x8a00, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0002_1be4);
    assert_eq!(f32::from_bits(cpu.fpu_s[17]), 0.5);
    assert!(decoded.note.starts_with("VMOV.F32 s17, #0.5"));
}

#[test]
fn decode_thumb32_vmov_double_from_core_pair() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    cpu.registers[0] = 0x89ab_cdef;
    cpu.registers[1] = 0x0123_4567;

    let decoded = decode_thumb32(&mut memory, 0x0003_7bba, 0xec41, 0x0b17, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_7bbe);
    assert_eq!(get_fpu_d(&cpu, 7), 0x0123_4567_89ab_cdef);
    assert!(decoded.note.starts_with("VMOV d7, r0, r1"));
}

#[test]
fn decode_thumb32_vmov_double_to_core_pair() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    set_fpu_d(&mut cpu, 7, 0x0123_4567_89ab_cdef);

    let decoded = decode_thumb32(&mut memory, 0x0003_7bba, 0xec51, 0x0b17, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_7bbe);
    assert_eq!(cpu.registers[0], 0x89ab_cdef);
    assert_eq!(cpu.registers[1], 0x0123_4567);
    assert!(decoded.note.starts_with("VMOV r0, r1, d7"));
}

#[test]
fn decode_thumb32_vmov_f64_register_copies_double_register() {
    let mut cpu = test_cpu();
    let mut memory = test_memory(vec![]);
    set_fpu_d(&mut cpu, 6, 0x0123_4567_89ab_cdef);

    let decoded = decode_thumb32(&mut memory, 0x0003_7e16, 0xeeb0, 0x0b46, &mut cpu);

    assert_eq!(decoded.next_pc, 0x0003_7e1a);
    assert_eq!(get_fpu_d(&cpu, 0), 0x0123_4567_89ab_cdef);
    assert!(decoded.note.starts_with("VMOV.F64 d0, d6"));
}

#[test]
fn expands_thumb_modified_immediates() {
    assert_eq!(thumb_expand_imm(0x02a), 0x0000_002a);
    assert_eq!(thumb_expand_imm(0x82a), 0x00aa_0000);
}

#[test]
fn expands_vfp_f32_immediates() {
    assert_eq!(f32::from_bits(vfp_expand_imm_f32_bits(0x24)), 10.0);
    assert_eq!(f32::from_bits(vfp_expand_imm_f32_bits(0x60)), 0.5);
}

#[test]
fn marks_teensy_clock_status_register_ready_after_write() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(0x400d_8070, 0x0011_201c);

    assert_eq!(memory.read_u32_or_zero(0x400d_8070), 0x8011_201c);
}

#[test]
fn exposes_cortex_m7_system_defaults_from_spec_layer() {
    let mut memory = test_memory(vec![]);

    assert_eq!(memory.read_u32_or_zero(SCB_CPUID), 0x411f_c271);
    assert_eq!(memory.read_u32_or_zero(MPU_TYPE), 0x0000_0800);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_CALIB), 0);
}

#[test]
fn ccm_busy_flags_self_clear_and_analog_plls_lock_after_enable() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(CCM_CDHIPR, 0xffff_ffff);
    memory.write_u32(0x400d_8030, 0x0000_2000);

    assert_eq!(memory.read_u32_or_zero(CCM_CDHIPR), 0);
    assert_eq!(memory.read_u32_or_zero(0x400d_8030), 0x8000_2000);
}

#[test]
fn dcdc_and_ccm_analog_set_clear_toggle_aliases_modify_base_register() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(0x4008_0000, 0x10);
    memory.write_u32(0x4008_0004, 0x03);
    memory.write_u32(0x4008_0008, 0x02);
    memory.write_u32(0x4008_000c, 0x04);

    assert_eq!(memory.read_u32_or_zero(0x4008_0000), 0x8000_0015);
}

#[test]
fn pit_timer_flags_become_ready_and_clear_with_write_one() {
    let mut memory = test_memory(vec![]);
    let ldval = pit_channel_address(0, 0x00);
    let cval = pit_channel_address(0, 0x04);
    let tctrl = pit_channel_address(0, 0x08);
    let tflg = pit_channel_address(0, 0x0c);

    memory.write_u32(ldval, 9);
    memory.write_u32(tctrl, 1);

    assert!(memory.read_u32_or_zero(cval) <= 9);
    assert_eq!(memory.read_u32_or_zero(tflg), 0);
    assert_eq!(memory.read_u32_or_zero(tflg), 1);

    memory.write_u32(tflg, 1);
    assert_eq!(memory.read_u32_or_zero(tflg), 0);
}

#[test]
fn flexspi_ip_commands_complete_and_lut_registers_read_back() {
    let mut memory = test_memory(vec![]);
    let base = FLEXSPI1_BASE;

    memory.write_u32(base + FLEXSPI_MCR0, 1);
    memory.write_u32(base + FLEXSPI_LUTKEY, 0x5af0_5af0);
    memory.write_u32(base + FLEXSPI_LUT_BASE, 0x0818_0420);
    memory.write_u32(base + FLEXSPI_IPCMD, 1);

    assert_eq!(memory.read_u32_or_zero(base + FLEXSPI_MCR0) & 1, 0);
    assert_eq!(memory.read_u32_or_zero(base + FLEXSPI_LUTKEY), 0x5af0_5af0);
    assert_eq!(
        memory.read_u32_or_zero(base + FLEXSPI_LUT_BASE),
        0x0818_0420
    );
    assert_eq!(memory.read_u32_or_zero(base + FLEXSPI_INTR) & 1, 1);
    assert_eq!(memory.read_u32_or_zero(base + FLEXSPI_IPRXFSTS) & 1, 1);
}

#[test]
fn systick_preserves_config_and_eventually_sets_countflag() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(SYSTICK_LOAD, 9);
    memory.write_u32(SYSTICK_VALUE, 0xffff_ffff);
    memory.write_u32(SYSTICK_CTRL, 0xffff_ffff);

    assert_eq!(memory.read_u32_or_zero(SYSTICK_LOAD), 9);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_CTRL) & 0x7, 0x7);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_VALUE), 9);
    memory.advance_virtual_cycles(4);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_VALUE), 5);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_CTRL) & (1 << 16), 0);
    memory.advance_virtual_cycles(6);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_VALUE), 9);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_CTRL) & (1 << 16), 1 << 16);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_CTRL) & (1 << 16), 0);
}

#[test]
fn systick_large_virtual_advance_sets_one_pending_tick() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(SYSTICK_LOAD, 9);
    memory.write_u32(SYSTICK_CTRL, 0x7);
    memory.advance_virtual_cycles(35);

    assert_eq!(
        memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSTSET,
        SCB_ICSR_PENDSTSET
    );
    assert_eq!(memory.read_u32_or_zero(SYSTICK_CTRL) & (1 << 16), 1 << 16);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_CTRL) & (1 << 16), 0);

    memory.write_u32(SCB_ICSR, SCB_ICSR_PENDSTCLR);
    memory.update_periodic_pending();
    assert_eq!(memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSTSET, 0);
}

#[test]
fn systick_external_clock_is_slower_than_core_clock() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(SYSTICK_LOAD, 9);
    memory.write_u32(SYSTICK_CTRL, 0x3);
    memory.advance_virtual_cycles(35);

    assert_eq!(memory.read_u32_or_zero(SCB_ICSR) & SCB_ICSR_PENDSTSET, 0);
    assert_eq!(memory.read_u32_or_zero(SYSTICK_VALUE), 9);
}

#[test]
fn nvic_enable_and_pending_registers_have_arm_w1_semantics() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(NVIC_ISER_BASE + 4, 0x24);
    memory.write_u32(NVIC_ICER_BASE + 4, 0x20);
    memory.write_u32(NVIC_ISPR_BASE, 0x8);
    memory.write_u32(NVIC_STIR, 35);

    assert_eq!(memory.read_u32_or_zero(NVIC_ISER_BASE + 4), 0x04);
    assert_eq!(memory.read_u32_or_zero(NVIC_ICER_BASE + 4), 0x04);
    assert_eq!(memory.read_u32_or_zero(NVIC_ISPR_BASE), 0x8);
    assert_eq!(memory.read_u32_or_zero(NVIC_ISPR_BASE + 4), 0x8);

    memory.write_u32(NVIC_ICPR_BASE, 0x8);
    memory.write_u32(NVIC_ICPR_BASE + 4, 0x8);
    assert_eq!(memory.read_u32_or_zero(NVIC_ISPR_BASE), 0);
    assert_eq!(memory.read_u32_or_zero(NVIC_ISPR_BASE + 4), 0);
}

#[test]
fn scb_fault_status_writes_clear_bits_and_aircr_does_not_reset_probe() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(SCB_CFSR, 0xffff_ffff);
    assert_eq!(memory.read_u32_or_zero(SCB_CFSR), 0);

    memory.write_u32(SCB_AIRCR, 0x05fa_0004);
    assert_eq!(memory.read_u32_or_zero(SCB_AIRCR), 0x05fa_0000);
}

#[test]
fn exposes_default_flexspi2_status_ready_bit() {
    let mut memory = test_memory(vec![]);

    assert_eq!(memory.read_u32_or_zero(0x402a_8014), 1);
}

#[test]
fn clears_flexspi2_busy_bit_after_control_write() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(0x402a_8000, 1);

    assert_eq!(memory.read_u32_or_zero(0x402a_8000), 0);
}

#[test]
fn exposes_default_usb2_status_ready_bit() {
    let mut memory = test_memory(vec![]);

    assert_eq!(memory.read_u32_or_zero(0x400c_8020), 1);
}

#[test]
fn clears_usdhc2_command_busy_bit_after_status_write() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(USDHC1_BASE + USDHC_SYS_CTRL, 0x0100_8000);

    assert_eq!(
        memory.read_u32_or_zero(USDHC1_BASE + USDHC_SYS_CTRL),
        0x0000_8000
    );
}

#[test]
fn clears_usdhc2_init_and_reset_bits_after_status_write() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(USDHC1_BASE + USDHC_SYS_CTRL, 0x080e_10f0);

    assert_eq!(
        memory.read_u32_or_zero(USDHC1_BASE + USDHC_SYS_CTRL),
        0x000e_10f0
    );
}

#[test]
fn exposes_default_usdhc2_transfer_ready_bit() {
    let mut memory = test_memory(vec![]);

    assert_eq!(
        memory.read_u32_or_zero(USDHC1_BASE + USDHC_PRES_STATE),
        USDHC_PRES_READY
    );
    assert_ne!(
        memory.read_u32_or_zero(USDHC1_BASE + USDHC_PRES_STATE)
            & (USDHC_PRES_DLSL | USDHC_PRES_CLSL),
        0
    );
}

#[test]
fn usdhc_int_status_is_latched_and_write_one_to_clear() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    assert_eq!(memory.read_u32_or_zero(base + USDHC_INT_STATUS), 0);

    memory.write_u32(base + USDHC_CMD_XFR_TYP, 17 << 24);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_CC
    );

    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_CC);
    assert_eq!(memory.read_u32_or_zero(base + USDHC_INT_STATUS), 0);

    memory.write_u32(base + USDHC_CMD_XFR_TYP, 17 << 24);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_CC
    );
}

#[test]
fn usdhc_data_command_sets_transfer_and_buffer_ready_status() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.write_u32(base + USDHC_CMD_XFR_TYP, (17 << 24) | (1 << 21));

    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_CC
    );
    assert_ne!(
        memory.read_u32_or_zero(base + USDHC_PRES_STATE) & USDHC_PRES_BREN,
        0
    );

    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_CC);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_TC | USDHC_INT_BRR
    );

    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_TC | USDHC_INT_BRR);
    assert_eq!(memory.read_u32_or_zero(base + USDHC_INT_STATUS), 0);
}

#[test]
fn usdhc_cmd6_switch_status_exposes_data_words() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.write_u32(base + USDHC_CMD_ARG, 0x00ff_ffff);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (6 << 24) | (1 << 21));

    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_CC
    );
    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_CC);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_TC | USDHC_INT_BRR
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0180_3200
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0180_0180
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0180_0180
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0000_0380
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0000_0100
    );
}

#[test]
fn usdhc_cmd6_switch_mode_reports_requested_high_speed_function() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.write_u32(base + USDHC_CMD_ARG, 0x80ff_fff1);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (6 << 24) | (1 << 21));
    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_CC);

    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0180_3200
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0180_0180
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0180_0180
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0000_0380
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0000_0101
    );

    memory.write_u32(base + USDHC_INT_STATUS, u32::MAX);
    memory.write_u32(base + USDHC_CMD_ARG, 0x80ff_ffff);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (6 << 24) | (1 << 21));
    for _ in 0..4 {
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT);
    }
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0000_0101
    );
}

#[test]
fn usdhc_cmd6_dma_sets_dint_and_copies_switch_status_to_ram() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.write_u32(NVIC_ISER_BASE + 12, 1 << 14);
    memory.write_u32(base + USDHC_INT_STATUS_EN, USDHC_INT_DINT);
    memory.write_u32(base + USDHC_INT_SIGNAL_EN, USDHC_INT_DINT);
    memory.write_u32(base + USDHC_DS_ADDR, 0x2000_0300);
    memory.write_u32(base + USDHC_BLK_ATT, 64 | (1 << 16));
    memory.write_u32(base + USDHC_MIX_CTRL, 1);
    memory.write_u32(base + USDHC_CMD_ARG, 0x00ff_ffff);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (6 << 24) | (1 << 21));

    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_CC
    );
    assert_eq!(memory.read_u32_or_zero(NVIC_ISPR_BASE + 12) & (1 << 14), 0);
    assert_eq!(memory.read_u32_or_zero(0x2000_0300), 0x0180_3200);

    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_CC);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_TC | USDHC_INT_BRR | USDHC_INT_DINT
    );
    assert_ne!(memory.read_u32_or_zero(NVIC_ISPR_BASE + 12) & (1 << 14), 0);

    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_DINT);
    assert_eq!(memory.read_u32_or_zero(NVIC_ISPR_BASE + 12) & (1 << 14), 0);
}

#[test]
fn usdhc_acmd51_returns_scr_after_cmd55() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.load_sd_image(&vec![0u8; 512]);
    memory.write_u32(base + USDHC_CMD_ARG, 0);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, 55 << 24);
    assert_eq!(memory.read_u32_or_zero(base + USDHC_CMD_RSP0), 0x0000_0120);
    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_CC);

    memory.write_u32(base + USDHC_DS_ADDR, 0x2000_0400);
    memory.write_u32(base + USDHC_BLK_ATT, 8 | (1 << 16));
    memory.write_u32(base + USDHC_MIX_CTRL, 1);
    memory.write_u32(base + USDHC_CMD_ARG, 0);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (51 << 24) | (1 << 21));

    assert_eq!(memory.read_u32_or_zero(base + USDHC_CMD_RSP0), 0x0000_0900);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_CC
    );
    assert_eq!(memory.read_u32_or_zero(0x2000_0400), 0x0000_0502);
    assert_eq!(memory.read_u32_or_zero(0x2000_0404), 0);

    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_CC);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_TC | USDHC_INT_BRR | USDHC_INT_DINT
    );
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x0000_0502
    );
}

#[test]
fn usdhc_acmd6_after_cmd55_sets_bus_width_without_switch_data() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.load_sd_image(&vec![0u8; 512]);
    memory.write_u32(base + USDHC_CMD_ARG, 0x0001_0000);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, 55 << 24);
    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_CC);

    memory.write_u32(base + USDHC_CMD_ARG, 2);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, 6 << 24);

    assert_eq!(memory.read_u32_or_zero(base + USDHC_CMD_RSP0), 0x0000_0900);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_CC
    );
    assert_eq!(memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT), 0);
    let recent = memory.recent_usdhc_commands(2);
    assert_eq!(recent[0].command_index, 6);
    assert!(recent[0].app_command);
    assert!(!recent[0].data_present);
}

#[test]
fn usdhc_cmd17_reads_from_loaded_sd_image() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;
    let mut image = vec![0u8; 1024];
    image[512..516].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);

    memory.load_sd_image(&image);
    memory.write_u32(base + USDHC_CMD_ARG, 1);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (17 << 24) | (1 << 21));

    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x7856_3412
    );
    let stats = memory.sd_card_stats();
    assert!(stats.present);
    assert_eq!(stats.blocks, 2);
    assert_eq!(stats.read_blocks, 1);
    assert_eq!(stats.last_read_block, Some(1));
}

#[test]
fn usdhc_cmd17_copies_loaded_sd_block_to_dma_address() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;
    let mut image = vec![0u8; 1024];
    image[512..520].copy_from_slice(&[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0]);

    memory.load_sd_image(&image);
    memory.write_u32(base + USDHC_DS_ADDR, 0x2000_0100);
    memory.write_u32(base + USDHC_BLK_ATT, 512 | (1 << 16));
    memory.write_u32(base + USDHC_CMD_ARG, 1);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (17 << 24) | (1 << 21));

    assert_eq!(memory.read_u32_or_zero(0x2000_0100), 0x7856_3412);
    assert_eq!(memory.read_u32_or_zero(0x2000_0104), 0xf0de_bc9a);
    assert_eq!(memory.sd_card_stats().read_blocks, 1);
}

#[test]
fn usdhc_cmd18_copies_multiple_blocks_to_dma_address() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;
    let mut image = vec![0u8; 1536];
    image[512..516].copy_from_slice(&[1, 2, 3, 4]);
    image[1024..1028].copy_from_slice(&[5, 6, 7, 8]);

    memory.load_sd_image(&image);
    memory.write_u32(base + USDHC_DS_ADDR, 0x2000_0200);
    memory.write_u32(base + USDHC_BLK_ATT, 512 | (2 << 16));
    memory.write_u32(base + USDHC_CMD_ARG, 1);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (18 << 24) | (1 << 21));

    assert_eq!(memory.read_u32_or_zero(0x2000_0200), 0x0403_0201);
    assert_eq!(memory.read_u32_or_zero(0x2000_0400), 0x0807_0605);
    assert_eq!(memory.sd_card_stats().read_blocks, 2);
    assert_eq!(memory.sd_card_stats().last_read_block, Some(2));
}

#[test]
fn usdhc_cmd24_writes_to_loaded_sd_image() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.load_sd_image(&vec![0u8; 1024]);
    memory.write_u32(base + USDHC_CMD_ARG, 1);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (24 << 24) | (1 << 21));
    memory.write_u32(base + USDHC_DATA_BUFF_ACC_PORT, 0x4433_2211);
    for _ in 1..128 {
        memory.write_u32(base + USDHC_DATA_BUFF_ACC_PORT, 0);
    }

    let stats = memory.sd_card_stats();
    assert_eq!(stats.written_blocks, 1);
    assert_eq!(stats.last_written_block, Some(1));
    assert_eq!(
        &memory.sd_image_bytes()[512..516],
        &[0x11, 0x22, 0x33, 0x44]
    );

    memory.write_u32(base + USDHC_CMD_ARG, 1);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (17 << 24) | (1 << 21));
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_DATA_BUFF_ACC_PORT),
        0x4433_2211
    );
}

#[test]
fn usdhc_cmd10_returns_cid_response() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.write_u32(base + USDHC_CMD_XFR_TYP, 10 << 24);

    assert_ne!(memory.read_u32_or_zero(base + USDHC_CMD_RSP0), 0);
}

#[test]
fn usdhc_cmd6_returns_ready_r1_response() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.write_u32(base + USDHC_CMD_ARG, 0x00ff_ffff);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, (6 << 24) | (1 << 21));

    assert_eq!(memory.read_u32_or_zero(base + USDHC_CMD_RSP0), 0x0000_0900);
}

#[test]
fn usdhc_interrupt_signal_enable_pends_and_clear_status_clears_irq() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.write_u32(NVIC_ISER_BASE + 12, 1 << 14);
    memory.write_u32(base + USDHC_INT_STATUS_EN, USDHC_INT_CC);
    memory.write_u32(base + USDHC_INT_SIGNAL_EN, USDHC_INT_CC);
    memory.write_u32(base + USDHC_CMD_XFR_TYP, 8 << 24);

    assert_ne!(memory.read_u32_or_zero(NVIC_ISPR_BASE + 12) & (1 << 14), 0);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_INT_STATUS),
        USDHC_INT_CC
    );

    memory.write_u32(base + USDHC_INT_STATUS, USDHC_INT_CC);
    assert_eq!(memory.read_u32_or_zero(NVIC_ISPR_BASE + 12) & (1 << 14), 0);
}

#[test]
fn usdhc_reset_clears_latched_status_and_keeps_present_state_ready() {
    let mut memory = test_memory(vec![]);
    let base = USDHC1_BASE;

    memory.write_u32(base + USDHC_CMD_XFR_TYP, (17 << 24) | (1 << 21));
    assert_ne!(memory.read_u32_or_zero(base + USDHC_INT_STATUS), 0);

    memory.write_u32(base + USDHC_SYS_CTRL, 0x0100_0000);

    assert_eq!(memory.read_u32_or_zero(base + USDHC_INT_STATUS), 0);
    assert_eq!(memory.read_u32_or_zero(base + USDHC_CMD_RSP0), 0);
    assert_eq!(
        memory.read_u32_or_zero(base + USDHC_PRES_STATE),
        USDHC_PRES_READY
    );
}

#[test]
fn mirrors_usdhc_ready_bits_on_second_controller() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(USDHC2_BASE + USDHC_SYS_CTRL, 0x0100_8000);
    assert_eq!(
        memory.read_u32_or_zero(USDHC2_BASE + USDHC_SYS_CTRL),
        0x0000_8000
    );
    assert_eq!(
        memory.read_u32_or_zero(USDHC2_BASE + USDHC_PRES_STATE),
        USDHC_PRES_READY
    );

    memory.write_u32(USDHC2_BASE + USDHC_CMD_XFR_TYP, 8 << 24);
    assert_eq!(
        memory.read_u32_or_zero(USDHC2_BASE + USDHC_INT_STATUS),
        USDHC_INT_CC
    );
    memory.write_u32(USDHC2_BASE + USDHC_INT_STATUS, USDHC_INT_CC);
    assert_eq!(memory.read_u32_or_zero(USDHC2_BASE + USDHC_INT_STATUS), 0);
}

#[test]
fn reports_usdhc_command_hotspots() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(USDHC1_BASE + USDHC_CMD_ARG, 0x1aa);
    memory.write_u32(USDHC1_BASE + USDHC_CMD_XFR_TYP, 8 << 24);
    memory.write_u32(USDHC1_BASE + USDHC_CMD_XFR_TYP, 8 << 24);
    memory.write_u32(USDHC2_BASE + USDHC_CMD_ARG, 0x2000);
    memory.write_u32(USDHC2_BASE + USDHC_CMD_XFR_TYP, (17 << 24) | (1 << 21));

    let commands = memory.top_usdhc_commands(4);
    assert_eq!(commands[0].instance, "USDHC1");
    assert_eq!(commands[0].command_index, 8);
    assert_eq!(commands[0].argument, 0x1aa);
    assert!(!commands[0].data_present);
    assert_eq!(commands[0].count, 2);
    assert_eq!(commands[1].instance, "USDHC2");
    assert_eq!(commands[1].command_index, 17);
    assert!(commands[1].data_present);
}

#[test]
fn reports_recent_usdhc_commands_newest_first() {
    let mut memory = test_memory(vec![]);

    memory.write_u32(USDHC1_BASE + USDHC_CMD_XFR_TYP, 0 << 24);
    memory.write_u32(USDHC1_BASE + USDHC_CMD_ARG, 0x1aa);
    memory.write_u32(USDHC1_BASE + USDHC_CMD_XFR_TYP, 8 << 24);
    memory.write_u32(USDHC1_BASE + USDHC_CMD_ARG, 0);
    memory.write_u32(USDHC1_BASE + USDHC_CMD_XFR_TYP, 55 << 24);

    let commands = memory.recent_usdhc_commands(2);
    assert_eq!(commands[0].command_index, 55);
    assert_eq!(commands[0].argument, 0);
    assert_eq!(commands[1].command_index, 8);
    assert_eq!(commands[1].argument, 0x1aa);
}

#[test]
fn scans_for_vector_table_beyond_common_offsets() {
    let mut hex = String::new();
    hex.push_str(&hex_record(0, 0x04, &[0x60, 0x00]));
    hex.push_str(&hex_record(
        0x1800,
        0x00,
        &[0x00, 0x00, 0x30, 0x20, 0x81, 0x18, 0x00, 0x60],
    ));
    hex.push_str(":00000001FF\n");

    let analysis = analyze_teensy_hex(hex.as_bytes()).expect("hex should parse");
    let vector = analysis.vector_table.expect("vector table");
    assert_eq!(vector.address, 0x6000_1800);
    assert_eq!(vector.initial_sp_region, Some("FlexRAM OCRAM"));
    assert_eq!(vector.reset_pc_aligned, 0x6000_1880);
}

#[test]
fn rejects_bad_checksum() {
    let error = analyze_teensy_hex(b":00000001FE\n").expect_err("checksum should fail");
    assert!(matches!(error, EmulatorError::Checksum { .. }));
}

fn hex_record(address: u16, record_type: u8, data: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(data.len() + 5);
    bytes.push(data.len() as u8);
    bytes.extend_from_slice(&address.to_be_bytes());
    bytes.push(record_type);
    bytes.extend_from_slice(data);
    let sum = bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    bytes.push(0u8.wrapping_sub(sum));

    let mut out = String::from(":");
    for byte in bytes {
        out.push_str(&format!("{byte:02X}"));
    }
    out.push('\n');
    out
}

fn minimal_teensy_ivt_hex() -> String {
    let mut hex = minimal_teensy_ivt_without_eof();
    hex.push_str(":00000001FF\n");
    hex
}

fn minimal_teensy_ivt_without_eof() -> String {
    let mut hex = String::new();
    hex.push_str(&hex_record(0, 0x04, &[0x60, 0x00]));
    hex.push_str(&hex_record(
        0x1000,
        0x00,
        &[
            0xd1, 0x00, 0x20, 0x43, 0x25, 0x5c, 0x00, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
    ));
    hex.push_str(&hex_record(
        0x1010,
        0x00,
        &[
            0x20, 0x10, 0x00, 0x60, 0x00, 0x10, 0x00, 0x60, 0xa4, 0x1c, 0x00, 0x60, 0x00, 0x00,
            0x00, 0x00,
        ],
    ));
    hex.push_str(&hex_record(
        0x1020,
        0x00,
        &[
            0x00, 0x00, 0x00, 0x60, 0x00, 0xb0, 0x1c, 0x00, 0x00, 0x00, 0x00, 0x00,
        ],
    ));
    hex
}

fn seed_m8_input_lane_series(session: &mut TeensyEmulatorSession, input_base: u32) {
    for lane in 0..M8_APP_INPUT_LANE_COUNT {
        let lane_base = input_base + lane * M8_APP_INPUT_LANE_STRIDE;
        for (offset, byte) in 0x2000_2d48u32.to_le_bytes().into_iter().enumerate() {
            session.debug_poke_u8(lane_base + offset as u32, byte);
        }
        session.debug_poke_u8(lane_base + 0x0c, lane as u8);
        session.debug_poke_u8(lane_base + M8_APP_INPUT_CURRENT_STATE_OFFSET, 0xff);
        session.debug_poke_u8(lane_base + M8_APP_INPUT_PREVIOUS_STATE_OFFSET, 0xff);
        session.debug_poke_u8(lane_base + M8_APP_INPUT_DIRTY_FLAG_OFFSET, 0);
        session.debug_poke_u8(lane_base + M8_APP_INPUT_DIRTY_FLAG_OFFSET + 1, 0);
    }
}

fn test_cpu() -> CpuState {
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

fn test_memory(records: Vec<DataRecord>) -> ExecutionMemory {
    ExecutionMemory::new(records)
}

fn decode_slip_frames(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut frame = Vec::new();
    let mut escaped = false;

    for byte in bytes {
        if escaped {
            frame.push(match *byte {
                0xdc => 0xc0,
                0xdd => 0xdb,
                value => value,
            });
            escaped = false;
            continue;
        }

        match *byte {
            0xc0 => frames.push(std::mem::take(&mut frame)),
            0xdb => escaped = true,
            value => frame.push(value),
        }
    }

    frames
}
