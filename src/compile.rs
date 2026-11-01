//! Re-applying translated strings to source documents.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::BuildHasher;

use quick_xml::Writer;
use quick_xml::escape::{escape, partial_escape};
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesCData, BytesStart, BytesText, Event};
use quick_xml::{Reader, XmlVersion};

use crate::error::{Error, Result};
use crate::extract::{literal_or_unescape, resolve_reference};
use crate::message::OwnedCatalogKey;
use crate::options::ExtractionOptions;
use crate::strategy::ExtractionStrategy;

/// Result of a compilation pass: the rewritten document plus counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileReport {
    /// The translated document.
    pub output: String,
    /// Number of text segments replaced by a translation.
    pub applied_texts: usize,
    /// Number of attribute values replaced by a translation.
    pub applied_attributes: usize,
}

/// Applies translations to `content`, replacing every translatable text
/// segment and attribute value whose source string has a translation.
///
/// Convenience wrapper around [`apply_translations_with`] using default
/// [`ExtractionOptions`].
///
/// # Errors
///
/// Returns [`Error::Xml`] when the document cannot be parsed.
pub fn apply_translations<S: BuildHasher>(
    content: &str,
    translations: &HashMap<OwnedCatalogKey, String, S>,
    strategy: &dyn ExtractionStrategy,
) -> Result<String> {
    apply_translations_with(
        content,
        translations,
        strategy,
        &ExtractionOptions::default(),
    )
    .map(|report| report.output)
}

/// Applies translations with explicit options and returns a full report.
///
/// Behavior notes:
/// - Text segments are delimited exactly like in
///   [`crate::extract_messages_with`], so extraction keys always match
///   compile lookups. Mixed content keeps all of its text.
/// - Original whitespace padding around a segment is restored on top of
///   the translation.
/// - Untranslated segments, non-translatable nodes and non-translatable
///   attributes pass through byte-for-byte (including their entity form).
/// - When `options.with_context` is enabled, lookups use the element name
///   (`msgctxt`) for text and `element@attribute` for attribute values.
///
/// # Errors
///
/// Returns [`Error::Xml`] when the document cannot be parsed.
pub fn apply_translations_with<S: BuildHasher>(
    content: &str,
    translations: &HashMap<OwnedCatalogKey, String, S>,
    strategy: &dyn ExtractionStrategy,
    options: &ExtractionOptions,
) -> Result<CompileReport> {
    let (buf, applied_texts, applied_attributes) = Compiler::<S, Vec<u8>> {
        translations,
        strategy,
        options,
        writer: Writer::new(Vec::new()),
        stack: Vec::new(),
        segment: None,
        applied_texts: 0,
        applied_attributes: 0,
    }
    .run(content)?;
    // Every byte written came from escaped `&str`s, so the buffer is valid
    // UTF-8 by construction.
    let output = String::from_utf8_lossy(&buf).into_owned();
    Ok(CompileReport {
        output,
        applied_texts,
        applied_attributes,
    })
}

/// Text accumulated between structural events.
#[derive(Default)]
struct Segment {
    raw: String,
    plain: String,
}

/// Streaming translation pass over one document.
///
/// `W` is injectable so tests can drive the IO error paths with a failing
/// writer; production always uses an in-memory `Vec<u8>`.
struct Compiler<'a, S: BuildHasher, W: std::io::Write> {
    translations: &'a HashMap<OwnedCatalogKey, String, S>,
    strategy: &'a dyn ExtractionStrategy,
    options: &'a ExtractionOptions,
    writer: Writer<W>,
    /// Open elements as `(name, ignored)`; `ignored` is inherited.
    stack: Vec<(String, bool)>,
    segment: Option<Segment>,
    applied_texts: usize,
    applied_attributes: usize,
}

impl<S: BuildHasher, W: std::io::Write> Compiler<'_, S, W> {
    /// Streams `content` through the compiler, returning the output buffer
    /// and application counters (the caller converts the buffer).
    fn run(mut self, content: &str) -> Result<(W, usize, usize)> {
        let mut reader = Reader::from_str(content);
        let mut buf = Vec::new();
        loop {
            let event = reader.read_event_into(&mut buf).map_err(|e| Error::Xml {
                offset: reader.buffer_position(),
                source: e,
            })?;
            match event {
                Event::Start(e) => {
                    self.flush_segment()?;
                    let element = e.name().0.to_string();
                    let rewritten = self.rewrite_attributes(&e, &element)?;
                    self.write(Event::Start(rewritten))?;
                    let ignored = self.parent_ignored()
                        || self.strategy.resolve(&element, None, self.options).ignored;
                    self.stack.push((element, ignored));
                }
                Event::Empty(e) => {
                    self.flush_segment()?;
                    let element = e.name().0.to_string();
                    let rewritten = self.rewrite_attributes(&e, &element)?;
                    self.write(Event::Empty(rewritten))?;
                }
                Event::Text(e) => {
                    let seg = self.segment.get_or_insert_default();
                    seg.raw.push_str(&e);
                    seg.plain.push_str(&literal_or_unescape(&e));
                }
                Event::GeneralRef(e) => {
                    let seg = self.segment.get_or_insert_default();
                    let name: &str = &e;
                    let (raw_form, plain_form) = resolve_reference(name);
                    seg.raw.push_str(&raw_form);
                    seg.plain.push_str(&plain_form);
                }
                Event::CData(e) => {
                    self.flush_segment()?;
                    let inner = e.into_inner().trim().to_string();
                    let translated = self
                        .top_element()
                        .and_then(|element| self.translate_segment(&inner, element));
                    match translated {
                        Some(text) => self.write(Event::CData(BytesCData::new(text.trim())))?,
                        None => self.write(Event::CData(BytesCData::new(inner)))?,
                    }
                }
                Event::End(e) => {
                    self.flush_segment()?;
                    self.write(Event::End(e))?;
                    self.stack.pop();
                }
                Event::Eof => break,
                other => {
                    self.flush_segment()?;
                    self.write(other)?;
                }
            }
            buf.clear();
        }
        self.flush_segment()?;
        self.writer
            .get_mut()
            .flush()
            .map_err(|source| Error::Write { source })?;
        Ok((
            self.writer.into_inner(),
            self.applied_texts,
            self.applied_attributes,
        ))
    }

    /// Writes an event, mapping quick-xml IO errors to the crate error.
    fn write(&mut self, event: Event<'_>) -> Result<()> {
        self.writer
            .write_event(event)
            .map_err(|source| Error::Write { source })
    }

    /// Name of the innermost open element, if any.
    fn top_element(&self) -> Option<&str> {
        self.stack.last().map(|(name, _)| name.as_str())
    }

    /// Whether the innermost open element sits in an ignored subtree.
    fn parent_ignored(&self) -> bool {
        self.stack.last().is_some_and(|(_, ignored)| *ignored)
    }

    /// Emits the pending text segment, translated when possible.
    fn flush_segment(&mut self) -> Result<()> {
        if let Some(seg) = self.segment.take() {
            let trimmed = seg.plain.trim();
            let translated = self
                .top_element()
                .and_then(|element| self.translate_segment(trimmed, element))
                .map(|t| restore_text_padding(&t, &seg.plain));
            if translated.is_some() {
                self.applied_texts += 1;
            }
            match translated {
                // partial_escape keeps `"` and `'` literal in text content,
                // matching what the source document most likely had.
                Some(text) => {
                    let escaped = BytesText::from_escaped(partial_escape(&text));
                    self.write(Event::Text(escaped))?;
                }
                None => self.write(Event::Text(BytesText::from_escaped(seg.raw)))?,
            }
        }
        Ok(())
    }

    /// Returns the translation for a trimmed text under `element`, if any.
    fn translate_segment(&self, trimmed: &str, element: &str) -> Option<String> {
        if self.parent_ignored()
            || !self
                .strategy
                .resolve(element, None, self.options)
                .text_translatable
        {
            return None;
        }
        if trimmed.is_empty() || !self.strategy.is_translatable_text(trimmed) {
            return None;
        }
        let key = if self.options.with_context {
            OwnedCatalogKey::new(Some(element.to_string()), trimmed.to_string())
        } else {
            OwnedCatalogKey::new(None, trimmed.to_string())
        };
        self.translations.get(&key).cloned()
    }

    /// Returns a copy of the start/empty tag with translated attributes.
    fn rewrite_attributes(
        &mut self,
        start: &BytesStart<'_>,
        element: &str,
    ) -> Result<BytesStart<'static>> {
        let mut rewritten = BytesStart::new(element.to_string());
        for attr in start.attributes() {
            let attr = attr.map_err(|e| Error::Xml {
                offset: 0,
                source: e.into(),
            })?;
            let name = attr.key.0;
            let translatable = self
                .strategy
                .resolve(element, Some(name), self.options)
                .attribute_translatable;
            if translatable {
                let raw = attr
                    .normalized_value(XmlVersion::default())
                    .map_err(|e| Error::Xml {
                        offset: 0,
                        source: e,
                    })?;
                let trimmed = raw.trim();
                if !trimmed.is_empty() && self.strategy.is_translatable_text(trimmed) {
                    let key = if self.options.with_context {
                        OwnedCatalogKey::new(Some(format!("{element}@{name}")), trimmed.to_string())
                    } else {
                        OwnedCatalogKey::new(None, trimmed.to_string())
                    };
                    if let Some(translation) = self.translations.get(&key) {
                        let padded = restore_text_padding(translation, raw.trim_start());
                        let escaped = escape(padded.as_str()).into_owned();
                        rewritten.push_attribute(Attribute {
                            key: attr.key,
                            value: Cow::Owned(escaped),
                        });
                        self.applied_attributes += 1;
                        continue;
                    }
                }
            }
            rewritten.push_attribute(attr);
        }
        Ok(rewritten)
    }
}

/// Restores the leading/trailing whitespace of `original` around `translation`.
fn restore_text_padding(translation: &str, original: &str) -> String {
    let leading = original.len() - original.trim_start().len();
    let trailing = original.len() - original.trim_end().len();
    let mut out = String::with_capacity(translation.len() + leading + trailing);
    out.push_str(&original[..leading]);
    out.push_str(translation);
    out.push_str(&original[original.len() - trailing..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::extract_messages;
    use crate::message::CatalogKey;
    use crate::strategy::HtmlStrategy;

    fn map(pairs: &[(&str, &str)]) -> HashMap<OwnedCatalogKey, String> {
        pairs
            .iter()
            .map(|(k, v)| {
                (
                    OwnedCatalogKey::new(None, (*k).to_string()),
                    (*v).to_string(),
                )
            })
            .collect()
    }

    fn ctx_map(pairs: &[((Option<&str>, &str), &str)]) -> HashMap<OwnedCatalogKey, String> {
        pairs
            .iter()
            .map(|((c, k), v)| {
                (
                    OwnedCatalogKey::new(c.map(str::to_string), (*k).to_string()),
                    (*v).to_string(),
                )
            })
            .collect()
    }

    /// Builds a translation map the way the CLI does: extract, then fill in
    /// translations for the msgids listed in `translated`.
    fn from_messages(
        content: &str,
        translated: &[(&str, &str)],
    ) -> HashMap<OwnedCatalogKey, String> {
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        let wanted: HashMap<&str, &str> = translated.iter().copied().collect();
        messages
            .into_iter()
            .filter_map(|m| {
                wanted
                    .get(m.msgid.as_str())
                    .map(|t| (OwnedCatalogKey::new(m.msgctxt, m.msgid), (*t).to_string()))
            })
            .collect()
    }

    #[test]
    fn restores_padding() {
        assert_eq!(restore_text_padding("Hi", "  old  "), "  Hi  ");
        assert_eq!(restore_text_padding("Hi", "old"), "Hi");
        assert_eq!(restore_text_padding("Hi", "\n\told\t"), "\n\tHi\t");
        assert_eq!(restore_text_padding("", "  x  "), "    ");
    }

    #[test]
    fn translates_leaf_text() {
        let m = map(&[("Hello", "Hola"), ("World", "Mundo")]);
        let out = apply_translations("<p>Hello</p><p>World</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hola</p><p>Mundo</p>");
    }

    #[test]
    fn report_counts_applications() {
        let m = map(&[("Hello", "Hola")]);
        let report = apply_translations_with(
            "<p>Hello</p><img alt=\"Sunset\" src=\"x\">",
            &m,
            &HtmlStrategy,
            &ExtractionOptions::default(),
        )
        .unwrap();
        assert_eq!(report.applied_texts, 1);
        assert_eq!(report.applied_attributes, 0);
    }

    #[test]
    fn untranslated_and_unknown_strings_pass_through() {
        let m = map(&[("Hello", "Hola")]);
        let out =
            apply_translations("<p>Hello</p><p>Unknown</p><p></p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hola</p><p>Unknown</p><p></p>");
    }

    #[test]
    fn mixed_content_keeps_everything() {
        let m = map(&[("bold", "negrita"), ("Hello", "Hola"), ("world", "mundo")]);
        let out = apply_translations("<p>Hello <b>bold</b> world</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hola <b>negrita</b> mundo</p>");
    }

    #[test]
    fn text_around_void_elements_is_translated() {
        let m = map(&[("Hello", "Hola"), ("world", "mundo")]);
        let out = apply_translations("<p>Hello<br/>world</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hola<br/>mundo</p>");
    }

    #[test]
    fn nested_translatable_elements_each_translate() {
        let m = map(&[("Outer", "FUERA"), ("Inner", "DENTRO")]);
        let out =
            apply_translations("<div>Outer<span>Inner</span></div>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<div>FUERA<span>DENTRO</span></div>");
    }

    #[test]
    fn structure_and_attributes_preserved() {
        let m = map(&[("Translatable text", "Translated text")]);
        let input = r#"<div class="container"><p style="color: red;">Translatable text</p></div>"#;
        let out = apply_translations(input, &m, &HtmlStrategy).unwrap();
        assert!(out.contains(r#"class="container""#));
        assert!(out.contains(r#"style="color: red;""#));
        assert!(out.contains("Translated text"));
        assert!(!out.contains("Translatable text"));
    }

    #[test]
    fn entities_survive_roundtrip() {
        let m = map(&[("Fish & Chips", "Pescado & Patatas")]);
        let out = apply_translations("<p>Fish &amp; Chips</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Pescado &amp; Patatas</p>");
    }

    #[test]
    fn untranslated_entities_are_kept_verbatim() {
        let out = apply_translations("<p>Fish &amp; Chips</p>", &map(&[]), &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Fish &amp; Chips</p>");
        // Unknown named entities are preserved too.
        let out = apply_translations("<p>a&nbsp;b</p>", &map(&[]), &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>a&nbsp;b</p>");
    }

    #[test]
    fn apostrophes_in_text_stay_literal() {
        let m = map(&[("The Loco Team", "L'équipe Loco")]);
        let out = apply_translations("<p>The Loco Team</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>L'équipe Loco</p>");
    }

    #[test]
    fn quotes_in_attributes_are_reescaped() {
        let m = map(&[("Say \"hi\"", "Di \"hola\"")]);
        let out = apply_translations(r#"<img alt='Say "hi"'>"#, &m, &HtmlStrategy).unwrap();
        assert!(out.contains(r#"alt="Di &quot;hola&quot;""#), "{out}");
    }

    #[test]
    fn attribute_translation() {
        let m = map(&[("A sunset", "Un atardecer"), ("Your name", "Tu nombre")]);
        let input = r#"<img alt="A sunset" src="x.png"><input placeholder="Your name">"#;
        let out = apply_translations(input, &m, &HtmlStrategy).unwrap();
        assert!(out.contains(r#"alt="Un atardecer""#));
        assert!(out.contains(r#"placeholder="Tu nombre""#));
        assert!(out.contains(r#"src="x.png""#));
        let report =
            apply_translations_with(input, &m, &HtmlStrategy, &ExtractionOptions::default())
                .unwrap();
        assert_eq!(report.applied_attributes, 2);
    }

    /// A writer failing on write, on flush, or both: exercises every
    /// `Error::Write` path with one helper instead of two structs.
    #[derive(Debug)]
    struct FailWriter {
        fail_write: bool,
        fail_flush: bool,
    }

    impl std::io::Write for FailWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.fail_write {
                Err(std::io::Error::other("boom"))
            } else {
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            if self.fail_flush {
                Err(std::io::Error::other("flush boom"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn flush_failure_is_reported_as_write_error() {
        let m = map(&[("Hello", "Hola")]);
        let compiler: Compiler<'_, std::collections::hash_map::RandomState, FailWriter> =
            Compiler {
                translations: &m,
                strategy: &HtmlStrategy,
                options: &ExtractionOptions::default(),
                writer: Writer::new(FailWriter {
                    fail_write: false,
                    fail_flush: true,
                }),
                stack: Vec::new(),
                segment: None,
                applied_texts: 0,
                applied_attributes: 0,
            };
        let err = compiler.run("<p>Hello</p>").unwrap_err();
        assert!(matches!(err, Error::Write { .. }), "{err}");
    }

    #[test]
    fn writer_failure_is_reported_as_write_error() {
        let m = map(&[("Hello", "Hola")]);
        let compiler: Compiler<'_, std::collections::hash_map::RandomState, FailWriter> =
            Compiler {
                translations: &m,
                strategy: &HtmlStrategy,
                options: &ExtractionOptions::default(),
                writer: Writer::new(FailWriter {
                    fail_write: true,
                    fail_flush: false,
                }),
                stack: Vec::new(),
                segment: None,
                applied_texts: 0,
                applied_attributes: 0,
            };
        let err = compiler.run("<p>Hello</p>").unwrap_err();
        assert!(matches!(err, Error::Write { .. }), "{err}");
    }

    #[test]
    fn non_failing_writer_compiles_normally() {
        // The success path of the [`FailWriter`] helper: a healthy writer
        // must let compilation finish untouched.
        let m = map(&[("Hello", "Hola")]);
        let compiler: Compiler<'_, std::collections::hash_map::RandomState, FailWriter> =
            Compiler {
                translations: &m,
                strategy: &HtmlStrategy,
                options: &ExtractionOptions::default(),
                writer: Writer::new(FailWriter {
                    fail_write: false,
                    fail_flush: false,
                }),
                stack: Vec::new(),
                segment: None,
                applied_texts: 0,
                applied_attributes: 0,
            };
        let (writer, texts, attrs) = compiler.run("<p>Hello</p>").unwrap();
        assert_eq!((texts, attrs), (1, 0));
        assert_eq!(
            std::mem::size_of_val(&writer),
            std::mem::size_of::<FailWriter>()
        );
    }

    #[test]
    fn translatable_attribute_with_unusable_value_stays_unchanged() {
        // `alt` is translatable but the value is empty: no lookup happens.
        let out = apply_translations(r#"<img alt="">"#, &map(&[]), &HtmlStrategy).unwrap();
        assert_eq!(out, r#"<img alt="">"#);
    }

    #[test]
    fn translatable_attribute_without_translation_stays_unchanged() {
        // `alt` is translatable but the map has no entry for it: the raw
        // attribute must be re-emitted verbatim.
        let out = apply_translations(
            r#"<img alt="Sunset" src="x.png">"#,
            &map(&[]),
            &HtmlStrategy,
        )
        .unwrap();
        assert_eq!(out, r#"<img alt="Sunset" src="x.png">"#);
    }

    #[test]
    fn attribute_on_self_closing_tag() {
        let m = map(&[("A sunset", "Un atardecer")]);
        let out = apply_translations(r#"<img alt="A sunset"/>"#, &m, &HtmlStrategy).unwrap();
        assert!(out.contains(r#"alt="Un atardecer""#));
    }

    #[test]
    fn context_mode_uses_msgctxt() {
        let m = ctx_map(&[((Some("p"), "Hello"), "Hola")]);
        let options = ExtractionOptions::builder().with_context(true).build();
        let out = apply_translations_with("<p>Hello</p>", &m, &HtmlStrategy, &options).unwrap();
        assert_eq!(out.output, "<p>Hola</p>");
        let plain = map(&[("Hello", "Hola")]);
        let out2 =
            apply_translations_with("<p>Hello</p>", &plain, &HtmlStrategy, &options).unwrap();
        assert_eq!(out2.output, "<p>Hello</p>");
    }

    #[test]
    fn context_mode_attributes() {
        let m = ctx_map(&[((Some("img@alt"), "Sunset"), "Atardecer")]);
        let options = ExtractionOptions::builder().with_context(true).build();
        let out =
            apply_translations_with(r#"<img alt="Sunset">"#, &m, &HtmlStrategy, &options).unwrap();
        assert!(out.output.contains(r#"alt="Atardecer""#));
    }

    #[test]
    fn cdata_translated_and_preserved() {
        let m = map(&[("Hello", "Hola")]);
        let out = apply_translations("<p><![CDATA[Hello]]></p>", &m, &HtmlStrategy).unwrap();
        assert!(out.contains("Hola"), "{out}");
        let out =
            apply_translations("<p><![CDATA[raw & stuff]]></p>", &map(&[]), &HtmlStrategy).unwrap();
        assert!(out.contains("raw & stuff"), "{out}");
    }

    #[test]
    fn comments_and_declarations_survive() {
        let input = "<?xml version=\"1.0\"?><!-- note --><p>Hi</p>";
        let out = apply_translations(input, &map(&[("Hi", "Hola")]), &HtmlStrategy).unwrap();
        assert!(out.contains("<!-- note -->"));
        assert!(out.contains("Hola"));
        assert!(out.contains("<?xml version=\"1.0\"?"));
    }

    #[test]
    fn mismatched_end_tag_is_reported() {
        let m = map(&[]);
        let err = apply_translations("<p></div>", &m, &HtmlStrategy).unwrap_err();
        assert!(matches!(err, Error::Xml { .. }));
    }

    #[test]
    fn non_translatable_element_stays_untouched() {
        let m = map(&[("Secret", "SECRETO")]);
        let out = apply_translations("<section>Secret</section>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<section>Secret</section>");
    }

    #[test]
    fn ignored_subtree_is_untouched() {
        let options = ExtractionOptions::builder()
            .ignored_element("section")
            .build();
        let m = map(&[("Secret", "SECRETO"), ("Shown", "VISTO")]);
        let out = apply_translations_with(
            "<section>Secret<span>Secret</span></section><p>Shown</p>",
            &m,
            &HtmlStrategy,
            &options,
        )
        .unwrap()
        .output;
        assert_eq!(
            out,
            "<section>Secret<span>Secret</span></section><p>VISTO</p>"
        );
    }

    #[test]
    fn malformed_attribute_is_reported() {
        let m = map(&[]);
        let err = apply_translations("<p =\"v\">text</p>", &m, &HtmlStrategy).unwrap_err();
        assert!(matches!(err, Error::Xml { .. }));
    }

    #[test]
    fn unknown_entity_in_attribute_is_reported() {
        let m = map(&[]);
        let err = apply_translations("<img alt=\"&nope;\">", &m, &HtmlStrategy).unwrap_err();
        assert!(matches!(err, Error::Xml { .. }));
    }

    #[test]
    fn empty_document() {
        let out = apply_translations("", &map(&[]), &HtmlStrategy).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn extract_then_compile_roundtrip() {
        let content = "<p>Greet</p><img alt=\"Pic\">";
        let m = from_messages(content, &[("Greet", "[Greet]"), ("Pic", "[Pic]")]);
        let out = apply_translations(content, &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>[Greet]</p><img alt=\"[Pic]\">");
    }

    #[test]
    fn multiline_text_translated_with_padding() {
        let m = map(&[("Hello", "Hola")]);
        let out = apply_translations("<p>\n  Hello\n</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>\n  Hola\n</p>");
    }

    #[test]
    fn whitespace_only_segments_pass_through() {
        let m = map(&[("Hello", "Hola")]);
        let out = apply_translations("<p>  <b> Hello </b>  </p>", &m, &HtmlStrategy).unwrap();
        assert!(out.contains("<b> Hola </b>"), "{out}");
    }

    #[test]
    fn segment_translation_key_matches_extraction() {
        // Entity references must not break the segment: extraction sees
        // "Fish & Chips", so compilation must translate the same key.
        let content = "<p>Fish &amp; Chips</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages[0].msgid, "Fish & Chips");
        let m = map(&[("Fish & Chips", "Pescado")]);
        let out = apply_translations(content, &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Pescado</p>");
    }

    #[test]
    fn catalog_key_to_owned() {
        let key = CatalogKey::new(Some("c"), "m");
        let owned = OwnedCatalogKey::new(key.msgctxt.map(str::to_string), key.msgid);
        assert_eq!(owned.borrowed(), key);
    }
}
