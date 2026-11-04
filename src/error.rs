//! Unified error type for the `xml2po` library.

use std::path::PathBuf;

/// Result alias used across the library.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every failure mode of the library, reported with actionable context.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The requested format name does not match any enabled strategy.
    #[error("unsupported format {name:?} (available formats: {})", formats.join(", "))]
    UnsupportedFormat {
        /// The unknown format name.
        name: String,
        /// Formats that are compiled in.
        formats: Vec<String>,
    },

    /// The XML/HTML input could not be parsed.
    ///
    /// `line`/`column` are 1-based positions derived from the byte offset
    /// at construction time, so callers never have to map offsets
    /// themselves.
    #[error("XML parse error at line {line}, column {column} (byte offset {offset}): {source}")]
    Xml {
        /// Byte offset of the failure.
        offset: u64,
        /// 1-based line of the failure.
        line: usize,
        /// 1-based column of the failure.
        column: usize,
        /// Underlying quick-xml error.
        #[source]
        source: quick_xml::Error,
    },

    /// A PO document could not be parsed.
    #[error("PO parse error on line {line}: {message}")]
    PoParse {
        /// 1-based line where parsing failed.
        line: usize,
        /// Human readable description of the problem.
        message: String,
    },

    /// A file could not be read.
    #[error("failed to read {}: {}", path.display(), source)]
    Io {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// Input was not valid UTF-8 and no decoding path was taken.
    #[error("{} is not valid UTF-8", path.display())]
    NotUtf8 {
        /// Path of the offending file.
        path: PathBuf,
    },

    /// No PO entries were found where at least one was required.
    #[error("PO document contains no entries")]
    EmptyPo,

    /// An output stream could not be written.
    #[error("failed to write output: {source}")]
    Write {
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },

    /// A translation's inline markup does not match its source string.
    ///
    /// Returned by strict compilation and used to reject translations
    /// whose placeholder tags are missing, reordered or unclosed.
    #[error(
        "translation does not match the source markup (msgid {msgid:?}, msgstr {msgstr:?}): {reason}"
    )]
    InvalidTranslation {
        /// The source string the translation was applied to.
        msgid: String,
        /// The rejected translation.
        msgstr: String,
        /// Why the translation was rejected.
        reason: String,
    },

    /// The format could not be inferred from a file extension.
    #[error(
        "cannot infer format from {:?}; pass --format <{}>",
        path.display(),
        "mjml|html|xml"
    )]
    FormatInference {
        /// Path whose extension mapped to no known format.
        path: PathBuf,
    },

    /// Two entries share the same `(msgctxt, msgid)` key and strict
    /// parsing was requested.
    #[error(
        "duplicate catalog entry on line {line}: msgid {msgid:?}{}",
        msgctxt.as_ref().map_or_else(String::new, |c| format!(" (msgctxt {c:?})"))
    )]
    DuplicateKey {
        /// The duplicated msgid.
        msgid: String,
        /// The duplicated msgctxt, if any.
        msgctxt: Option<String>,
        /// 1-based line of the second occurrence.
        line: usize,
    },
}

impl Error {
    /// Builds an [`Error::UnsupportedFormat`] from a name and the enabled formats.
    pub fn unsupported_format(name: impl Into<String>, formats: &[&str]) -> Self {
        Self::UnsupportedFormat {
            name: name.into(),
            formats: formats.iter().map(|f| (*f).to_string()).collect(),
        }
    }

    /// Builds an [`Error::Xml`] from an offset plus a precomputed
    /// 1-based `line`/`column` pair.
    #[must_use]
    pub fn xml(offset: u64, line: usize, column: usize, source: quick_xml::Error) -> Self {
        Self::Xml {
            offset,
            line,
            column,
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_format_lists_available() {
        let err = Error::unsupported_format("klingon", &["mjml", "html"]);
        let text = err.to_string();
        assert!(text.contains("klingon"));
        assert!(text.contains("mjml, html"));
    }

    #[test]
    fn xml_error_reports_line_and_column() {
        let err = Error::Xml {
            offset: 42,
            line: 3,
            column: 17,
            source: quick_xml::Error::Syntax(quick_xml::errors::SyntaxError::UnclosedTag),
        };
        let text = err.to_string();
        assert!(text.contains("line 3"), "{text}");
        assert!(text.contains("column 17"), "{text}");
        assert!(text.contains("byte offset 42"), "{text}");
        // The underlying quick-xml error stays reachable as `source`.
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn invalid_translation_error_is_actionable() {
        let err = Error::InvalidTranslation {
            msgid: "Hello <b>bold</b> world".to_string(),
            msgstr: "Hola <b>mundo".to_string(),
            reason: "unclosed placeholder tag <b>".to_string(),
        };
        let text = err.to_string();
        assert!(text.contains("Hola <b>mundo"), "{text}");
        assert!(text.contains("unclosed placeholder tag <b>"), "{text}");
    }

    #[test]
    fn format_inference_error_names_the_file() {
        let err = Error::FormatInference {
            path: "notes.txt".into(),
        };
        let text = err.to_string();
        assert!(text.contains("notes.txt"), "{text}");
        assert!(text.contains("--format"), "{text}");
    }

    #[test]
    fn duplicate_key_error_reports_context_and_line() {
        let err = Error::DuplicateKey {
            msgid: "Save".to_string(),
            msgctxt: Some("button".to_string()),
            line: 7,
        };
        let text = err.to_string();
        assert!(text.contains("Save"), "{text}");
        assert!(text.contains("button"), "{text}");
        assert!(text.contains('7'), "{text}");
    }

    #[test]
    fn empty_po_message_is_stable() {
        assert_eq!(
            Error::EmptyPo.to_string(),
            "PO document contains no entries"
        );
    }
}
