use std::fs;
use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{Parser, Subcommand};
use m8convert::{
    ConversionOptions, convert_tracker,
    converter::events::build_event_regression_report,
    emulator::{TeensyEmulatorSession, probe_teensy_hex_boot_with_sd},
};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Convert MOD/HVL/S3M/XM tracker files into an editable Dirtywave M8 bundle"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    input: Option<PathBuf>,

    #[arg(short, long, default_value = "m8convert-out")]
    out: PathBuf,

    #[arg(long)]
    song_name: Option<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Convert(ConvertArgs),
    Analyze(AnalyzeArgs),
    ProbeFirmware(ProbeFirmwareArgs),
    #[command(hide = true)]
    SearchFirmwareInput(SearchFirmwareInputArgs),
    #[command(hide = true)]
    DumpFirmwareMemory(DumpFirmwareMemoryArgs),
    #[command(hide = true)]
    ProbeFirmwareInputBase(ProbeFirmwareInputBaseArgs),
    #[command(hide = true)]
    ProbeFirmwareHostInput(ProbeFirmwareHostInputArgs),
}

#[derive(Debug, Parser)]
struct ConvertArgs {
    input: PathBuf,

    #[arg(short, long, default_value = "m8convert-out")]
    out: PathBuf,

    #[arg(long)]
    song_name: Option<String>,
}

#[derive(Debug, Parser)]
struct AnalyzeArgs {
    input: PathBuf,

    #[arg(long)]
    out_json: Option<PathBuf>,

    #[arg(long)]
    song_name: Option<String>,
}

#[derive(Debug, Parser)]
struct ProbeFirmwareArgs {
    input: PathBuf,

    #[arg(long, default_value_t = 25_000_000)]
    steps: u32,

    #[arg(long)]
    out_json: Option<PathBuf>,

    #[arg(long)]
    sd_image: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct SearchFirmwareInputArgs {
    input: PathBuf,

    #[arg(long, default_value_t = 15_000_000)]
    boot_steps: u32,

    #[arg(long, default_value_t = 50_000)]
    run_steps: u32,

    #[arg(long)]
    start: String,

    #[arg(long)]
    end: String,

    #[arg(long, default_value = "0x20")]
    value: String,

    #[arg(long, default_value_t = 64)]
    limit: usize,
}

#[derive(Debug, Parser)]
struct DumpFirmwareMemoryArgs {
    input: PathBuf,

    #[arg(long)]
    sd_image: Option<PathBuf>,

    #[arg(long, default_value_t = 15_000_000)]
    boot_steps: u32,

    #[arg(long)]
    start: String,

    #[arg(long)]
    end: String,

    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Parser)]
struct ProbeFirmwareInputBaseArgs {
    input: PathBuf,

    #[arg(long, default_value_t = 15_000_000)]
    boot_steps: u32,

    #[arg(long, default_value_t = 500_000)]
    run_steps: u32,

    #[arg(long)]
    bases: String,

    #[arg(long, default_value = "0x20")]
    value: String,
}

#[derive(Debug, Parser)]
struct ProbeFirmwareHostInputArgs {
    input: PathBuf,

    #[arg(long)]
    sd_image: Option<PathBuf>,

    #[arg(long, default_value_t = 15_000_000)]
    boot_steps: u32,

    #[arg(long, default_value_t = 500_000)]
    run_steps: u32,

    #[arg(long, default_value = "43:20,43:00")]
    packets: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    match args.command {
        Some(Command::Convert(args)) => convert_command(args.input, args.out, args.song_name),
        Some(Command::Analyze(args)) => analyze_command(args.input, args.out_json, args.song_name),
        Some(Command::ProbeFirmware(args)) => {
            probe_firmware_command(args.input, args.steps, args.out_json, args.sd_image)
        }
        Some(Command::SearchFirmwareInput(args)) => search_firmware_input_command(args),
        Some(Command::DumpFirmwareMemory(args)) => dump_firmware_memory_command(args),
        Some(Command::ProbeFirmwareInputBase(args)) => probe_firmware_input_base_command(args),
        Some(Command::ProbeFirmwareHostInput(args)) => probe_firmware_host_input_command(args),
        None => {
            let Some(input) = args.input else {
                return Err("missing input path".into());
            };
            convert_command(input, args.out, args.song_name)
        }
    }
}

fn probe_firmware_host_input_command(
    args: ProbeFirmwareHostInputArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = fs::read(&args.input)?;
    let mut session = TeensyEmulatorSession::new(&input)?;
    if let Some(sd_image) = &args.sd_image {
        session.load_sd_image(&fs::read(sd_image)?);
    }
    session.receive_host_bytes(&[0x45]);
    session.run_steps_live(args.boot_steps);
    let mut previous_rows = preferred_display_rows(&session);
    println!(
        "after boot: status={} pc=0x{:08x} host={}",
        session.status(),
        session.snapshot().cpu.registers[15],
        serde_json::to_string(&session.host_input_stats())?
    );

    for packet in parse_packet_list(&args.packets)? {
        println!("send packet: {:02x?}", packet);
        session.receive_host_bytes(&packet);
        let executed = session.run_steps_live(args.run_steps);
        println!(
            "after packet: executed={} status={} pc=0x{:08x} host={}",
            executed,
            session.status(),
            session.snapshot().cpu.registers[15],
            serde_json::to_string(&session.host_input_stats())?
        );
        let rows = preferred_display_rows(&session);
        if rows != previous_rows {
            println!("display changed:");
            for (row, (before, after)) in previous_rows.iter().zip(rows.iter()).enumerate() {
                if before != after {
                    println!("  row {row:02}: {before:?} -> {after:?}");
                }
            }
        } else {
            println!("display unchanged");
        }
        previous_rows = rows;
        let snapshot = session.snapshot();
        for event in snapshot
            .trace
            .iter()
            .rev()
            .take(80)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if event.note.contains("unmapped")
                || (0x0003_6000..=0x0003_6330).contains(&event.pc)
                || (0x6004_af20..=0x6004_b290).contains(&event.pc)
                || event.note.contains("M8 input")
            {
                println!(
                    "trace {:>12} pc=0x{:08x} {}",
                    event.cycle, event.pc, event.note
                );
            }
        }
        if session.is_stopped() {
            println!("trace tail before stop:");
            for event in snapshot
                .trace
                .iter()
                .rev()
                .take(80)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                println!(
                    "trace {:>12} pc=0x{:08x} {}",
                    event.cycle, event.pc, event.note
                );
            }
            break;
        }
    }
    Ok(())
}

fn probe_firmware_input_base_command(
    args: ProbeFirmwareInputBaseArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = fs::read(&args.input)?;
    let value = parse_u32_arg(&args.value)? as u8;
    let mut baseline = TeensyEmulatorSession::new(&input)?;
    baseline.run_steps_live(args.boot_steps);

    let mut control = baseline.clone();
    control.run_steps_live(args.run_steps);
    let control_rows = preferred_display_rows(&control);

    for base_arg in args
        .bases
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let base = parse_u32_arg(base_arg)?;
        for mode in ["handler", "parser"] {
            let mut candidate = baseline.clone();
            let returned = if mode == "handler" {
                candidate.debug_call_m8_controller_handler_at(base, value)
            } else {
                candidate.debug_call_m8_event_parser_at(base, 0, 0x43, u32::from(value))
            };
            candidate.run_steps_live(args.run_steps);
            let rows = preferred_display_rows(&candidate);
            let changed = rows != control_rows;
            println!(
                "{mode} base=0x{base:08x} returned={returned} changed={changed} pc=0x{:08x}",
                candidate.snapshot().cpu.registers[15]
            );
            for (row, (before, after)) in control_rows.iter().zip(rows.iter()).enumerate() {
                if before != after {
                    println!("  row {row:02}: {before:?} -> {after:?}");
                }
            }
        }
    }
    Ok(())
}

fn dump_firmware_memory_command(
    args: DumpFirmwareMemoryArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = fs::read(&args.input)?;
    let start = parse_u32_arg(&args.start)?;
    let end = parse_u32_arg(&args.end)?;
    let mut session = TeensyEmulatorSession::new(&input)?;
    if let Some(sd_image) = &args.sd_image {
        session.load_sd_image(&fs::read(sd_image)?);
    }
    session.run_steps_live(args.boot_steps);

    let mut bytes = Vec::with_capacity(end.saturating_sub(start) as usize);
    for address in start..end {
        bytes.push(session.debug_read_u8(address).unwrap_or(0));
    }
    ensure_parent(&args.out)?;
    fs::write(&args.out, bytes)?;
    println!(
        "Dumped 0x{:08x}..0x{:08x} after {} steps to {}",
        start,
        end,
        args.boot_steps,
        args.out.display()
    );
    Ok(())
}

fn search_firmware_input_command(
    args: SearchFirmwareInputArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = fs::read(&args.input)?;
    let start = parse_u32_arg(&args.start)?;
    let end = parse_u32_arg(&args.end)?;
    let value = parse_u32_arg(&args.value)? as u8;
    let mut baseline = TeensyEmulatorSession::new(&input)?;
    baseline.run_steps_live(args.boot_steps);

    let mut control = baseline.clone();
    control.run_steps_live(args.run_steps);
    let control_rows = preferred_display_rows(&control);

    let mut found = 0usize;
    for address in start..end {
        let mut candidate = baseline.clone();
        candidate.debug_poke_u8(address, value);
        candidate.run_steps_live(args.run_steps);
        let rows = preferred_display_rows(&candidate);
        if rows != control_rows {
            println!(
                "changed address=0x{address:08x} value=0x{value:02x} pc=0x{:08x}",
                candidate.snapshot().cpu.registers[15]
            );
            for (row, (before, after)) in control_rows.iter().zip(rows.iter()).enumerate() {
                if before != after {
                    println!("  row {row:02}: {before:?} -> {after:?}");
                }
            }
            found += 1;
            if found >= args.limit {
                break;
            }
        }
    }
    println!("found {found} candidate(s)");
    Ok(())
}

fn preferred_display_rows(session: &TeensyEmulatorSession) -> Vec<String> {
    let snapshot = session.snapshot();
    if let Some(preferred) = snapshot.display.preferred.as_deref()
        && let Some(candidate) = snapshot
            .display
            .candidates
            .iter()
            .find(|candidate| candidate.name == preferred)
    {
        return candidate.rows.clone();
    }
    Vec::new()
}

fn parse_u32_arg(value: &str) -> Result<u32, Box<dyn std::error::Error>> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        Ok(u32::from_str_radix(hex, 16)?)
    } else {
        Ok(trimmed.parse()?)
    }
}

fn parse_packet_list(value: &str) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|packet| !packet.is_empty())
        .map(|packet| {
            packet
                .split([':', '-', ' '])
                .map(str::trim)
                .filter(|byte| !byte.is_empty())
                .map(|byte| {
                    let parsed = if byte.starts_with("0x") || byte.starts_with("0X") {
                        parse_u32_arg(byte)?
                    } else {
                        u32::from_str_radix(byte, 16)?
                    };
                    Ok(parsed as u8)
                })
                .collect()
        })
        .collect()
}

fn convert_command(
    input_path: PathBuf,
    out: PathBuf,
    song_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = fs::read(&input_path)?;
    let bundle = convert_tracker(
        &input,
        ConversionOptions {
            song_name,
            ..ConversionOptions::default()
        },
    )?;

    fs::create_dir_all(&out)?;
    for file in &bundle.files {
        let path = out.join(&file.path);
        ensure_parent(&path)?;
        let data = STANDARD.decode(&file.data_base64)?;
        fs::write(path, data)?;
    }

    println!("Wrote {} files to {}", bundle.files.len(), out.display());
    println!(
        "M8: {} phrases, {} chains, {} tables, {} unsupported source effects",
        bundle.report.m8.used_phrases,
        bundle.report.m8.used_chains,
        bundle.report.m8.used_tables,
        bundle.report.m8.unsupported_effects.len()
    );
    if !bundle.report.m8.approximations.is_empty() {
        println!(
            "M8 approximations: {} mapped-near cases",
            bundle.report.m8.approximations.len()
        );
    }
    for warning in &bundle.report.m8.warnings {
        println!("warning: {warning}");
    }

    Ok(())
}

fn probe_firmware_command(
    input_path: PathBuf,
    steps: u32,
    out_json: Option<PathBuf>,
    sd_image: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = fs::read(&input_path)?;
    let sd_image_bytes = sd_image.as_ref().map(fs::read).transpose()?;
    let probe = probe_teensy_hex_boot_with_sd(&input, steps, sd_image_bytes.as_deref())?;
    let bad: Vec<_> = probe
        .trace
        .iter()
        .filter(|event| {
            event.opcode16.is_none()
                || event.note.starts_with("BKPT")
                || event.note.contains("decoded semantics not implemented")
                || event.note.contains("unmapped data read")
                || event.note.contains("outside loaded")
        })
        .collect();

    if let Some(out_json) = out_json {
        ensure_parent(&out_json)?;
        fs::write(&out_json, serde_json::to_vec_pretty(&probe)?)?;
        println!("Wrote firmware probe to {}", out_json.display());
    }

    println!(
        "Firmware probe: status={}, trace_total={}, shown={}, truncated={}, pc=0x{:08x}, sp=0x{:08x}, lr=0x{:08x}, pointer_guards={}, bad={}",
        probe.status,
        probe.trace_total,
        probe.trace.len(),
        probe.trace_truncated,
        probe.cpu.registers[15],
        probe.cpu.registers[13],
        probe.cpu.registers[14],
        probe
            .stats
            .pointer_sanitizations
            .first()
            .map(|item| item.count)
            .unwrap_or(0),
        bad.len()
    );
    for event in bad.iter().rev().take(5).rev() {
        println!(
            "bad: cycle={} pc=0x{:08x} opcode16={:?} opcode32={:?} {}",
            event.cycle, event.pc, event.opcode16, event.opcode32, event.note
        );
    }

    Ok(())
}

fn analyze_command(
    input_path: PathBuf,
    out_json: Option<PathBuf>,
    song_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = fs::read(&input_path)?;
    let report = build_event_regression_report(
        &input,
        ConversionOptions {
            song_name,
            ..ConversionOptions::default()
        },
    )?;
    let json = serde_json::to_vec_pretty(&report)?;

    if let Some(out_json) = out_json {
        ensure_parent(&out_json)?;
        fs::write(&out_json, json)?;
        println!("Wrote event report to {}", out_json.display());
    } else {
        println!("{}", String::from_utf8(json)?);
    }

    if report.comparison.differences.is_empty() {
        println!("Event counters match for tracked note/pan/pitch buckets.");
    } else {
        for difference in &report.comparison.differences {
            println!("difference: {difference}");
        }
    }

    Ok(())
}

fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}
