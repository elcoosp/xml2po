//! Streaming extraction of translatable strings from XML-like documents.

use std::collections::BTreeMap;
use std::path::PathBuf;

use std::borrow::Cow;

use quick_xml::XmlVersion;
use quick_xml::escape::unescape;
use quick_xml::events::Event;
use quick_xml::events::attributes::Attribute;
use quick_xml::reader::Reader;

use crate::error::{Error, Result};
use crate::message::{Message, OwnedCatalogKey, SourceRef};
use crate::options::ExtractionOptions;
use crate::strategy::ExtractionStrategy;

/// Extracts translatable strings with default options.
///
/// Convenience wrapper around [`extract_messages_with`].
///
/// # Errors
///
/// Returns [`Error::Xml`] when the document cannot be parsed.
pub fn extract_messages(
    content: &str,
    source_name: &str,
    strategy: &dyn ExtractionStrategy,
) -> Result<Vec<Message>> {
    extract_messages_with(
        content,
        source_name,
        strategy,
        &ExtractionOptions::default(),
    )
}

/// Extracts translatable strings and groups them by `(msgctxt, msgid)`.
///
/// - A *segment* is the text between structural markup (tags, comments,
///   CDATA); character/entity references (`&amp;`, `&#233;`) are resolved
///   and joined into the surrounding segment, so `Fish &amp; Chips`
///   yields a single `Fish & Chips` msgid.
/// - Only segments under translatable elements (or translatable attribute
///   values) are kept, and only when the text heuristic accepts them.
/// - When [`ExtractionOptions::with_context`] is enabled, text gets the
///   enclosing element name as `msgctxt` and attribute values get
///   `element@attribute`.
/// - References carry real 1-based line numbers, resolved through a
///   line index built once per document.
/// - Output is deterministically ordered by `(msgctxt, msgid)` so
///   repeated runs produce byte-identical PO files.
///
/// # Errors
///
/// Returns [`Error::Xml`] when the document cannot be parsed.
pub fn extract_messages_with(
    content: &str,
    source_name: &str,
    strategy: &dyn ExtractionStrategy,
    options: &ExtractionOptions,
) -> Result<Vec<Message>> {
    let mut extractor = Extractor::new(content, source_name, strategy, options);
    let mut reader = Reader::from_str(content);
    // Real-world HTML is full of unclosed void elements (`<img>`, `<br>`)
    // and browser-style tag soup; only XML-family formats get the strict
    // end-tag check.
    if strategy.format_name() == "html" {
        reader.config_mut().check_end_names = false;
    }
    let mut buf = Vec::new();
    loop {
        let offset = reader.buffer_position();
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| extractor.xml_error(offset, e))?;
        if extractor.handle_event(event, offset)? {
            break;
        }
        buf.clear();
    }
    extractor.flush();
    Ok(extractor.finish())
}

/// Extracts from raw bytes for callers that already have file content in
/// memory. The bytes must be valid UTF-8; use [`crate::decode_file_bytes`]
/// (CLI) or your own decoder for other encodings.
///
/// # Errors
///
/// Returns [`Error::NotUtf8`] for non-UTF-8 input and [`Error::Xml`] when
/// the document cannot be parsed.
pub fn extract_messages_from_bytes(
    content: &[u8],
    source_name: &str,
    strategy: &dyn ExtractionStrategy,
) -> Result<Vec<Message>> {
    let text = std::str::from_utf8(content).map_err(|_| Error::NotUtf8 {
        path: PathBuf::from(source_name),
    })?;
    extract_messages(text, source_name, strategy)
}

/// Text accumulated between structural events.
struct Segment {
    /// Message text under construction. In inline mode this is the msgid:
    /// XML-escaped text runs interleaved with `<tag>` placeholders.
    plain: String,
    /// Byte offset where the segment started (for line references).
    offset: u64,
}

impl Segment {
    const fn new(start: u64) -> Self {
        Self {
            plain: String::new(),
            offset: start,
        }
    }
}

/// One open element: name plus flags resolved at push time so the
/// `translate` directive does not need re-evaluation.
#[derive(Debug, Clone)]
struct ElemState {
    name: String,
    ignored: bool,
    text_translatable: bool,
}

/// Streaming extraction state for one document.
struct Extractor<'a> {
    lines: LineIndex,
    source_name: &'a str,
    strategy: &'a dyn ExtractionStrategy,
    options: &'a ExtractionOptions,
    grouped: BTreeMap<OwnedCatalogKey, Vec<SourceRef>>,
    /// Open elements; `ignored` and `text_translatable` are inherited, so
    /// any element inside an ignored subtree is ignored as well.
    stack: Vec<ElemState>,
    /// Open inline placeholders (`<b>` without its `</b>` yet).
    inline_stack: Vec<String>,
    /// When set, the extractor is copying an ignored inline element
    /// verbatim into the message (itstool behavior for
    /// `translate="no"`/ignored inline content).
    copy_root: Option<String>,
    copy_depth: usize,
    segment: Option<Segment>,
}

impl<'a> Extractor<'a> {
    fn new(
        content: &str,
        source_name: &'a str,
        strategy: &'a dyn ExtractionStrategy,
        options: &'a ExtractionOptions,
    ) -> Self {
        Self {
            lines: LineIndex::new(content),
            source_name,
            strategy,
            options,
            grouped: BTreeMap::new(),
            stack: Vec::new(),
            inline_stack: Vec::new(),
            copy_root: None,
            copy_depth: 0,
            segment: None,
        }
    }

    /// Builds a position-annotated XML error for the given byte offset.
    fn xml_error(&self, offset: u64, source: quick_xml::Error) -> Error {
        let (line, column) = self
            .lines
            .line_col(usize::try_from(offset).unwrap_or(usize::MAX));
        Error::Xml {
            offset,
            line,
            column,
            source,
        }
    }

    /// Whether inline folding is active for the current context.
    fn folding(&self, element: &str) -> bool {
        self.options.inline_placeholders && self.strategy.is_inline_element(element)
    }

    /// Appends text to the active segment, creating it if needed.
    fn append_plain(&mut self, text: &str, offset: u64) {
        self.segment
            .get_or_insert_with(|| Segment::new(offset))
            .plain
            .push_str(text);
    }

    fn top(&self) -> Option<&ElemState> {
        self.stack.last()
    }

    /// Handles one event; returns `true` at end-of-file.
    fn handle_event(&mut self, event: Event<'_>, offset: u64) -> Result<bool> {
        match event {
            Event::Start(e) => self.handle_start(&e, offset)?,
            Event::Empty(e) => self.handle_empty(&e, offset)?,
            Event::Text(e) => {
                let copying = self.copy_root.is_some();
                let inline = self.inline_active();
                let seg = self.segment.get_or_insert_with(|| Segment::new(offset));
                if copying {
                    // Verbatim copy: source text is already escaped.
                    seg.plain.push_str(&e);
                } else {
                    let plain = literal_or_unescape(&e);
                    if inline {
                        seg.plain.push_str(&escape_inline_text(&plain));
                    } else {
                        seg.plain.push_str(&plain);
                    }
                }
            }
            Event::GeneralRef(e) => {
                let name: &str = &e; // Deref<Target = str>: e.g. "amp" or "#x30"
                let (raw_form, plain_form) = resolve_reference(name);
                let copying = self.copy_root.is_some();
                let inline = self.inline_active();
                let seg = self.segment.get_or_insert_with(|| Segment::new(offset));
                if copying {
                    seg.plain.push_str(&raw_form);
                } else {
                    let form = if inline {
                        escape_inline_text(&plain_form)
                    } else {
                        plain_form
                    };
                    seg.plain.push_str(&form);
                }
            }
            Event::CData(e) => {
                let inner = e.into_inner();
                if self.copy_root.is_some() {
                    // Verbatim copy: CDATA content is raw source text.
                    self.segment
                        .get_or_insert_with(|| Segment::new(offset))
                        .plain
                        .push_str(&inner);
                    return Ok(false);
                }
                self.flush_segment(None);
                let text = inner.trim().to_string();
                let msgid = if self.inline_active() {
                    escape_inline_text(&text)
                } else {
                    text
                };
                let element = self.top().cloned();
                self.record(&msgid, element.as_ref(), None, offset);
            }
            Event::End(e) => self.handle_end(&e),
            Event::Comment(_) | Event::PI(_) | Event::Decl(_) | Event::DocType(_) => {
                self.flush_segment(None);
            }
            Event::Eof => return Ok(true),
        }
        Ok(false)
    }

    /// True when inline placeholders are enabled and a foldable segment
    /// could receive placeholders (used to pick the msgid escaping).
    fn inline_active(&self) -> bool {
        self.options.inline_placeholders
    }

    /// Start tag: fold inline placeholders, enter copy mode for ignored
    /// inline content, or flush and continue structurally.
    fn handle_start(&mut self, e: &quick_xml::events::BytesStart<'_>, offset: u64) -> Result<()> {
        let raw_tag = e.as_ref().to_string();
        let element = e.name().0.to_string();
        let parent_ignored = self.top().is_some_and(|s| s.ignored);
        let parent_trans = self.top().is_some_and(|s| s.text_translatable);

        if let Some(root) = self.copy_root.clone() {
            let open = format!("<{raw_tag}>");
            self.append_plain(&open, offset);
            if element == root {
                self.copy_depth += 1;
            }
            self.stack.push(ElemState {
                name: element,
                ignored: true,
                text_translatable: false,
            });
            return Ok(());
        }

        // Attributes are only needed for non-ignored subtrees (directive,
        // translatable values); this preserves the old error behavior of
        // skipping malformed attributes inside ignored elements.
        let base_rule = self.strategy.resolve(&element, None, self.options);
        let need_attrs = !parent_ignored && !base_rule.ignored;
        let attrs = if need_attrs {
            self.collect_attributes(e.attributes(), offset)?
        } else {
            Vec::new()
        };
        let directive = crate::strategy::translate_directive(&attrs);
        let rule =
            crate::strategy::effective_rule(self.strategy, &element, directive, self.options);
        let ignored = parent_ignored || rule.ignored;
        let folding = !ignored && self.folding(&element) && parent_trans;

        // HTML void elements never emit an end tag; treating them as
        // self-closing keeps their state from leaking to later siblings.
        if is_void_element(&element) {
            if folding {
                let open = format!("<{element}/>");
                self.append_plain(&open, offset);
            } else {
                self.flush_segment(None);
                if ignored && self.folding(&element) && parent_trans {
                    let verbatim = if raw_tag.ends_with('/') {
                        format!("<{raw_tag}>")
                    } else {
                        format!("<{raw_tag}/>")
                    };
                    self.segment
                        .get_or_insert_with(|| Segment::new(offset))
                        .plain
                        .push_str(&verbatim);
                }
            }
            if !ignored {
                self.extract_attribute_messages(&attrs, &element, directive, offset);
            }
            return Ok(());
        }

        if folding {
            // Open an inline placeholder; translatable attributes of the
            // folded element are still extracted on their own.
            let open = format!("<{element}>");
            self.append_plain(&open, offset);
            self.inline_stack.push(element.clone());
            self.extract_attribute_messages(&attrs, &element, directive, offset);
            self.stack.push(ElemState {
                name: element,
                ignored: false,
                text_translatable: rule.text_translatable,
            });
            return Ok(());
        }

        if ignored && self.folding(&element) && parent_trans {
            // Ignored inline content inside a live message: copy it
            // verbatim so translators can move it around.
            let open = format!("<{raw_tag}>");
            self.append_plain(&open, offset);
            self.copy_root = Some(element.clone());
            self.copy_depth = 1;
            self.stack.push(ElemState {
                name: element,
                ignored: true,
                text_translatable: false,
            });
            return Ok(());
        }

        self.flush_segment(None);

        if !ignored {
            self.extract_attribute_messages(&attrs, &element, directive, offset);
        }
        self.stack.push(ElemState {
            name: element,
            ignored,
            text_translatable: rule.text_translatable,
        });
        Ok(())
    }

    /// Empty (self-closing) tag: fold `<tag/>`, copy verbatim, or flush.
    fn handle_empty(&mut self, e: &quick_xml::events::BytesStart<'_>, offset: u64) -> Result<()> {
        let raw_tag = e.as_ref().to_string();
        let element = e.name().0.to_string();
        let parent_ignored = self.top().is_some_and(|s| s.ignored);
        let parent_trans = self.top().is_some_and(|s| s.text_translatable);

        if self.copy_root.is_some() {
            let verbatim = if raw_tag.ends_with('/') {
                format!("<{raw_tag}>")
            } else {
                format!("<{raw_tag}/>")
            };
            self.segment
                .get_or_insert_with(|| Segment::new(offset))
                .plain
                .push_str(&verbatim);
            return Ok(());
        }

        let base_rule = self.strategy.resolve(&element, None, self.options);
        let need_attrs = !parent_ignored && !base_rule.ignored;
        let attrs = if need_attrs {
            self.collect_attributes(e.attributes(), offset)?
        } else {
            Vec::new()
        };
        let directive = crate::strategy::translate_directive(&attrs);
        let rule =
            crate::strategy::effective_rule(self.strategy, &element, directive, self.options);
        let ignored = parent_ignored || rule.ignored;
        let folding = !ignored && self.folding(&element) && parent_trans;

        if folding {
            let open = format!("<{element}/>");
            self.append_plain(&open, offset);
            self.extract_attribute_messages(&attrs, &element, directive, offset);
            return Ok(());
        }

        self.flush_segment(None);
        if ignored && self.folding(&element) && parent_trans {
            let verbatim = if raw_tag.ends_with('/') {
                format!("<{raw_tag}>")
            } else {
                format!("<{raw_tag}/>")
            };
            self.segment
                .get_or_insert_with(|| Segment::new(offset))
                .plain
                .push_str(&verbatim);
            return Ok(());
        }
        if !ignored {
            self.extract_attribute_messages(&attrs, &element, directive, offset);
        }
        Ok(())
    }

    /// End tag: close inline placeholders, exit copy mode, or flush the
    /// segment under the element being closed.
    fn handle_end(&mut self, e: &quick_xml::events::BytesEnd<'_>) {
        let name = e.name().0.to_string();

        if let Some(root) = self.copy_root.clone() {
            let raw = format!("</{name}>");
            if let Some(seg) = &mut self.segment {
                seg.plain.push_str(&raw);
            }
            if name == root {
                self.copy_depth -= 1;
                if self.copy_depth == 0 {
                    self.copy_root = None;
                }
            }
            self.stack.pop();
            return;
        }

        let popped = self.stack.pop();
        let Some(state) = popped else {
            self.flush_segment(None);
            return;
        };
        if self.folding(&state.name) && self.inline_stack.last().is_some_and(|t| *t == state.name) {
            self.inline_stack.pop();
            if let Some(seg) = &mut self.segment {
                let close = format!("</{}>", state.name);
                seg.plain.push_str(&close);
            }
            return;
        }
        self.flush_segment(Some(state));
    }

    /// Emits the pending segment as a message when it qualifies.
    ///
    /// `closed` is the element state of the tag being closed (when the
    /// flush is triggered by an end tag); otherwise the segment belongs
    /// to the innermost open element.
    fn flush_segment(&mut self, closed: Option<ElemState>) {
        let Some(seg) = self.segment.take() else {
            return;
        };
        // Dangling inline placeholders (unbalanced markup at a boundary)
        // are stripped so translators never see broken tags.
        let mut msgid = seg.plain;
        if self.inline_active() {
            if !self.inline_stack.is_empty() {
                self.inline_stack.clear();
                msgid = strip_placeholders(&msgid);
            }
            let stripped = strip_placeholders(&msgid);
            if stripped != msgid {
                // Placeholders were folded: gate the heuristic on the
                // human-readable text, not on the markup.
                if !self.strategy.is_translatable_text(stripped.trim()) {
                    return;
                }
            }
        }
        let element = closed.or_else(|| self.top().cloned());
        self.record(&msgid, element.as_ref(), None, seg.offset);
    }

    /// Final flush after the event loop.
    fn flush(&mut self) {
        self.flush_segment(None);
    }

    /// Consumes the extractor, returning the deterministic message list.
    fn finish(self) -> Vec<Message> {
        self.grouped
            .into_iter()
            .map(|(key, references)| Message {
                msgid: key.msgid,
                msgctxt: key.msgctxt,
                references,
            })
            .collect()
    }

    /// Records one text candidate if the strategy + options accept it.
    fn record(
        &mut self,
        text: &str,
        element: Option<&ElemState>,
        attribute: Option<&str>,
        offset: u64,
    ) {
        let Some(element) = element else {
            return;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if element.ignored
            || self.top().is_some_and(|s| s.ignored)
            || !element.text_translatable
            || !self.strategy.is_translatable_text(trimmed)
        {
            return;
        }
        let msgctxt = self.context_for(&element.name, attribute);
        self.grouped
            .entry(OwnedCatalogKey::new(msgctxt, trimmed.to_string()))
            .or_default()
            .push(SourceRef::new(
                self.source_name,
                self.lines
                    .line_of(usize::try_from(offset).unwrap_or(usize::MAX)),
            ));
    }

    /// Builds the gettext context honoring context mode.
    fn context_for(&self, element: &str, attribute: Option<&str>) -> Option<String> {
        if self.options.with_context {
            Some(attribute.map_or_else(|| element.to_string(), |attr| format!("{element}@{attr}")))
        } else {
            None
        }
    }

    /// Parses all attributes of a tag into `(name, normalized value)`
    /// pairs, mapping failures to position-annotated XML errors.
    fn collect_attributes<'e, I>(&self, attributes: I, offset: u64) -> Result<Vec<(String, String)>>
    where
        I: Iterator<
            Item = std::result::Result<Attribute<'e>, quick_xml::events::attributes::AttrError>,
        >,
    {
        let mut collected = Vec::new();
        for attr in attributes {
            let attr = attr.map_err(|e| self.xml_error(offset, e.into()))?;
            let value = attr
                .normalized_value(XmlVersion::default())
                .map_err(|e| self.xml_error(offset, e))?;
            collected.push((attr.key.0.to_string(), value.into_owned()));
        }
        Ok(collected)
    }

    /// Records translatable attribute values from a collected attribute
    /// list, honoring the element-level `translate` directive.
    fn extract_attribute_messages(
        &mut self,
        attrs: &[(String, String)],
        element: &str,
        directive: Option<bool>,
        offset: u64,
    ) {
        if directive == Some(false) {
            return;
        }
        for (name, value) in attrs {
            // `translate="yes"` forces text content only; attribute
            // values keep the strategy rules (and `translate` itself is
            // never a translatable value).
            if name.eq_ignore_ascii_case("translate") || name.eq_ignore_ascii_case("its:translate")
            {
                continue;
            }
            let attr_translatable = self
                .strategy
                .resolve(element, Some(name.as_str()), self.options)
                .attribute_translatable;
            if !attr_translatable {
                continue;
            }
            let trimmed = value.trim();
            if trimmed.is_empty() || !self.strategy.is_translatable_text(trimmed) {
                continue;
            }
            let msgctxt = self.context_for(element, Some(name.as_str()));
            self.grouped
                .entry(OwnedCatalogKey::new(msgctxt, trimmed.to_string()))
                .or_default()
                .push(SourceRef::new(
                    self.source_name,
                    self.lines
                        .line_of(usize::try_from(offset).unwrap_or(usize::MAX)),
                ));
        }
    }
}

/// HTML void elements: written without an end tag, so a `Start` event
/// for them must not push element state.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Whether the element is an HTML void element.
#[must_use]
pub fn is_void_element(element: &str) -> bool {
    VOID_ELEMENTS.contains(&element)
}

/// XML-escapes literal text so inline msgids stay unambiguous: only
/// placeholder tags appear as raw markup.
#[must_use]
pub fn escape_inline_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

/// Removes `<…>` markup runs from an inline msgid, leaving plain text.
///
/// Only well-formed-looking tags (`<b>`, `</b>`, `<br/>`,
/// `<span translate="no">`) are dropped; an unterminated tag prefix
/// (`Hello <b|world`) drops just the marker so trailing text survives.
#[must_use]
pub fn strip_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'/' {
                j += 1;
            }
            let name_start = j;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric()
                    || bytes[j] == b':'
                    || bytes[j] == b'_'
                    || bytes[j] == b'-')
            {
                j += 1;
            }
            if j > name_start {
                // Tag found: skip attributes through the closing `>` when
                // present; otherwise stop right after the name.
                while j < bytes.len() && bytes[j] != b'>' {
                    j += 1;
                }
                if j < bytes.len() {
                    j += 1; // consume '>'
                }
                i = j;
                continue;
            }
            // Not a tag: literal `<` (cannot happen in inline msgids but
            // keeps the function total).
            out.push('<');
            i += 1;
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or('<');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Unescapes a text run, falling back to the literal text when it contains
/// an entity the XML core does not know (should not happen: references are
/// delivered as separate events).
#[must_use]
pub fn literal_or_unescape(raw: &str) -> String {
    unescape(raw).map_or_else(|_| raw.to_string(), Cow::into_owned)
}

/// Resolves a general entity/character reference to its raw and plain forms.
///
/// Unknown named references are kept verbatim (`&nbsp;`), matching the
/// behavior of gettext tools that leave custom entities untouched.
#[must_use]
pub fn resolve_reference(name: &str) -> (String, String) {
    let raw = format!("&{name};");
    let plain = name
        .strip_prefix('#')
        .map_or_else(|| named_reference(name), char_reference)
        .map_or_else(|| raw.clone(), String::from);
    (raw, plain)
}

/// Character for a named XML predefined entity, if known.
fn named_reference(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// Character for a numeric character reference (`#65`, `#x41`), if valid.
fn char_reference(number: &str) -> Option<char> {
    number.strip_prefix(['x', 'X']).map_or_else(
        || number.parse::<u32>().ok().and_then(char::from_u32),
        |hex| u32::from_str_radix(hex, 16).ok().and_then(char::from_u32),
    )
}

/// Precomputed start offsets of every line, enabling O(log n) line lookup.
#[derive(Debug, Clone)]
pub struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    /// Builds the index; line 1 starts at offset 0.
    #[must_use]
    pub fn new(content: &str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, b) in content.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { line_starts }
    }

    /// 1-based line containing the given byte offset.
    #[must_use]
    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i + 1,
            Err(i) => i, // offset is inside line i (line_starts[i-1] < offset < line_starts[i])
        }
    }

    /// Total number of lines in the indexed content.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// 1-based `(line, column)` of the given byte offset. Column counts
    /// bytes, matching the convention of most editors and compilers.
    #[must_use]
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = self.line_of(offset);
        let line_start = self.line_starts.get(line - 1).copied().unwrap_or(0);
        (line, offset.saturating_sub(line_start) + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{HtmlStrategy, MjmlStrategy, XmlStrategy};

    #[test]
    fn line_index_basics() {
        let idx = LineIndex::new("ab\ncd\n\nef");
        assert_eq!(idx.line_count(), 4);
        assert_eq!(idx.line_of(0), 1);
        assert_eq!(idx.line_of(2), 1);
        assert_eq!(idx.line_of(3), 2);
        assert_eq!(idx.line_of(6), 3);
        assert_eq!(idx.line_of(7), 4);
        assert_eq!(idx.line_of(9), 4);
    }

    #[test]
    fn line_index_single_line_and_empty() {
        assert_eq!(LineIndex::new("hello").line_of(4), 1);
        assert_eq!(LineIndex::new("").line_of(0), 1);
        assert_eq!(LineIndex::new("\n").line_of(1), 2);
    }

    #[test]
    fn line_index_columns() {
        let idx = LineIndex::new("ab\ncd\nef");
        assert_eq!(idx.line_col(0), (1, 1));
        assert_eq!(idx.line_col(2), (1, 3));
        assert_eq!(idx.line_col(3), (2, 1));
        assert_eq!(idx.line_col(5), (2, 3));
        assert_eq!(idx.line_col(6), (3, 1));
        assert_eq!(idx.line_col(9), (3, 4));
        // Multi-byte characters keep column == byte distance from line start.
        let idx = LineIndex::new("é\n");
        assert_eq!(idx.line_col(2), (1, 3));
    }

    #[test]
    fn xml_parse_error_carries_line_and_column() {
        let err = extract_messages("<p>ok</p>\n\n<p =\"v\">bad</p>", "t.html", &HtmlStrategy)
            .unwrap_err();
        let Error::Xml { line, column, .. } = &err else {
            panic!("expected XML error, got {err:?}")
        };
        assert_eq!((*line, *column), (3, 1));
        assert!(err.to_string().contains("line 3"));
    }

    #[test]
    fn extracts_leaf_text_with_line_numbers() {
        let content = "<html><body>\n  <p>Hello</p>\n  <p>World</p>\n</body></html>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].msgid, "Hello");
        assert_eq!(messages[0].references[0].line, 2);
        assert_eq!(messages[0].references[0].file, "t.html");
        assert_eq!(messages[1].msgid, "World");
        assert_eq!(messages[1].references[0].line, 3);
        assert!(messages[0].msgctxt.is_none());
    }

    #[test]
    fn deterministic_ordering() {
        let content = "<p>Zebra</p><p>Apple</p><p>Mango</p>";
        let a = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let b = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = a.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["Apple", "Mango", "Zebra"]);
        assert_eq!(a, b);
    }

    #[test]
    fn groups_repeated_strings() {
        let content = "<p>Same</p><p>Same</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].references.len(), 2);
    }

    #[test]
    fn mixed_content_yields_all_segments() {
        // With inline placeholders (the default), the inline `span` folds
        // into the surrounding message instead of splitting it.
        let content = "<div>Before <span>Middle</span> After</div>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Before <span>Middle</span> After");

        // Opting out restores the classic per-segment split.
        let options = ExtractionOptions::builder()
            .inline_placeholders(false)
            .build();
        let messages = extract_messages_with(content, "t.html", &HtmlStrategy, &options).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["After", "Before", "Middle"]);
    }

    #[test]
    fn inline_mixed_content_folds_placeholders() {
        let content = "<p>Hello <b>bold</b> world</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Hello <b>bold</b> world");
        assert!(messages[0].msgctxt.is_none());
    }

    #[test]
    fn inline_nested_placeholders() {
        let content = "<p>Hi <em>very <b>bold</b></em> end</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Hi <em>very <b>bold</b></em> end");
    }

    #[test]
    fn inline_void_elements_fold_self_closing() {
        let content = "<p>Line<br/>break</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Line<br/>break");
    }

    #[test]
    fn inline_tag_attributes_are_dropped_but_extracted_separately() {
        let content = r#"<p><a href="/x" title="Home">link</a></p>"#;
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        // `href` is never extracted; `title` becomes its own message and
        // the folded link carries no attributes. Order is the
        // deterministic msgid order.
        assert_eq!(ids, vec!["<a>link</a>", "Home"]);
    }

    #[test]
    fn literal_markup_is_escaped_in_inline_msgids() {
        let content = "<p>5 &lt; 6 &amp; counting</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages[0].msgid, "5 &lt; 6 &amp; counting");
    }

    #[test]
    fn entities_in_inline_msgids_stay_escaped() {
        let content = "<p>Fish &amp; <b>Chips</b></p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages[0].msgid, "Fish &amp; <b>Chips</b>");
    }

    #[test]
    fn block_children_still_split() {
        let content = "<div>One <p>Para</p> Two</div>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["One", "Para", "Two"]);
    }

    #[test]
    fn mjml_inline_strong_folds() {
        let content = "<mj-text>Hello <strong>bold</strong> world</mj-text>";
        let messages = extract_messages(content, "t.mjml", &MjmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Hello <strong>bold</strong> world");
    }

    #[test]
    fn inline_context_mode_uses_enclosing_element() {
        let options = ExtractionOptions::builder().with_context(true).build();
        let content = "<p>Hello <b>bold</b> world</p>";
        let messages = extract_messages_with(content, "t.html", &HtmlStrategy, &options).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgctxt.as_deref(), Some("p"));
    }

    #[test]
    fn unbalanced_inline_at_eof_falls_back_to_plain_text() {
        // quick-xml accepts a document cut mid-tag; the dangling
        // placeholder is stripped so translators never see broken markup.
        let messages = extract_messages("<p>Hello <b>world", "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Hello world");
    }

    #[test]
    fn cdata_is_a_hard_boundary_for_inline_segments() {
        let content = "<p>Before <![CDATA[ CD ]]> after</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["Before", "CD", "after"]);
    }

    #[test]
    fn translate_no_skips_subtree() {
        // Block element: skipped entirely.
        let content = "<div><section translate=\"no\">Skip</section>Keep</div>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["Keep"]);

        // Attributes of a translate="no" element are skipped too.
        let content = r#"<img alt="Nope" translate="no"><p>Shown</p>"#;
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Shown");
    }

    #[test]
    fn its_translate_no_is_recognized() {
        let content = r#"<p its:translate="no">Skip</p><p>Shown</p>"#;
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Shown");
    }

    #[test]
    fn translate_no_inline_content_is_kept_verbatim_in_msgid() {
        let content = r#"<p>Name: <span translate="no">ACME Corp</span> inc.</p>"#;
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].msgid,
            r#"Name: <span translate="no">ACME Corp</span> inc."#
        );
    }

    #[test]
    fn translate_yes_forces_extraction() {
        let content = r#"<figcaption translate="yes">Caption</figcaption>"#;
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Caption");
    }

    #[test]
    fn respect_translate_attribute_can_be_disabled() {
        let options = ExtractionOptions::builder()
            .respect_translate_attribute(false)
            .build();
        let content = "<p translate=\"no\">Skip? No: extract</p>";
        let messages = extract_messages_with(content, "t.html", &HtmlStrategy, &options).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Skip? No: extract");
    }

    #[test]
    fn non_translatable_container_still_yields_inline_leaf() {
        // The inline span must NOT fold into the (untranslatable) section;
        // it stays its own message.
        let content = "<section>Before <span>Middle</span> After</section>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["Middle"]);
    }

    #[test]
    fn ignored_elements_are_skipped() {
        let content = "<div><script>var x = 1;</script><style>.a{}</style><p>Real</p></div>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Real");
    }

    #[test]
    fn attribute_extraction() {
        let content = r#"<img alt="A sunset" src="x.png"><input placeholder="Your name"><a title="Home" href="/">link</a>"#;
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["A sunset", "Home", "Your name", "link"]);
    }

    #[test]
    fn attribute_extraction_with_context() {
        let options = ExtractionOptions::builder().with_context(true).build();
        let content = r#"<img alt="A sunset">"#;
        let messages = extract_messages_with(content, "t.html", &HtmlStrategy, &options).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgctxt.as_deref(), Some("img@alt"));
    }

    #[test]
    fn text_context_mode() {
        let options = ExtractionOptions::builder().with_context(true).build();
        let content = "<p>Hello</p>";
        let messages = extract_messages_with(content, "t.html", &HtmlStrategy, &options).unwrap();
        assert_eq!(messages[0].msgctxt.as_deref(), Some("p"));
    }

    #[test]
    fn extra_elements_via_options() {
        let options = ExtractionOptions::builder()
            .extra_translatable_element("figcaption")
            .build();
        let content = "<figcaption>Caption text</figcaption><p>Body</p>";
        let messages = extract_messages_with(content, "t.html", &HtmlStrategy, &options).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["Body", "Caption text"]);
    }

    #[test]
    fn ignored_via_options_overrides() {
        let options = ExtractionOptions::builder().ignored_element("p").build();
        let content = "<p>Hidden</p><div>Shown</div>";
        let messages = extract_messages_with(content, "t.html", &HtmlStrategy, &options).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Shown");
    }

    #[test]
    fn cdata_text_is_extracted() {
        let content = "<p><![CDATA[Literal & raw]]></p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        // Inline msgids are XML-escaped fragments.
        assert_eq!(messages[0].msgid, "Literal &amp; raw");
    }

    #[test]
    fn entities_are_unescaped_into_the_segment() {
        let content = "<p>Fish &amp; Chips &lt;3 &#65;</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        // Entities resolve, then the msgid is re-escaped as XML.
        assert_eq!(messages[0].msgid, "Fish &amp; Chips &lt;3 A");

        // With inline placeholders off, msgids are plain text.
        let options = ExtractionOptions::builder()
            .inline_placeholders(false)
            .build();
        let messages = extract_messages_with(content, "t.html", &HtmlStrategy, &options).unwrap();
        assert_eq!(messages[0].msgid, "Fish & Chips <3 A");
    }

    #[test]
    fn unknown_entities_pass_through_verbatim() {
        let content = "<p>Fish&nbsp;Chips</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        // The `&` of the unknown entity is XML-escaped like any other.
        assert_eq!(messages[0].msgid, "Fish&amp;nbsp;Chips");
    }

    #[test]
    fn mismatched_end_tag_is_reported_for_xml() {
        // XML-family formats keep the strict end-tag check.
        let err = extract_messages("<p></div>", "t.xml", &XmlStrategy).unwrap_err();
        assert!(matches!(err, Error::Xml { .. }));
    }

    #[test]
    fn html_tolerates_tag_soup_and_void_elements() {
        // Browsers accept unclosed void elements and mismatched end tags;
        // the HTML strategy matches that leniency.
        let content = r#"<html><body><img src="x.jpg"><p>Hello</p><p>World</div></body></html>"#;
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert!(ids.contains(&"Hello"), "{ids:?}");
        assert!(ids.contains(&"World"), "{ids:?}");
    }

    #[test]
    fn unclosed_tag_at_eof_is_accepted_by_the_parser() {
        // quick-xml does not flag EOF with open elements; the string is
        // still extracted from the open element.
        let messages = extract_messages("<p>unclosed", "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages[0].msgid, "unclosed");
    }

    #[test]
    fn malformed_attribute_error_is_reported() {
        let err = extract_messages("<p =\"v\">text</p>", "t.html", &HtmlStrategy).unwrap_err();
        assert!(matches!(err, Error::Xml { .. }));
    }

    #[test]
    fn unknown_entity_in_attribute_is_reported() {
        let err = extract_messages("<img alt=\"&nope;\">", "t.html", &HtmlStrategy).unwrap_err();
        assert!(matches!(err, Error::Xml { .. }));
    }

    #[test]
    fn comments_pi_and_doctype_split_segments() {
        let content = "<?xml version=\"1.0\"?><!-- note --><p>Hello</p><?render mode=\"fast\"?><!DOCTYPE html><p>World</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["Hello", "World"]);
    }

    #[test]
    fn ignored_empty_elements_and_untranslatable_attribute_values() {
        // `<mj-style/>` exercises the ignored-Empty path.
        let content = r"<mjml><mj-style/><mj-text>Hi</mj-text></mjml>";
        let messages = extract_messages(content, "t.mjml", &MjmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Hi");

        // Numeric and empty alt values exercise the non-translatable
        // attribute value paths of the HTML strategy.
        let content = r#"<html><img alt="12345"/><img alt=""/><p>Real</p></html>"#;
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Real");
    }

    #[test]
    fn empty_document_yields_nothing() {
        let messages = extract_messages("", "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages, [] as [Message; 0]);
    }

    #[test]
    fn from_bytes_requires_utf8() {
        let err =
            extract_messages_from_bytes(&[0xFF, 0xFE, 0x00], "t.html", &HtmlStrategy).unwrap_err();
        assert!(matches!(err, Error::NotUtf8 { .. }));
    }

    #[test]
    fn from_bytes_works() {
        let messages =
            extract_messages_from_bytes(b"<p>Bytes</p>", "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages[0].msgid, "Bytes");
    }

    #[test]
    fn namespaced_elements_use_full_name() {
        let content = "<svg:t xmlns:svg=\"urn:x\">Label</svg:t>";
        let options = ExtractionOptions::builder()
            .extra_translatable_element("svg:t")
            .build();
        let messages = extract_messages_with(content, "t.svg", &XmlStrategy, &options).unwrap();
        assert_eq!(messages[0].msgid, "Label");
    }

    #[test]
    fn xml_strategy_extracts_everything() {
        let content = "<root><item>One</item><item>Two</item><note>ignored?</note></root>";
        let messages = extract_messages(content, "t.xml", &XmlStrategy).unwrap();
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn text_outside_root_is_ignored() {
        let content = "  <p>Inside</p>  ";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn resolve_reference_forms() {
        let (raw, plain) = resolve_reference("amp");
        assert_eq!(raw, "&amp;");
        assert_eq!(plain, "&");
        let (raw, plain) = resolve_reference("#65");
        assert_eq!(plain, "A");
        assert_eq!(raw, "&#65;");
        let (raw, plain) = resolve_reference("#x41");
        assert_eq!(plain, "A");
        assert_eq!(raw, "&#x41;");
        // Unknown named refs pass through verbatim.
        let (raw, plain) = resolve_reference("nbsp");
        assert_eq!(raw, "&nbsp;");
        assert_eq!(plain, "&nbsp;");
        // Invalid char refs pass through verbatim.
        let (raw, plain) = resolve_reference("#zz");
        assert_eq!(raw, "&#zz;");
        assert_eq!(plain, "&#zz;");
        let (raw, plain) = resolve_reference("#xGG");
        assert_eq!(raw, "&#xGG;");
        assert_eq!(plain, "&#xGG;");
    }

    #[test]
    fn literal_or_unescape_fallback() {
        assert_eq!(literal_or_unescape("plain"), "plain");
        assert_eq!(literal_or_unescape("a&amp;b"), "a&b");
        assert_eq!(literal_or_unescape("&notanentity"), "&notanentity");
    }
}
