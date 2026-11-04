//! # xml2po
//!
//! Extract translatable strings from XML, HTML and MJML documents into
//! gettext PO catalogs, and compile translated PO catalogs back into the
//! source documents.
//!
//! ## Why xml2po
//!
//! - **Markup-aware extraction.** Mixed content is folded into *one*
//!   message with inline placeholders:
//!   `<p>Hello <b>bold</b> world</p>` becomes the msgid
//!   `Hello <b>bold</b> world`. Translators reorder freely; compilation
//!   re-inserts the original tags (attributes included) and rejects
//!   translations whose markup does not match.
//! - **ITS semantics.** The HTML5/ITS `translate="yes|no"` attribute
//!   (and `its:translate`) is honored out of the box; ignored inline
//!   content such as `<span translate="no">ACME</span>` is kept verbatim
//!   inside the msgid, itstool-style.
//! - **Lossless PO layer.** Plural forms (`msgstr[N]`), `#, flags`,
//!   `#|` previous msgids and `#~` obsolete entries all round-trip, so
//!   an `extract --merge` cycle never loses translator work. Removed
//!   strings become obsolete; returning strings are resurrected with
//!   their old translation.
//! - **CI-friendly.** `check` gates catalogs on drift and coverage,
//!   `stats --json` and `check --json` emit machine-readable output.
//! - **XLIFF bridge.** Export catalogs to XLIFF 1.2 for CAT tools and
//!   import them back (feature `xliff`).
//!
//! ## Example: extract → translate → compile
//!
//! ```no_run
//! use xml2po::{extract_messages, apply_translations, get_strategy};
//! use xml2po::po::PoCatalog;
//! use std::collections::HashMap;
//!
//! # fn main() -> Result<(), xml2po::Error> {
//! let content = r#"<p>Hello <b>bold</b> world</p>"#;
//! let strategy = get_strategy("html")?;
//!
//! // 1. Extract: one message, inline tags folded in as placeholders.
//! let messages = extract_messages(content, "page.html", strategy)?;
//! assert_eq!(messages[0].msgid, "Hello <b>bold</b> world");
//!
//! // 2. Produce a PO file, hand it to translators, read it back.
//! let po = xml2po::po::messages_to_po(&messages, None);
//! let mut catalog = PoCatalog::parse(&po)?;
//! catalog.set_translation(
//!     xml2po::message::CatalogKey::new(None, "Hello <b>bold</b> world"),
//!     "Hola <b>negrita</b> mundo",
//! );
//!
//! // 3. Compile: the original `<b>` tag is re-inserted verbatim.
//! let translated = apply_translations(content, &catalog.translations(), strategy)?;
//! assert!(translated.contains("Hola <b>negrita</b> mundo"));
//! # Ok(())
//! # }
//! ```
//!
//! ## Example: XLIFF round trip
//!
//! See the [`xliff`] module (feature `xliff`) for the CAT-tool bridge:
//! [`catalog_to_xliff`] and [`catalog_from_xliff`] exchange catalogs with
//! professional translation tools while preserving inline markup.
//!
//! ## Feature flags
//!
//! | Feature | Default | Description |
//! |---|---|---|
//! | `mjml` | ✅ | MJML strategy |
//! | `html` | ✅ | HTML strategy (incl. `alt`/`title`/`placeholder`/`aria-label` attributes) |
//! | `xml` | ✅ | Generic XML strategy |
//! | `chrono` | ✅ | Timestamps in generated PO headers |
//! | `serde` | ❌ | `Serialize`/`Deserialize` for messages, options and catalogs |
//! | `xliff` | ✅ | XLIFF 1.2 import/export ([`catalog_to_xliff`], [`catalog_from_xliff`]) |
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

#[cfg(feature = "xliff")]
pub mod xliff;

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

#[cfg(feature = "xliff")]
pub use xliff::{catalog_from_xliff, catalog_to_xliff};

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
        let strategy = &HtmlStrategy;

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
        // Every advertised format resolves.
        for name in available_formats() {
            assert!(get_strategy(name).is_ok(), "{name}");
        }
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
        assert!(text.contains("unsupported format"));
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
