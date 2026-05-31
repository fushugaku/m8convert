pub mod convert;
pub mod hvlfile;
pub mod m8;
pub mod modfile;
pub mod s3mfile;
pub mod wav;

use wasm_bindgen::prelude::*;

pub use convert::{
    ConversionOptions, ConversionReport, ConvertedBundle, convert_hvl, convert_mod, convert_s3m,
    convert_tracker,
};
pub use modfile::Module;

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
