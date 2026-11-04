//! PO (Portable Object) catalog parsing and serialization.
//!
//! The catalog model covers the gettext subset needed for day-to-day
//! workflows: header entry, `msgctxt`, `msgid`, `msgid_plural`, `msgstr`,
//! `#:` references, translator comments and line wrapping at 79 columns.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use crate::error::{Error, Result};
use crate::message::{CatalogKey, Message, OwnedCatalogKey};

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
    /// Translation (empty when untranslated). For plural entries this is
    /// `msgstr[0]`.
    pub msgstr: String,
    /// Additional plural translations (`msgstr[1]`, `msgstr[2]`, …).
    pub msgstrs_plural: Vec<String>,
    /// `file:line` locations collected by extraction tools.
    pub references: Vec<String>,
    /// Translator comments (`# …` lines).
    pub comments: Vec<String>,
    /// Format flags (`#, fuzzy, c-format, …`).
    pub flags: BTreeSet<String>,
    /// Previous msgid recorded by tools that renamed a string (`#| msgid`).
    pub previous_msgid: Option<String>,
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

    /// Whether the entry carries the `fuzzy` flag. Fuzzy translations are
    /// skipped by [`PoCatalog::translations`] and by compilation unless
    /// explicitly requested.
    #[must_use]
    pub fn is_fuzzy(&self) -> bool {
        self.flags.contains("fuzzy")
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
        if !self.flags.is_empty() {
            let _ = writeln!(
                out,
                "#, {}",
                self.flags
                    .clone()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if let Some(previous) = &self.previous_msgid {
            write_field(out, "#| msgid", previous, width);
        }
        if let Some(ctx) = &self.msgctxt {
            write_field(out, "msgctxt", ctx, width);
        }
        write_field(out, "msgid", &self.msgid, width);
        if let Some(plural) = &self.msgid_plural {
            write_field(out, "msgid_plural", plural, width);
            write_field(out, "msgstr[0]", &self.msgstr, width);
            for (i, plural_str) in self.msgstrs_plural.iter().enumerate() {
                write_field(out, &format!("msgstr[{}]", i + 1), plural_str, width);
            }
            return;
        }
        write_field(out, "msgstr", &self.msgstr, width);
    }

    /// Serializes this entry in obsolete form: every line prefixed with
    /// `#~`, never wrapped (matching gettext output).
    fn write_obsolete(&self, out: &mut String) {
        let mut body = String::new();
        if let Some(ctx) = &self.msgctxt {
            let _ = writeln!(body, "msgctxt \"{}\"", escape_po_string(ctx));
        }
        let _ = writeln!(body, "msgid \"{}\"", escape_po_string(&self.msgid));
        if let Some(plural) = &self.msgid_plural {
            let _ = writeln!(body, "msgid_plural \"{}\"", escape_po_string(plural));
            let _ = writeln!(body, "msgstr[0] \"{}\"", escape_po_string(&self.msgstr));
            for (i, plural_str) in self.msgstrs_plural.iter().enumerate() {
                let _ = writeln!(
                    body,
                    "msgstr[{}] \"{}\"",
                    i + 1,
                    escape_po_string(plural_str)
                );
            }
        } else {
            let _ = writeln!(body, "msgstr \"{}\"", escape_po_string(&self.msgstr));
        }
        for line in body.lines() {
            let _ = writeln!(out, "#~ {line}");
        }
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
    /// Obsolete entries (`#~`), preserved across merges so translations
    /// for removed strings are never lost.
    pub obsolete: Vec<PoEntry>,
}

impl PoCatalog {
    /// Parses a PO document.
    ///
    /// Plural entries keep every `msgstr[N]`: index 0 lands in
    /// [`PoEntry::msgstr`], higher indices in [`PoEntry::msgstrs_plural`].
    /// Obsolete (`#~`) entries are preserved in [`PoCatalog::obsolete`].
    /// Duplicate keys are tolerated (the last entry wins in
    /// [`PoCatalog::translations`]); use [`PoCatalog::parse_strict`] to
    /// reject them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PoParse`] when a line cannot be understood.
    pub fn parse(po: &str) -> Result<Self> {
        PoParser::default().run(po)
    }

    /// Like [`PoCatalog::parse`] but rejects catalogs containing two
    /// entries with the same `(msgctxt, msgid)` key.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PoParse`] for malformed lines and
    /// [`Error::DuplicateKey`] for repeated keys.
    pub fn parse_strict(po: &str) -> Result<Self> {
        PoParser {
            strict: true,
            ..PoParser::default()
        }
        .run(po)
    }

    /// Parses and returns the `(msgctxt, msgid) -> msgstr` lookup map,
    /// skipping untranslated, fuzzy and header entries.
    #[must_use]
    pub fn translations(&self) -> HashMap<OwnedCatalogKey, String> {
        self.translations_with(false)
    }

    /// Like [`PoCatalog::translations`], with control over fuzzy entries:
    /// `include_fuzzy` adds translations flagged `#, fuzzy`.
    #[must_use]
    pub fn translations_with(&self, include_fuzzy: bool) -> HashMap<OwnedCatalogKey, String> {
        self.entries
            .iter()
            .filter(|e| e.is_translated() && (include_fuzzy || !e.is_fuzzy()))
            .map(|e| (e.key(), e.msgstr.clone()))
            .collect()
    }

    /// Parses the header entry into typed metadata.
    #[must_use]
    pub fn metadata(&self) -> PoMetadata {
        PoMetadata::parse(&self.header)
    }

    /// Replaces the header entry with the serialization of `metadata`.
    pub fn set_metadata(&mut self, metadata: &PoMetadata) {
        self.header = metadata.to_header_string();
    }

    /// Updates the `Language:` header field, keeping all other fields.
    pub fn set_language(&mut self, language: &str) {
        let mut meta = self.metadata();
        meta.language = Some(language.to_string());
        self.set_metadata(&meta);
    }

    /// Returns the entry with the given key, if present.
    #[must_use]
    pub fn from_key(&self, key: CatalogKey<'_>) -> Option<&PoEntry> {
        self.entries
            .iter()
            .find(|e| e.msgid == key.msgid && e.msgctxt.as_deref() == key.msgctxt)
    }

    /// Returns a mutable reference to the entry with the given key.
    pub fn get_mut(&mut self, key: CatalogKey<'_>) -> Option<&mut PoEntry> {
        self.entries
            .iter_mut()
            .find(|e| e.msgid == key.msgid && e.msgctxt.as_deref() == key.msgctxt)
    }

    /// Sets the translation of the entry with the given key.
    ///
    /// Returns `false` when no entry matches (nothing is inserted;
    /// catalogs are keyed by extraction, see
    /// [`PoCatalog::from_messages`]).
    pub fn set_translation(&mut self, key: CatalogKey<'_>, msgstr: impl Into<String>) -> bool {
        match self.get_mut(key) {
            Some(entry) => {
                entry.msgstr = msgstr.into();
                true
            }
            None => false,
        }
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
    /// Obsolete entries are written last, `#~`-prefixed and unwrapped.
    #[must_use]
    pub fn to_po_string_with_width(&self, width: usize) -> String {
        let mut out = String::new();
        write_header(&mut out, &self.header);
        for entry in &self.entries {
            out.push('\n');
            entry.write_po(&mut out, width);
        }
        for entry in &self.obsolete {
            out.push('\n');
            entry.write_obsolete(&mut out);
        }
        out
    }

    /// Builds a catalog from extracted messages, preserving translations
    /// (and references/comments/flags/plural forms) of entries that still
    /// exist.
    ///
    /// Strings present in `existing` but no longer in `messages` become
    /// obsolete instead of being dropped, so an `extract --merge` cycle
    /// never loses translations; strings that come back are resurrected
    /// with their old translation.
    #[must_use]
    pub fn from_messages_with_merge(messages: &[Message], existing: Option<&Self>) -> Self {
        let header =
            existing.map_or_else(|| default_header(None), |catalog| catalog.header.clone());
        let mut catalog = Self {
            header,
            entries: Vec::new(),
            obsolete: Vec::new(),
        };
        let existing_map: HashMap<OwnedCatalogKey, &PoEntry> = existing
            .map(|c| {
                c.entries
                    .iter()
                    .chain(c.obsolete.iter())
                    .map(|e| (e.key(), e))
                    .collect()
            })
            .unwrap_or_default();
        let live_keys: HashSet<OwnedCatalogKey> = messages
            .iter()
            .map(|m| OwnedCatalogKey::new(m.msgctxt.clone(), m.msgid.clone()))
            .collect();

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
                    msgstrs_plural: old.msgstrs_plural.clone(),
                    references: refs,
                    comments: old.comments.clone(),
                    flags: old.flags.clone(),
                    previous_msgid: old.previous_msgid.clone(),
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
        if let Some(existing) = existing {
            // Strings that disappeared become obsolete instead of being
            // dropped, mirroring gettext `msgmerge` behavior.
            for entry in &existing.entries {
                if !entry.is_header() && !live_keys.contains(&entry.key()) {
                    catalog.obsolete.push(entry.clone());
                }
            }
            for entry in &existing.obsolete {
                if !live_keys.contains(&entry.key()) {
                    catalog.obsolete.push(entry.clone());
                }
            }
        }
        catalog
    }

    /// Builds a catalog from extracted messages without an existing one.
    #[must_use]
    pub fn from_messages(messages: &[Message]) -> Self {
        Self::from_messages_with_merge(messages, None)
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

/// Assembly state for one obsolete (`#~`) entry.
#[derive(Default)]
struct ObsoleteState {
    /// Entry being assembled from `#~` lines.
    current: Option<PoEntry>,
    /// Which field continuation lines append to.
    last_field: Option<LastField>,
    /// True once a `msgstr`/`msgstr[N]` was seen for the entry.
    saw_msgstr: bool,
}

/// Incremental state for parsing one PO document.
#[derive(Default)]
struct PoParser {
    catalog: PoCatalog,
    current: PoEntry,
    have_current: bool,
    /// Line where the current entry started (for duplicate reporting).
    current_line: usize,
    /// True once `msgstr` (or `msgstr[0]`) was seen for the current entry;
    /// a new `msgid` after that starts the next entry.
    saw_msgstr: bool,
    /// Which field continuation lines append to.
    last_field: Option<LastField>,
    /// Comments/references seen before an entry (or after the header);
    /// they belong to the *next* entry.
    pending_comments: Vec<String>,
    pending_refs: Vec<String>,
    pending_flags: BTreeSet<String>,
    pending_previous: Option<String>,
    /// Obsolete entries being assembled from `#~` lines.
    obsolete: ObsoleteState,
    /// Reject duplicate keys.
    strict: bool,
    seen_keys: HashSet<OwnedCatalogKey>,
    /// First duplicate detected in strict mode.
    strict_error: Option<Error>,
}

impl PoParser {
    fn run(mut self, po: &str) -> Result<PoCatalog> {
        for (idx, raw_line) in po.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = raw_line.trim_end_matches('\r').trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("#~") {
                self.handle_obsolete_line(rest, line_no)?;
                continue;
            }
            if trimmed.starts_with('#') {
                self.handle_comment(trimmed);
                continue;
            }
            // A non-obsolete field line closes any pending obsolete entry.
            self.flush_obsolete();
            self.handle_field(trimmed, line_no)?;
        }
        if self.have_current {
            self.flush_entry();
        }
        self.flush_obsolete();
        if let Some(err) = self.strict_error {
            return Err(err);
        }
        Ok(self.catalog)
    }

    fn handle_comment(&mut self, trimmed: &str) {
        if let Some(rest) = trimmed.strip_prefix("#:") {
            self.pending_refs
                .extend(rest.split_whitespace().map(str::to_string));
        } else if let Some(rest) = trimmed.strip_prefix("#,") {
            for flag in rest.split(',') {
                let flag = flag.trim();
                if !flag.is_empty() {
                    self.pending_flags.insert(flag.to_string());
                }
            }
        } else if let Some(rest) = trimmed.strip_prefix("#|") {
            let rest = rest.trim();
            if let Some(value) = rest.strip_prefix("msgid ") {
                let piece = unescape_po_string(value);
                match &mut self.pending_previous {
                    Some(previous) => previous.push_str(&piece),
                    None => self.pending_previous = Some(piece),
                }
            }
            // `#| msgctxt`/`#| msgid_plural` are informational; skipped.
        } else {
            self.pending_comments
                .push(trimmed.trim_start_matches('#').trim().to_string());
        }
    }

    fn handle_obsolete_line(&mut self, rest: &str, line_no: usize) -> Result<()> {
        let rest = rest.trim();
        if rest.is_empty() {
            return Ok(());
        }
        // A new entry begins at `msgid` (or at `msgctxt`, which gettext
        // writes before `msgid`); flush the previous one when complete.
        let starts_entry = rest.starts_with("msgid ") || rest.starts_with("msgctxt ");
        if starts_entry && self.obsolete.current.is_some() && self.obsolete.saw_msgstr {
            self.flush_obsolete();
        }
        if let Some(value) = rest.strip_prefix("msgid ") {
            let entry = self.obsolete.current.get_or_insert_with(PoEntry::default);
            entry.msgid = unescape_po_string(value);
            self.obsolete.last_field = Some(LastField::Msgid);
            self.obsolete.saw_msgstr = false;
            return Ok(());
        }
        if let Some(value) = rest.strip_prefix("msgctxt ") {
            let entry = self.obsolete.current.get_or_insert_with(PoEntry::default);
            entry.msgctxt = Some(unescape_po_string(value));
            self.obsolete.last_field = Some(LastField::Msgctxt);
            return Ok(());
        }
        if self.obsolete.current.is_none() {
            return Err(Error::PoParse {
                line: line_no,
                message: format!("obsolete continuation outside of an entry: {rest:?}"),
            });
        }
        if let Some(value) = rest.strip_prefix("msgid_plural ") {
            self.obsolete
                .current
                .as_mut()
                .expect("checked")
                .msgid_plural = Some(unescape_po_string(value));
            self.obsolete.last_field = Some(LastField::MsgidPlural);
        } else if let Some((index, value)) = parse_plural_field(rest) {
            let entry = self.obsolete.current.as_mut().expect("checked");
            store_plural(entry, index, unescape_po_string(value));
            self.obsolete.saw_msgstr = true;
            self.obsolete.last_field = Some(LastField::MsgstrPlural(index));
        } else if let Some(value) = rest.strip_prefix("msgstr ") {
            self.obsolete.current.as_mut().expect("checked").msgstr = unescape_po_string(value);
            self.obsolete.saw_msgstr = true;
            self.obsolete.last_field = Some(LastField::Msgstr);
        } else if rest.starts_with('"') {
            let value = unescape_po_string(rest);
            let entry = self.obsolete.current.as_mut().expect("checked");
            match self.obsolete.last_field {
                Some(LastField::Msgctxt) => {
                    entry
                        .msgctxt
                        .get_or_insert_with(String::new)
                        .push_str(&value);
                }
                Some(LastField::Msgid) => entry.msgid.push_str(&value),
                Some(LastField::MsgidPlural) => {
                    entry
                        .msgid_plural
                        .get_or_insert_with(String::new)
                        .push_str(&value);
                }
                Some(LastField::Msgstr) => entry.msgstr.push_str(&value),
                Some(LastField::MsgstrPlural(index)) => {
                    if let Some(target) = plural_slot_mut(entry, index) {
                        target.push_str(&value);
                    }
                }
                None => {
                    return Err(Error::PoParse {
                        line: line_no,
                        message: "obsolete continuation before any msgid/msgstr".to_string(),
                    });
                }
            }
        } else {
            return Err(Error::PoParse {
                line: line_no,
                message: format!("unexpected obsolete line: {rest:?}"),
            });
        }
        Ok(())
    }

    fn flush_obsolete(&mut self) {
        if let Some(entry) = self.obsolete.current.take() {
            if !entry.is_header() {
                self.catalog.obsolete.push(entry);
            }
        }
        self.obsolete.last_field = None;
        self.obsolete.saw_msgstr = false;
    }

    fn start_entry(&mut self, msgctxt: Option<String>, line_no: usize) {
        if self.have_current {
            self.flush_entry();
        }
        self.current = PoEntry {
            msgctxt,
            comments: std::mem::take(&mut self.pending_comments),
            references: std::mem::take(&mut self.pending_refs),
            flags: std::mem::take(&mut self.pending_flags),
            previous_msgid: self.pending_previous.take(),
            ..PoEntry::default()
        };
        self.have_current = true;
        self.saw_msgstr = false;
        self.current_line = line_no;
    }

    /// Moves the finished entry into the catalog, enforcing strict
    /// duplicate detection.
    fn flush_entry(&mut self) {
        if !self.have_current {
            return;
        }
        let entry = std::mem::take(&mut self.current);
        self.have_current = false;
        if entry.is_header() {
            self.catalog.header = entry.msgstr;
            return;
        }
        if self.strict {
            let key = entry.key();
            if !self.seen_keys.insert(key) {
                // Strict mode reports the duplicate; keep parsing short.
                // The error is surfaced by `run` via `strict_error`.
                if self.strict_error.is_none() {
                    self.strict_error = Some(Error::DuplicateKey {
                        msgid: entry.msgid.clone(),
                        msgctxt: entry.msgctxt.clone(),
                        line: self.current_line,
                    });
                }
            }
        }
        self.catalog.entries.push(entry);
    }

    fn handle_field(&mut self, trimmed: &str, line_no: usize) -> Result<()> {
        if let Some(rest) = trimmed.strip_prefix("msgctxt ") {
            self.start_entry(Some(unescape_po_string(rest)), line_no);
            self.last_field = Some(LastField::Msgctxt);
        } else if let Some(rest) = trimmed.strip_prefix("msgid ") {
            if !self.have_current || self.saw_msgstr {
                self.start_entry(None, line_no);
            }
            self.current.msgid = unescape_po_string(rest);
            self.last_field = Some(LastField::Msgid);
        } else if let Some(rest) = trimmed.strip_prefix("msgid_plural ") {
            require_entry(self.have_current, line_no)?;
            self.current.msgid_plural = Some(unescape_po_string(rest));
            self.last_field = Some(LastField::MsgidPlural);
        } else if let Some((index, value)) = parse_plural_field(trimmed) {
            require_entry(self.have_current, line_no)?;
            store_plural(&mut self.current, index, unescape_po_string(value));
            self.saw_msgstr = true;
            self.last_field = Some(LastField::MsgstrPlural(index));
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
            Some(LastField::MsgstrPlural(index)) => {
                if let Some(target) = plural_slot_mut(&mut self.current, index) {
                    target.push_str(&value);
                }
            }
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
    /// Continuations of `msgstr[N]` with `N >= 1`.
    MsgstrPlural(usize),
}

/// Splits `msgstr[N] "value"` into `(N, "value")`, if the line matches.
fn parse_plural_field(line: &str) -> Option<(usize, &str)> {
    let rest = line.strip_prefix("msgstr[")?;
    let close = rest.find(']')?;
    let index: usize = rest[..close].parse().ok()?;
    let value = rest[close + 1..].strip_prefix(' ')?;
    Some((index, value))
}

/// Stores a plural translation, filling gaps with empty strings.
fn store_plural(entry: &mut PoEntry, index: usize, value: String) {
    if index == 0 {
        entry.msgstr = value;
        return;
    }
    let slot = index - 1;
    while entry.msgstrs_plural.len() <= slot {
        entry.msgstrs_plural.push(String::new());
    }
    entry.msgstrs_plural[slot] = value;
}

/// Mutable access to the translation for plural index `index`.
fn plural_slot_mut(entry: &mut PoEntry, index: usize) -> Option<&mut String> {
    if index == 0 {
        return Some(&mut entry.msgstr);
    }
    entry.msgstrs_plural.get_mut(index - 1)
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

/// Typed view over the PO header entry.
///
/// Standard gettext fields get named accessors; unknown `X-…` fields are
/// preserved in [`PoMetadata::extended`] so a parse → serialize round
/// trip never loses information.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(
    any(feature = "serde", target_arch = "wasm32"),
    derive(serde::Serialize, serde::Deserialize)
)]
pub struct PoMetadata {
    /// `Project-Id-Version`
    pub project_id_version: Option<String>,
    /// `Report-Msgid-Bugs-To`
    pub report_msgid_bugs_to: Option<String>,
    /// `POT-Creation-Date`
    pub pot_creation_date: Option<String>,
    /// `PO-Revision-Date`
    pub po_revision_date: Option<String>,
    /// `Last-Translator`
    pub last_translator: Option<String>,
    /// `Language-Team`
    pub language_team: Option<String>,
    /// `Language`
    pub language: Option<String>,
    /// `MIME-Version`
    pub mime_version: Option<String>,
    /// `Content-Type`
    pub content_type: Option<String>,
    /// `Content-Transfer-Encoding`
    pub content_transfer_encoding: Option<String>,
    /// `Plural-Forms`
    pub plural_forms: Option<String>,
    /// Unknown fields, in document order.
    pub extended: Vec<(String, String)>,
}

impl PoMetadata {
    /// Parses a header value (the `msgstr` of the `msgid ""` entry).
    #[must_use]
    pub fn parse(header: &str) -> Self {
        let mut meta = Self::default();
        for line in header.split('\n') {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                meta.extended.push((String::new(), line.to_string()));
                continue;
            };
            let value = value.strip_prefix(' ').unwrap_or(value).to_string();
            match key {
                "Project-Id-Version" => meta.project_id_version = Some(value),
                "Report-Msgid-Bugs-To" => meta.report_msgid_bugs_to = Some(value),
                "POT-Creation-Date" => meta.pot_creation_date = Some(value),
                "PO-Revision-Date" => meta.po_revision_date = Some(value),
                "Last-Translator" => meta.last_translator = Some(value),
                "Language-Team" => meta.language_team = Some(value),
                "Language" => meta.language = Some(value),
                "MIME-Version" => meta.mime_version = Some(value),
                "Content-Type" => meta.content_type = Some(value),
                "Content-Transfer-Encoding" => meta.content_transfer_encoding = Some(value),
                "Plural-Forms" => meta.plural_forms = Some(value),
                _ => meta.extended.push((key.to_string(), value)),
            }
        }
        meta
    }

    /// Serializes back to the header value, one `Key: value\n` per field.
    #[must_use]
    pub fn to_header_string(&self) -> String {
        let fields = [
            ("Project-Id-Version", &self.project_id_version),
            ("Report-Msgid-Bugs-To", &self.report_msgid_bugs_to),
            ("POT-Creation-Date", &self.pot_creation_date),
            ("PO-Revision-Date", &self.po_revision_date),
            ("Last-Translator", &self.last_translator),
            ("Language-Team", &self.language_team),
            ("Language", &self.language),
            ("MIME-Version", &self.mime_version),
            ("Content-Type", &self.content_type),
            ("Content-Transfer-Encoding", &self.content_transfer_encoding),
            ("Plural-Forms", &self.plural_forms),
        ];
        let mut out = String::new();
        for (key, value) in fields {
            if let Some(value) = value {
                let _ = writeln!(out, "{key}: {value}");
            }
        }
        for (key, value) in &self.extended {
            let _ = writeln!(out, "{key}: {value}");
        }
        out
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
            obsolete: Vec::new(),
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
            obsolete: Vec::new(),
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

    // ------------------------------------------------------------------
    // v0.3.0: lossless PO layer.
    // ------------------------------------------------------------------

    #[test]
    fn plural_forms_roundtrip() {
        let po = "msgid \"one\"\nmsgid_plural \"many\"\nmsgstr[0] \"uno\"\nmsgstr[1] \"muchos\"\nmsgstr[2] \"muchisimos\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        let entry = &catalog.entries[0];
        assert_eq!(entry.msgstr, "uno");
        assert_eq!(entry.msgstrs_plural, vec!["muchos", "muchisimos"]);

        let out = catalog.to_po_string();
        let reparsed = PoCatalog::parse(&out).unwrap();
        assert_eq!(reparsed.entries, catalog.entries);
    }

    #[test]
    fn plural_continuation_lines_are_parsed() {
        let po = "msgid \"one\"\nmsgid_plural \"many\"\nmsgstr[0] \"\"\n\"uno largo \"\n\"que sigue\"\nmsgstr[1] \"muchos\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(catalog.entries[0].msgstr, "uno largo que sigue");
        assert_eq!(catalog.entries[0].msgstrs_plural, vec!["muchos"]);
    }

    #[test]
    fn flags_parse_and_serialize() {
        let po = "#, fuzzy, c-format\nmsgid \"a\"\nmsgstr \"A\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        let entry = &catalog.entries[0];
        assert!(entry.is_fuzzy());
        assert!(entry.flags.contains("c-format"));
        assert!(!entry.flags.contains("fuzz"));

        let out = catalog.to_po_string();
        assert!(out.contains("#, c-format, fuzzy"), "{out}");
        let reparsed = PoCatalog::parse(&out).unwrap();
        assert_eq!(reparsed.entries, catalog.entries);
    }

    #[test]
    fn translations_skip_fuzzy_by_default() {
        let po = "#, fuzzy\nmsgid \"a\"\nmsgstr \"A\"\n\nmsgid \"b\"\nmsgstr \"B\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        let map = catalog.translations();
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&OwnedCatalogKey::new(None, "b"))
                .map(String::as_str),
            Some("B")
        );

        let with_fuzzy = catalog.translations_with(true);
        assert_eq!(with_fuzzy.len(), 2);
    }

    #[test]
    fn obsolete_entries_roundtrip() {
        let po = "#~ msgid \"old\"\n#~ msgstr \"viejo\"\n\nmsgid \"live\"\nmsgstr \"vivo\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.obsolete.len(), 1);
        assert_eq!(catalog.obsolete[0].msgid, "old");
        assert_eq!(catalog.obsolete[0].msgstr, "viejo");

        let out = catalog.to_po_string();
        assert!(out.contains("#~ msgid \"old\""), "{out}");
        assert!(out.contains("#~ msgstr \"viejo\""), "{out}");
        let reparsed = PoCatalog::parse(&out).unwrap();
        assert_eq!(reparsed.obsolete, catalog.obsolete);
        assert_eq!(reparsed.entries, catalog.entries);
    }

    #[test]
    fn obsolete_plural_and_msgctxt_roundtrip() {
        let po = "#~ msgctxt \"ctx\"\n#~ msgid \"one\"\n#~ msgid_plural \"many\"\n#~ msgstr[0] \"uno\"\n#~ msgstr[1] \"muchos\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(catalog.obsolete.len(), 1);
        let entry = &catalog.obsolete[0];
        assert_eq!(entry.msgctxt.as_deref(), Some("ctx"));
        assert_eq!(entry.msgstrs_plural, vec!["muchos"]);
        let reparsed = PoCatalog::parse(&catalog.to_po_string()).unwrap();
        assert_eq!(reparsed.obsolete, catalog.obsolete);
    }

    #[test]
    fn merge_keeps_obsolete_entries_and_resurrects_translations() {
        // An obsolete entry holds the old translation; the string comes
        // back to life in the sources.
        let existing = PoCatalog::parse(
            "#~ msgid \"Back\"\n#~ msgstr \"Vuelta\"\n\nmsgid \"Stay\"\nmsgstr \"Queda\"\n",
        )
        .unwrap();
        let messages = vec![msg("Back", &[("a.html", 1)]), msg("Stay", &[("a.html", 2)])];
        let merged = PoCatalog::from_messages_with_merge(&messages, Some(&existing));
        assert!(
            merged.obsolete.is_empty(),
            "resurrected entry must not stay obsolete"
        );
        let back = merged.entries.iter().find(|e| e.msgid == "Back").unwrap();
        assert_eq!(back.msgstr, "Vuelta");

        // A string that disappears becomes obsolete instead of being lost.
        let existing =
            PoCatalog::parse("msgid \"Gone\"\nmsgstr \"Ido\"\n\nmsgid \"Keep\"\nmsgstr \"Keep\"\n")
                .unwrap();
        let messages = vec![msg("Keep", &[("a.html", 1)])];
        let merged = PoCatalog::from_messages_with_merge(&messages, Some(&existing));
        assert_eq!(merged.entries.len(), 1);
        assert_eq!(merged.obsolete.len(), 1);
        assert_eq!(merged.obsolete[0].msgid, "Gone");
        assert_eq!(merged.obsolete[0].msgstr, "Ido");
    }

    #[test]
    fn previous_msgid_parse_and_serialize() {
        let po = "#, fuzzy\n#| msgid \"old text\"\nmsgid \"new text\"\nmsgstr \"nuevo\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        assert_eq!(
            catalog.entries[0].previous_msgid.as_deref(),
            Some("old text")
        );
        let out = catalog.to_po_string();
        assert!(out.contains("#| msgid \"old text\""), "{out}");
        let reparsed = PoCatalog::parse(&out).unwrap();
        assert_eq!(reparsed.entries, catalog.entries);
    }

    #[test]
    fn metadata_parses_known_and_extended_fields() {
        let header = "Project-Id-Version: demo 1.0\nLanguage: fr\nPlural-Forms: nplurals=2; plural=(n > 1);\nX-Custom: keepme\n";
        let meta = PoMetadata::parse(header);
        assert_eq!(meta.project_id_version.as_deref(), Some("demo 1.0"));
        assert_eq!(meta.language.as_deref(), Some("fr"));
        assert_eq!(
            meta.plural_forms.as_deref(),
            Some("nplurals=2; plural=(n > 1);")
        );
        assert_eq!(
            meta.extended,
            vec![("X-Custom".to_string(), "keepme".to_string())]
        );
        assert_eq!(meta.to_header_string(), header);
    }

    #[test]
    fn catalog_metadata_accessors() {
        let mut catalog =
            PoCatalog::parse("msgid \"\"\nmsgstr \"\"\n\"Project-Id-Version: base\\n\"\n").unwrap();
        let mut meta = catalog.metadata();
        assert_eq!(meta.project_id_version.as_deref(), Some("base"));
        assert_eq!(meta.language, None);

        meta.language = Some("de".to_string());
        catalog.set_metadata(&meta);
        assert_eq!(catalog.metadata().language.as_deref(), Some("de"));
        assert_eq!(
            catalog.metadata().project_id_version.as_deref(),
            Some("base")
        );

        catalog.set_language("es");
        assert_eq!(catalog.metadata().language.as_deref(), Some("es"));
        assert_eq!(
            catalog.metadata().project_id_version.as_deref(),
            Some("base")
        );
    }

    #[test]
    fn parse_strict_rejects_duplicates() {
        let po = "msgid \"a\"\nmsgstr \"A\"\n\nmsgid \"a\"\nmsgstr \"A2\"\n";
        let err = PoCatalog::parse_strict(po).unwrap_err();
        assert!(matches!(err, Error::DuplicateKey { .. }), "{err}");
        // Lenient parse still succeeds (last one wins in the map).
        assert!(PoCatalog::parse(po).is_ok());

        let ok = PoCatalog::parse_strict(
            "msgctxt \"c\"\nmsgid \"a\"\nmsgstr \"A\"\n\nmsgid \"a\"\nmsgstr \"B\"\n",
        )
        .unwrap();
        assert_eq!(ok.entries.len(), 2);
    }

    #[test]
    fn catalog_lookup_helpers() {
        let mut catalog = PoCatalog::parse(
            "msgid \"a\"\nmsgstr \"\"\n\nmsgctxt \"c\"\nmsgid \"a\"\nmsgstr \"\"\n",
        )
        .unwrap();
        assert_eq!(
            catalog.from_key(CatalogKey::new(None, "a")).unwrap().msgid,
            "a"
        );
        assert_eq!(
            catalog
                .from_key(CatalogKey::new(Some("c"), "a"))
                .unwrap()
                .msgid,
            "a"
        );
        assert!(catalog.from_key(CatalogKey::new(None, "zzz")).is_none());

        assert!(catalog.set_translation(CatalogKey::new(None, "a"), "A!"));
        assert!(catalog.set_translation(CatalogKey::new(Some("c"), "a"), "ctx-A!"));
        assert!(!catalog.set_translation(CatalogKey::new(None, "zzz"), "nope"));
        assert_eq!(catalog.entries[0].msgstr, "A!");
        assert_eq!(catalog.entries[1].msgstr, "ctx-A!");

        // get_mut for in-place edits.
        if let Some(entry) = catalog.get_mut(CatalogKey::new(None, "a")) {
            entry.flags.insert("fuzzy".to_string());
        }
        assert!(catalog.entries[0].is_fuzzy());
    }

    #[test]
    fn from_messages_convenience_matches_none_merge() {
        let messages = vec![msg("Hello", &[("a.html", 1)])];
        let a = PoCatalog::from_messages(&messages);
        let b = PoCatalog::from_messages_with_merge(&messages, None);
        assert_eq!(a, b);
        assert_eq!(a.entries[0].msgstr, "");
    }

    #[test]
    fn merge_keeps_flags_and_plural_forms() {
        let existing = PoCatalog::parse(
            "#, c-format\nmsgid \"Hello\"\nmsgid_plural \"Hellos\"\nmsgstr[0] \"Hola\"\nmsgstr[1] \"Holass\"\n",
        )
        .unwrap();
        let messages = vec![msg("Hello", &[("b.html", 1)])];
        let merged = PoCatalog::from_messages_with_merge(&messages, Some(&existing));
        let entry = &merged.entries[0];
        assert!(entry.flags.contains("c-format"));
        assert_eq!(entry.msgstrs_plural, vec!["Holass"]);
    }
}
