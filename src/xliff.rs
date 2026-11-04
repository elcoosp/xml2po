//! XLIFF 1.2 import and export (behind the `xliff` feature).
//!
//! XLIFF is the interchange format of professional translation tools
//! (Trados, memoQ, Lokalise, Transifex, …). This module bridges PO
//! catalogs to XLIFF 1.2 so `xml2po` output can be finished in a CAT
//! tool and merged back.
//!
//! Mapping overview:
//!
//! | PO                                   | XLIFF 1.2                              |
//! |--------------------------------------|----------------------------------------|
//! | `msgid`                              | `<trans-unit>/<source>`                |
//! | `msgstr`                             | `<trans-unit>/<target>` (when set)     |
//! | `msgctxt`                            | `resname` attribute                    |
//! | inline `<tag>` placeholders          | `<bpt>`/`<ept>` pairs, `<ph>` for self-closing |
//! | `#:` references                      | `<context-group>` (`sourcefile`/`linenumber`) |
//! | translator comments                  | `<note>`                               |
//!
//! Plural entries export only their `msgstr[0]`; obsolete entries are
//! not exported. `#, fuzzy` maps to `translate="no"` on the unit.
//!
//! # Example
//!
//! ```no_run
//! use xml2po::{extract_messages, get_strategy};
//! use xml2po::po::PoCatalog;
//!
//! # fn main() -> Result<(), xml2po::Error> {
//! let content = "<p>Hi there</p>";
//! let strategy = get_strategy("html")?;
//! let messages = extract_messages(content, "page.html", strategy)?;
//! let catalog = PoCatalog::from_messages(&messages);
//!
//! let xliff = xml2po::catalog_to_xliff(&catalog, "en", Some("fr"));
//! // … hand the XLIFF to a CAT tool, then read it back:
//! let imported = xml2po::catalog_from_xliff(&xliff)?;
//! assert_eq!(imported.entries[0].msgid, "Hi there");
//! # Ok(())
//! # }
//! ```

use crate::error::{Error, Result};
use crate::extract::literal_or_unescape;
use crate::message::{CatalogKey, Message, OwnedCatalogKey, SourceRef};
use crate::po::{PoCatalog, PoEntry};
use quick_xml::Reader;
use quick_xml::escape::escape;
use quick_xml::events::Event;
use std::fmt::Write as _;

/// One inline code in a PO string: a placeholder tag form.
#[derive(Debug, Clone, PartialEq, Eq)]
enum InlineCode {
    /// `<tag>`
    Begin(String),
    /// `</tag>`
    End(String),
    /// `<tag/>`
    Standalone(String),
}

impl InlineCode {
    /// The XLIFF element name (`bpt`, `ept` or `ph`).
    fn element(&self) -> &'static str {
        match self {
            Self::Begin(_) => "bpt",
            Self::End(_) => "ept",
            Self::Standalone(_) => "ph",
        }
    }

    /// The literal tag text, escaped for inclusion in the XLIFF body.
    fn escaped(&self) -> String {
        match self {
            Self::Begin(tag) => escape(format!("<{tag}>")).into_owned(),
            Self::End(tag) => escape(format!("</{tag}>")).into_owned(),
            Self::Standalone(tag) => escape(format!("<{tag}/>")).into_owned(),
        }
    }
}

/// Serializes a PO string into XLIFF inline content, mapping inline
/// placeholder tags to `bpt`/`ept`/`ph` codes with sequential ids.
fn po_string_to_xliff_content(value: &str, out: &mut String) {
    let mut id = 0usize;
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        let (text, tail) = rest.split_at(open);
        push_escaped_text(out, text);
        if let Some((code, consumed)) = parse_inline_code(tail) {
            id += 1;
            let elem = code.element();
            let body = code.escaped();
            let _ = write!(out, "<{elem} id=\"{id}\">{body}</{elem}>");
            rest = &tail[consumed..];
        } else {
            // A `<` that is not a placeholder tag: literal text.
            push_escaped_text(out, "<");
            rest = &tail[1..];
        }
    }
    push_escaped_text(out, rest);
}

/// Appends `text` with XML escaping (empty text produces nothing).
fn push_escaped_text(out: &mut String, text: &str) {
    if !text.is_empty() {
        out.push_str(&escape(text));
    }
}

/// Parses one inline placeholder at the start of `text`, returning the
/// code and the number of bytes consumed.
fn parse_inline_code(text: &str) -> Option<(InlineCode, usize)> {
    let bytes = text.as_bytes();
    let mut i = 1; // skip '<'
    let close = i < bytes.len() && bytes[i] == b'/';
    if close {
        i += 1;
    }
    let name_start = i;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric()
            || bytes[i] == b':'
            || bytes[i] == b'_'
            || bytes[i] == b'-')
    {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    let tag = &text[name_start..i];
    // Attributes (copied verbatim tags keep theirs) run until `>`.
    while i < bytes.len() && bytes[i] != b'>' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let self_closing = text[..i].ends_with('/');
    let consumed = i + 1;
    let code = match (close, self_closing) {
        (true, _) => InlineCode::End(tag.to_string()),
        (false, true) => InlineCode::Standalone(tag.to_string()),
        (false, false) => InlineCode::Begin(tag.to_string()),
    };
    Some((code, consumed))
}

/// Exports a catalog to an XLIFF 1.2 document.
///
/// `source_language` is written as the file's `source-language`;
/// `target_language` (when given) as `target-language`. Untranslated
/// entries get no `<target>`, fuzzy entries are marked
/// `translate="no"`.
#[must_use]
pub fn catalog_to_xliff(
    catalog: &PoCatalog,
    source_language: &str,
    target_language: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<xliff xmlns=\"urn:oasis:names:tc:xliff:document:1.2\" version=\"1.2\">\n");
    let _ = write!(
        out,
        "  <file original=\"messages.po\" source-language=\"{}\"",
        escape(source_language)
    );
    if let Some(target) = target_language {
        let _ = write!(out, " target-language=\"{}\"", escape(target));
    }
    out.push_str(" datatype=\"plaintext\">\n");
    out.push_str("    <body>\n");

    for (index, entry) in catalog.entries.iter().enumerate() {
        let id = index + 1;
        out.push_str(&format_entry(id, entry));
    }

    out.push_str("    </body>\n  </file>\n</xliff>\n");
    out
}

/// Serializes one entry as a `trans-unit`.
fn format_entry(id: usize, entry: &PoEntry) -> String {
    let mut unit = String::new();
    let _ = write!(unit, "      <trans-unit id=\"{id}\"");
    if let Some(ctx) = &entry.msgctxt {
        let _ = write!(unit, " resname=\"{}\"", escape(ctx));
    }
    if entry.is_fuzzy() {
        unit.push_str(" translate=\"no\"");
    }
    unit.push_str(" xml:space=\"preserve\">\n");

    unit.push_str("        <source>");
    po_string_to_xliff_content(&entry.msgid, &mut unit);
    unit.push_str("</source>\n");

    if entry.is_translated() {
        unit.push_str("        <target>");
        po_string_to_xliff_content(&entry.msgstr, &mut unit);
        unit.push_str("</target>\n");
    }

    let references = grouped_references(&entry.references);
    if !references.is_empty() {
        unit.push_str("        <context-group name=\"po-references\">\n");
        for (file, line) in references {
            let sourcefile = escape(&file);
            let _ = writeln!(
                unit,
                "          <context context-type=\"sourcefile\">{sourcefile}</context>"
            );
            let _ = writeln!(
                unit,
                "          <context context-type=\"linenumber\">{line}</context>"
            );
        }
        unit.push_str("        </context-group>\n");
    }

    for comment in &entry.comments {
        let note = escape(comment);
        let _ = writeln!(unit, "        <note from=\"translator\">{note}</note>");
    }

    unit.push_str("      </trans-unit>\n");
    unit
}

/// Splits `file:line` reference strings into `(file, line)` pairs;
/// unparsable entries keep `line` empty.
fn grouped_references(references: &[String]) -> Vec<(String, String)> {
    references
        .iter()
        .map(|reference| match reference.rsplit_once(':') {
            Some((file, line)) if !file.is_empty() && line.chars().all(|c| c.is_ascii_digit()) => {
                (file.to_string(), line.to_string())
            }
            _ => (reference.clone(), String::new()),
        })
        .collect()
}

/// Imports an XLIFF 1.2 document into a catalog.
///
/// `resname` becomes the `msgctxt`; `<bpt>`/`<ept>`/`<ph>` codes are
/// turned back into inline placeholder tags; `context-group` entries
/// become `file:line` references and `<note>` translator comments.
///
/// # Errors
///
/// Returns [`crate::Error::PoParse`] when the document is not well-formed
/// XLIFF (no `trans-unit`s can be read from it).
pub fn catalog_from_xliff(xliff: &str) -> Result<PoCatalog> {
    parse_xliff(xliff)
}

/// Which buffer the current character data belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    None,
    Source,
    Target,
    Note,
    Sourcefile,
    Linenumber,
    /// Inside `<bpt>`/`<ept>`/`<ph>` within a source.
    CodeInSource,
    /// Inside `<bpt>`/`<ept>`/`<ph>` within a target.
    CodeInTarget,
}

/// Streaming XLIFF 1.2 reader state.
struct XliffReader {
    catalog: PoCatalog,
    saw_unit: bool,
    source_language: Option<String>,
    target_language: Option<String>,
    slot: Slot,
    resname: Option<String>,
    msgid: String,
    msgstr: String,
    comments: Vec<String>,
    references: Vec<String>,
    note: String,
    pending_file: String,
    pending_line: String,
}

impl XliffReader {
    fn new() -> Self {
        Self {
            catalog: PoCatalog::default(),
            saw_unit: false,
            source_language: None,
            target_language: None,
            slot: Slot::None,
            resname: None,
            msgid: String::new(),
            msgstr: String::new(),
            comments: Vec::new(),
            references: Vec::new(),
            note: String::new(),
            pending_file: String::new(),
            pending_line: String::new(),
        }
    }

    fn handle_start(&mut self, element: &str, attrs: &[(String, String)]) {
        if element == "file" {
            for (name, value) in attrs {
                match name.as_str() {
                    "source-language" => self.source_language = Some(value.clone()),
                    "target-language" => self.target_language = Some(value.clone()),
                    _ => {}
                }
            }
        }
        match element {
            "trans-unit" => {
                self.saw_unit = true;
                self.resname = None;
                self.msgid.clear();
                self.msgstr.clear();
                self.comments.clear();
                self.references.clear();
                self.note.clear();
                self.pending_file.clear();
                self.pending_line.clear();
                self.slot = Slot::None;
                for (name, value) in attrs {
                    if name == "resname" {
                        let value = value.trim();
                        self.resname = (!value.is_empty()).then(|| value.to_string());
                    }
                }
            }
            "source" => self.slot = Slot::Source,
            "target" => self.slot = Slot::Target,
            "note" => {
                self.note.clear();
                self.slot = Slot::Note;
            }
            "bpt" | "ept" | "ph" => {
                self.slot = match self.slot {
                    Slot::Source => Slot::CodeInSource,
                    Slot::Target => Slot::CodeInTarget,
                    other => other,
                };
            }
            "context" => {
                let kind = attrs
                    .iter()
                    .find(|(name, _)| name == "context-type")
                    .map_or(String::new(), |(_, value)| value.clone());
                self.slot = match kind.as_str() {
                    "sourcefile" => Slot::Sourcefile,
                    "linenumber" => Slot::Linenumber,
                    _ => Slot::None,
                };
            }
            _ => self.slot = Slot::None,
        }
    }

    fn handle_text(&mut self, raw: &str) {
        match self.slot {
            Slot::Source => {
                // XLIFF text is XML-escaped; msgids keep inline tags
                // literal and escape everything else.
                let plain = literal_or_unescape(raw);
                self.msgid
                    .push_str(&crate::extract::escape_inline_text(&plain));
            }
            Slot::CodeInSource => self.msgid.push_str(&literal_or_unescape(raw)),
            Slot::Target => {
                let plain = literal_or_unescape(raw);
                self.msgstr
                    .push_str(&crate::extract::escape_inline_text(&plain));
            }
            Slot::CodeInTarget => self.msgstr.push_str(&literal_or_unescape(raw)),
            Slot::Note => self.note.push_str(&literal_or_unescape(raw)),
            Slot::Sourcefile => self.pending_file.push_str(&literal_or_unescape(raw)),
            Slot::Linenumber => self.pending_line.push_str(&literal_or_unescape(raw)),
            Slot::None => {}
        }
    }

    fn handle_general_ref(&mut self, name: &str) {
        let (_, plain) = crate::extract::resolve_reference(name);
        match self.slot {
            Slot::Source => {
                self.msgid
                    .push_str(&crate::extract::escape_inline_text(&plain));
            }
            Slot::CodeInSource => self.msgid.push_str(&plain),
            Slot::Target => {
                self.msgstr
                    .push_str(&crate::extract::escape_inline_text(&plain));
            }
            Slot::CodeInTarget => self.msgstr.push_str(&plain),
            Slot::Note => self.note.push_str(&plain),
            Slot::Sourcefile => self.pending_file.push_str(&plain),
            Slot::Linenumber => self.pending_line.push_str(&plain),
            Slot::None => {}
        }
    }

    fn handle_end(&mut self, element: &str) {
        match element {
            "bpt" | "ept" | "ph" => {
                // Return to the enclosing source/target context.
                self.slot = if msgid_slot_active(self.slot) {
                    Slot::Source
                } else {
                    Slot::Target
                };
            }
            "source" | "target" => self.slot = Slot::None,
            "note" => {
                let note = self.note.trim().to_string();
                if !note.is_empty() {
                    self.comments.push(note);
                }
                self.slot = Slot::None;
            }
            "context" => match self.slot {
                Slot::Sourcefile => {
                    self.pending_file = self.pending_file.trim().to_string();
                    self.slot = Slot::None;
                }
                Slot::Linenumber => {
                    self.pending_line = self.pending_line.trim().to_string();
                    self.references.push(if self.pending_line.is_empty() {
                        self.pending_file.clone()
                    } else {
                        format!("{}:{}", self.pending_file, self.pending_line)
                    });
                    self.pending_file.clear();
                    self.pending_line.clear();
                    self.slot = Slot::None;
                }
                _ => self.slot = Slot::None,
            },
            "trans-unit" => {
                self.catalog.entries.push(PoEntry {
                    msgctxt: self.resname.clone(),
                    msgid: self.msgid.trim().to_string(),
                    msgstr: self.msgstr.trim().to_string(),
                    references: self.references.clone(),
                    comments: self.comments.clone(),
                    ..PoEntry::default()
                });
                self.slot = Slot::None;
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Result<PoCatalog> {
        if !self.saw_unit {
            return Err(Error::PoParse {
                line: 0,
                message: "no <trans-unit> elements found: not an XLIFF 1.2 document".to_string(),
            });
        }
        // Rebuild a PO header from the file-level languages.
        let meta = crate::po::PoMetadata {
            project_id_version: self
                .source_language
                .as_ref()
                .map(|source| format!("XLIFF import ({source})")),
            language: self.target_language,
            mime_version: Some("1.0".to_string()),
            content_type: Some("text/plain; charset=UTF-8".to_string()),
            content_transfer_encoding: Some("8bit".to_string()),
            ..crate::po::PoMetadata::default()
        };
        self.catalog.set_metadata(&meta);
        Ok(self.catalog)
    }
}

/// Streaming XLIFF 1.2 reader driver.
fn parse_xliff(xliff: &str) -> Result<PoCatalog> {
    let mut reader = Reader::from_str(xliff);
    reader.config_mut().expand_empty_elements = true;
    let mut state = XliffReader::new();
    let mut buf = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::PoParse {
                line: 0,
                message: format!("invalid XLIFF document: {e}"),
            })?;
        match event {
            Event::Start(e) => {
                let element = local_name(e.name().0).to_string();
                let attrs: Vec<(String, String)> = e
                    .attributes()
                    .flatten()
                    .filter_map(|attr| {
                        let name = local_name(attr.key.0).to_string();
                        attr.normalized_value(quick_xml::XmlVersion::default())
                            .ok()
                            .map(|value| (name, value.into_owned()))
                    })
                    .collect();
                state.handle_start(&element, &attrs);
            }
            Event::Text(text) => state.handle_text(text.as_ref()),
            Event::GeneralRef(reference) => {
                let name: &str = &reference;
                state.handle_general_ref(name);
            }
            Event::End(e) => {
                let element = local_name(e.name().0).to_string();
                state.handle_end(&element);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    state.finish()
}

/// Whether the slot is a source-side buffer (used to restore context
/// after an inline code closes).
fn msgid_slot_active(slot: Slot) -> bool {
    matches!(slot, Slot::CodeInSource | Slot::Source)
}

/// Local name of a possibly prefixed XML name.
fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// Converts an imported catalog back to messages (for `diff`/`check`).
#[must_use]
pub fn catalog_to_messages(catalog: &PoCatalog) -> Vec<Message> {
    catalog
        .entries
        .iter()
        .map(|entry| Message {
            msgid: entry.msgid.clone(),
            msgctxt: entry.msgctxt.clone(),
            references: entry
                .references
                .iter()
                .filter_map(|reference| match reference.rsplit_once(':') {
                    Some((file, line)) if line.chars().all(|c| c.is_ascii_digit()) => {
                        Some(SourceRef::new(file, line.parse().unwrap_or(1)))
                    }
                    _ => None,
                })
                .collect(),
        })
        .collect()
}

/// Looks up a translated unit by context and msgid.
#[must_use]
pub fn translation_for<'a>(
    catalog: &'a PoCatalog,
    msgctxt: Option<&str>,
    msgid: &str,
) -> Option<&'a str> {
    let key = CatalogKey::new(msgctxt, msgid);
    catalog
        .from_key(key)
        .filter(|entry| entry.is_translated())
        .map(|entry| entry.msgstr.as_str())
}

/// Convenience: builds an owned key for an imported entry.
#[must_use]
pub fn entry_key(entry: &PoEntry) -> OwnedCatalogKey {
    entry.key()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::extract_messages;
    use crate::strategy::HtmlStrategy;

    #[test]
    fn export_contains_units_refs_and_target() {
        let messages = vec![
            Message::new("Hello", vec![SourceRef::new("a.html", 3)]),
            Message::with_context("Save", "button", vec![SourceRef::new("b.html", 9)]),
        ];
        let catalog = PoCatalog::from_messages(&messages);
        let mut catalog = catalog;
        catalog.set_translation(CatalogKey::new(None, "Hello"), "Hola");
        let xliff = catalog_to_xliff(&catalog, "en", Some("fr"));

        assert!(xliff.contains("version=\"1.2\""), "{xliff}");
        assert!(xliff.contains("source-language=\"en\""), "{xliff}");
        assert!(xliff.contains("target-language=\"fr\""), "{xliff}");
        assert!(xliff.contains("<source>Hello</source>"), "{xliff}");
        assert!(xliff.contains("<target>Hola</target>"), "{xliff}");
        assert!(xliff.contains("resname=\"button\""), "{xliff}");
        assert!(
            xliff.contains("context-type=\"sourcefile\">a.html<"),
            "{xliff}"
        );
        assert!(xliff.contains("context-type=\"linenumber\">3<"), "{xliff}");
        // Untranslated entries have no target.
        assert!(!xliff.contains("<target>Guardar</target>"), "{xliff}");
        let save_unit = xliff.split("resname=\"button\"").nth(1).unwrap();
        assert!(!save_unit.contains("<target>"), "{save_unit}");
    }

    #[test]
    fn inline_placeholders_become_bpt_ept_ph() {
        let messages = vec![Message::new("Hello <b>bold</b> Line<br/>break", vec![])];
        let catalog = PoCatalog::from_messages(&messages);
        let xliff = catalog_to_xliff(&catalog, "en", None);
        assert!(xliff.contains("<bpt id=\"1\">&lt;b&gt;</bpt>"), "{xliff}");
        assert!(xliff.contains("<ept id=\"2\">&lt;/b&gt;</ept>"), "{xliff}");
        assert!(xliff.contains("<ph id=\"3\">&lt;br/&gt;</ph>"), "{xliff}");
    }

    #[test]
    fn fuzzy_exports_as_untranslatable() {
        let po = "#, fuzzy\nmsgid \"a\"\nmsgstr \"A\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        let xliff = catalog_to_xliff(&catalog, "en", None);
        assert!(xliff.contains("translate=\"no\""), "{xliff}");
    }

    #[test]
    fn roundtrip_preserves_entries() {
        let po = "# translator note\n#: a.html:3\nmsgid \"Hello <b>bold</b>\"\nmsgstr \"Hola <b>negrita</b>\"\n\nmsgctxt \"button\"\nmsgid \"Save\"\nmsgstr \"Guardar\"\n";
        let catalog = PoCatalog::parse(po).unwrap();
        let xliff = catalog_to_xliff(&catalog, "en", Some("fr"));
        let imported = catalog_from_xliff(&xliff).unwrap();
        assert_eq!(imported.entries.len(), 2);

        let hello = &imported.entries[0];
        assert_eq!(hello.msgid, "Hello <b>bold</b>");
        assert_eq!(hello.msgstr, "Hola <b>negrita</b>");
        assert_eq!(hello.comments, vec!["translator note"]);
        assert_eq!(hello.references, vec!["a.html:3"]);

        let save = &imported.entries[1];
        assert_eq!(save.msgctxt.as_deref(), Some("button"));
        assert_eq!(save.msgid, "Save");
        assert_eq!(save.msgstr, "Guardar");
    }

    #[test]
    fn import_minimal_xliff_from_other_tools() {
        let xliff = r#"<?xml version="1.0"?>
<xliff xmlns="urn:oasis:names:tc:xliff:document:1.2" version="1.2">
  <file original="web.xml" source-language="en" datatype="plaintext">
    <body>
      <trans-unit id="1" resname="title">
        <source>Welcome to <bpt id="1">&lt;em&gt;</bpt>our site<ept id="1">&lt;/em&gt;</ept></source>
        <target>Bienvenue sur <bpt id="1">&lt;em&gt;</bpt>notre site<ept id="1">&lt;/em&gt;</ept></target>
      </trans-unit>
      <trans-unit id="2">
        <source>Plain text</source>
      </trans-unit>
    </body>
  </file>
</xliff>
"#;
        let catalog = catalog_from_xliff(xliff).unwrap();
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.entries[0].msgid, "Welcome to <em>our site</em>");
        assert_eq!(
            catalog.entries[0].msgstr,
            "Bienvenue sur <em>notre site</em>"
        );
        assert_eq!(catalog.entries[0].msgctxt.as_deref(), Some("title"));
        assert_eq!(catalog.entries[1].msgid, "Plain text");
        assert_eq!(catalog.entries[1].msgstr, "");
    }

    #[test]
    fn extracted_msgids_survive_xliff_roundtrip() {
        let content = "<p>Hello <b>bold</b> world</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let catalog = PoCatalog::from_messages(&messages);
        let xliff = catalog_to_xliff(&catalog, "en", None);
        let imported = catalog_from_xliff(&xliff).unwrap();
        assert_eq!(imported.entries, catalog.entries);
    }

    #[test]
    fn invalid_xliff_is_reported() {
        let err = catalog_from_xliff("this is not xml").unwrap_err();
        assert!(err.to_string().contains("XLIFF"), "{err}");
    }

    #[test]
    fn catalog_to_messages_and_helpers() {
        let messages = vec![Message::new("Hi", vec![SourceRef::new("a.html", 1)])];
        let catalog = PoCatalog::from_messages(&messages);
        let back = catalog_to_messages(&catalog);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].msgid, "Hi");
        assert_eq!(back[0].references[0].line, 1);

        assert!(translation_for(&catalog, None, "Hi").is_none());
        let mut catalog = catalog;
        catalog.set_translation(CatalogKey::new(None, "Hi"), "Hola");
        assert_eq!(translation_for(&catalog, None, "Hi"), Some("Hola"));
        assert!(translation_for(&catalog, None, "nope").is_none());

        let entry = &catalog.entries[0];
        assert_eq!(entry_key(entry), OwnedCatalogKey::new(None, "Hi"));
    }
}
