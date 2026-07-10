pub mod converter;
pub mod emulator;

use wasm_bindgen::prelude::*;

pub use converter::modfile::Module;
pub use converter::{
    ConversionOptions, ConversionReport, ConvertedBundle, ReferenceRenderMode, convert_hvl,
    convert_mod, convert_s3m, convert_tracker, convert_xm,
};

#[wasm_bindgen]
pub fn convert_mod_to_m8_bundle_json(
    input: &[u8],
    song_name: Option<String>,
) -> Result<String, JsValue> {
    let options = ConversionOptions {
        song_name,
        ..ConversionOptions::default()
    };

    let bundle = convert_mod(input, options).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_json::to_string(&bundle).map_err(|err| JsValue::from_str(&err.to_string()))
}

#[wasm_bindgen]
pub fn convert_xm_to_m8_bundle_json(
    input: &[u8],
    song_name: Option<String>,
) -> Result<String, JsValue> {
    let options = ConversionOptions {
        song_name,
        ..ConversionOptions::default()
    };

    let bundle = convert_xm(input, options).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_json::to_string(&bundle).map_err(|err| JsValue::from_str(&err.to_string()))
}

#[wasm_bindgen]
pub fn convert_tracker_to_m8_bundle_json(
    input: &[u8],
    song_name: Option<String>,
) -> Result<String, JsValue> {
    let options = ConversionOptions {
        song_name,
        ..ConversionOptions::default()
    };

    let bundle =
        convert_tracker(input, options).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_json::to_string(&bundle).map_err(|err| JsValue::from_str(&err.to_string()))
}

#[wasm_bindgen]
pub fn analyze_teensy_hex_json(input: &[u8]) -> Result<String, JsValue> {
    let analysis =
        emulator::analyze_teensy_hex(input).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_json::to_string_pretty(&analysis).map_err(|err| JsValue::from_str(&err.to_string()))
}

#[wasm_bindgen]
pub fn probe_teensy_hex_boot_json(input: &[u8], max_steps: u32) -> Result<String, JsValue> {
    let probe = emulator::probe_teensy_hex_boot(input, max_steps)
        .map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_json::to_string_pretty(&probe).map_err(|err| JsValue::from_str(&err.to_string()))
}

#[wasm_bindgen]
pub struct TeensyEmulator {
    session: emulator::TeensyEmulatorSession,
}

#[wasm_bindgen]
impl TeensyEmulator {
    #[wasm_bindgen(constructor)]
    pub fn new(input: &[u8]) -> Result<TeensyEmulator, JsValue> {
        let session = emulator::TeensyEmulatorSession::new(input)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
        Ok(Self { session })
    }

    pub fn run_steps_json(&mut self, max_steps: u32) -> Result<String, JsValue> {
        let summary = self.session.run_steps_summary(max_steps);
        serde_json::to_string(&summary).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn run_steps_live_json(&mut self, max_steps: u32) -> Result<String, JsValue> {
        let summary = self.session.run_steps_live_summary(max_steps);
        serde_json::to_string(&summary).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn take_display_slip(&mut self) -> Vec<u8> {
        self.session.take_display_slip()
    }

    pub fn display_stats_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.session.display_stats())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn display_snapshot_json(&self) -> Result<String, JsValue> {
        serde_json::to_string_pretty(&self.session.snapshot())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn send_joypad_state(&mut self, state: u8) -> bool {
        self.session.send_joypad_state(state)
    }

    pub fn load_sd_image(&mut self, bytes: &[u8]) {
        self.session.load_sd_image(bytes);
    }

    pub fn clear_sd_image(&mut self) {
        self.session.clear_sd_image();
    }

    pub fn sd_card_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.session.sd_card_stats())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn audio_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.session.audio_stats())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn take_audio_pcm_words(&mut self) -> Vec<u32> {
        self.session.take_audio_pcm_words()
    }

    pub fn sd_image_bytes(&self) -> Vec<u8> {
        self.session.sd_image_bytes()
    }

    pub fn host_input_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.session.host_input_stats())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    pub fn send_note_on(&mut self, note: u8, velocity: u8) {
        self.session.send_note_on(note, velocity);
    }

    pub fn send_note_off(&mut self) {
        self.session.send_note_off();
    }

    pub fn send_host_bytes(&mut self, bytes: &[u8]) {
        self.session.receive_host_bytes(bytes);
    }
}
