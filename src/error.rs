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
    #[error("XML parse error at byte offset {offset}: {source}")]
    Xml {
        /// Byte offset of the failure.
        offset: u64,
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
}

impl Error {
    /// Builds an [`Error::UnsupportedFormat`] from a name and the enabled formats.
    pub fn unsupported_format(name: impl Into<String>, formats: &[&str]) -> Self {
        Self::UnsupportedFormat {
            name: name.into(),
            formats: formats.iter().map(|f| (*f).to_string()).collect(),
        }
    }
}
