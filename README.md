# m8convert

Rust + WebAssembly converter from ProTracker/NoiseTracker `.mod`, HivelyTracker `.hvl`, and Scream Tracker 3 `.s3m` modules to an editable Dirtywave M8 bundle.

The converter currently emits:

- a browser-downloadable `.zip` containing the complete converted bundle
- `*.m8s` song file based on a firmware 6.2 empty M8 template
- `Samples/*.wav` files extracted from MOD and S3M sample data
- `conversion-report.json` with unsupported effects and limit warnings
- `m8convert-project.json` with source sample metadata

## Why The Report Matters

MOD, HVL, S3M, and M8 are all trackers, but their sequencing and instrument models are not identical. A MOD/S3M pattern has 64 rows per channel; HVL has positions that reference reusable tracks plus synthetic instruments; M8 uses song rows, chains, and 16-step phrases. Large source modules can exceed M8's 255 phrase / 255 chain limits, and tick-time effects do not map 1:1 to static M8 phrase cells.

This project does not silently pretend those conversions are exact. Notes, instruments, volumes, order table, sample data, and sample loops are exported; unsupported effects are preserved in the JSON report so the next pass can map them deliberately.

## CLI

```bash
cargo run --bin m8convert -- path/to/song.mod --out out/m8-song
cargo run --bin m8convert -- path/to/song.hvl --out out/m8-song
cargo run --bin m8convert -- path/to/song.s3m --out out/m8-song
```

Extract the generated ZIP at the SD card root. It creates `Bundles/<project>/` with the `*.m8s`, `Samples/`, report, and manifest in the same bundle directory, matching the sample paths written into the M8 song file.

## WebAssembly

Install `wasm-pack`, then build the browser package:

```bash
cargo install wasm-pack
wasm-pack build --target web --out-dir web/pkg
```

Open `web/index.html` from a local web server:

```bash
python3 -m http.server 8080
```

Then visit `http://localhost:8080/web/`.

## Supported Input

- 15-sample SoundTracker-style modules
- 31-sample ProTracker modules with signatures such as `M.K.`, `M!K!`, `FLT4`, `4CHN`, `6CHN`, `8CHN`, and `xxCH`
- Up to 8 channels are mapped to M8 tracks; additional channels are reported as dropped
- HVL0/HVL1 HivelyTracker modules with 4-16 channels
- HVL instruments are approximated as M8 WavSynth patches; exact Hively synthesis is not implemented yet
- S3M modules with PCM sample instruments and packed patterns
- S3M AdLib/OPL instruments and tick effects are reported but not exactly converted yet

## Implementation Notes

The embedded empty M8 template comes from the MIT-licensed `m8-file-parser` crate examples. The project writes editable M8 structures through that crate instead of hand-writing undocumented offsets.
