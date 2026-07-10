use super::*;
use crate::converter::modfile::{Pattern, Sample};
use crate::converter::s3mfile::{S3mInstrument, S3mPattern};

#[test]
fn mod_flow_break_starts_next_pattern_at_requested_row() {
    let mut module = test_mod_module();
    module.orders = vec![0, 1];
    module.pattern_count = 2;
    module.patterns.push(empty_mod_pattern(4));
    module.patterns[0].rows[3][0] = Cell {
        effect: 0x0d,
        effect_param: 0x12,
        ..empty_mod_cell()
    };

    let mut report = M8Report::default();
    let blocks = build_mod_playback_blocks(&module, &mut report);
    assert_eq!(source_rows(&blocks[0].rows[..5]), vec![0, 1, 2, 3, 12]);
    assert_eq!(source_patterns(&blocks[0].rows[..5]), vec![0, 0, 0, 0, 1]);
}

#[test]
fn s3m_flow_delay_repeats_source_row() {
    let mut module = test_s3m_module();
    module.patterns[0].rows[2][0] = S3mCell {
        command: 19,
        info: 0xe2,
        ..S3mCell::EMPTY
    };

    let mut report = M8Report::default();
    let blocks = build_s3m_playback_blocks(&module, &mut report);
    assert_eq!(source_rows(&blocks[0].rows[..6]), vec![0, 1, 2, 2, 2, 3]);
    assert!(blocks[0].rows[2].emit_cells);
    assert!(!blocks[0].rows[3].emit_cells);
    assert!(!blocks[0].rows[4].emit_cells);
}

#[test]
fn s3m_early_pattern_break_packs_next_pattern_without_silent_gap() {
    let mut module = test_s3m_module();
    module.orders = vec![0, 1];
    module.patterns.push(S3mPattern {
        rows: vec![vec![S3mCell::EMPTY; 32]; 64],
    });
    module.patterns[0].rows[0][0] = S3mCell {
        note: 0x40,
        instrument: 1,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[15][0] = S3mCell {
        command: 3,
        info: 0,
        ..S3mCell::EMPTY
    };
    module.patterns[1].rows[0][0] = S3mCell {
        note: 0x42,
        instrument: 1,
        ..S3mCell::EMPTY
    };

    let song = export_s3m_song(&module);
    let chain_id = song.song.steps[0];
    assert_ne!(chain_id, EMPTY);
    let chain = &song.chains[chain_id as usize];

    let first_phrase = &song.phrases[chain.steps[0].phrase as usize];
    assert_ne!(first_phrase.steps[0].note.0, EMPTY);

    let second_phrase_id = chain.steps[1].phrase;
    assert_ne!(second_phrase_id, EMPTY);
    let second_phrase = &song.phrases[second_phrase_id as usize];
    assert_ne!(second_phrase.steps[0].note.0, EMPTY);
}

#[test]
fn s3m_global_volume_scales_velocity() {
    assert_eq!(volume_to_s3m_velocity(64, 64), 255);
    assert_eq!(volume_to_s3m_velocity(64, 32), 127);
}

#[test]
fn s3m_global_volume_slide_uses_memory() {
    let mut module = test_s3m_module();
    module.global_volume = 32;
    module.patterns[0].rows[0][0] = S3mCell {
        command: 23,
        info: 0x10,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[1][0] = S3mCell {
        command: 23,
        info: 0x00,
        ..S3mCell::EMPTY
    };

    let mut report = M8Report::default();
    let blocks = build_s3m_playback_blocks(&module, &mut report);
    let contexts = s3m_global_volume_contexts(&module, &blocks);
    assert_eq!(contexts[0][0].current, 37);
    assert_eq!(contexts[0][1].current, 42);
}

#[test]
fn s3m_global_volume_change_updates_an_already_playing_voice() {
    let mut module = test_s3m_module();
    module.patterns[0].rows[0][0] = S3mCell {
        note: 0x40,
        instrument: 1,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[1][0] = S3mCell {
        command: 22,
        info: 32,
        ..S3mCell::EMPTY
    };

    let song = export_s3m_song(&module);
    let chain = &song.chains[song.song.steps[0] as usize];
    let phrase = &song.phrases[chain.steps[0].phrase as usize];

    assert_eq!(phrase.steps[1].velocity, 127);
}

#[test]
fn s3m_channel_volume_commands_update_sustained_velocity_and_use_memory() {
    let mut module = test_s3m_module();
    module.patterns[0].rows[0][0] = S3mCell {
        note: 0x40,
        instrument: 1,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[1][0] = S3mCell {
        command: 13,
        info: 32,
        ..S3mCell::EMPTY
    };

    let song = export_s3m_song(&module);
    let chain = &song.chains[song.song.steps[0] as usize];
    let phrase = &song.phrases[chain.steps[0].phrase as usize];
    assert_eq!(phrase.steps[1].velocity, 127);

    let mut channel_volume = 32;
    let mut memory = 0;
    update_s3m_channel_volume(14, 0x10, 6, &mut channel_volume, &mut memory);
    assert_eq!(channel_volume, 37);
    update_s3m_channel_volume(14, 0, 6, &mut channel_volume, &mut memory);
    assert_eq!(channel_volume, 42);
}

#[test]
fn xm_ping_pong_metadata_selects_m8_ping_pong_playback() {
    let mut module = test_s3m_module();
    module.instruments[0].length = 4;
    module.instruments[0].data = vec![0; 4];
    module.instruments[0].loop_start = 1;
    module.instruments[0].loop_end = 3;
    module.instruments[0].flags |= 0x01;
    module.instruments[0].ping_pong_loop = true;

    let song = export_s3m_song(&module);
    let Instrument::Sampler(sampler) = &song.instruments[0] else {
        panic!("sampler instrument");
    };
    assert_eq!(sampler.play_mode, SamplePlayMode::FWD_PP);
}

#[test]
fn s3m_pan_slide_uses_effect_memory_and_tracker_ticks() {
    let mut step = empty_step();
    let mut memory = 0;
    let mut pan = 0x80;

    assert!(map_s3m_pan_slide(
        &mut step,
        0x01,
        6,
        &mut memory,
        &mut pan,
        None,
    ));
    assert_eq!(pan, 0x94);
    assert!(map_s3m_pan_slide(
        &mut step,
        0,
        6,
        &mut memory,
        &mut pan,
        None,
    ));
    assert_eq!(pan, 0xa8);
}

#[test]
fn timing_contexts_ignore_orders_skipped_by_playback_flow() {
    let mut module = test_mod_module();
    module.orders = vec![0, 1, 2];
    module.pattern_count = 3;
    module.patterns = vec![empty_mod_pattern(4); 3];
    module.patterns[0].rows[0][0] = Cell {
        effect: 0x0b,
        effect_param: 2,
        ..empty_mod_cell()
    };
    module.patterns[1].rows[0][0] = Cell {
        effect: 0x0f,
        effect_param: 64,
        ..empty_mod_cell()
    };
    module.patterns[2].rows[0][0] = Cell {
        effect: 0x0f,
        effect_param: 6,
        ..empty_mod_cell()
    };

    let mut report = M8Report::default();
    let blocks = build_mod_playback_blocks(&module, &mut report);
    let contexts = mod_timing_contexts(&module, &blocks);
    let (block_index, row_index) = blocks
        .iter()
        .enumerate()
        .find_map(|(block_index, block)| {
            block
                .rows
                .iter()
                .position(|row| row.pattern_id == 2)
                .map(|row_index| (block_index, row_index))
        })
        .expect("jump target is present");

    assert_eq!(contexts[block_index][row_index][0].tpo, Some(125));
}

#[test]
fn mod_f00_stops_playback_flow() {
    let mut module = test_mod_module();
    module.orders = vec![0, 1];
    module.pattern_count = 2;
    module.patterns = vec![empty_mod_pattern(4); 2];
    module.patterns[0].rows[0][0] = Cell {
        effect: 0x0f,
        effect_param: 0,
        ..empty_mod_cell()
    };

    let mut report = M8Report::default();
    let blocks = build_mod_playback_blocks(&module, &mut report);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].rows.len(), 1);
}

#[test]
fn pattern_delay_rows_continue_mod_effects_without_retriggering_notes() {
    let cell = Cell {
        period: 428,
        sample_number: 1,
        effect: 0x01,
        effect_param: 3,
    };
    let continued = continued_mod_cell(cell);
    assert_eq!(continued.period, 0);
    assert_eq!(continued.sample_number, 0);
    assert_eq!((continued.effect, continued.effect_param), (0x01, 3));
}

#[test]
fn global_volume_contexts_ignore_orders_skipped_by_playback_flow() {
    let mut module = test_s3m_module();
    module.global_volume = 32;
    module.orders = vec![0, 1, 2];
    module.patterns = vec![module.patterns[0].clone(); 3];
    module.patterns[0].rows[0][0] = S3mCell {
        command: 2,
        info: 2,
        ..S3mCell::EMPTY
    };
    module.patterns[1].rows[0][0] = S3mCell {
        command: 22,
        info: 8,
        ..S3mCell::EMPTY
    };

    let mut report = M8Report::default();
    let blocks = build_s3m_playback_blocks(&module, &mut report);
    let contexts = s3m_global_volume_contexts(&module, &blocks);
    let (block_index, row_index) = blocks
        .iter()
        .enumerate()
        .find_map(|(block_index, block)| {
            block
                .rows
                .iter()
                .position(|row| row.pattern_id == 2)
                .map(|row_index| (block_index, row_index))
        })
        .expect("jump target is present");

    assert_eq!(contexts[block_index][row_index].current, 32);
}

#[test]
fn pattern_delay_continues_normal_s3m_global_volume_slide() {
    let mut module = test_s3m_module();
    module.global_volume = 32;
    module.patterns[0].rows[0][0] = S3mCell {
        command: 23,
        info: 0x10,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[0][1] = S3mCell {
        command: 19,
        info: 0xe1,
        ..S3mCell::EMPTY
    };

    let mut report = M8Report::default();
    let blocks = build_s3m_playback_blocks(&module, &mut report);
    let contexts = s3m_global_volume_contexts(&module, &blocks);

    assert_eq!(
        blocks.iter().map(|block| block.rows.len()).sum::<usize>(),
        65
    );
    assert_eq!(contexts[0][0].current, 37);
    assert_eq!(contexts[0][1].current, 42);
}

#[test]
fn s3m_pitch_slide_uses_m8_pitch_bend_scale() {
    assert_eq!(s3m_pitch_bend_amount(0x10), Some(0x40));
    assert_eq!(s3m_pitch_bend_amount(0xf2), Some(0x08));
    assert_eq!(s3m_pitch_bend_amount(0xe2), Some(0x02));
    assert_eq!(pitch_bend_value(0x08, true), 0x08);
    assert_eq!(pitch_bend_value(0x08, false), 0xf8);
}

#[test]
fn s3m_periods_follow_tracker_octaves() {
    assert_eq!(s3m_period_for_m8_note(0), 1712);
    assert_eq!(s3m_period_for_m8_note(12), 856);
    assert!(s3m_period_for_m8_note(37) < s3m_period_for_m8_note(36));
}

#[test]
fn s3m_table_tick_stretches_fast_tracker_speeds() {
    assert_eq!(s3m_table_tick(3), 2);
    assert_eq!(s3m_table_tick(6), 1);
}

#[test]
fn mod_f20_sets_tempo_instead_of_speed() {
    let mut module = test_mod_module();
    module.patterns[0].rows[0][0] = Cell {
        effect: 0x0f,
        effect_param: 0x20,
        ..empty_mod_cell()
    };

    assert!((mod_m8_tempo(&module) - 32.0).abs() < f32::EPSILON);
}

#[test]
fn mod_notes_keep_classic_lr_rl_channel_panning() {
    let mut module = test_mod_module();
    module.samples[0] = Sample {
        name: "tone".to_string(),
        length_bytes: 1,
        data: vec![0],
        ..empty_sample()
    };
    for channel in 0..4 {
        module.patterns[0].rows[0][channel] = Cell {
            period: 428,
            sample_number: 1,
            ..empty_mod_cell()
        };
    }

    let song = export_mod_song(&module);
    let expected = [0x00, 0xff, 0xff, 0x00];
    for (channel, expected_pan) in expected.into_iter().enumerate() {
        let chain_id = song.song.steps[channel];
        let chain = &song.chains[chain_id as usize];
        let phrase = &song.phrases[chain.steps[0].phrase as usize];
        assert!(
            [
                phrase.steps[0].fx1,
                phrase.steps[0].fx2,
                phrase.steps[0].fx3
            ]
            .iter()
            .any(|fx| fx.command == FX_SAMPLER_PAN && fx.value == expected_pan),
            "channel {channel} is missing pan {expected_pan:02x}"
        );
    }
}

#[test]
fn mod_tone_portamento_uses_period_distance() {
    let mut step = empty_step();
    let mut state = TonePortamentoState {
        current_note: Some(15),
        target_note: Some(12),
        current_period: Some(360),
        target_period: Some(428),
        speed: 0,
    };

    assert!(map_tone_portamento(&mut step, 0x0f, 3, &mut state));
    assert_eq!(step.fx1.command, FX_PSL);
    assert_eq!(step.fx1.value, 5);
    assert_eq!(state.current_period, Some(428));
}

#[test]
fn s3m_volume_slide_sets_table_tick_before_table() {
    let mut reader = Reader::new(TEMPLATE.to_vec());
    let song = Song::read_from_reader(&mut reader).expect("template");
    let mut tables = song.tables;
    let mut allocator = TableAllocator::new();
    let mut step = empty_step();
    let mut current_velocity = 0x80;
    let mut memory = 0;
    let mut used_table_effect = false;

    assert!(map_volume_slide(
        &mut step,
        &mut allocator,
        &mut tables,
        0x80,
        0x01,
        3,
        &mut current_velocity,
        &mut memory,
        Some(s3m_table_tick(3)),
        &mut used_table_effect,
    ));

    assert_eq!(step.fx1.command, FX_TIC);
    assert_eq!(step.fx1.value, 2);
    assert_eq!(step.fx2.command, FX_TBL);
    assert!(used_table_effect);
}

#[test]
fn mod_flow_loop_is_exported_as_song_hop() {
    let mut module = test_mod_module();
    module.patterns[0].rows[0][0] = Cell {
        effect: 0x0b,
        effect_param: 0,
        ..empty_mod_cell()
    };

    let mut report = M8Report::default();
    let blocks = build_mod_playback_blocks(&module, &mut report);
    assert_eq!(blocks.last().and_then(|block| block.song_hop), Some(0));
    assert!(report.warnings[0].contains("SNG hop"));
}

#[test]
fn mod_restart_position_is_exported_as_song_hop() {
    let mut module = test_mod_module();
    module.orders = vec![0, 1, 2];
    module.pattern_count = 3;
    module.restart_position = 1;
    module.patterns.push(empty_mod_pattern(4));
    module.patterns.push(empty_mod_pattern(4));

    let mut report = M8Report::default();
    let blocks = build_mod_playback_blocks(&module, &mut report);
    assert_eq!(blocks.last().and_then(|block| block.song_hop), Some(1));
}

#[test]
fn maps_sample_loop_end_and_mod_finetune() {
    assert_eq!(scaled_sample_position(25, 100), 64);
    assert_eq!(scaled_sample_position(75, 100), 191);
    assert_eq!(scaled_sample_position(100, 100), 0xff);
    assert_eq!(mod_finetune_nibble_to_signed(0x0f), -1);
    assert_eq!(mod_finetune_to_m8(-8), 0x00);
    assert_eq!(mod_finetune_to_m8(0), 0x80);
    assert_eq!(mod_finetune_to_m8(7), 0xf0);
}

#[test]
fn maps_s3m_volume_column_pan() {
    assert_eq!(s3m_volume_column_pan(0x80), Some(0x00));
    assert_eq!(s3m_volume_column_pan(0xa0), Some(0x80));
    assert_eq!(s3m_volume_column_pan(0xc0), Some(0xff));
    assert_eq!(s3m_volume_column_pan(0x40), None);
}

#[test]
fn maps_s3m_volume_column_effect_ranges() {
    assert_eq!(
        s3m_volume_column_command(65),
        S3mVolumeColumnCommand::FineVolumeUp(0)
    );
    assert_eq!(
        s3m_volume_column_command(84),
        S3mVolumeColumnCommand::FineVolumeDown(9)
    );
    assert_eq!(
        s3m_volume_column_command(105),
        S3mVolumeColumnCommand::PortamentoDown(0)
    );
    assert_eq!(
        s3m_volume_column_command(124),
        S3mVolumeColumnCommand::PortamentoUp(9)
    );
    assert_eq!(
        s3m_volume_column_command(193),
        S3mVolumeColumnCommand::TonePortamento(0)
    );
    assert_eq!(s3m_volume_column_tone_portamento(9), 0xff);
}

#[test]
fn mod_retrigger_uses_m8_interval_nibble() {
    let mut step = empty_step();
    let mut memory = RetriggerMemory::default();

    assert!(map_mod_retrigger(&mut step, 3, &mut memory));
    assert_eq!(step.fx1.command, FX_RET);
    assert_eq!(step.fx1.value, 0x83);
}

#[test]
fn s3m_retrigger_maps_volume_change_to_m8_ret() {
    let mut step = empty_step();
    let mut memory = RetriggerMemory::default();
    let mut report = M8Report::default();

    assert!(map_s3m_retrigger(
        &mut step,
        0xa4,
        &mut memory,
        EffectLocation {
            order: 0,
            pattern: 0,
            row: 0,
            channel: 0,
        },
        &mut report,
    ));

    assert_eq!(step.fx1.command, FX_RET);
    assert_eq!(step.fx1.value, 0xa4);
    assert_eq!(s3m_retrigger_volume_to_m8(0x00), 0x08);
    assert_eq!(s3m_retrigger_volume_to_m8(0x03), 0x04);
}

#[test]
fn sample_offset_scales_to_sampler_start() {
    let mut module = test_mod_module();
    module.samples[0].length_bytes = 1024;
    module.samples[0].data = vec![0; 1024];
    module.patterns[0].rows[0][0] = Cell {
        period: 428,
        sample_number: 1,
        effect: 0x09,
        effect_param: 0x02,
    };

    let song = export_mod_song(&module);
    let step = &song.phrases[0].steps[0];
    assert!(
        [&step.fx1, &step.fx2, &step.fx3]
            .iter()
            .any(|fx| fx.command == FX_SAMPLER_STA && fx.value == 128)
    );
}

#[test]
fn waveform_control_is_reported_as_approximation() {
    let mut module = test_mod_module();
    module.patterns[0].rows[0][0] = Cell {
        effect: 0x0e,
        effect_param: 0x41,
        ..empty_mod_cell()
    };

    let export = export_editable_m8(&module, &ConversionOptions::default())
        .expect("mod conversion succeeds");
    assert!(
        export
            .report
            .approximations
            .iter()
            .any(|approx| approx.category == "waveform")
    );
}

#[test]
fn mod_note_cut_maps_to_kil() {
    let mut module = test_mod_module();
    module.patterns[0].rows[0][0] = Cell {
        effect: 0x0e,
        effect_param: 0xc3,
        ..empty_mod_cell()
    };

    let song = export_mod_song(&module);
    let step = &song.phrases[0].steps[0];
    assert!(
        [&step.fx1, &step.fx2, &step.fx3]
            .iter()
            .any(|fx| fx.command == FX_KIL && fx.value == 3)
    );
}

#[test]
fn s3m_note_cut_exports_note_off() {
    let mut module = test_s3m_module();
    module.patterns[0].rows[0][0] = S3mCell {
        note: 0x40,
        instrument: 1,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[1][0] = S3mCell {
        note: 0xfe,
        ..S3mCell::EMPTY
    };

    let song = export_s3m_song(&module);
    let step = &song.phrases[0].steps[1];
    assert_eq!(step.note.0, NOTE_OFF);
    assert_eq!(step.instrument, EMPTY);
    assert_eq!(step.velocity, EMPTY);
}

#[test]
fn s3m_note_cut_still_maps_row_effects() {
    let mut module = test_s3m_module();
    module.patterns[0].rows[0][0] = S3mCell {
        note: 0x40,
        instrument: 1,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[1][0] = S3mCell {
        note: 0xfe,
        command: 19,
        info: 0xc3,
        ..S3mCell::EMPTY
    };

    let song = export_s3m_song(&module);
    let step = &song.phrases[0].steps[1];
    assert_eq!(step.note.0, NOTE_OFF);
    assert!(
        [&step.fx1, &step.fx2, &step.fx3]
            .iter()
            .any(|fx| fx.command == FX_KIL && fx.value == 3)
    );
}

#[test]
fn s3m_static_channel_pan_uses_instrument_pan() {
    let mut module = test_s3m_module();
    module.channel_pans[0] = 0x33;
    module.patterns[0].rows[0][0] = S3mCell {
        note: 0x40,
        instrument: 1,
        ..S3mCell::EMPTY
    };

    let song = export_s3m_song(&module);
    let Instrument::Sampler(sampler) = &song.instruments[0] else {
        panic!("expected sampler");
    };
    assert_eq!(sampler.synth_params.mixer_pan, 0x33);

    let step = &song.phrases[0].steps[0];
    assert_eq!(step.instrument, 0);
    assert!(
        [&step.fx1, &step.fx2, &step.fx3]
            .iter()
            .all(|fx| fx.command != FX_SAMPLER_PAN)
    );
}

#[test]
fn s3m_redundant_pan_command_uses_instrument_pan() {
    let mut module = test_s3m_module();
    module.patterns[0].rows[0][0] = S3mCell {
        command: 24,
        info: 0x33,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[1][0] = S3mCell {
        note: 0x40,
        instrument: 1,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[2][0] = S3mCell {
        note: 0x42,
        ..S3mCell::EMPTY
    };

    let song = export_s3m_song(&module);
    let Instrument::Sampler(sampler) = &song.instruments[0] else {
        panic!("expected sampler");
    };
    assert_eq!(sampler.synth_params.mixer_pan, 0x66);

    for row in 0..3 {
        let step = &song.phrases[0].steps[row];
        assert!(
            [&step.fx1, &step.fx2, &step.fx3]
                .iter()
                .all(|fx| fx.command != FX_SAMPLER_PAN)
        );
    }
}

#[test]
fn s3m_dynamic_channel_pan_keeps_phrase_pan() {
    let mut module = test_s3m_module();
    module.channel_pans[0] = 0x33;
    module.patterns[0].rows[0][0] = S3mCell {
        note: 0x40,
        instrument: 1,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[1][0] = S3mCell {
        command: 24,
        info: 0x80,
        ..S3mCell::EMPTY
    };
    module.patterns[0].rows[2][0] = S3mCell {
        note: 0x42,
        ..S3mCell::EMPTY
    };

    let song = export_s3m_song(&module);
    let Instrument::Sampler(sampler) = &song.instruments[0] else {
        panic!("expected sampler");
    };
    assert_eq!(sampler.synth_params.mixer_pan, 0x80);

    let step = &song.phrases[0].steps[0];
    assert!(
        [&step.fx1, &step.fx2, &step.fx3]
            .iter()
            .any(|fx| fx.command == FX_SAMPLER_PAN && fx.value == 0x33)
    );
}

#[test]
fn tremor_table_alternates_velocity_and_silence() {
    let mut reader = Reader::new(TEMPLATE.to_vec());
    let song = Song::read_from_reader(&mut reader).expect("template");
    let mut table = song.tables[0].clone();
    table.clear();

    build_table(
        &mut table,
        TableSpec::Tremor {
            velocity: 200,
            on: 2,
            off: 1,
            rows: 5,
        },
    );

    let velocities = table
        .steps
        .iter()
        .take(5)
        .map(|step| step.velocity)
        .collect::<Vec<_>>();
    assert_eq!(velocities, vec![200, 200, 0, 200, 200]);
}

#[test]
fn tremolo_waveforms_change_table_shape() {
    assert_eq!(tremolo_waveform_value(0, 0), -8);
    assert_eq!(tremolo_waveform_value(0, 4), 0);
    assert_eq!(tremolo_waveform_value(2, 0), 8);
    assert_eq!(tremolo_waveform_value(2, 8), -8);
}

#[test]
fn panbrello_table_contains_sampler_pan_values() {
    let mut reader = Reader::new(TEMPLATE.to_vec());
    let song = Song::read_from_reader(&mut reader).expect("template");
    let mut table = song.tables[0].clone();
    table.clear();

    build_table(
        &mut table,
        TableSpec::Panbrello {
            center: 0x80,
            depth: 4,
            speed: 1,
            waveform: 0,
            phase: 0,
            rows: 2,
        },
    );

    assert_eq!(table.steps[0].fx1.command, FX_SAMPLER_PAN);
    assert_eq!(table.steps[0].fx1.value, 0x60);
    assert_eq!(table.steps[1].fx1.value, 0x68);
}

#[test]
fn hvl_filter_override_maps_to_wavsynth_cutoff_not_vibrato() {
    let mut module = test_hvl_module(1);
    module.tracks[0].rows[0] = HvlStep {
        note: 25,
        instrument: 1,
        fx: 0x04,
        fx_param: 0x20,
        ..empty_hvl_step()
    };

    let song = export_hvl_song(&module);
    let chain = &song.chains[song.song.steps[0] as usize];
    let phrase = &song.phrases[chain.steps[0].phrase as usize];
    let effects = [
        phrase.steps[0].fx1,
        phrase.steps[0].fx2,
        phrase.steps[0].fx3,
    ];
    assert!(
        effects
            .iter()
            .any(|effect| effect.command == FX_WAVSYNTH_CUT)
    );
    assert!(effects.iter().all(|effect| effect.command != FX_PVB));
}

#[test]
fn hvl_volume_effect_overrides_instrument_volume_on_the_same_note() {
    let mut module = test_hvl_module(1);
    module.tracks[0].rows[0] = HvlStep {
        note: 25,
        instrument: 1,
        fx: 0x0c,
        fx_param: 32,
        ..empty_hvl_step()
    };

    let song = export_hvl_song(&module);
    let chain = &song.chains[song.song.steps[0] as usize];
    let phrase = &song.phrases[chain.steps[0].phrase as usize];
    assert_eq!(phrase.steps[0].velocity, 127);
}

#[test]
fn hvl_playback_flow_skips_jumped_positions() {
    let mut module = test_hvl_module(3);
    module.tracks[0].rows[0] = HvlStep {
        fx: 0x0b,
        fx_param: 0x02,
        ..empty_hvl_step()
    };
    let mut report = M8Report::default();
    let blocks = build_hvl_playback_blocks(&module, &mut report);
    let positions = blocks
        .iter()
        .filter_map(|block| block.rows.first().map(|row| row.order_index))
        .collect::<Vec<_>>();

    assert_eq!(positions, vec![0, 2]);
}

#[test]
fn table_allocator_uses_table_255_once_and_then_reports_exhaustion() {
    let mut reader = Reader::new(TEMPLATE.to_vec());
    let mut song = Song::read_from_reader(&mut reader).expect("template");
    let mut allocator = TableAllocator::new();

    for start in 0..=u8::MAX {
        let id = allocator.allocate(
            &mut song.tables,
            TableSpec::VolumeSlide {
                start,
                delta: 1,
                rows: 1,
            },
        );
        assert_eq!(id, Some(start));
    }

    assert_eq!(allocator.next_table, 256);
    assert_eq!(allocator.empty_table(&mut song.tables), None);
    assert_eq!(allocator.dropped, 1);
}

fn test_mod_module() -> Module {
    Module {
        title: "test".to_string(),
        channel_count: 4,
        restart_position: 0,
        orders: vec![0],
        pattern_count: 1,
        samples: vec![empty_sample(); 31],
        patterns: vec![empty_mod_pattern(4)],
        signature: Some("M.K.".to_string()),
    }
}

fn source_rows(rows: &[PlaybackRow]) -> Vec<usize> {
    rows.iter().map(|row| row.source_row).collect()
}

fn source_patterns(rows: &[PlaybackRow]) -> Vec<u8> {
    rows.iter().map(|row| row.pattern_id).collect()
}

fn empty_mod_pattern(channels: usize) -> Pattern {
    Pattern {
        rows: vec![vec![empty_mod_cell(); channels]; 64],
    }
}

fn empty_mod_cell() -> Cell {
    Cell {
        period: 0,
        sample_number: 0,
        effect: 0,
        effect_param: 0,
    }
}

fn empty_sample() -> Sample {
    Sample {
        name: String::new(),
        length_bytes: 0,
        finetune: 0,
        volume: 64,
        loop_start_bytes: 0,
        loop_length_bytes: 0,
        data: Vec::new(),
    }
}

fn export_s3m_song(module: &S3mModule) -> Song {
    let export = export_s3m_editable_m8(module, &ConversionOptions::default())
        .expect("s3m conversion succeeds");
    let mut reader = Reader::new(export.song_bytes);
    Song::read_from_reader(&mut reader).expect("read exported song")
}

fn export_mod_song(module: &Module) -> Song {
    let export =
        export_editable_m8(module, &ConversionOptions::default()).expect("mod conversion succeeds");
    let mut reader = Reader::new(export.song_bytes);
    Song::read_from_reader(&mut reader).expect("read exported song")
}

fn export_hvl_song(module: &HvlModule) -> Song {
    let export = export_hvl_editable_m8(module, &ConversionOptions::default())
        .expect("hvl conversion succeeds");
    let mut reader = Reader::new(export.song_bytes);
    Song::read_from_reader(&mut reader).expect("read exported song")
}

fn test_hvl_module(position_count: usize) -> HvlModule {
    use crate::converter::hvlfile::{HvlEnvelope, HvlInstrument, HvlPosition, HvlTrack};

    HvlModule {
        title: "test".to_string(),
        version: 1,
        position_count,
        channel_count: 4,
        restart_position: 0,
        speed_multiplier: 1,
        track_length: 1,
        track_count: position_count.saturating_sub(1),
        instrument_count: 1,
        subsong_count: 0,
        positions: (0..position_count)
            .map(|index| HvlPosition {
                tracks: vec![index as u8; 4],
                transposes: vec![0; 4],
            })
            .collect(),
        tracks: vec![
            HvlTrack {
                rows: vec![empty_hvl_step()],
            };
            position_count
        ],
        instruments: vec![HvlInstrument {
            name: "wave".to_string(),
            volume: 64,
            wave_length: 3,
            filter_speed: 0,
            envelope: HvlEnvelope {
                attack_frames: 0,
                attack_volume: 64,
                decay_frames: 0,
                decay_volume: 64,
                sustain_frames: 0,
                release_frames: 0,
                release_volume: 0,
            },
            vibrato_delay: 0,
            vibrato_depth: 0,
            vibrato_speed: 0,
            square_lower_limit: 0,
            square_upper_limit: 0,
            square_speed: 0,
            filter_lower_limit: 0,
            filter_upper_limit: 0,
            plist_speed: 1,
            plist: Vec::new(),
        }],
    }
}

fn test_s3m_module() -> S3mModule {
    S3mModule {
        title: "test".to_string(),
        orders: vec![0],
        restart_position: None,
        active_channels: vec![0],
        channel_pans: vec![0x80; 32],
        instruments: vec![S3mInstrument {
            kind: 1,
            name: "sample".to_string(),
            length: 0,
            loop_start: 0,
            loop_end: 0,
            volume: 64,
            flags: 0,
            c5_speed: 8363,
            pack: 0,
            default_pan: None,
            amp_envelope: None,
            auto_vibrato: None,
            data: vec![0],
            right_data: None,
            ping_pong_loop: false,
        }],
        patterns: vec![S3mPattern {
            rows: vec![vec![S3mCell::EMPTY; 32]; 64],
        }],
        initial_speed: 6,
        initial_tempo: 125,
        global_volume: 64,
        tracker_version: 0,
        ffi: 1,
        pan_command_uses_8bit: false,
    }
}
