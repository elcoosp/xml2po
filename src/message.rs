//! Translation unit types: source references and extracted messages.

/// A `file:line` reference to the place a string was found.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    any(feature = "serde", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize)
)]
pub struct SourceRef {
    /// Path (or display name) of the source document.
    pub file: String,
    /// 1-based line number inside the source document.
    pub line: usize,
}

impl SourceRef {
    /// Creates a new reference.
    #[must_use]
    pub fn new(file: impl Into<String>, line: usize) -> Self {
        Self {
            file: file.into(),
            line,
        }
    }
}

impl std::fmt::Display for SourceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

/// An extracted translatable string with every location it appears at.
///
/// When element-context mode is enabled the message also carries a
/// `msgctxt` (the enclosing element name, or `element@attribute` for
/// attribute values), matching the gettext notion of a translation
/// context.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "serde", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize)
)]
pub struct Message {
    /// The source string (gettext `msgid`).
    pub msgid: String,
    /// Optional gettext context (`msgctxt`).
    pub msgctxt: Option<String>,
    /// Every location the string was found at, sorted by `(file, line)`.
    pub references: Vec<SourceRef>,
}

impl Message {
    /// Creates a message without a context.
    #[must_use]
    pub fn new(msgid: impl Into<String>, references: Vec<SourceRef>) -> Self {
        Self {
            msgid: msgid.into(),
            msgctxt: None,
            references,
        }
    }

    /// Creates a message with a context.
    #[must_use]
    pub fn with_context(
        msgid: impl Into<String>,
        msgctxt: impl Into<String>,
        references: Vec<SourceRef>,
    ) -> Self {
        Self {
            msgid: msgid.into(),
            msgctxt: Some(msgctxt.into()),
            references,
        }
    }

    /// Catalog key used to group and look up this message.
    #[must_use]
    pub fn key(&self) -> CatalogKey<'_> {
        CatalogKey {
            msgctxt: self.msgctxt.as_deref(),
            msgid: &self.msgid,
        }
    }
}

/// Borrowed lookup key combining an optional gettext context with a msgid.
///
/// Two strings with identical text but different contexts are distinct
/// catalog entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CatalogKey<'a> {
    /// The `msgctxt` value, if any.
    pub msgctxt: Option<&'a str>,
    /// The `msgid` value.
    pub msgid: &'a str,
}

impl<'a> CatalogKey<'a> {
    /// Creates a key from an optional context and a msgid.
    #[must_use]
    pub const fn new(msgctxt: Option<&'a str>, msgid: &'a str) -> Self {
        Self { msgctxt, msgid }
    }
}

/// Owned lookup key (context + msgid) usable as a map key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[cfg_attr(
    any(feature = "serde", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize)
)]
pub struct OwnedCatalogKey {
    /// The `msgctxt` value, if any.
    pub msgctxt: Option<String>,
    /// The `msgid` value.
    pub msgid: String,
}

impl OwnedCatalogKey {
    /// Creates an owned key.
    #[must_use]
    pub fn new(msgctxt: Option<String>, msgid: impl Into<String>) -> Self {
        Self {
            msgctxt,
            msgid: msgid.into(),
        }
    }

    /// Borrows the key.
    #[must_use]
    pub fn borrowed(&self) -> CatalogKey<'_> {
        CatalogKey {
            msgctxt: self.msgctxt.as_deref(),
            msgid: &self.msgid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ref_display_and_ordering() {
        let r = SourceRef::new("a.mjml", 12);
        assert_eq!(r.to_string(), "a.mjml:12");
        assert!(SourceRef::new("a.mjml", 3) < SourceRef::new("a.mjml", 9));
        assert!(SourceRef::new("a.mjml", 1) < SourceRef::new("b.mjml", 1));
    }

    #[test]
    fn message_constructors_and_key() {
        let plain = Message::new("Hi", vec![SourceRef::new("f.html", 1)]);
        assert_eq!(plain.msgctxt, None);
        assert_eq!(plain.key(), CatalogKey::new(None, "Hi"));

        let ctx = Message::with_context("Hi", "button", vec![SourceRef::new("f.html", 2)]);
        assert_eq!(ctx.msgctxt.as_deref(), Some("button"));
        assert_eq!(ctx.key(), CatalogKey::new(Some("button"), "Hi"));
    }

    #[test]
    fn owned_key_roundtrip() {
        let owned = OwnedCatalogKey::new(Some("div".to_string()), "Hi".to_string());
        assert_eq!(owned.borrowed(), CatalogKey::new(Some("div"), "Hi"));
        let empty = OwnedCatalogKey::new(None, "x");
        assert_eq!(empty.borrowed().msgctxt, None);
    }
}
