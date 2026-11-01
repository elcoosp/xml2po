//! PO (Portable Object) catalog parsing and serialization.
//!
//! The catalog model covers the gettext subset needed for day-to-day
//! workflows: header entry, `msgctxt`, `msgid`, `msgid_plural`, `msgstr`,
//! `#:` references, translator comments and line wrapping at 79 columns.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::error::{Error, Result};
use crate::message::{Message, OwnedCatalogKey};

/// Default wrap width used by gettext tooling.
pub const DEFAULT_WRAP_WIDTH: usize = 79;

/// One PO entry (a translatable unit).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "serde", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize)
)]
pub struct PoEntry {
    /// Optional gettext context.
    pub msgctxt: Option<String>,
    /// Source string.
    pub msgid: String,
    /// Plural source string, when present.
    pub msgid_plural: Option<String>,
    /// Translation (empty when untranslated).
    pub msgstr: String,
    /// `file:line` locations collected by extraction tools.
    pub references: Vec<String>,
    /// Translator comments (`# …` lines).
    pub comments: Vec<String>,
}

impl PoEntry {
    /// Whether this is the metadata header entry (`msgid ""`).
    #[must_use]
    pub fn is_header(&self) -> bool {
        self.msgid.is_empty()
    }

    /// Whether the entry carries a non-empty translation.
    #[must_use]
    pub fn is_translated(&self) -> bool {
        !self.msgstr.trim().is_empty()
    }

    /// The catalog key of this entry.
    #[must_use]
    pub fn key(&self) -> OwnedCatalogKey {
        OwnedCatalogKey::new(self.msgctxt.clone(), self.msgid.clone())
    }

    /// Serializes this entry (without a trailing blank line).
    fn write_po(&self, out: &mut String, width: usize) {
        for comment in &self.comments {
            for line in comment.lines() {
                let _ = writeln!(out, "# {line}");
            }
        }
        if !self.references.is_empty() {
            let _ = writeln!(out, "#: {}", self.references.join(" "));
        }
        if let Some(ctx) = &self.msgctxt {
            write_field(out, "msgctxt", ctx, width);
        }
        write_field(out, "msgid", &self.msgid, width);
        if let Some(plural) = &self.msgid_plural {
            write_field(out, "msgid_plural", plural, width);
        }
        write_field(out, "msgstr", &self.msgstr, width);
    }
}

/// A parsed PO document: header metadata plus entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "serde", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize)
)]
pub struct PoCatalog {
    /// Raw `msgstr` of the header entry (empty when the document has none).
    pub header: String,
    /// All non-header entries, in document order.
    pub entries: Vec<PoEntry>,
}

impl PoCatalog {
    /// Parses a PO document.
    ///
    /// Plural `msgstr[N]` values keep `N == 0` as the translation; higher
    /// indices are ignored (the extractor never emits plural entries).
    ///
    /// # Errors
    ///
    /// Returns [`Error::PoParse`] when a line cannot be understood.
    pub fn parse(po: &str) -> Result<Self> {
        PoParser::default().run(po)
    }

    /// Parses and returns the `(msgctxt, msgid) -> msgstr` lookup map,
    /// skipping untranslated and header entries.
    #[must_use]
    pub fn translations(&self) -> HashMap<OwnedCatalogKey, String> {
        self.entries
            .iter()
            .filter(|e| e.is_translated())
            .map(|e| (e.key(), e.msgstr.clone()))
            .collect()
    }

    /// Translation coverage statistics.
    #[must_use]
    pub fn stats(&self) -> PoStats {
        let total = self.entries.len();
        let translated = self.entries.iter().filter(|e| e.is_translated()).count();
        PoStats {
            total,
            translated,
            untranslated: total - translated,
        }
    }

    /// Serializes the catalog with the default wrap width.
    #[must_use]
    pub fn to_po_string(&self) -> String {
        self.to_po_string_with_width(DEFAULT_WRAP_WIDTH)
    }

    /// Serializes the catalog, wrapping long values at `width` columns.
    ///
    /// A `width` of 0 disables wrapping (gettext `--no-wrap` semantics).
    #[must_use]
    pub fn to_po_string_with_width(&self, width: usize) -> String {
        let mut out = String::new();
        write_header(&mut out, &self.header);
        for entry in &self.entries {
            out.push('\n');
            entry.write_po(&mut out, width);
        }
        out
    }

    /// Builds a catalog from extracted messages, preserving translations
    /// (and references/comments) of entries that still exist.
    #[must_use]
    pub fn from_messages_with_merge(messages: &[Message], existing: Option<&Self>) -> Self {
        let header =
            existing.map_or_else(|| default_header(None), |catalog| catalog.header.clone());
        let mut catalog = Self {
            header,
            entries: Vec::new(),
        };
        let existing_map: HashMap<OwnedCatalogKey, &PoEntry> = existing
            .map(|c| c.entries.iter().map(|e| (e.key(), e)).collect())
            .unwrap_or_default();

        for message in messages {
            let key = OwnedCatalogKey::new(message.msgctxt.clone(), message.msgid.clone());
            let references: Vec<String> =
                message.references.iter().map(ToString::to_string).collect();
            if let Some(old) = existing_map.get(&key) {
                let mut refs = old.references.clone();
                for r in &references {
                    if !refs.contains(r) {
                        refs.push(r.clone());
                    }
                }
                catalog.entries.push(PoEntry {
                    msgctxt: message.msgctxt.clone(),
                    msgid: message.msgid.clone(),
                    msgid_plural: old.msgid_plural.clone(),
                    msgstr: old.msgstr.clone(),
                    references: refs,
                    comments: old.comments.clone(),
                });
            } else {
                catalog.entries.push(PoEntry {
                    msgctxt: message.msgctxt.clone(),
                    msgid: message.msgid.clone(),
                    msgstr: String::new(),
                    references,
                    ..PoEntry::default()
                });
            }
        }
        catalog
    }

    /// Difference between extracted messages and this catalog.
    #[must_use]
    pub fn diff_messages(&self, messages: &[Message]) -> MessageDiff {
        let catalog_keys: std::collections::HashSet<OwnedCatalogKey> =
            self.entries.iter().map(PoEntry::key).collect();
        let mut added = Vec::new();
        let mut removed = catalog_keys;
        for message in messages {
            let key = OwnedCatalogKey::new(message.msgctxt.clone(), message.msgid.clone());
            if !removed.remove(&key) {
                added.push(key);
            }
        }
        MessageDiff {
            added,
            removed: removed.into_iter().collect(),
        }
    }
}

/// Incremental state for parsing one PO document.
#[derive(Default)]
struct PoParser {
    catalog: PoCatalog,
    current: PoEntry,
    have_current: bool,
    /// True once `msgstr` (or `msgstr[0]`) was seen for the current entry;
    /// a new `msgid` after that starts the next entry.
    saw_msgstr: bool,
    /// Which field continuation lines append to.
    last_field: Option<LastField>,
    /// Comments/references seen before an entry (or after the header);
    /// they belong to the *next* entry.
    pending_comments: Vec<String>,
    pending_refs: Vec<String>,
}

impl PoParser {
    fn run(mut self, po: &str) -> Result<PoCatalog> {
        for (idx, raw_line) in po.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = raw_line.trim_end_matches('\r').trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('#') {
                self.handle_comment(trimmed);
                continue;
            }
            self.handle_field(trimmed, line_no)?;
        }
        if self.have_current {
            flush_entry(&mut self.catalog, &mut self.current);
        }
        Ok(self.catalog)
    }

    fn handle_comment(&mut self, trimmed: &str) {
        if let Some(rest) = trimmed.strip_prefix("#:") {
            self.pending_refs
                .extend(rest.split_whitespace().map(str::to_string));
        } else {
            self.pending_comments
                .push(trimmed.trim_start_matches('#').trim().to_string());
        }
    }

    fn start_entry(&mut self, msgctxt: Option<String>) {
        if self.have_current {
            flush_entry(&mut self.catalog, &mut self.current);
        }
        self.current = PoEntry {
            msgctxt,
            comments: std::mem::take(&mut self.pending_comments),
            references: std::mem::take(&mut self.pending_refs),
            ..PoEntry::default()
        };
        self.have_current = true;
        self.saw_msgstr = false;
    }

    fn handle_field(&mut self, trimmed: &str, line_no: usize) -> Result<()> {
        if let Some(rest) = trimmed.strip_prefix("msgctxt ") {
            self.start_entry(Some(unescape_po_string(rest)));
            self.last_field = Some(LastField::Msgctxt);
        } else if let Some(rest) = trimmed.strip_prefix("msgid ") {
            if !self.have_current || self.saw_msgstr {
                self.start_entry(None);
            }
            self.current.msgid = unescape_po_string(rest);
            self.last_field = Some(LastField::Msgid);
        } else if let Some(rest) = trimmed.strip_prefix("msgid_plural ") {
            require_entry(self.have_current, line_no)?;
            self.current.msgid_plural = Some(unescape_po_string(rest));
            self.last_field = Some(LastField::MsgidPlural);
        } else if let Some(rest) = trimmed.strip_prefix("msgstr[0] ") {
            require_entry(self.have_current, line_no)?;
            self.current.msgstr = unescape_po_string(rest);
            self.saw_msgstr = true;
            self.last_field = Some(LastField::Msgstr);
        } else if trimmed.starts_with("msgstr[") {
            require_entry(self.have_current, line_no)?;
            // Only msgstr[0] is retained; higher plural forms are dropped.
            self.last_field = None;
        } else if let Some(rest) = trimmed.strip_prefix("msgstr ") {
            require_entry(self.have_current, line_no)?;
            self.current.msgstr = unescape_po_string(rest);
            self.saw_msgstr = true;
            self.last_field = Some(LastField::Msgstr);
        } else if trimmed.starts_with('"') {
            self.handle_continuation(trimmed, line_no)?;
        } else {
            return Err(Error::PoParse {
                line: line_no,
                message: format!("unexpected line: {trimmed:?}"),
            });
        }
        Ok(())
    }

    fn handle_continuation(&mut self, trimmed: &str, line_no: usize) -> Result<()> {
        let value = unescape_po_string(trimmed);
        match self.last_field {
            Some(LastField::Msgctxt) => {
                self.current
                    .msgctxt
                    .get_or_insert_with(String::new)
                    .push_str(&value);
            }
            Some(LastField::Msgid) => self.current.msgid.push_str(&value),
            Some(LastField::MsgidPlural) => {
                self.current
                    .msgid_plural
                    .get_or_insert_with(String::new)
                    .push_str(&value);
            }
            Some(LastField::Msgstr) => self.current.msgstr.push_str(&value),
            None => {
                return Err(Error::PoParse {
                    line: line_no,
                    message: "continuation string before any msgid/msgstr".to_string(),
                });
            }
        }
        Ok(())
    }
}

/// Coverage statistics of a catalog.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoStats {
    /// Total non-header entries.
    pub total: usize,
    /// Entries with a non-empty translation.
    pub translated: usize,
    /// Entries still untranslated.
    pub untranslated: usize,
}

impl PoStats {
    /// Coverage as a percentage (0-100), 100 when the catalog is empty.
    #[must_use]
    pub fn coverage_percent(&self) -> f64 {
        if self.total == 0 {
            return 100.0;
        }
        let translated = f64::from(u32::try_from(self.translated).unwrap_or(u32::MAX));
        let total = f64::from(u32::try_from(self.total).unwrap_or(u32::MAX));
        (translated / total) * 100.0
    }
}

/// Result of comparing extracted messages against a catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageDiff {
    /// Strings present in the sources but missing from the catalog.
    pub added: Vec<OwnedCatalogKey>,
    /// Strings present in the catalog but no longer in the sources.
    pub removed: Vec<OwnedCatalogKey>,
}

impl MessageDiff {
    /// Whether the sources and the catalog agree.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

#[derive(Debug, Clone, Copy)]
enum LastField {
    Msgctxt,
    Msgid,
    MsgidPlural,
    Msgstr,
}

fn require_entry(have_current: bool, line: usize) -> Result<()> {
    if have_current {
        Ok(())
    } else {
        Err(Error::PoParse {
            line,
            message: "msgstr/msgid_plural outside of an entry".to_string(),
        })
    }
}

/// Moves a finished entry into the catalog; header entries only donate
/// their `msgstr`.
fn flush_entry(catalog: &mut PoCatalog, current: &mut PoEntry) {
    if current.is_header() {
        catalog.header.clone_from(&current.msgstr);
    } else {
        catalog.entries.push(std::mem::take(current));
    }
}

/// Writes one `field "value"` construct with wrapping.
///
/// A `width` of 0 means no wrapping: the field is always emitted on a
/// single line (gettext `--no-wrap` semantics).
fn write_field(out: &mut String, field: &str, value: &str, width: usize) {
    let escaped = escape_po_string(value);
    let single = format!("{field} \"{escaped}\"");
    if width == 0 || single.chars().count() <= width {
        out.push_str(&single);
        out.push('\n');
        return;
    }
    let _ = writeln!(out, "{field} \"\"");
    for chunk in wrap_escaped(&escaped, width) {
        let _ = writeln!(out, "\"{chunk}\"");
    }
}

/// Writes the header entry (always wrapped as `msgid ""` + continuations).
fn write_header(out: &mut String, header: &str) {
    out.push_str("msgid \"\"\n");
    out.push_str("msgstr \"\"\n");
    for line in header.split("\\n").filter(|l| !l.is_empty()) {
        let _ = writeln!(out, "\"{}\\n\"", escape_po_string(line));
    }
}

/// Splits an escaped PO string into continuation-line chunks, keeping
/// `\\n` sequences so unescaping reproduces the original value.
fn wrap_escaped(escaped: &str, width: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let segments: Vec<&str> = escaped.split("\\n").collect();
    let last = segments.len().saturating_sub(1);
    for (i, segment) in segments.iter().enumerate() {
        let mut current = String::new();
        for word in segment.split_inclusive(' ') {
            if current.chars().count() + word.chars().count() + 2 > width && !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            current.push_str(word);
        }
        if i < last {
            current.push_str("\\n");
        }
        if !current.is_empty() || (chunks.is_empty() && i == last) {
            chunks.push(current);
        }
    }
    chunks
}

/// Escapes a raw string for inclusion between PO quotes.
#[must_use]
pub fn escape_po_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// Reverses [`escape_po_string`].
#[must_use]
pub fn unescape_po_string(s: &str) -> String {
    let trimmed = s.trim();
    let inner = if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(escaped @ ('"' | '\\')) => out.push(escaped),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses a PO document into a `(msgid -> msgstr)` map without contexts.
///
/// Kept as a convenience for callers that do not use `msgctxt`; entries
/// with a context are skipped. Untranslated entries are skipped too.
///
/// # Errors
///
/// Returns [`Error::PoParse`] when the document cannot be parsed.
pub fn parse_po_to_translations(po: &str) -> Result<HashMap<String, String>> {
    Ok(PoCatalog::parse(po)?
        .entries
        .into_iter()
        .filter(|e| e.msgctxt.is_none() && e.is_translated())
        .map(|e| (e.msgid, e.msgstr))
        .collect())
}

/// Serializes extracted messages into a fresh PO document, optionally
/// preserving translations from an existing catalog.
#[must_use]
pub fn messages_to_po(messages: &[Message], existing: Option<&PoCatalog>) -> String {
    PoCatalog::from_messages_with_merge(messages, existing).to_po_string()
}

/// Builds the standard PO header value, with a creation timestamp when the
/// `chrono` feature is enabled.
#[must_use]
pub fn default_header(project_id_version: Option<&str>) -> String {
    let creation_date = {
        #[cfg(feature = "chrono")]
        {
            chrono::Utc::now().format("%Y-%m-%d %H:%M%z").to_string()
        }
        #[cfg(not(feature = "chrono"))]
        {
            String::new()
        }
    };
    let mut header = String::new();
    let _ = write!(
        header,
        "Project-Id-Version: {}\\nReport-Msgid-Bugs-To: \\nPOT-Creation-Date: {}\\n\
         PO-Revision-Date: \\nLast-Translator: \\nLanguage-Team: \\nLanguage: \\n\
         MIME-Version: 1.0\\nContent-Type: text/plain; charset=UTF-8\\n\
         Content-Transfer-Encoding: 8bit\\nPlural-Forms: nplurals=2; plural=(n != 1);\\n",
        project_id_version.unwrap_or(""),
        creation_date
    );
    header
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::SourceRef;

    fn msg(msgid: &str, refs: &[(&str, usize)]) -> Message {
        Message::new(
            msgid.to_string(),
            refs.iter().map(|(f, l)| SourceRef::new(*f, *l)).collect(),
        )
    }

    #[test]
    fn roundtrip_simple_catalog() {
        let po = "msgid \"\"\nmsgstr \"\"\n\"Project-Id-Version: demo\\n\"\n\n#: a.html:1\nmsgid \"Hello\"\nmsgstr \"Hola\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(catalog.header, "Project-Id-Version: demo\n");
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.entries[0].msgid, "Hello");
        assert_eq!(catalog.entries[0].msgstr, "Hola");
        assert_eq!(catalog.entries[0].references, vec!["a.html:1"]);

        let out = catalog.to_po_string();
        let reparsed = PoCatalog::parse(&out).unwrap();
        assert_eq!(reparsed.entries, catalog.entries);
    }

    #[test]
    fn parse_msgctxt_and_plural() {
        let po = "msgctxt \"button\"\nmsgid \"Save\"\nmsgstr \"Guardar\"\n\nmsgid \"one\"\nmsgid_plural \"many\"\nmsgstr[0] \"uno\"\nmsgstr[1] \"muchos\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.entries[0].msgctxt.as_deref(), Some("button"));
        assert_eq!(catalog.entries[1].msgid_plural.as_deref(), Some("many"));
        assert_eq!(catalog.entries[1].msgstr, "uno");
    }

    #[test]
    fn parse_continuation_lines() {
        let po = "msgid \"\"\nmsgstr \"\"\n\nmsgid \"line one\"\n\" line two\"\nmsgstr \"\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(catalog.entries[0].msgid, "line one line two");
    }

    #[test]
    fn parse_multiline_msgstr() {
        let po = "msgid \"a\"\nmsgstr \"\"\n\"primera\\n\"\n\"segunda\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(catalog.entries[0].msgstr, "primera\nsegunda");
    }

    #[test]
    fn parse_comments() {
        let po = "# translator note\n#: f.html:3\nmsgid \"x\"\nmsgstr \"y\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(catalog.entries[0].comments, vec!["translator note"]);
        assert_eq!(catalog.entries[0].references, vec!["f.html:3"]);
    }

    #[test]
    fn parse_errors() {
        let err = PoCatalog::parse("garbage here\n").unwrap_err();
        assert!(matches!(err, Error::PoParse { line: 1, .. }));
        let err = PoCatalog::parse("\"dangling\"\n").unwrap_err();
        assert!(matches!(err, Error::PoParse { line: 1, .. }));
        let err = PoCatalog::parse("msgstr \"x\"\n").unwrap_err();
        assert!(matches!(err, Error::PoParse { line: 1, .. }));
        let err = PoCatalog::parse("msgid_plural \"x\"\n").unwrap_err();
        assert!(matches!(err, Error::PoParse { line: 1, .. }));
        let err = PoCatalog::parse("msgstr[0] \"x\"\n").unwrap_err();
        assert!(matches!(err, Error::PoParse { line: 1, .. }));
        let err = PoCatalog::parse("msgstr[1] \"x\"\n").unwrap_err();
        assert!(matches!(err, Error::PoParse { line: 1, .. }));
    }

    #[test]
    fn translations_map_and_stats() {
        let po = "msgid \"a\"\nmsgstr \"A\"\n\nmsgid \"b\"\nmsgstr \"\"\n\nmsgctxt \"c\"\nmsgid \"a\"\nmsgstr \"ctx-A\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        let map = catalog.translations();
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get(&OwnedCatalogKey::new(None, "a"))
                .map(String::as_str),
            Some("A")
        );
        assert_eq!(
            map.get(&OwnedCatalogKey::new(Some("c".into()), "a"))
                .map(String::as_str),
            Some("ctx-A")
        );
        let stats = catalog.stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.translated, 2);
        assert_eq!(stats.untranslated, 1);
        assert!((stats.coverage_percent() - 200.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn empty_catalog_stats_are_full_coverage() {
        let stats = PoCatalog::default().stats();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.coverage_percent(), 100.0);
    }

    #[test]
    fn wrapping_produces_continuation_lines() {
        let long = "This is a very long translatable string that certainly exceeds the default gettext wrap width used by the tool";
        let entry = PoEntry {
            msgid: long.to_string(),
            msgstr: "translation".to_string(),
            ..PoEntry::default()
        };
        let mut out = String::new();
        entry.write_po(&mut out, DEFAULT_WRAP_WIDTH);
        let reparsed = PoCatalog::parse(&out).unwrap();
        assert_eq!(reparsed.entries[0].msgid, long);
        assert!(
            out.lines()
                .all(|l| l.chars().count() <= DEFAULT_WRAP_WIDTH + 2)
        );
    }

    #[test]
    fn width_zero_disables_wrapping() {
        let long =
            "This is a very long translatable string that certainly exceeds any reasonable width";
        let entry = PoEntry {
            msgid: long.to_string(),
            msgstr: "translation".to_string(),
            ..PoEntry::default()
        };
        let mut out = String::new();
        entry.write_po(&mut out, 0);
        assert_eq!(out, format!("msgid \"{long}\"\nmsgstr \"translation\"\n"));
    }

    #[test]
    fn wrapping_handles_newlines_and_long_words() {
        let value = "first line\nsecond line with quite a few words that will need to be wrapped eventually\nsupercalifragilisticexpialidocious-superlongtokenwithoutspaces";
        let entry = PoEntry {
            msgid: value.to_string(),
            msgstr: "x".to_string(),
            ..PoEntry::default()
        };
        let mut out = String::new();
        entry.write_po(&mut out, DEFAULT_WRAP_WIDTH);
        let reparsed = PoCatalog::parse(&out).unwrap();
        assert_eq!(reparsed.entries[0].msgid, value);
    }

    #[test]
    fn escape_and_unescape_roundtrip() {
        let raw = "quote \" back \\ slash \n newline \t tab \r cr";
        let escaped = escape_po_string(raw);
        assert!(!escaped.contains('\n'));
        assert_eq!(unescape_po_string(&format!("\"{escaped}\"")), raw);
        assert_eq!(unescape_po_string(raw), raw);
        // Unknown escapes pass through.
        assert_eq!(unescape_po_string("\\q"), "\\q");
        // Trailing lone backslash.
        assert_eq!(unescape_po_string("a\\"), "a\\");
    }

    #[test]
    fn parse_po_to_translations_skips_context_and_untranslated() {
        let po = "msgid \"a\"\nmsgstr \"A\"\n\nmsgid \"b\"\nmsgstr \"\"\n\nmsgctxt \"c\"\nmsgid \"a\"\nmsgstr \"ctx-A\"\n";
        let map = parse_po_to_translations(po).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("a").map(String::as_str), Some("A"));
    }

    #[test]
    fn messages_to_po_without_existing() {
        let messages = vec![msg("Hello", &[("a.html", 3)])];
        let po = messages_to_po(&messages, None);
        assert!(po.starts_with("msgid \"\""));
        assert!(po.contains("#: a.html:3"));
        assert!(po.contains("msgid \"Hello\""));
        assert!(po.contains("msgstr \"\""));
        assert!(po.contains("Project-Id-Version"));
        assert!(po.contains("Content-Type: text/plain; charset=UTF-8"));
        assert!(po.contains("Plural-Forms"));
    }

    #[test]
    fn messages_to_po_merges_existing_translations() {
        let existing = PoCatalog::parse("msgid \"Hello\"\nmsgstr \"Hola\"\n").unwrap();
        let messages = vec![msg("Hello", &[("b.html", 1)]), msg("New", &[("b.html", 2)])];
        let po = messages_to_po(&messages, Some(&existing));
        let reparsed = PoCatalog::parse(&po).unwrap();
        let hello = reparsed
            .entries
            .iter()
            .find(|e| e.msgid == "Hello")
            .unwrap();
        assert_eq!(hello.msgstr, "Hola");
        assert_eq!(hello.references, vec!["b.html:1"]);
        assert!(reparsed.entries.iter().any(|e| e.msgid == "New"));
    }

    #[test]
    fn merge_preserves_references_comments_and_plural() {
        let existing = PoCatalog::parse("# keep me\n#: old.html:9\nmsgid \"Hello\"\nmsgid_plural \"Hellos\"\nmsgstr[0] \"Hola\"\n").unwrap();
        let messages = vec![msg("Hello", &[("new.html", 4)])];
        let merged = PoCatalog::from_messages_with_merge(&messages, Some(&existing));
        let entry = &merged.entries[0];
        assert_eq!(entry.msgstr, "Hola");
        assert_eq!(entry.msgid_plural.as_deref(), Some("Hellos"));
        assert_eq!(entry.comments, vec!["keep me"]);
        assert!(entry.references.contains(&"old.html:9".to_string()));
        assert!(entry.references.contains(&"new.html:4".to_string()));
    }

    #[test]
    fn diff_messages_reports_added_and_removed() {
        let catalog =
            PoCatalog::parse("msgid \"kept\"\nmsgstr \"K\"\n\nmsgid \"gone\"\nmsgstr \"G\"\n")
                .unwrap();
        let messages = vec![msg("kept", &[("a", 1)]), msg("added", &[("a", 2)])];
        let diff = catalog.diff_messages(&messages);
        assert!(!diff.is_clean());
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.added[0].msgid, "added");
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.removed[0].msgid, "gone");

        let messages = vec![msg("kept", &[("a", 1)]), msg("gone", &[("a", 3)])];
        let diff = catalog.diff_messages(&messages);
        assert!(diff.is_clean());
    }

    #[test]
    fn default_header_includes_standard_fields() {
        let header = default_header(Some("xml2po"));
        assert!(header.contains("Project-Id-Version: xml2po"));
        assert!(header.contains("POT-Creation-Date: "));
        assert!(header.contains("MIME-Version: 1.0"));
        let header = default_header(None);
        assert!(header.starts_with("Project-Id-Version: "));
    }

    #[test]
    fn entry_helpers() {
        let header_entry = PoEntry::default();
        assert!(header_entry.is_header());
        assert!(!header_entry.is_translated());
        let translated = PoEntry {
            msgid: "a".into(),
            msgstr: "A".into(),
            ..PoEntry::default()
        };
        assert!(!translated.is_header());
        assert!(translated.is_translated());
        assert_eq!(translated.key(), OwnedCatalogKey::new(None, "a"));
    }

    #[test]
    fn write_field_short_and_long() {
        let mut out = String::new();
        write_field(&mut out, "msgid", "short", DEFAULT_WRAP_WIDTH);
        assert_eq!(out, "msgid \"short\"\n");

        let mut out = String::new();
        write_field(&mut out, "msgctxt", "short ctx", DEFAULT_WRAP_WIDTH);
        assert_eq!(out, "msgctxt \"short ctx\"\n");
    }

    #[test]
    fn parse_empty_and_header_only() {
        let catalog = PoCatalog::parse("").unwrap();
        assert_eq!(catalog.entries, [] as [PoEntry; 0]);
        let catalog = PoCatalog::parse("msgid \"\"\nmsgstr \"\"\n").unwrap();
        assert_eq!(catalog.entries, [] as [PoEntry; 0]);
        assert_eq!(catalog.header, "");
    }

    #[test]
    fn wrap_escaped_empty_chunk_edge() {
        // A value that is only a separator produces a single empty chunk.
        let chunks = wrap_escaped("\\n", 79);
        assert_eq!(chunks, vec!["\\n"]);
        let chunks = wrap_escaped("", 79);
        assert_eq!(chunks, vec![""]);
    }

    #[test]
    fn catalog_equality_and_debug() {
        let a = PoCatalog::parse("msgid \"a\"\nmsgstr \"A\"\n").unwrap();
        let b = PoCatalog::parse("msgid \"a\"\nmsgstr \"A\"\n").unwrap();
        assert_eq!(a, b);
        assert!(format!("{a:?}").contains("entries"));
    }

    #[test]
    fn serialization_emits_comments_references_and_msgctxt() {
        let entry = PoEntry {
            msgctxt: Some("button".to_string()),
            msgid: "Save".to_string(),
            msgstr: "Guardar".to_string(),
            references: vec!["a.html:1".to_string(), "b.html:2".to_string()],
            comments: vec!["translator note".to_string(), "another".to_string()],
            ..PoEntry::default()
        };
        let catalog = PoCatalog {
            header: String::new(),
            entries: vec![entry],
        };
        let po = catalog.to_po_string();
        assert!(po.contains("# translator note"), "{po}");
        assert!(po.contains("# another"), "{po}");
        assert!(po.contains("#: a.html:1 b.html:2"), "{po}");
        assert!(po.contains("msgctxt \"button\""), "{po}");
        let reparsed = PoCatalog::parse(&po).unwrap();
        assert_eq!(reparsed.entries, catalog.entries);
    }

    #[test]
    fn serialization_emits_msgid_plural() {
        let entry = PoEntry {
            msgid: "one".to_string(),
            msgid_plural: Some("many".to_string()),
            msgstr: "uno".to_string(),
            ..PoEntry::default()
        };
        let po = PoCatalog {
            header: String::new(),
            entries: vec![entry],
        }
        .to_po_string();
        assert!(po.contains("msgid \"one\""), "{po}");
        assert!(po.contains("msgid_plural \"many\""), "{po}");
    }

    #[test]
    fn parse_msgctxt_and_plural_continuations() {
        let po = "msgctxt \"very long context that will be wrapped across \"\n\"several lines\"\nmsgid \"k\"\nmsgstr \"v\"\n\nmsgid \"one\"\nmsgid_plural \"many many many many many many many many many many many many many many many \"\n\"many more\"\nmsgstr[0] \"uno\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(
            catalog.entries[0].msgctxt.as_deref(),
            Some("very long context that will be wrapped across several lines")
        );
        assert_eq!(
            catalog.entries[1].msgid_plural.as_deref(),
            Some(
                "many many many many many many many many many many many many many many many many more"
            )
        );
    }

    #[test]
    fn comments_and_refs_between_entries_go_to_next_entry() {
        let po =
            "msgid \"a\"\nmsgstr \"A\"\n\n#: x.html:9\n# note for b\nmsgid \"b\"\nmsgstr \"B\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(
            catalog.entries[0].references,
            [] as [std::string::String; 0]
        );
        assert_eq!(catalog.entries[0].comments, [] as [std::string::String; 0]);
        assert_eq!(catalog.entries[1].references, vec!["x.html:9"]);
        assert_eq!(catalog.entries[1].comments, vec!["note for b"]);
    }

    #[test]
    fn msgstr_before_msgid_is_error_line() {
        let err = PoCatalog::parse("\n\nmsgstr \"x\"\n").unwrap_err();
        assert_eq!(
            err.to_string(),
            "PO parse error on line 3: msgstr/msgid_plural outside of an entry"
        );
    }
}
