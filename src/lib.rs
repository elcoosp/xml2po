// lib.rs
use anyhow::Result;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::{collections::HashMap, path::PathBuf};

#[cfg(feature = "rspolib")]
use rspolib::{FileOptions, POFile, Save, pofile};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Trait for format-specific extraction rules
pub trait ExtractionStrategy: Send + Sync {
    /// Determine if an element should have its text content extracted
    fn is_translatable_element(&self, element_name: &str) -> bool;

    /// Determine if text content should be translated
    fn is_translatable_text(&self, text: &str) -> bool;

    /// Get the file extension for this format
    fn file_extension(&self) -> &str;

    /// Get the format name
    fn format_name(&self) -> &str;
}

#[cfg(feature = "mjml")]
pub struct MjmlStrategy;

#[cfg(feature = "mjml")]
impl ExtractionStrategy for MjmlStrategy {
    fn is_translatable_element(&self, element_name: &str) -> bool {
        matches!(
            element_name,
            "mj-title" | "mj-text" | "mj-button" | "strong"
        )
    }

    fn is_translatable_text(&self, text: &str) -> bool {
        is_translatable_text_common(text)
    }

    fn file_extension(&self) -> &str {
        ".mjml"
    }

    fn format_name(&self) -> &str {
        "mjml"
    }
}

#[cfg(feature = "html")]
pub struct HtmlStrategy;

#[cfg(feature = "html")]
impl ExtractionStrategy for HtmlStrategy {
    fn is_translatable_element(&self, element_name: &str) -> bool {
        matches!(
            element_name,
            "p" | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "span"
                | "div"
                | "a"
                | "button"
                | "label"
                | "li"
                | "td"
                | "th"
                | "title"
                | "option"
                | "strong"
                | "em"
                | "b"
                | "i"
        )
    }

    fn is_translatable_text(&self, text: &str) -> bool {
        is_translatable_text_common(text)
    }

    fn file_extension(&self) -> &str {
        ".html"
    }

    fn format_name(&self) -> &str {
        "html"
    }
}

pub fn is_translatable_text_common(text: &str) -> bool {
    // Exclude URLs
    if text.starts_with("http://") || text.starts_with("https://") {
        return false;
    }

    // Exclude text that looks like URLs or file paths
    if text.contains("://") {
        return false;
    }

    // Exclude text that looks like email addresses (but allow placeholders)
    if text.contains("@") && text.contains(".") && !text.contains("{{") {
        return false;
    }

    // Exclude pure whitespace
    if text.trim().is_empty() {
        return false;
    }

    // Exclude text that's purely numbers or symbols
    let has_letter = text.chars().any(|c| c.is_alphabetic());
    if !has_letter {
        return false;
    }

    true
}

/// Get a strategy instance by format name
pub fn get_strategy(format: &str) -> Result<Box<dyn ExtractionStrategy>> {
    match format.to_lowercase().as_str() {
        #[cfg(feature = "mjml")]
        "mjml" => Ok(Box::new(MjmlStrategy)),
        #[cfg(feature = "html")]
        "html" => Ok(Box::new(HtmlStrategy)),
        _ => {
            anyhow::bail!(
                "Unsupported format: {}. Available formats: {:#?}",
                format,
                get_available_formats()
            )
        }
    }
}

/// Get list of available formats based on enabled features
pub fn get_available_formats() -> Vec<String> {
    let mut formats = Vec::new();
    #[cfg(feature = "mjml")]
    formats.push("mjml".to_string());
    #[cfg(feature = "html")]
    formats.push("html".to_string());
    formats
}

/// A translatable message with its locations
#[derive(Debug, Clone)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(getter_with_clone))]
pub struct Message {
    pub msgid: String,
    pub references: Vec<String>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl Message {
    #[wasm_bindgen(constructor)]
    pub fn new(msgid: String, references: Vec<String>) -> Self {
        Self { msgid, references }
    }
}

/// Extract translatable strings from content
pub fn extract_messages(
    content: &str,
    source_name: &str,
    strategy: &dyn ExtractionStrategy,
) -> Result<Vec<Message>> {
    let mut messages: HashMap<String, Vec<String>> = HashMap::new();
    let mut reader = Reader::from_str(content);

    let mut buf = Vec::new();
    let mut current_element = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                current_element = name;
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape()?;
                let trimmed_text = text.trim();

                if !trimmed_text.is_empty()
                    && strategy.is_translatable_element(&current_element)
                    && strategy.is_translatable_text(trimmed_text)
                {
                    let line = reader.buffer_position();
                    let reference = format!("{}:{}", source_name, line);
                    messages
                        .entry(trimmed_text.to_string())
                        .or_default()
                        .push(reference);
                }
            }
            Ok(Event::CData(e)) => {
                let text = String::from_utf8(e.into_inner().to_vec())?;
                let trimmed_text = text.trim();

                if !trimmed_text.is_empty()
                    && strategy.is_translatable_element(&current_element)
                    && strategy.is_translatable_text(trimmed_text)
                {
                    let line = reader.buffer_position();
                    let reference = format!("{}:{}", source_name, line);
                    messages
                        .entry(trimmed_text.to_string())
                        .or_default()
                        .push(reference);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.into()),
            _ => (),
        }
        buf.clear();
    }

    // Convert to sorted vector
    let mut result: Vec<Message> = messages
        .into_iter()
        .map(|(msgid, references)| Message { msgid, references })
        .collect();

    result.sort_by(|a, b| {
        let a_line = a
            .references
            .first()
            .and_then(|r| r.split(':').next_back())
            .and_then(|line| line.parse::<usize>().ok())
            .unwrap_or(0);
        let b_line = b
            .references
            .first()
            .and_then(|r| r.split(':').next_back())
            .and_then(|line| line.parse::<usize>().ok())
            .unwrap_or(0);
        a_line.cmp(&b_line)
    });

    Ok(result)
}

/// Apply translations to content
pub fn apply_translations(
    content: &str,
    translations: &HashMap<String, String>,
    strategy: &dyn ExtractionStrategy,
) -> Result<String> {
    let mut reader = Reader::from_str(content);
    let mut writer = quick_xml::Writer::new(Vec::new());

    let mut buf = Vec::new();
    let mut current_element = String::new();
    let mut text_buffer = String::new();
    let mut inside_text_content = false;
    let mut element_stack = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                element_stack.push(name.clone());
                current_element = name;
                writer.write_event(Event::Start(e.clone()))?;

                if strategy.is_translatable_element(&current_element) {
                    inside_text_content = true;
                    text_buffer.clear();
                } else {
                    inside_text_content = false;
                }
            }
            Ok(Event::Text(e)) => {
                if inside_text_content {
                    text_buffer.push_str(&e.unescape()?);
                } else {
                    writer.write_event(Event::Text(e))?;
                }
            }
            Ok(Event::End(ref e)) => {
                if inside_text_content && !element_stack.is_empty() {
                    let element_name = &element_stack[element_stack.len() - 1];
                    if strategy.is_translatable_element(element_name) {
                        let trimmed_text = text_buffer.trim();
                        if !trimmed_text.is_empty() && strategy.is_translatable_text(trimmed_text) {
                            if let Some(translation) = translations.get(trimmed_text) {
                                let translated_with_padding =
                                    restore_text_padding(translation, &text_buffer);
                                writer.write_event(Event::Text(
                                    quick_xml::events::BytesText::from_escaped(
                                        &translated_with_padding,
                                    ),
                                ))?;
                            } else {
                                writer.write_event(Event::Text(
                                    quick_xml::events::BytesText::from_escaped(&text_buffer),
                                ))?;
                            }
                        } else {
                            writer.write_event(Event::Text(
                                quick_xml::events::BytesText::from_escaped(&text_buffer),
                            ))?;
                        }
                    } else {
                        writer.write_event(Event::Text(
                            quick_xml::events::BytesText::from_escaped(&text_buffer),
                        ))?;
                    }
                    inside_text_content = false;
                    text_buffer.clear();
                }
                writer.write_event(Event::End(e.clone()))?;

                if !element_stack.is_empty() {
                    element_stack.pop();
                }
                if !element_stack.is_empty() {
                    current_element = element_stack[element_stack.len() - 1].clone();
                } else {
                    current_element.clear();
                }
            }
            Ok(Event::CData(e)) => {
                if inside_text_content {
                    text_buffer
                        .push_str(&String::from_utf8(e.into_inner().to_vec()).unwrap_or_default());
                } else {
                    writer.write_event(Event::CData(e))?;
                }
            }
            Ok(Event::Eof) => break,
            Ok(event) => {
                writer.write_event(&event)?;
            }
            Err(e) => return Err(e.into()),
        }
        buf.clear();
    }

    let result = writer.into_inner();
    String::from_utf8(result).map_err(|e| anyhow::anyhow!("UTF-8 conversion error: {}", e))
}

fn restore_text_padding(translation: &str, original: &str) -> String {
    let original_leading = original.chars().take_while(|c| c.is_whitespace()).count();
    let original_trailing = original
        .chars()
        .rev()
        .take_while(|c| c.is_whitespace())
        .count();

    let leading_whitespace: String = original.chars().take(original_leading).collect();
    let trailing_whitespace: String = original.chars().rev().take(original_trailing).collect();
    let trailing = trailing_whitespace.chars().rev().collect::<String>();

    format!("{}{}{}", leading_whitespace, translation, trailing)
}

/// Convert messages to PO format string
/// Convert messages to PO format string, preserving existing translations
pub fn messages_to_po(messages: &[Message], existing_po_path: Option<PathBuf>) -> Result<String> {
    let mut output = String::new();

    // Load existing translations if path provided
    let existing_translations = if let Some(path) = existing_po_path {
        match std::fs::read_to_string(path) {
            Ok(content) => parse_po_to_translations(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    // Add header
    output.push_str("msgid \"\"\n");
    output.push_str("msgstr \"\"\n");
    output.push_str("\"Project-Id-Version: \\n\"\n");
    output.push_str("\"Report-Msgid-Bugs-To: \\n\"\n");

    #[cfg(feature = "chrono")]
    {
        output.push_str(&format!(
            "\"POT-Creation-Date: {}\\n\"\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M%z")
        ));
    }
    #[cfg(not(feature = "chrono"))]
    {
        output.push_str("\"POT-Creation-Date: \\n\"\n");
    }

    output.push_str("\"PO-Revision-Date: \\n\"\n");
    output.push_str("\"Last-Translator: \\n\"\n");
    output.push_str("\"Language-Team: \\n\"\n");
    output.push_str("\"Language: \\n\"\n");
    output.push_str("\"MIME-Version: 1.0\\n\"\n");
    output.push_str("\"Content-Type: text/plain; charset=UTF-8\\n\"\n");
    output.push_str("\"Content-Transfer-Encoding: 8bit\\n\"\n");
    output.push_str("\"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"\n");
    output.push('\n');

    // Add messages
    for message in messages {
        // Add references
        output.push_str(&format!("#: {}\n", message.references.join(" ")));

        // Add msgid
        output.push_str(&format!("msgid \"{}\"\n", escape_po_string(&message.msgid)));

        // Add msgstr - preserve existing translation if available
        if let Some(translation) = existing_translations.get(&message.msgid) {
            output.push_str(&format!("msgstr \"{}\"\n", escape_po_string(translation)));
        } else {
            output.push_str("msgstr \"\"\n");
        }
        output.push('\n');
    }

    Ok(output)
}

// Update the WASM binding as well
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn extract_to_po_with_existing(
    content: &str,
    format: &str,
    source_name: &str,
    existing_po_content: Option<String>,
) -> Result<String, JsValue> {
    let strategy = get_strategy(format).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let messages = extract_messages(content, source_name, strategy.as_ref())
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    // Parse existing translations if provided
    let existing_translations = if let Some(po_content) = existing_po_content {
        parse_po_to_translations(&po_content).map_err(|e| JsValue::from_str(&e.to_string()))?
    } else {
        HashMap::new()
    };

    // Generate PO content with preserved translations
    let mut output = String::new();

    // Add header
    output.push_str("msgid \"\"\n");
    output.push_str("msgstr \"\"\n");
    output.push_str("\"Project-Id-Version: \\n\"\n");
    output.push_str("\"Report-Msgid-Bugs-To: \\n\"\n");
    output.push_str("\"POT-Creation-Date: \\n\"\n");
    output.push_str("\"PO-Revision-Date: \\n\"\n");
    output.push_str("\"Last-Translator: \\n\"\n");
    output.push_str("\"Language-Team: \\n\"\n");
    output.push_str("\"Language: \\n\"\n");
    output.push_str("\"MIME-Version: 1.0\\n\"\n");
    output.push_str("\"Content-Type: text/plain; charset=UTF-8\\n\"\n");
    output.push_str("\"Content-Transfer-Encoding: 8bit\\n\"\n");
    output.push_str("\"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"\n");
    output.push_str("\n");

    // Add messages
    for message in &messages {
        output.push_str(&format!("#: {}\n", message.references.join(" ")));
        output.push_str(&format!("msgid \"{}\"\n", escape_po_string(&message.msgid)));

        if let Some(translation) = existing_translations.get(&message.msgid) {
            output.push_str(&format!("msgstr \"{}\"\n", escape_po_string(translation)));
        } else {
            output.push_str("msgstr \"\"\n");
        }
        output.push_str("\n");
    }

    Ok(output)
}

// Keep the old function for backward compatibility (non-WASM)
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn extract_to_po(content: &str, format: &str, source_name: &str) -> Result<String, JsValue> {
    extract_to_po_with_existing(content, format, source_name, None)
}

/// Parse PO format string to translation map
pub fn parse_po_to_translations(po_content: &str) -> Result<HashMap<String, String>> {
    let mut translations = HashMap::new();
    let mut current_msgid: Option<String> = None;
    let mut in_msgid = false;
    let mut current_value = String::new();

    for line in po_content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("#") || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("msgid ") {
            // Save previous entry if exists
            if let Some(msgid) = current_msgid.take()
                && !msgid.is_empty()
                && !current_value.trim().is_empty()
            {
                translations.insert(msgid, current_value.clone());
            }

            in_msgid = true;
            current_value = extract_po_string(&trimmed["msgid ".len()..]);
        } else if trimmed.starts_with("msgstr ") {
            if in_msgid {
                current_msgid = Some(current_value.clone());
            }
            in_msgid = false;
            current_value = extract_po_string(&trimmed["msgstr ".len()..]);
        } else if trimmed.starts_with("\"") {
            // Continuation line
            current_value.push_str(&extract_po_string(trimmed));
        }
    }

    // Save last entry
    if let Some(msgid) = current_msgid
        && !msgid.is_empty()
        && !current_value.trim().is_empty()
    {
        translations.insert(msgid, current_value);
    }

    Ok(translations)
}

fn escape_po_string(s: &str) -> String {
    s.replace("\\", "\\\\")
        .replace("\"", "\\\"")
        .replace("\n", "\\n")
        .replace("\t", "\\t")
}

fn extract_po_string(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("\"") && trimmed.ends_with("\"") {
        let inner = &trimmed[1..trimmed.len() - 1];
        inner
            .replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        trimmed.to_string()
    }
}

// WASM bindings
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn extract_from_content(
    content: &str,
    format: &str,
    source_name: &str,
) -> Result<JsValue, JsValue> {
    let strategy = get_strategy(format).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let messages = extract_messages(content, source_name, strategy.as_ref())
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    serde_wasm_bindgen::to_value(&messages).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn extract_to_po(content: &str, format: &str, source_name: &str) -> Result<String, JsValue> {
    let strategy = get_strategy(format).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let messages = extract_messages(content, source_name, strategy.as_ref())
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    Ok(messages_to_po(&messages))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn compile_with_po(content: &str, po_content: &str, format: &str) -> Result<String, JsValue> {
    let strategy = get_strategy(format).map_err(|e| JsValue::from_str(&e.to_string()))?;

    let translations =
        parse_po_to_translations(po_content).map_err(|e| JsValue::from_str(&e.to_string()))?;

    apply_translations(content, &translations, strategy.as_ref())
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn get_supported_formats() -> Vec<String> {
    get_available_formats()
}

#[cfg(all(target_arch = "wasm32", feature = "wasm-bindgen"))]
#[wasm_bindgen(start)]
pub fn main() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_is_translatable_text_common() {
        // Test URLs
        assert!(!is_translatable_text_common("https://example.com"));
        assert!(!is_translatable_text_common("http://localhost:3000"));
        assert!(!is_translatable_text_common("ftp://example.com/file"));

        // Test email addresses
        assert!(!is_translatable_text_common("user@example.com"));
        assert!(!is_translatable_text_common("test.email@domain.co.uk"));

        // Test placeholders with emails (should be allowed)
        assert!(is_translatable_text_common("{{ user.email }}"));

        // Test whitespace
        assert!(!is_translatable_text_common("   "));
        assert!(!is_translatable_text_common("\t\n"));

        // Test numbers and symbols
        assert!(!is_translatable_text_common("12345"));
        assert!(!is_translatable_text_common("!@#$%"));
        assert!(!is_translatable_text_common("123-456-7890"));

        // Test valid translatable text
        assert!(is_translatable_text_common("Hello World"));
        assert!(is_translatable_text_common("Welcome to our service"));
        assert!(is_translatable_text_common("Click here"));
    }

    #[cfg(feature = "mjml")]
    #[test]
    fn test_mjml_strategy() {
        let strategy = MjmlStrategy;

        // Test translatable elements
        assert!(strategy.is_translatable_element("mj-title"));
        assert!(strategy.is_translatable_element("mj-text"));
        assert!(strategy.is_translatable_element("mj-button"));

        // Test non-translatable elements
        assert!(!strategy.is_translatable_element("mj-section"));
        assert!(!strategy.is_translatable_element("mj-column"));
        assert!(!strategy.is_translatable_element("mj-image"));
        assert!(!strategy.is_translatable_element("div"));

        // Test file extension
        assert_eq!(strategy.file_extension(), ".mjml");
    }

    #[cfg(feature = "html")]
    #[test]
    fn test_html_strategy() {
        let strategy = HtmlStrategy;

        // Test translatable elements
        assert!(strategy.is_translatable_element("p"));
        assert!(strategy.is_translatable_element("h1"));
        assert!(strategy.is_translatable_element("a"));
        assert!(strategy.is_translatable_element("button"));
        assert!(strategy.is_translatable_element("title"));

        // Test non-translatable elements
        assert!(!strategy.is_translatable_element("html"));
        assert!(!strategy.is_translatable_element("body"));
        assert!(!strategy.is_translatable_element("head"));
        assert!(!strategy.is_translatable_element("meta"));

        // Test file extension
        assert_eq!(strategy.file_extension(), ".html");
    }

    #[test]
    fn test_get_strategy() {
        // Test with available formats
        #[cfg(feature = "mjml")]
        assert!(get_strategy("mjml").is_ok());

        #[cfg(feature = "html")]
        assert!(get_strategy("html").is_ok());

        // Test with unsupported format
        let result = get_strategy("unsupported");
        assert!(result.is_err());

        // Test case insensitivity
        #[cfg(feature = "mjml")]
        assert!(get_strategy("MJML").is_ok());

        #[cfg(feature = "html")]
        assert!(get_strategy("HTML").is_ok());
    }

    #[test]
    fn test_restore_text_padding() {
        // Test with leading and trailing whitespace
        assert_eq!(
            restore_text_padding("Translated", "   Original   "),
            "   Translated   "
        );

        // Test with only leading whitespace
        assert_eq!(
            restore_text_padding("Translated", "   Original"),
            "   Translated"
        );

        // Test with only trailing whitespace
        assert_eq!(
            restore_text_padding("Translated", "Original   "),
            "Translated   "
        );

        // Test with newlines
        assert_eq!(
            restore_text_padding("Translated", "\nOriginal\n"),
            "\nTranslated\n"
        );

        // Test with tabs
        assert_eq!(
            restore_text_padding("Translated", "\tOriginal\t"),
            "\tTranslated\t"
        );

        // Test with mixed whitespace
        assert_eq!(
            restore_text_padding("Translated", " \t Original \t "),
            " \t Translated \t "
        );
    }

    #[test]
    fn test_extract_messages_basic() {
        let strategy = get_strategy("html").unwrap();

        // Create a simple HTML content
        let html_content = r#"
            <html>
                <body>
                    <h1>Welcome</h1>
                    <p>This is a test paragraph.</p>
                    <div>Another text element</div>
                    <span>http://example.com</span>
                    <p>user@example.com</p>
                </body>
            </html>
        "#;

        let messages = extract_messages(html_content, "test.html", strategy.as_ref()).unwrap();

        // Should extract translatable text but skip URLs and emails
        let msgids: Vec<String> = messages.iter().map(|m| m.msgid.clone()).collect();
        assert!(msgids.contains(&"Welcome".to_string()));
        assert!(msgids.contains(&"This is a test paragraph.".to_string()));
        assert!(msgids.contains(&"Another text element".to_string()));
        assert!(!msgids.contains(&"http://example.com".to_string()));
        assert!(!msgids.contains(&"user@example.com".to_string()));
    }

    #[test]
    fn test_extract_messages_with_cdata() {
        let strategy = get_strategy("html").unwrap();

        // Create HTML content with CDATA
        let html_content = r#"
            <html>
                <script>
                <![CDATA[
                    var message = "Hello from CDATA";
                ]]>
                </script>
                <p>Regular text</p>
            </html>
        "#;

        let messages =
            extract_messages(html_content, "test_cdata.html", strategy.as_ref()).unwrap();

        // Should extract regular text but not CDATA content in script tags
        let msgids: Vec<String> = messages.iter().map(|m| m.msgid.clone()).collect();
        assert!(msgids.contains(&"Regular text".to_string()));
        // CDATA in non-translatable elements should not be extracted
    }

    #[test]
    fn test_apply_translations_basic() {
        let strategy = get_strategy("html").unwrap();
        let mut translations = HashMap::new();
        translations.insert("Hello".to_string(), "Hola".to_string());
        translations.insert("World".to_string(), "Mundo".to_string());

        let input = r#"<html><body><p>Hello</p><div>World</div></body></html>"#;

        let result = apply_translations(input, &translations, strategy.as_ref()).unwrap();

        assert!(result.contains("Hola"));
        assert!(result.contains("Mundo"));
        assert!(!result.contains("Hello"));
        assert!(!result.contains("World"));
    }

    #[test]
    fn test_apply_translations_preserves_structure() {
        let strategy = get_strategy("html").unwrap();
        let mut translations = HashMap::new();
        translations.insert(
            "Translatable text".to_string(),
            "Translated text".to_string(),
        );

        let input = r#"
            <html>
                <body>
                    <div class="container">
                        <p style="color: red;">
                            Translatable text
                        </p>
                    </div>
                </body>
            </html>
        "#;

        let result = apply_translations(input, &translations, strategy.as_ref()).unwrap();

        // Should preserve HTML structure and attributes
        assert!(result.contains("class=\"container\""));
        assert!(result.contains("style=\"color: red;\""));
        assert!(result.contains("Translated text"));
        assert!(!result.contains("Translatable text"));
    }

    #[test]
    fn test_apply_translations_whitespace_preservation() {
        let strategy = get_strategy("html").unwrap();
        let mut translations = HashMap::new();
        translations.insert(
            "Text with spaces".to_string(),
            "Translated with spaces".to_string(),
        );

        let input = r#"<p>  Text with spaces  </p>"#;

        let result = apply_translations(input, &translations, strategy.as_ref()).unwrap();

        // Should preserve whitespace around translated text
        assert_eq!(result.trim(), r#"<p>  Translated with spaces  </p>"#);
    }

    #[test]
    fn test_messages_to_po() {
        let messages = vec![
            Message {
                msgid: "Hello World".to_string(),
                references: vec!["file1.html:10".to_string(), "file2.html:20".to_string()],
            },
            Message {
                msgid: "Welcome".to_string(),
                references: vec!["file1.html:15".to_string()],
            },
        ];

        let po_content = messages_to_po(&messages, None).unwrap();

        // Verify PO content contains expected strings
        assert!(po_content.contains("Hello World"));
        assert!(po_content.contains("Welcome"));
        assert!(po_content.contains("file1.html:10"));
        assert!(po_content.contains("file1.html:15"));
        assert!(po_content.contains("file2.html:20"));

        // Verify PO file header
        assert!(po_content.contains("Project-Id-Version"));
        assert!(po_content.contains("POT-Creation-Date"));
        assert!(po_content.contains("Content-Type: text/plain; charset=UTF-8"));
    }

    #[test]
    fn test_parse_po_to_translations() {
        let po_content = r#"
msgid ""
msgstr ""
"Project-Id-Version: \n"
"Content-Type: text/plain; charset=UTF-8\n"

#: file1.html:10
msgid "Hello"
msgstr "Hola"

#: file1.html:15
msgid "World"
msgstr "Mundo"

msgid "Untranslated"
msgstr ""
"#;

        let translations = parse_po_to_translations(po_content).unwrap();

        // Should parse translations correctly
        assert_eq!(translations.get("Hello"), Some(&"Hola".to_string()));
        assert_eq!(translations.get("World"), Some(&"Mundo".to_string()));

        // Should skip empty translations
        assert!(!translations.contains_key("Untranslated"));

        // Should have correct number of translations
        assert_eq!(translations.len(), 2);
    }

    #[test]
    fn test_mixed_content_handling() {
        let strategy = get_strategy("html").unwrap();
        let input = r#"
            <div>
                Before
                <span>Middle</span>
                After
            </div>
        "#;

        let messages = extract_messages(input, "test.html", strategy.as_ref()).unwrap();

        // Only "Middle" should be extracted (inside span)
        let msgids: Vec<String> = messages.iter().map(|m| m.msgid.clone()).collect();
        assert!(msgids.contains(&"Middle".to_string()));
        assert!(!msgids.contains(&"Before".to_string()));
        assert!(!msgids.contains(&"After".to_string()));
    }

    #[test]
    fn test_empty_and_missing_translations() {
        let strategy = get_strategy("html").unwrap();
        let mut translations = HashMap::new();
        // Only provide translation for one string
        translations.insert("Translated text".to_string(), "Translated".to_string());

        let input = r#"
            <html>
                <p>Translated text</p>
                <p>Untranslated text</p>
                <p></p>
            </html>
        "#;

        let result = apply_translations(input, &translations, strategy.as_ref()).unwrap();

        // Should translate the first, keep the second unchanged, ignore empty
        assert!(result.contains("Translated"));
        assert!(result.contains("Untranslated text"));
    }

    #[test]
    fn test_message_struct() {
        let message = Message {
            msgid: "Test message".to_string(),
            references: vec!["file.html:10".to_string(), "file.html:20".to_string()],
        };

        assert_eq!(message.msgid, "Test message");
        assert_eq!(message.references.len(), 2);
        assert!(message.references.contains(&"file.html:10".to_string()));
        assert!(message.references.contains(&"file.html:20".to_string()));
    }
}
