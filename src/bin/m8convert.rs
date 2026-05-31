use std::fs;
use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Parser;
use m8convert::{ConversionOptions, convert_tracker};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Convert MOD/HVL tracker files into an editable Dirtywave M8 bundle"
)]
struct Args {
    input: PathBuf,

    #[arg(short, long, default_value = "m8convert-out")]
    out: PathBuf,

    #[arg(long)]
    song_name: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let input = fs::read(&args.input)?;
    let bundle = convert_tracker(
        &input,
        ConversionOptions {
            song_name: args.song_name,
            ..ConversionOptions::default()
        },
    )?;

    fs::create_dir_all(&args.out)?;
    for file in &bundle.files {
        let path = args.out.join(&file.path);
        ensure_parent(&path)?;
        let data = STANDARD.decode(&file.data_base64)?;
        fs::write(path, data)?;
    }

    println!(
        "Wrote {} files to {}",
        bundle.files.len(),
        args.out.display()
    );
    println!(
        "M8: {} phrases, {} chains, {} unsupported source effects",
        bundle.report.m8.used_phrases,
        bundle.report.m8.used_chains,
        bundle.report.m8.unsupported_effects.len()
    );
    for warning in &bundle.report.m8.warnings {
        println!("warning: {warning}");
    }

    Ok(())
}

fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}
