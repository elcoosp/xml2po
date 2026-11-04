//! Re-applying translated strings to source documents.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::BuildHasher;

use quick_xml::Writer;
use quick_xml::escape::{escape, partial_escape};
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesCData, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, XmlVersion};

use crate::error::{Error, Result};
use crate::extract::{LineIndex, escape_inline_text, literal_or_unescape, resolve_reference};
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
    /// Translations rejected because their inline markup did not match
    /// the source string; the affected segments keep their source text.
    pub skipped_invalid: usize,
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
    let (buf, applied_texts, applied_attributes, skipped_invalid) = Compiler::<S, Vec<u8>> {
        translations,
        strategy,
        options,
        lines: LineIndex::new(content),
        writer: Writer::new(Vec::new()),
        stack: Vec::new(),
        inline_stack: Vec::new(),
        copy_root: None,
        copy_depth: 0,
        segment: None,
        applied_texts: 0,
        applied_attributes: 0,
        skipped_invalid: 0,
    }
    .run(content)?;
    // Every byte written came from escaped `&str`s, so the buffer is valid
    // UTF-8 by construction.
    let output = String::from_utf8_lossy(&buf).into_owned();
    Ok(CompileReport {
        output,
        applied_texts,
        applied_attributes,
        skipped_invalid,
    })
}

/// One piece of a buffered segment: literal text or (folded) markup.
#[derive(Debug, Clone)]
enum SegPart {
    /// Text run. `plain` is the msgid form: unescaped in classic mode,
    /// XML-escaped when inline placeholders are enabled.
    Text { plain: String },
    /// `<tag …>` start tag. `raw` is the re-emittable form including
    /// brackets. `verbatim` marks copied ignored content, whose raw form
    /// (attributes included) also becomes part of the msgid; folded
    /// placeholders contribute the attribute-free `<tag>` form.
    Open {
        tag: String,
        raw: String,
        verbatim: bool,
    },
    /// `</tag>` end tag.
    Close { tag: String },
    /// `<tag …/>` empty tag (or an HTML void start tag).
    Empty {
        tag: String,
        raw: String,
        verbatim: bool,
    },
}

impl SegPart {
    fn msgid_form(&self) -> String {
        match self {
            Self::Text { plain } => plain.clone(),
            Self::Open { tag, raw, verbatim } => {
                if *verbatim {
                    raw.clone()
                } else {
                    format!("<{tag}>")
                }
            }
            Self::Close { tag } => format!("</{tag}>"),
            Self::Empty { tag, raw, verbatim } => {
                if *verbatim {
                    raw.clone()
                } else {
                    format!("<{tag}/>")
                }
            }
        }
    }

    fn tag_name(&self) -> Option<&str> {
        match self {
            Self::Text { .. } => None,
            Self::Open { tag, .. } | Self::Close { tag } | Self::Empty { tag, .. } => {
                Some(tag.as_str())
            }
        }
    }

    fn is_close(&self) -> bool {
        matches!(self, Self::Close { .. })
    }
}

/// Text accumulated between structural events.
#[derive(Default)]
struct Segment {
    /// Source form of everything buffered (written back verbatim when
    /// untranslated), including folded markup.
    raw: String,
    parts: Vec<SegPart>,
}

/// One open element with flags resolved at push time.
#[derive(Debug, Clone)]
struct ElemState {
    name: String,
    ignored: bool,
    text_translatable: bool,
}

/// A scanned translation piece.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    Markup { tag: String, close: bool },
}

/// Streaming translation pass over one document.
///
/// `W` is injectable so tests can drive the IO error paths with a failing
/// writer; production always uses an in-memory `Vec<u8>`.
struct Compiler<'a, S: BuildHasher, W: std::io::Write> {
    translations: &'a HashMap<OwnedCatalogKey, String, S>,
    strategy: &'a dyn ExtractionStrategy,
    options: &'a ExtractionOptions,
    /// Byte-offset → line/column resolution for XML errors.
    lines: LineIndex,
    writer: Writer<W>,
    /// Open elements; `ignored`/`text_translatable` are inherited.
    stack: Vec<ElemState>,
    /// Open inline placeholders awaiting their end tag.
    inline_stack: Vec<String>,
    /// Active verbatim copy of an ignored inline element.
    copy_root: Option<String>,
    copy_depth: usize,
    segment: Option<Segment>,
    applied_texts: usize,
    applied_attributes: usize,
    skipped_invalid: usize,
}

impl<S: BuildHasher, W: std::io::Write> Compiler<'_, S, W> {
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

    /// Streams `content` through the compiler, returning the output buffer
    /// and application counters (the caller converts the buffer).
    ///
    /// The event loop is one readable `match` arm per XML event; slicing
    /// it up would scatter the state transitions.
    #[allow(clippy::too_many_lines)]
    fn run(mut self, content: &str) -> Result<(W, usize, usize, usize)> {
        let mut reader = Reader::from_str(content);
        // Mirror the extractor: HTML is parsed browser-style, XML strictly.
        if self.strategy.format_name() == "html" {
            reader.config_mut().check_end_names = false;
        }
        let mut buf = Vec::new();
        loop {
            let offset = reader.buffer_position();
            let event = reader
                .read_event_into(&mut buf)
                .map_err(|e| self.xml_error(offset, e))?;
            match event {
                Event::Start(e) => self.handle_start(&e, offset)?,
                Event::Empty(e) => self.handle_empty(&e, offset)?,
                Event::Text(e) => {
                    let copying = self.copy_root.is_some();
                    let inline = self.options.inline_placeholders;
                    let seg = self.segment.get_or_insert_default();
                    if copying {
                        // Verbatim copy: source text is already escaped.
                        let raw = e.as_ref().to_string();
                        seg.raw.push_str(&raw);
                        seg.parts.push(SegPart::Text { plain: raw });
                    } else {
                        seg.raw.push_str(&e);
                        let plain = literal_or_unescape(&e);
                        seg.parts.push(SegPart::Text {
                            plain: if inline {
                                escape_inline_text(&plain)
                            } else {
                                plain
                            },
                        });
                    }
                }
                Event::GeneralRef(e) => {
                    let name: &str = &e; // Deref<Target = str>: e.g. "amp" or "#x30"
                    let (raw_form, plain_form) = resolve_reference(name);
                    let copying = self.copy_root.is_some();
                    let inline = self.options.inline_placeholders;
                    let seg = self.segment.get_or_insert_default();
                    if copying {
                        seg.raw.push_str(&raw_form);
                        seg.parts.push(SegPart::Text { plain: raw_form });
                    } else {
                        seg.raw.push_str(&raw_form);
                        seg.parts.push(SegPart::Text {
                            plain: if inline {
                                escape_inline_text(&plain_form)
                            } else {
                                plain_form
                            },
                        });
                    }
                }
                Event::CData(e) => {
                    let inner = e.into_inner();
                    if self.copy_root.is_some() {
                        let seg = self.segment.get_or_insert_default();
                        seg.raw.push_str(&inner);
                        seg.parts.push(SegPart::Text {
                            plain: inner.clone().into_owned(),
                        });
                        continue;
                    }
                    self.flush_segment(None)?;
                    let trimmed = inner.trim().to_string();
                    let element = self.top_element().cloned();
                    let key = if self.options.inline_placeholders {
                        escape_inline_text(&trimmed)
                    } else {
                        trimmed.clone()
                    };
                    let translated = element.and_then(|el| self.lookup(&key, &el));
                    match translated {
                        Some(text) => {
                            self.applied_texts += 1;
                            // CDATA content is literal: unescape entities.
                            let plain = unescape_xml_text(&text);
                            self.write(Event::CData(BytesCData::new(plain.trim())))?;
                        }
                        None => self.write(Event::CData(BytesCData::new(inner)))?,
                    }
                }
                Event::End(e) => self.handle_end(&e)?,
                Event::Eof => break,
                other => {
                    self.flush_segment(None)?;
                    self.write(other)?;
                }
            }
            buf.clear();
        }
        self.flush_segment(None)?;
        self.writer
            .get_mut()
            .flush()
            .map_err(|source| Error::Write { source })?;
        Ok((
            self.writer.into_inner(),
            self.applied_texts,
            self.applied_attributes,
            self.skipped_invalid,
        ))
    }

    /// Writes an event, mapping quick-xml IO errors to the crate error.
    fn write(&mut self, event: Event<'_>) -> Result<()> {
        self.writer
            .write_event(event)
            .map_err(|source| Error::Write { source })
    }

    fn top_element(&self) -> Option<&ElemState> {
        self.stack.last()
    }

    fn folding(&self, element: &str) -> bool {
        self.options.inline_placeholders && self.strategy.is_inline_element(element)
    }

    /// Serializes a rewritten start tag back to `<name attr="v" …>` form.
    fn tag_string(rewritten: &BytesStart<'_>) -> String {
        format!("<{}>", rewritten.as_ref())
    }

    fn handle_start(&mut self, e: &BytesStart<'_>, offset: u64) -> Result<()> {
        let element = e.name().0.to_string();
        let parent_ignored = self.top_element().is_some_and(|s| s.ignored);
        let parent_trans = self.top_element().is_some_and(|s| s.text_translatable);

        // Rewrite attributes first: folded tags re-emit this form, so
        // attribute translations inside folded elements are honored too.
        let (rewritten, attrs) = self.rewrite_attributes(e, element.as_str(), offset)?;
        let directive = crate::strategy::translate_directive(&attrs);
        let rule = crate::strategy::effective_rule(
            self.strategy,
            element.as_str(),
            directive,
            self.options,
        );
        let ignored = parent_ignored || rule.ignored;

        // Inside a verbatim copy: buffer the raw form, keep translating
        // nothing (the attrs were rewritten but ignored rules skip them).
        if let Some(root) = self.copy_root.clone() {
            let raw = Self::tag_string(&rewritten);
            let seg = self.segment.get_or_insert_default();
            seg.raw.push_str(&raw);
            seg.parts.push(SegPart::Open {
                tag: element.clone(),
                verbatim: true,
                raw,
            });
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

        // Folded inline placeholder: buffer, no eager write.
        if !ignored && self.folding(&element) && parent_trans {
            let raw = Self::tag_string(&rewritten);
            let seg = self.segment.get_or_insert_default();
            seg.raw.push_str(&raw);
            seg.parts.push(SegPart::Open {
                tag: element.clone(),
                verbatim: false,
                raw,
            });
            self.inline_stack.push(element.clone());
            self.stack.push(ElemState {
                name: element,
                ignored: false,
                text_translatable: rule.text_translatable,
            });
            return Ok(());
        }

        // Verbatim copy entry for ignored inline content.
        if ignored && self.folding(&element) && parent_trans {
            let raw = Self::tag_string(&rewritten);
            let seg = self.segment.get_or_insert_default();
            seg.raw.push_str(&raw);
            seg.parts.push(SegPart::Open {
                tag: element.clone(),
                verbatim: true,
                raw,
            });
            self.copy_root = Some(element.clone());
            self.copy_depth = 1;
            self.stack.push(ElemState {
                name: element,
                ignored: true,
                text_translatable: false,
            });
            return Ok(());
        }

        self.flush_segment(None)?;
        // HTML void elements never close: behave like empty tags and do
        // not push their state onto either stack.
        if crate::extract::is_void_element(&element) {
            if self.folding(&element) && parent_trans {
                let raw = Self::tag_string(&rewritten);
                let seg = self.segment.get_or_insert_default();
                seg.raw.push_str(&raw);
                seg.parts.push(SegPart::Empty {
                    tag: element,
                    verbatim: ignored,
                    raw,
                });
                return Ok(());
            }
            self.write(Event::Start(rewritten))?;
            return Ok(());
        }
        self.write(Event::Start(rewritten))?;
        self.stack.push(ElemState {
            name: element,
            ignored,
            text_translatable: rule.text_translatable,
        });
        Ok(())
    }

    fn handle_empty(&mut self, e: &BytesStart<'_>, offset: u64) -> Result<()> {
        let element = e.name().0.to_string();
        let parent_ignored = self.top_element().is_some_and(|s| s.ignored);
        let parent_trans = self.top_element().is_some_and(|s| s.text_translatable);

        let (rewritten, attrs) = self.rewrite_attributes(e, element.as_str(), offset)?;
        let directive = crate::strategy::translate_directive(&attrs);
        let rule = crate::strategy::effective_rule(
            self.strategy,
            element.as_str(),
            directive,
            self.options,
        );
        let ignored = parent_ignored || rule.ignored;
        let raw = if raw_looks_self_closing(e.as_ref()) {
            Self::tag_string(&rewritten)
        } else {
            format!("<{}/>", rewritten.as_ref())
        };

        if self.copy_root.is_some() {
            let seg = self.segment.get_or_insert_default();
            seg.raw.push_str(&raw);
            seg.parts.push(SegPart::Empty {
                tag: element,
                verbatim: true,
                raw,
            });
            return Ok(());
        }

        if !ignored && self.folding(&element) && parent_trans {
            let seg = self.segment.get_or_insert_default();
            seg.raw.push_str(&raw);
            seg.parts.push(SegPart::Empty {
                tag: element,
                verbatim: false,
                raw,
            });
            return Ok(());
        }

        self.flush_segment(None)?;
        if ignored && self.folding(&element) && parent_trans {
            let seg = self.segment.get_or_insert_default();
            seg.raw.push_str(&raw);
            seg.parts.push(SegPart::Empty {
                tag: element,
                verbatim: true,
                raw,
            });
            return Ok(());
        }
        self.write(Event::Empty(rewritten))
    }

    fn handle_end(&mut self, e: &BytesEnd<'_>) -> Result<()> {
        let name = e.name().0.to_string();

        if let Some(root) = self.copy_root.clone() {
            let raw = format!("</{name}>");
            let seg = self.segment.get_or_insert_default();
            seg.raw.push_str(&raw);
            seg.parts.push(SegPart::Close { tag: name.clone() });
            if name == root {
                self.copy_depth -= 1;
                if self.copy_depth == 0 {
                    self.copy_root = None;
                }
            }
            self.stack.pop();
            return Ok(());
        }

        let closing = self.top_element().is_some_and(|s| s.name == name);
        let closing_state = if closing {
            self.stack.last().cloned()
        } else {
            None
        };
        if closing && self.folding(&name) && self.inline_stack.last().is_some_and(|t| *t == name) {
            self.inline_stack.pop();
            let raw = format!("</{name}>");
            let seg = self.segment.get_or_insert_default();
            seg.raw.push_str(&raw);
            seg.parts.push(SegPart::Close { tag: name });
            self.stack.pop();
            return Ok(());
        }
        self.flush_segment(closing_state)?;
        self.write(Event::End(e.clone()))?;
        self.stack.pop();
        Ok(())
    }

    /// Emits the pending segment, translated when possible.
    fn flush_segment(&mut self, closed: Option<ElemState>) -> Result<()> {
        let Some(mut seg) = self.segment.take() else {
            return Ok(());
        };
        if seg.parts.is_empty() {
            return Ok(());
        }
        let element = closed.or_else(|| self.top_element().cloned());
        let Some(element) = element else {
            self.write(Event::Text(BytesText::from_escaped(seg.raw)))?;
            return Ok(());
        };

        let inline = self.options.inline_placeholders;
        let full = seg
            .parts
            .iter()
            .map(SegPart::msgid_form)
            .collect::<String>();
        // Trimmed view: leading/trailing whitespace lives in the outer
        // text parts and is restored via padding, so both sides derive
        // the same key from the same trimmed string.
        let mut tparts = std::mem::take(&mut seg.parts);
        if let Some(SegPart::Text { plain }) = tparts.first_mut() {
            *plain = plain.trim_start().to_string();
        }
        if let Some(SegPart::Text { plain }) = tparts.last_mut() {
            *plain = plain.trim_end().to_string();
        }
        tparts.retain(|p| !matches!(p, SegPart::Text { plain } if plain.is_empty()));
        seg.parts = tparts;
        let trimmed = full.trim();
        let _ = trimmed;

        // Mirror extraction: dangling placeholders at a boundary strip the
        // markup so both sides derive the same key.
        let key_full = seg
            .parts
            .iter()
            .map(SegPart::msgid_form)
            .collect::<String>();
        let key_trimmed = key_full.trim();
        if inline && !self.inline_stack.is_empty() {
            self.inline_stack.clear();
            let stripped = crate::extract::strip_placeholders(key_trimmed)
                .trim()
                .to_string();
            if let Some(translation) = self.lookup(&stripped, &element) {
                let translation = unescape_xml_text(&translation);
                let padded = restore_text_padding(&translation, &full);
                self.applied_texts += 1;
                self.write(Event::Text(BytesText::from_escaped(partial_escape(
                    &padded,
                ))))?;
                return Ok(());
            }
            self.write(Event::Text(BytesText::from_escaped(seg.raw)))?;
            return Ok(());
        }

        if inline {
            let stripped = crate::extract::strip_placeholders(key_trimmed);
            if stripped != key_trimmed && !self.strategy.is_translatable_text(stripped.trim()) {
                self.write(Event::Text(BytesText::from_escaped(seg.raw)))?;
                return Ok(());
            }
        }

        let Some(translation) = self.lookup(key_trimmed, &element) else {
            self.write(Event::Text(BytesText::from_escaped(seg.raw)))?;
            return Ok(());
        };

        if !inline || !seg.parts.iter().any(|p| p.tag_name().is_some()) {
            // Classic path: a single escaped text run. Entity forms in
            // the translation resolve before re-escaping.
            let translation = unescape_xml_text(&translation);
            let padded = restore_text_padding(&translation, &full);
            self.applied_texts += 1;
            self.write(Event::Text(BytesText::from_escaped(partial_escape(
                &padded,
            ))))?;
            return Ok(());
        }

        // Inline path: rebuild the segment from the translation, keeping
        // the source tag forms.
        let Some(final_text) = rebuild_with_translation(&seg.parts, &translation) else {
            self.skipped_invalid += 1;
            self.write(Event::Text(BytesText::from_escaped(seg.raw)))?;
            return Ok(());
        };
        let padded = restore_text_padding(&final_text, &full);
        self.applied_texts += 1;
        self.write(Event::Text(BytesText::from_escaped(padded)))?;
        Ok(())
    }

    /// Gates and resolves the translation for a trimmed key under
    /// `element`.
    fn lookup(&self, trimmed: &str, element: &ElemState) -> Option<String> {
        if trimmed.is_empty()
            || element.ignored
            || self.top_element().is_some_and(|s| s.ignored)
            || !element.text_translatable
        {
            return None;
        }
        if !self.strategy.is_translatable_text(trimmed) {
            return None;
        }
        let key = if self.options.with_context {
            OwnedCatalogKey::new(Some(element.name.clone()), trimmed.to_string())
        } else {
            OwnedCatalogKey::new(None, trimmed.to_string())
        };
        self.translations.get(&key).cloned()
    }

    /// Returns a copy of the start/empty tag with translated attributes
    /// plus the normalized `(name, value)` pairs for rule resolution.
    fn rewrite_attributes(
        &mut self,
        start: &BytesStart<'_>,
        element: &str,
        offset: u64,
    ) -> Result<(BytesStart<'static>, Vec<(String, String)>)> {
        // Collect everything first so the element-level `translate`
        // directive can veto attribute translation.
        let mut collected = Vec::new();
        for attr in start.attributes() {
            let attr = attr.map_err(|e| self.xml_error(offset, e.into()))?;
            let value = attr
                .normalized_value(XmlVersion::default())
                .map_err(|e| self.xml_error(offset, e))?
                .into_owned();
            collected.push((attr.key, attr.key.0.to_string(), value));
        }
        let normalized: Vec<(String, String)> = collected
            .iter()
            .map(|(_, name, value)| (name.clone(), value.clone()))
            .collect();
        let directive = crate::strategy::translate_directive(&normalized);

        let mut rewritten = BytesStart::new(element.to_string());
        for (key, name, value) in collected {
            let translatable = directive != Some(false)
                && self
                    .strategy
                    .resolve(element, Some(name.as_str()), self.options)
                    .attribute_translatable;
            if translatable {
                let trimmed = value.trim();
                if !trimmed.is_empty() && self.strategy.is_translatable_text(trimmed) {
                    let catalog_key = if self.options.with_context {
                        OwnedCatalogKey::new(Some(format!("{element}@{name}")), trimmed.to_string())
                    } else {
                        OwnedCatalogKey::new(None, trimmed.to_string())
                    };
                    if let Some(translation) = self.translations.get(&catalog_key) {
                        let padded = restore_text_padding(translation, value.trim_start());
                        let escaped = escape(padded.as_str()).into_owned();
                        rewritten.push_attribute(Attribute {
                            key,
                            value: Cow::Owned(escaped),
                        });
                        self.applied_attributes += 1;
                        continue;
                    }
                }
            }
            rewritten.push_attribute(Attribute {
                key,
                value: Cow::Owned(value),
            });
        }
        Ok((rewritten, normalized))
    }
}

/// Whether the raw tag content already ends with a self-closing slash.
fn raw_looks_self_closing(raw: &str) -> bool {
    raw.ends_with('/')
}

/// Unescapes the XML entities a translation may carry; unknown escapes
/// pass through untouched.
#[must_use]
pub fn unescape_xml_text(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        let mut entity = String::new();
        let mut closed = false;
        while let Some(&n) = chars.peek() {
            if n == ';' {
                chars.next();
                closed = true;
                break;
            }
            if !n.is_ascii_alphanumeric() && n != '#' {
                break;
            }
            entity.push(n);
            chars.next();
        }
        if closed {
            match entity.as_str() {
                "amp" => out.push('&'),
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "quot" => out.push('"'),
                "apos" => out.push('\''),
                other => {
                    out.push('&');
                    out.push_str(other);
                    out.push(';');
                }
            }
        } else {
            out.push('&');
            out.push_str(&entity);
        }
    }
    out
}

/// Scans a translation into text and markup pieces. Text pieces are
/// unescaped; `<` runs that do not form a plausible tag stay literal.
fn scan_pieces(text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let mut j = i + 1;
            let close = j < bytes.len() && bytes[j] == b'/';
            if close {
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
                // A tag: skip attributes through `>` (or stop at end).
                while j < bytes.len() && bytes[j] != b'>' {
                    j += 1;
                }
                if j < bytes.len() {
                    j += 1;
                }
                if !current.is_empty() {
                    pieces.push(Piece::Text(unescape_xml_text(&current)));
                    current.clear();
                }
                pieces.push(Piece::Markup {
                    tag: text[name_start..]
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == ':' || *c == '_' || *c == '-')
                        .collect(),
                    close,
                });
                i = j;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap_or('<');
        current.push(ch);
        i += ch.len_utf8();
    }
    if !current.is_empty() {
        pieces.push(Piece::Text(unescape_xml_text(&current)));
    }
    pieces
}

/// Rebuilds the translated segment from the translation, itstool-style:
/// the *tag sequences* of source and translation must be identical (same
/// names, same order, same open/close roles) and every translation tag
/// maps to the corresponding source tag form, which is re-emitted with
/// its original attributes. Text between tags may be freely reordered,
/// split or merged by the translator.
///
/// Returns the composed (untrimmed, unescaped-where-markup) output text,
/// or `None` when the translation's markup does not match the source.
fn rebuild_with_translation(parts: &[SegPart], translation: &str) -> Option<String> {
    let pieces = scan_pieces(translation);
    let src_tags: Vec<(String, bool)> = parts
        .iter()
        .filter_map(|p| p.tag_name().map(|t| (t.to_string(), p.is_close())))
        .collect();
    let dst_tags: Vec<(String, bool)> = pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Markup { tag, close } => Some((tag.clone(), *close)),
            Piece::Text(_) => None,
        })
        .collect();
    if src_tags != dst_tags {
        return None;
    }
    let mut source_tags = parts.iter().filter(|p| p.tag_name().is_some());
    let mut out = String::with_capacity(translation.len());
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(&partial_escape(&text)),
            Piece::Markup { .. } => match source_tags.next()? {
                SegPart::Open { raw, .. } | SegPart::Empty { raw, .. } => out.push_str(raw),
                SegPart::Close { tag } => {
                    let close = format!("</{tag}>");
                    out.push_str(&close);
                }
                SegPart::Text { .. } => return None,
            },
        }
    }
    Some(out)
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
        // Inline placeholders (default): one message, tags re-inserted.
        let m = map(&[("Hello <b>bold</b> world", "Hola <b>negrita</b> mundo")]);
        let out = apply_translations("<p>Hello <b>bold</b> world</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hola <b>negrita</b> mundo</p>");

        // Classic split behavior with inline placeholders disabled.
        let m = map(&[("bold", "negrita"), ("Hello", "Hola"), ("world", "mundo")]);
        let options = ExtractionOptions::builder()
            .inline_placeholders(false)
            .build();
        let out = apply_translations_with(
            "<p>Hello <b>bold</b> world</p>",
            &m,
            &HtmlStrategy,
            &options,
        )
        .unwrap()
        .output;
        assert_eq!(out, "<p>Hola <b>negrita</b> mundo</p>");
    }

    #[test]
    fn text_around_void_elements_is_translated() {
        // The void `<br/>` folds into the message as a `<br/>` placeholder.
        let m = map(&[("Hello<br/>world", "Hola<br/>mundo")]);
        let out = apply_translations("<p>Hello<br/>world</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hola<br/>mundo</p>");
    }

    #[test]
    fn nested_translatable_elements_each_translate() {
        // Inline folding: one message carries both texts.
        let m = map(&[("Outer<span>Inner</span>", "FUERA<span>DENTRO</span>")]);
        let out =
            apply_translations("<div>Outer<span>Inner</span></div>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<div>FUERA<span>DENTRO</span></div>");

        // Classic split behavior.
        let m = map(&[("Outer", "FUERA"), ("Inner", "DENTRO")]);
        let options = ExtractionOptions::builder()
            .inline_placeholders(false)
            .build();
        let out = apply_translations_with(
            "<div>Outer<span>Inner</span></div>",
            &m,
            &HtmlStrategy,
            &options,
        )
        .unwrap()
        .output;
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
        let m = map(&[("Fish &amp; Chips", "Pescado &amp; Patatas")]);
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
                lines: LineIndex::new("<p>Hello</p>"),
                writer: Writer::new(FailWriter {
                    fail_write: false,
                    fail_flush: true,
                }),
                stack: Vec::new(),
                inline_stack: Vec::new(),
                copy_root: None,
                copy_depth: 0,
                segment: None,
                applied_texts: 0,
                applied_attributes: 0,
                skipped_invalid: 0,
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
                lines: LineIndex::new("<p>Hello</p>"),
                writer: Writer::new(FailWriter {
                    fail_write: true,
                    fail_flush: false,
                }),
                stack: Vec::new(),
                inline_stack: Vec::new(),
                copy_root: None,
                copy_depth: 0,
                segment: None,
                applied_texts: 0,
                applied_attributes: 0,
                skipped_invalid: 0,
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
                lines: LineIndex::new("<p>Hello</p>"),
                writer: Writer::new(FailWriter {
                    fail_write: false,
                    fail_flush: false,
                }),
                stack: Vec::new(),
                inline_stack: Vec::new(),
                copy_root: None,
                copy_depth: 0,
                segment: None,
                applied_texts: 0,
                applied_attributes: 0,
                skipped_invalid: 0,
            };
        let (writer, texts, attrs, invalid) = compiler.run("<p>Hello</p>").unwrap();
        assert_eq!((texts, attrs, invalid), (1, 0, 0));
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
    fn mismatched_end_tag_is_reported_for_xml() {
        let m = map(&[]);
        let err = apply_translations("<p></div>", &m, &crate::strategy::XmlStrategy).unwrap_err();
        assert!(matches!(err, Error::Xml { .. }));
    }

    #[test]
    fn html_tag_soup_is_translated_and_preserved() {
        let m = map(&[("Hello", "Hola")]);
        let out = apply_translations(
            r#"<html><body><img src="x.jpg"><p>Hello</p></body></html>"#,
            &m,
            &HtmlStrategy,
        )
        .unwrap();
        assert!(out.contains(r#"<img src="x.jpg">"#), "{out}");
        assert!(out.contains("Hola"), "{out}");
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
        // With inline folding the whole padded segment is one message.
        let m = map(&[("<b> Hello </b>", "<b> Hola </b>")]);
        let out = apply_translations("<p>  <b> Hello </b>  </p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>  <b> Hola </b>  </p>");
    }

    #[test]
    fn segment_translation_key_matches_extraction() {
        // Inline msgids are XML-escaped: extraction and compilation must
        // derive the exact same key from the same source.
        let content = "<p>Fish &amp; Chips</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages[0].msgid, "Fish &amp; Chips");
        let m = map(&[("Fish &amp; Chips", "Pescado &amp; Papas")]);
        let out = apply_translations(content, &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Pescado &amp; Papas</p>");
    }

    #[test]
    fn catalog_key_to_owned() {
        let key = CatalogKey::new(Some("c"), "m");
        let owned = OwnedCatalogKey::new(key.msgctxt.map(str::to_string), key.msgid);
        assert_eq!(owned.borrowed(), key);
    }
    // ------------------------------------------------------------------
    // v0.3.0: inline placeholder reconstruction.
    // ------------------------------------------------------------------

    #[test]
    fn inline_tag_attributes_are_restored_from_source() {
        let m = map(&[("<a>link</a>", "<a>enlace</a>")]);
        let out = apply_translations(
            r#"<p><a href="/x" class="btn">link</a></p>"#,
            &m,
            &HtmlStrategy,
        )
        .unwrap();
        assert_eq!(out, r#"<p><a href="/x" class="btn">enlace</a></p>"#);
    }

    #[test]
    fn inline_nested_placeholder_reordering_is_allowed() {
        // Word order changes between languages: the tag *sequence* must
        // match, the text between tags may move.
        let m = map(&[(
            "Hi <em>very <b>bold</b></em> end",
            "Hi <em><b>negrita</b> muy</em> fin",
        )]);
        let out = apply_translations("<p>Hi <em>very <b>bold</b></em> end</p>", &m, &HtmlStrategy)
            .unwrap();
        assert_eq!(out, "<p>Hi <em><b>negrita</b> muy</em> fin</p>");
    }

    #[test]
    fn inline_unclosed_placeholder_in_translation_is_rejected() {
        let m = map(&[("Hello <b>bold</b> world", "Hola <b>mundo")]);
        let report = apply_translations_with(
            "<p>Hello <b>bold</b> world</p>",
            &m,
            &HtmlStrategy,
            &ExtractionOptions::default(),
        )
        .unwrap();
        // Source text is kept, the invalid translation is counted.
        assert_eq!(report.output, "<p>Hello <b>bold</b> world</p>");
        assert_eq!(report.skipped_invalid, 1);
        assert_eq!(report.applied_texts, 0);
    }

    #[test]
    fn inline_reordered_tags_are_rejected() {
        let m = map(&[("Hello <b>bold</b> world", "Hola </b><b>mundo")]);
        let out = apply_translations("<p>Hello <b>bold</b> world</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hello <b>bold</b> world</p>");
    }

    #[test]
    fn inline_missing_tags_are_rejected() {
        let m = map(&[("Hello <b>bold</b> world", "Hola mundo sin etiquetas")]);
        let out = apply_translations("<p>Hello <b>bold</b> world</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hello <b>bold</b> world</p>");
    }

    #[test]
    fn inline_extra_unknown_tags_are_rejected() {
        let m = map(&[("Hello <b>bold</b> world", "Hola <x>mundo</x>")]);
        let out = apply_translations("<p>Hello <b>bold</b> world</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hello <b>bold</b> world</p>");
    }

    #[test]
    fn inline_void_start_tag_preserved_in_placeholder_output() {
        // The `<br/>` folds into the message; the source form (including
        // any attributes) is re-emitted.
        let m = map(&[("Line<br/>break", "Línea<br/>ruptura")]);
        let out = apply_translations("<p>Line<br/>break</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Línea<br/>ruptura</p>");
    }

    #[test]
    fn inline_escaped_text_roundtrip() {
        let m = map(&[("5 &lt; 6 &amp; counting", "5 &lt; 6 y contando")]);
        let out = apply_translations("<p>5 &lt; 6 &amp; counting</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>5 &lt; 6 y contando</p>");
    }

    #[test]
    fn inline_entities_in_translations_are_unescaped() {
        let m = map(&[("Fish &amp; <b>Chips</b>", "Pescado &amp; <b>Patatas</b>")]);
        let out = apply_translations("<p>Fish &amp; <b>Chips</b></p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Pescado &amp; <b>Patatas</b></p>");
    }

    #[test]
    fn inline_translate_no_content_passes_through_verbatim() {
        let key = r#"Name: <span translate="no">ACME Corp</span> inc."#;
        let m = map(&[(key, r#"Nombre: <span translate="no">ACME Corp</span> S.A."#)]);
        let out = apply_translations(
            r#"<p>Name: <span translate="no">ACME Corp</span> inc.</p>"#,
            &m,
            &HtmlStrategy,
        )
        .unwrap();
        assert_eq!(
            out,
            r#"<p>Nombre: <span translate="no">ACME Corp</span> S.A.</p>"#
        );
    }

    #[test]
    fn translate_no_block_is_never_translated() {
        let m = map(&[("Skip", "Nope"), ("Go", "Va")]);
        let out = apply_translations(
            r#"<section translate="no">Skip</section><p>Go</p>"#,
            &m,
            &HtmlStrategy,
        )
        .unwrap();
        assert_eq!(out, r#"<section translate="no">Skip</section><p>Va</p>"#);
    }

    #[test]
    fn translate_yes_forces_compilation() {
        let m = map(&[("Caption", "Subtítulo")]);
        let out = apply_translations(
            r#"<figcaption translate="yes">Caption</figcaption>"#,
            &m,
            &HtmlStrategy,
        )
        .unwrap();
        assert!(out.contains("Subtítulo"), "{out}");
    }

    #[test]
    fn void_translate_no_does_not_poison_siblings() {
        // `<img>` has no end tag; its ignored state must not swallow the
        // following paragraph.
        let m = map(&[("Shown", "Visto"), ("Nope", "No")]);
        let out = apply_translations(
            r#"<img alt="Nope" translate="no"><p>Shown</p>"#,
            &m,
            &HtmlStrategy,
        )
        .unwrap();
        assert!(out.contains(r#"alt="Nope""#), "{out}");
        assert!(out.contains("Visto"), "{out}");
        assert!(!out.contains(r#"alt="No""#), "{out}");
    }

    #[test]
    fn inline_extract_then_compile_roundtrip() {
        let content = "<p>Hello <b>bold</b> world</p>";
        let messages = extract_messages(content, "t.html", &HtmlStrategy).unwrap();
        assert_eq!(messages.len(), 1);
        let m: HashMap<OwnedCatalogKey, String> = messages
            .iter()
            .map(|msg| {
                (
                    OwnedCatalogKey::new(msg.msgctxt.clone(), msg.msgid.clone()),
                    msg.msgid
                        .replace("Hello", "Hola")
                        .replace("bold", "negrita"),
                )
            })
            .collect();
        let out = apply_translations(content, &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>Hola <b>negrita</b> world</p>");
    }

    #[test]
    fn inline_context_mode_compile() {
        let m = ctx_map(&[(
            (Some("p"), "Hello <b>bold</b> world"),
            "Hola <b>negrita</b> mundo",
        )]);
        let options = ExtractionOptions::builder().with_context(true).build();
        let out = apply_translations_with(
            "<p>Hello <b>bold</b> world</p>",
            &m,
            &HtmlStrategy,
            &options,
        )
        .unwrap();
        assert_eq!(out.output, "<p>Hola <b>negrita</b> mundo</p>");
    }

    #[test]
    fn mjml_inline_strong_compile() {
        use crate::strategy::MjmlStrategy;
        let m = map(&[(
            "Hello <strong>bold</strong> world",
            "Hola <strong>negrita</strong> mundo",
        )]);
        let out = apply_translations(
            "<mj-text>Hello <strong>bold</strong> world</mj-text>",
            &m,
            &MjmlStrategy,
        )
        .unwrap();
        assert_eq!(
            out,
            "<mj-text>Hola <strong>negrita</strong> mundo</mj-text>"
        );
    }

    #[test]
    fn inline_padding_restored_around_placeholder_message() {
        let m = map(&[("<b> Hello </b>", "<b> Hola </b>")]);
        let out = apply_translations("<p>\n  <b> Hello </b>\n</p>", &m, &HtmlStrategy).unwrap();
        assert_eq!(out, "<p>\n  <b> Hola </b>\n</p>");
    }

    #[test]
    fn untranslated_inline_segment_passes_through_byte_identical() {
        let out = apply_translations(
            "<p>Hello <b class=\"x\">bold</b> &amp; more</p>",
            &map(&[]),
            &HtmlStrategy,
        )
        .unwrap();
        assert_eq!(out, "<p>Hello <b class=\"x\">bold</b> &amp; more</p>");
    }
}
