//! WebAssembly bindings.
//!
//! Only compiled for the `wasm32` target. The bindings mirror the core
//! library API and serialize [`Message`] values with
//! `serde-wasm-bindgen`.

use std::collections::HashMap;

use wasm_bindgen::prelude::*;

use crate::extract::extract_messages_with;
use crate::message::OwnedCatalogKey;
use crate::options::ExtractionOptions;
use crate::po::PoCatalog;
use crate::strategy::get_strategy;

/// Extracts translatable strings as an array of `Message` objects.
#[must_use]
#[wasm_bindgen]
pub fn extract_from_content(
    content: &str,
    format: &str,
    source_name: &str,
) -> Result<JsValue, JsValue> {
    let strategy = get_strategy(format).map_err(js_error)?;
    let messages = extract_messages_with(
        content,
        source_name,
        strategy,
        &ExtractionOptions::default(),
    )
    .map_err(js_error)?;
    serde_wasm_bindgen::to_value(&messages).map_err(js_error)
}

/// Extracts translatable strings and serializes them as a PO document.
#[must_use]
#[wasm_bindgen]
pub fn extract_to_po(content: &str, format: &str, source_name: &str) -> Result<String, JsValue> {
    extract_to_po_with_existing(content, format, source_name, None)
}

/// Like [`extract_to_po`] but merges the result with an existing PO
/// document, keeping its translations for strings that still exist.
#[must_use]
#[wasm_bindgen]
pub fn extract_to_po_with_existing(
    content: &str,
    format: &str,
    source_name: &str,
    existing_po_content: Option<String>,
) -> Result<String, JsValue> {
    let strategy = get_strategy(format).map_err(js_error)?;
    let messages = extract_messages_with(
        content,
        source_name,
        strategy,
        &ExtractionOptions::default(),
    )
    .map_err(js_error)?;
    let existing = existing_po_content
        .map(|po| PoCatalog::parse(&po).map_err(js_error))
        .transpose()?;
    Ok(crate::po::messages_to_po(&messages, existing.as_ref()))
}

/// Compiles a document by applying translations from a PO document.
#[must_use]
#[wasm_bindgen]
pub fn compile_with_po(content: &str, po_content: &str, format: &str) -> Result<String, JsValue> {
    let strategy = get_strategy(format).map_err(js_error)?;
    let translations = PoCatalog::parse(po_content)
        .map_err(js_error)?
        .translations();
    crate::compile::apply_translations(&content, &translations, strategy).map_err(js_error)
}

/// Lists the formats available in this build.
#[must_use]
#[wasm_bindgen]
pub fn get_supported_formats() -> Vec<String> {
    crate::strategy::available_formats()
        .into_iter()
        .map(String::from)
        .collect()
}

/// Applies a translation map directly (no PO parsing).
#[must_use]
#[wasm_bindgen]
pub fn apply_translation_map(
    content: &str,
    format: &str,
    keys: Vec<String>,
    values: Vec<String>,
) -> Result<String, JsValue> {
    let strategy = get_strategy(format).map_err(js_error)?;
    if keys.len() != values.len() {
        return Err(JsValue::from_str(
            "keys and values must have the same length",
        ));
    }
    let translations: HashMap<OwnedCatalogKey, String> = keys
        .into_iter()
        .zip(values)
        .map(|(k, v)| (OwnedCatalogKey::new(None, k), v))
        .collect();
    crate::compile::apply_translations(content, &translations, strategy).map_err(js_error)
}

fn js_error<E: std::fmt::Display>(e: E) -> JsValue {
    JsValue::from_str(&e.to_string())
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn extract_and_compile() {
        let po = extract_to_po("<mj-text>Hello</mj-text>", "mjml", "t.mjml");
        assert!(po.contains("Hello"));
        let compiled = compile_with_po(
            "<mj-text>Hello</mj-text>",
            "msgid \"Hello\"\nmsgstr \"Hola\"\n",
            "mjml",
        )
        .unwrap();
        assert!(compiled.contains("Hola"));
    }
}
