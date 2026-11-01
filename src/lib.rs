//! # xml2po
//!
//! Extract translatable strings from XML, HTML and MJML documents into
//! gettext PO catalogs, and compile translated PO catalogs back into the
//! source documents.
//!
//! The crate is organized around three building blocks:
//!
//! 1. **Strategies** ([`ExtractionStrategy`], [`Format`]) decide what is
//!    translatable per format — elements, ignored elements and translatable
//!    attributes. Built-ins: MJML, HTML, generic XML.
//! 2. **Extraction** ([`extract_messages`]) streams a document and yields
//!    [`Message`]s with real line-number [`SourceRef`]s, deterministically
//!    ordered.
//! 3. **Compilation** ([`apply_translations`]) streams a document again and
//!    replaces leaf text segments and attribute values whose source string
//!    has a translation, preserving whitespace padding and entities.
//!
//! [`PoCatalog`] handles PO parsing/serialization (with gettext-style
//! wrapping, `msgctxt`, references and merge support).
//!
//! ## Example: extract → translate → compile
//!
//! ```no_run
//! use xml2po::{extract_messages, apply_translations, get_strategy};
//! use xml2po::po::PoCatalog;
//! use std::collections::HashMap;
//!
//! # fn main() -> Result<(), xml2po::Error> {
//! let content = r#"<mjml><mj-text>Hello World</mj-text></mjml>"#;
//! let strategy = get_strategy("mjml")?;
//!
//! // 1. Extract (references carry real line numbers).
//! let messages = extract_messages(content, "template.mjml", strategy)?;
//!
//! // 2. Produce a PO file, hand it to translators, read it back.
//! let po = xml2po::po::messages_to_po(&messages, None);
//! let catalog = PoCatalog::parse(&po)?;
//!
//! // 3. Compile translations back into the document.
//! let translated =
//!     apply_translations(content, &catalog.translations(), strategy)?;
//! # assert!(translated.contains("Hello World"));
//! # Ok(())
//! # }
//! ```
//!
//! ## Features
//!
//! | Feature | Default | Description |
//! |---|---|---|
//! | `mjml` | ✅ | MJML strategy |
//! | `html` | ✅ | HTML strategy (incl. `alt`/`title`/`placeholder`/`aria-label` attributes) |
//! | `xml` | ✅ | Generic XML strategy |
//! | `chrono` | ✅ | Timestamps in generated PO headers |
//! | `serde` | ❌ | `Serialize`/`Deserialize` for messages and catalogs |
//! | `cli` | ✅ | The command-line interface binary |
//! | `parallel` | ✅ | Rayon-parallel extraction of multiple files in the CLI |
//!
//! Library users can opt out of everything with `default-features = false`.

#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod compile;
pub mod error;
pub mod extract;
pub mod filter;
pub mod message;
pub mod options;
pub mod po;
pub mod strategy;

#[cfg(feature = "cli")]
pub mod cli;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

pub use compile::{CompileReport, apply_translations, apply_translations_with};
pub use error::{Error, Result};
pub use extract::{extract_messages, extract_messages_from_bytes, extract_messages_with};
pub use filter::is_translatable_text;
pub use message::{CatalogKey, Message, OwnedCatalogKey, SourceRef};
pub use options::{ExtractionOptions, ExtractionOptionsBuilder};
pub use po::{PoCatalog, PoEntry, PoStats, parse_po_to_translations};
pub use strategy::{
    ExtractionStrategy, Format, HtmlStrategy, MjmlStrategy, ResolvedRule, XmlStrategy,
    available_formats, get_format, get_strategy,
};

/// Decodes raw file bytes into a UTF-8 string, honoring UTF-8/UTF-16 BOMs.
///
/// The CLI uses this so `--format` inputs may be UTF-8, UTF-16 LE or
/// UTF-16 BE; documents without a BOM must be valid UTF-8.
///
/// # Errors
///
/// Returns [`Error::NotUtf8`] when the bytes are neither valid UTF-8 nor
/// a decodable UTF-16 document.
#[cfg(feature = "cli-encoding")]
pub fn decode_file_bytes(path: &std::path::Path, bytes: &[u8]) -> Result<String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(bytes[3..].to_vec()).map_err(|_| Error::NotUtf8 {
            path: path.to_path_buf(),
        });
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        // encoding_rs sniffs the BOM itself and picks LE/BE accordingly.
        let (decoded, _, had_errors) = encoding_rs::UTF_16LE.decode(bytes);
        if had_errors {
            return Err(Error::NotUtf8 {
                path: path.to_path_buf(),
            });
        }
        return Ok(decoded.into_owned());
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| Error::NotUtf8 {
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn end_to_end_extract_merge_compile() {
        let content = r#"<html><body><p>Welcome</p><img alt="Logo"/><p>Welcome</p></body></html>"#;
        let strategy = get_strategy("html").unwrap();

        // First run: fresh extraction.
        let messages = extract_messages(content, "page.html", strategy).unwrap();
        assert_eq!(messages.len(), 2);

        // Second run: merge with translations filled in.
        let po = po::messages_to_po(&messages, None);
        let mut filled = PoCatalog::parse(&po).unwrap();
        for entry in &mut filled.entries {
            entry.msgstr = format!("T[{}]", entry.msgid);
        }

        let messages2 = extract_messages(content, "page.html", strategy).unwrap();
        let merged_catalog = PoCatalog::from_messages_with_merge(&messages2, Some(&filled));
        let po2 = merged_catalog.to_po_string();
        assert!(po2.contains("T[Welcome]"));

        // Compile.
        let catalog = PoCatalog::parse(&po2).unwrap();
        let out = apply_translations(content, &catalog.translations(), strategy).unwrap();
        assert!(out.contains("T[Welcome]"));
        assert!(out.contains(r#"alt="T[Logo]""#));
        assert_eq!(out.matches("T[Welcome]").count(), 2);
    }

    #[test]
    fn lib_reexports_are_usable() {
        // available_formats
        assert!(available_formats().contains(&"mjml"));
        // parse_po_to_translations
        let m = parse_po_to_translations("msgid \"a\"\nmsgstr \"A\"\n").unwrap();
        assert_eq!(m.get("a").map(String::as_str), Some("A"));
        // Format enum
        assert_eq!(Format::Html.strategy().format_name(), "html");
        // is_translatable_text re-export
        assert!(is_translatable_text("Hello"));
        // Error type is std Error
        let _: &dyn std::error::Error = &Error::EmptyPo;
    }

    #[test]
    fn options_default_matches_manual_default() {
        let a = ExtractionOptions::default();
        let b = ExtractionOptions::builder().build();
        assert_eq!(a, b);
    }

    #[test]
    fn unsupported_format_lists_available() {
        assert!(get_strategy("klingon").is_err());
        let text = get_format("klingon").unwrap_err().to_string();
        assert!(text.contains("klingon"));
        assert!(text.contains("mjml"));
    }

    #[test]
    fn empty_po_error_message() {
        assert_eq!(
            Error::EmptyPo.to_string(),
            "PO document contains no entries"
        );
    }

    #[test]
    fn message_and_options_types_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Message>();
        assert_send_sync::<ExtractionOptions>();
        assert_send_sync::<PoCatalog>();
        assert_send_sync::<HashMap<OwnedCatalogKey, String>>();
    }
}
