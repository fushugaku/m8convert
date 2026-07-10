# m8convert

Rust + WebAssembly converter from ProTracker/NoiseTracker `.mod`, HivelyTracker `.hvl`, Scream Tracker 3 `.s3m`, and FastTracker `.xm` modules to an editable Dirtywave M8 bundle.

The converter currently emits:

- a browser-downloadable `.zip` containing the complete converted bundle
- `*.m8s` song file based on a firmware 6.2 empty M8 template
- `Samples/*.wav` files extracted from MOD, S3M, and XM sample data
- `conversion-report.json` with unsupported effects, mapped approximations, and limit warnings
- `m8convert-project.json` with source sample metadata
- when editable conversion would lose behavior, `Samples/reference-mix.wav` plus a separate `*-reference.m8s` stem project rendered by the pure-Rust tracker replayer

## Why The Report Matters

MOD, HVL, S3M, XM, and M8 are all trackers, but their sequencing and instrument models are not identical. MOD/S3M/XM store pattern rows per channel, HVL has positions that reference reusable tracks plus synthetic instruments, and M8 uses song rows, chains, 16-step phrases, and tables for tick-time behavior. Large source modules can exceed M8's 255 phrase / 255 chain limits, and some tracker effects still need deliberate translation.

This project does not silently pretend those conversions are exact. Notes, instruments, volumes, playback flow, restart/loop hops, stereo PCM, sample loops, ping-pong mode, MOD finetune, S3M C5 speed, XM relative note/finetune, tempo, panning, volume-column shortcuts, direct effects, slides, vibrato, tremor/tremolo, sample offsets, and retrigger effects are exported where M8 has a compatible representation. Unsupported effects and deliberate near-matches are preserved in the JSON report. For MOD/S3M/XM conversions with unavoidable loss (including S3M OPL/AdLib), the bundle also contains a stereo reference render and a separate M8 stem project. HVL keeps the original source under `Reference/` because exact Hively synthesis is not yet available in the pure-Rust render path.

## CLI

```bash
cargo run --bin m8convert -- path/to/song.mod --out out/m8-song
cargo run --bin m8convert -- path/to/song.hvl --out out/m8-song
cargo run --bin m8convert -- path/to/song.s3m --out out/m8-song
cargo run --bin m8convert -- path/to/song.xm --out out/m8-song
cargo run --bin m8convert -- analyze path/to/song.s3m --out-json out/events.json
```

Place the ZIP contents so the converted song lives in `/Bundles/<ProjectName>/` on the M8 SD card. The ZIP already contains that `Bundles/<ProjectName>/` folder, so extracting it at the SD card root is enough.

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
- HVL instruments are approximated as M8 WavSynth patches; playback flow and common track effects are translated, and the original HVL is retained as a lossless source fallback
- S3M modules with PCM sample instruments and packed patterns
- S3M AdLib/OPL instruments are reported in the editable project and preserved audibly in the automatic reference-render stem; supported flow, mix, and tick effects are translated to phrase FX or M8 tables
- XM modules with up to 32 channels, packed patterns, PCM samples, common flow/effect commands, note-off events, and volume-column commands
- XM instruments are adapted through the S3M-compatible exporter; populated samples are flattened to M8 sampler instruments, note sample maps are applied, finetune uses the FT2 1/128-semitone scale, and ping-pong loops use M8 `FWD_PP`

## Implementation Notes

The embedded empty M8 template comes from the MIT-licensed `m8-file-parser` crate examples. The project writes editable M8 structures through that crate instead of hand-writing undocumented offsets.
