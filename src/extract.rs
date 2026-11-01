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
    let mut buf = Vec::new();
    loop {
        let offset = reader.buffer_position();
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::Xml { offset, source: e })?;
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
    /// Re-escapable form (source text + `&name;` forms), written back verbatim.
    raw: String,
    /// Unescaped form used for heuristics and PO msgids.
    plain: String,
    /// Byte offset where the segment started (for line references).
    offset: u64,
}

impl Segment {
    const fn new(start: u64) -> Self {
        Self {
            raw: String::new(),
            plain: String::new(),
            offset: start,
        }
    }
}

/// Streaming extraction state for one document.
struct Extractor<'a> {
    lines: LineIndex,
    source_name: &'a str,
    strategy: &'a dyn ExtractionStrategy,
    options: &'a ExtractionOptions,
    grouped: BTreeMap<OwnedCatalogKey, Vec<SourceRef>>,
    /// Open elements as `(name, ignored)`; `ignored` is inherited, so any
    /// element inside an ignored subtree is ignored as well.
    stack: Vec<(String, bool)>,
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
            segment: None,
        }
    }

    /// Handles one event; returns `true` at end-of-file.
    fn handle_event(&mut self, event: Event<'_>, offset: u64) -> Result<bool> {
        match event {
            Event::Start(e) => {
                self.flush_segment();
                let element = e.name().0.to_string();
                let ignored = self.parent_ignored()
                    || self.strategy.resolve(&element, None, self.options).ignored;
                if !ignored {
                    self.extract_attributes(e.attributes(), &element, offset)?;
                }
                self.stack.push((element, ignored));
            }
            Event::Empty(e) => {
                self.flush_segment();
                let element = e.name().0.to_string();
                let ignored = self.parent_ignored()
                    || self.strategy.resolve(&element, None, self.options).ignored;
                if !ignored {
                    self.extract_attributes(e.attributes(), &element, offset)?;
                }
            }
            Event::Text(e) => {
                let seg = self.segment.get_or_insert_with(|| Segment::new(offset));
                seg.raw.push_str(&e);
                seg.plain.push_str(&literal_or_unescape(&e));
            }
            Event::GeneralRef(e) => {
                let seg = self.segment.get_or_insert_with(|| Segment::new(offset));
                let name: &str = &e; // Deref<Target = str>: e.g. "amp" or "#x30"
                let (raw_form, plain_form) = resolve_reference(name);
                seg.raw.push_str(&raw_form);
                seg.plain.push_str(&plain_form);
            }
            Event::CData(e) => {
                self.flush_segment();
                let text = e.into_inner().trim().to_string();
                let element = self.top_element().map(str::to_string);
                self.record(&text, element.as_deref(), None, offset);
            }
            Event::End(_) => {
                self.flush_segment();
                self.stack.pop();
            }
            Event::Comment(_) | Event::PI(_) | Event::Decl(_) | Event::DocType(_) => {
                self.flush_segment();
            }
            Event::Eof => return Ok(true),
        }
        Ok(false)
    }

    /// Emits the pending segment as a message when it qualifies.
    fn flush_segment(&mut self) {
        let Some(seg) = self.segment.take() else {
            return;
        };
        let text = seg.plain;
        let element = self.top_element().map(str::to_string);
        self.record(&text, element.as_deref(), None, seg.offset);
    }

    /// Final flush after the event loop.
    fn flush(&mut self) {
        self.flush_segment();
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
    fn record(&mut self, text: &str, element: Option<&str>, attribute: Option<&str>, offset: u64) {
        let Some(element) = element else {
            return;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let rule = self.strategy.resolve(element, attribute, self.options);
        if rule.ignored
            || self.parent_ignored()
            || !rule.text_translatable
            || !self.strategy.is_translatable_text(trimmed)
        {
            return;
        }
        let msgctxt = self.context_for(element, attribute);
        self.grouped
            .entry(OwnedCatalogKey::new(msgctxt, trimmed.to_string()))
            .or_default()
            .push(SourceRef::new(
                self.source_name,
                self.lines
                    .line_of(usize::try_from(offset).unwrap_or(usize::MAX)),
            ));
    }

    /// Name of the innermost open element, if any.
    fn top_element(&self) -> Option<&str> {
        self.stack.last().map(|(name, _)| name.as_str())
    }

    /// Whether the innermost open element sits in an ignored subtree.
    fn parent_ignored(&self) -> bool {
        self.stack.last().is_some_and(|(_, ignored)| *ignored)
    }

    /// Builds the gettext context honoring context mode.
    fn context_for(&self, element: &str, attribute: Option<&str>) -> Option<String> {
        if self.options.with_context {
            Some(attribute.map_or_else(|| element.to_string(), |attr| format!("{element}@{attr}")))
        } else {
            None
        }
    }

    /// Scans the attributes of a start/empty tag for translatable values.
    fn extract_attributes<'e, I>(&mut self, attributes: I, element: &str, offset: u64) -> Result<()>
    where
        I: Iterator<
            Item = std::result::Result<Attribute<'e>, quick_xml::events::attributes::AttrError>,
        >,
    {
        for attr in attributes {
            let attr = attr.map_err(|e| Error::Xml {
                offset,
                source: e.into(),
            })?;
            let name = attr.key.0.to_string();
            if !self
                .strategy
                .resolve(element, Some(name.as_str()), self.options)
                .attribute_translatable
            {
                continue;
            }
            let value = attr
                .normalized_value(XmlVersion::default())
                .map_err(|e| Error::Xml { offset, source: e })?;
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
        Ok(())
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{HtmlStrategy, MjmlStrategy, XmlStrategy, get_strategy};

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
        // div is translatable, so "Before"/"After" are its own segments;
        // the span contributes its own "Middle".
        let content = "<div>Before <span>Middle</span> After</div>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let ids: Vec<&str> = messages.iter().map(|m| m.msgid.as_str()).collect();
        assert_eq!(ids, vec!["After", "Before", "Middle"]);
    }

    #[test]
    fn non_translatable_container_yields_only_leaf() {
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
        assert_eq!(messages[0].msgid, "Literal & raw");
    }

    #[test]
    fn entities_are_unescaped_into_the_segment() {
        let content = "<p>Fish &amp; Chips &lt;3 &#65;</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msgid, "Fish & Chips <3 A");
    }

    #[test]
    fn unknown_entities_pass_through_verbatim() {
        let content = "<p>Fish&nbsp;Chips</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages[0].msgid, "Fish&nbsp;Chips");
    }

    #[test]
    fn mismatched_end_tag_is_reported() {
        let err = extract_messages("<p></div>", "t.html", &HtmlStrategy).unwrap_err();
        assert!(matches!(err, Error::Xml { .. }));
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
        let messages = extract_messages(content, "t.xml", get_strategy("xml").unwrap()).unwrap();
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
