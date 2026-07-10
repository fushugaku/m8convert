use super::*;

pub(super) fn parse_intel_hex(input: &[u8]) -> Result<HexImage, EmulatorError> {
    let text = std::str::from_utf8(input).map_err(|_| EmulatorError::Utf8)?;
    let mut records = Vec::new();
    let mut upper_address = 0u32;
    let mut start_linear_address = None;

    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with(':') {
            return Err(EmulatorError::MissingRecordStart { line: line_number });
        }

        let bytes = parse_record_bytes(&line[1..], line_number)?;
        if bytes.len() < 5 {
            return Err(EmulatorError::LengthMismatch { line: line_number });
        }

        let byte_count = bytes[0] as usize;
        let expected_len = 5 + byte_count;
        if bytes.len() != expected_len {
            return Err(EmulatorError::LengthMismatch { line: line_number });
        }
        let checksum = bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        if checksum != 0 {
            return Err(EmulatorError::Checksum { line: line_number });
        }

        let address = u16::from_be_bytes([bytes[1], bytes[2]]) as u32;
        let record_type = bytes[3];
        let data = &bytes[4..4 + byte_count];

        match record_type {
            0x00 => records.push(DataRecord {
                address: upper_address + address,
                bytes: data.to_vec(),
            }),
            0x01 => break,
            0x02 => {
                if data.len() != 2 {
                    return Err(EmulatorError::LengthMismatch { line: line_number });
                }
                upper_address = (u16::from_be_bytes([data[0], data[1]]) as u32) << 4;
            }
            0x04 => {
                if data.len() != 2 {
                    return Err(EmulatorError::LengthMismatch { line: line_number });
                }
                upper_address = (u16::from_be_bytes([data[0], data[1]]) as u32) << 16;
            }
            0x05 => {
                if data.len() != 4 {
                    return Err(EmulatorError::LengthMismatch { line: line_number });
                }
                start_linear_address =
                    Some(u32::from_be_bytes([data[0], data[1], data[2], data[3]]));
            }
            0x03 => {}
            _ => {
                return Err(EmulatorError::UnsupportedRecord {
                    line: line_number,
                    record_type,
                });
            }
        }
    }

    if records.is_empty() {
        return Err(EmulatorError::EmptyImage);
    }

    Ok(HexImage {
        records,
        start_linear_address,
    })
}

fn parse_record_bytes(record: &str, line: usize) -> Result<Vec<u8>, EmulatorError> {
    if !record.len().is_multiple_of(2) {
        return Err(EmulatorError::InvalidHex {
            line,
            offset: record.len(),
        });
    }

    let mut out = Vec::with_capacity(record.len() / 2);
    for offset in (0..record.len()).step_by(2) {
        let byte = u8::from_str_radix(&record[offset..offset + 2], 16)
            .map_err(|_| EmulatorError::InvalidHex { line, offset })?;
        out.push(byte);
    }
    Ok(out)
}

pub(super) fn normalize_for_teensy_runtime(
    image: &HexImage,
) -> Result<RuntimeImage, EmulatorError> {
    let (min_address, max_address) = address_span(&image.records)?;
    let should_apply_base = min_address < TEENSY_FLASH_BASE && max_address <= TEENSY_FLASH_SIZE;
    let base_applied = should_apply_base.then_some(TEENSY_FLASH_BASE);
    let records = image
        .records
        .iter()
        .map(|record| DataRecord {
            address: record.address + base_applied.unwrap_or(0),
            bytes: record.bytes.clone(),
        })
        .collect();

    Ok(RuntimeImage {
        records,
        base_applied,
    })
}

pub(super) fn address_span(records: &[DataRecord]) -> Result<(u32, u32), EmulatorError> {
    let mut min_address = u32::MAX;
    let mut max_address = 0u32;

    for record in records {
        min_address = min_address.min(record.address);
        max_address = max_address.max(record.address + record.bytes.len() as u32);
    }

    if min_address == u32::MAX {
        Err(EmulatorError::EmptyImage)
    } else {
        Ok((min_address, max_address))
    }
}

pub(super) fn merge_segments(records: &[DataRecord], tag_regions: bool) -> Vec<MemorySegment> {
    let mut sorted = records.to_vec();
    sorted.sort_by_key(|record| record.address);

    let mut segments: Vec<MemorySegment> = Vec::new();
    for record in sorted {
        let start = record.address;
        let end_exclusive = start + record.bytes.len() as u32;
        if let Some(last) = segments.last_mut()
            && start <= last.end_exclusive
        {
            last.end_exclusive = last.end_exclusive.max(end_exclusive);
            last.size = (last.end_exclusive - last.start) as usize;
            last.region = tag_regions.then(|| region_name(last.start)).flatten();
            continue;
        }

        segments.push(MemorySegment {
            start,
            end_exclusive,
            size: record.bytes.len(),
            region: tag_regions.then(|| region_name(start)).flatten(),
        });
    }

    segments
}

pub(super) fn probe_vector_table(
    image: &RuntimeImage,
    start_linear_address: Option<u32>,
) -> Option<VectorTableProbe> {
    let first_address = image.records.iter().map(|record| record.address).min()?;
    let mut candidates = Vec::new();
    if let Some(start) = start_linear_address {
        candidates.push(normalize_candidate_address(start, image.base_applied));
    }
    candidates.extend([
        first_address,
        TEENSY_FLASH_BASE,
        TEENSY_FLASH_BASE + 0x1000,
        TEENSY_FLASH_BASE + 0x2000,
        TEENSY_FLASH_BASE + 0x4000,
    ]);
    candidates.sort_unstable();
    candidates.dedup();

    candidates
        .into_iter()
        .find_map(|address| probe_vector_candidate(&image.records, address))
        .or_else(|| scan_vector_table(image))
}

fn normalize_candidate_address(address: u32, base_applied: Option<u32>) -> u32 {
    if let Some(base) = base_applied
        && address < TEENSY_FLASH_SIZE
    {
        return address + base;
    }
    address
}

fn scan_vector_table(image: &RuntimeImage) -> Option<VectorTableProbe> {
    let scan_end = TEENSY_FLASH_BASE + 0x20000;
    let mut records = image.records.clone();
    records.sort_by_key(|record| record.address);

    for record in &records {
        if record.address < TEENSY_FLASH_BASE
            || record.address >= scan_end
            || record.bytes.len() < 8
        {
            continue;
        }
        for offset in (0..=record.bytes.len().saturating_sub(8)).step_by(4) {
            let address = record.address + offset as u32;
            if address & 0x7f != 0 {
                continue;
            }
            if let Some(vector) = probe_vector_candidate(&records, address) {
                return Some(vector);
            }
        }
    }
    None
}

fn probe_vector_candidate(records: &[DataRecord], address: u32) -> Option<VectorTableProbe> {
    let initial_sp = read_u32(records, address)?;
    let reset_pc = read_u32(records, address + 4)?;
    let reset_pc_aligned = reset_pc & !1;
    let valid_thumb_entry = reset_pc & 1 == 1;
    let plausible_stack_pointer = matches!(
        stack_region_name(initial_sp),
        Some("DTCM") | Some("OCRAM") | Some("FlexRAM OCRAM")
    );
    let reset_pc_region = region_name(reset_pc_aligned);
    let reset_points_to_code = matches!(reset_pc_region, Some("FlexSPI flash") | Some("ITCM"));

    (valid_thumb_entry && plausible_stack_pointer && reset_points_to_code).then(|| {
        VectorTableProbe {
            address,
            initial_sp,
            reset_pc,
            reset_pc_aligned,
            initial_sp_region: stack_region_name(initial_sp),
            reset_pc_region,
            valid_thumb_entry,
            plausible_stack_pointer,
        }
    })
}

pub(super) fn probe_teensy_boot_image(
    image: &RuntimeImage,
    start_linear_address: Option<u32>,
) -> Option<TeensyBootImageProbe> {
    let first_address = image.records.iter().map(|record| record.address).min()?;
    let mut candidates = Vec::new();
    if let Some(start) = start_linear_address {
        candidates.push(normalize_candidate_address(start, image.base_applied));
    }
    candidates.extend([
        first_address + 0x1000,
        TEENSY_FLASH_BASE + 0x1000,
        TEENSY_FLASH_BASE + 0x2000,
        TEENSY_FLASH_BASE + 0x4000,
    ]);
    candidates.sort_unstable();
    candidates.dedup();

    candidates
        .into_iter()
        .find_map(|address| probe_teensy_boot_candidate(&image.records, address))
        .or_else(|| scan_teensy_boot_image(image))
}

fn scan_teensy_boot_image(image: &RuntimeImage) -> Option<TeensyBootImageProbe> {
    let scan_end = TEENSY_FLASH_BASE + 0x20000;
    let mut records = image.records.clone();
    records.sort_by_key(|record| record.address);

    for record in &records {
        if record.address < TEENSY_FLASH_BASE
            || record.address >= scan_end
            || record.bytes.len() < 32
        {
            continue;
        }
        for offset in (0..=record.bytes.len().saturating_sub(32)).step_by(4) {
            let address = record.address + offset as u32;
            if let Some(image) = probe_teensy_boot_candidate(&records, address) {
                return Some(image);
            }
        }
    }
    None
}

fn probe_teensy_boot_candidate(
    records: &[DataRecord],
    address: u32,
) -> Option<TeensyBootImageProbe> {
    let header = read_u32(records, address)?;
    let tag = header & 0xff;
    let length = (((header >> 8) & 0xff) << 8) | ((header >> 16) & 0xff);
    let version = (header >> 24) & 0xff;
    if tag != 0xd1 || length != 0x20 || !(0x40..=0x46).contains(&version) {
        return None;
    }

    let entry = read_u32(records, address + 4)?;
    let entry_aligned = entry & !1;
    let boot_data_address = read_u32(records, address + 0x10)?;
    let self_address = read_u32(records, address + 0x14)?;
    let csf_address = read_u32(records, address + 0x18)?;
    if self_address != address || region_name(entry_aligned) != Some("FlexSPI flash") {
        return None;
    }
    if region_name(boot_data_address) != Some("FlexSPI flash") {
        return None;
    }

    let image_start = read_u32(records, boot_data_address)?;
    let image_size = read_u32(records, boot_data_address + 4)?;
    let plugin_flags = read_u32(records, boot_data_address + 8)?;
    if region_name(image_start) != Some("FlexSPI flash") || image_size == 0 {
        return None;
    }

    Some(TeensyBootImageProbe {
        ivt_address: address,
        header,
        entry,
        entry_aligned,
        entry_region: region_name(entry_aligned),
        boot_data_address,
        self_address,
        csf_address,
        image_start,
        image_size,
        plugin_flags,
    })
}

pub(super) fn read_u16(records: &[DataRecord], address: u32) -> Option<u16> {
    Some(u16::from_le_bytes([
        read_u8(records, address)?,
        read_u8(records, address + 1)?,
    ]))
}

pub(super) fn read_u32(records: &[DataRecord], address: u32) -> Option<u32> {
    Some(u32::from_le_bytes([
        read_u8(records, address)?,
        read_u8(records, address + 1)?,
        read_u8(records, address + 2)?,
        read_u8(records, address + 3)?,
    ]))
}

pub(super) fn read_u8(records: &[DataRecord], address: u32) -> Option<u8> {
    records.iter().find_map(|record| {
        let offset = address.checked_sub(record.address)?;
        record.bytes.get(offset as usize).copied()
    })
}

pub(super) fn known_regions() -> Vec<KnownRegion> {
    vec![
        KnownRegion {
            name: "ITCM",
            start: 0x0000_0000,
            end_exclusive: 0x0008_0000,
        },
        KnownRegion {
            name: "DTCM",
            start: 0x2000_0000,
            end_exclusive: 0x2008_0000,
        },
        KnownRegion {
            name: "OCRAM",
            start: 0x2020_0000,
            end_exclusive: 0x2028_0000,
        },
        KnownRegion {
            name: "FlexRAM OCRAM",
            start: 0x2028_0000,
            end_exclusive: 0x2030_0000,
        },
        KnownRegion {
            name: "FlexSPI flash",
            start: TEENSY_FLASH_BASE,
            end_exclusive: TEENSY_FLASH_BASE + TEENSY_FLASH_SIZE,
        },
    ]
}

fn region_name(address: u32) -> Option<&'static str> {
    known_regions()
        .into_iter()
        .find(|region| address >= region.start && address < region.end_exclusive)
        .map(|region| region.name)
}

fn stack_region_name(address: u32) -> Option<&'static str> {
    known_regions()
        .into_iter()
        .find(|region| address > region.start && address <= region.end_exclusive)
        .map(|region| region.name)
        .or_else(|| region_name(address))
}
