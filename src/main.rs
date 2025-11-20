use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use rspolib::Save;
use rspolib::{FileOptions, POFile, pofile};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};

/// Generic XML/HTML to PO converter and reverse
#[derive(Parser)]
#[clap(author, version, about)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,

    /// Enable verbose output
    #[clap(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract translatable strings from XML/HTML files to create PO files
    Extract {
        /// Input files
        #[clap(required = true)]
        input: Vec<String>,

        /// Output PO file
        #[clap(short, long)]
        output: Option<String>,

        /// Format type (mjml, html)
        #[clap(short, long, default_value = "mjml")]
        format: String,
    },

    /// Compile PO file back into XML/HTML with translated content
    Compile {
        /// Input file
        #[clap(short, long)]
        input: String,

        /// Input PO file
        #[clap(short, long)]
        po: String,

        /// Output file (stdout if not specified)
        #[clap(short, long)]
        output: Option<String>,

        /// Format type (mjml, html)
        #[clap(short, long, default_value = "mjml")]
        format: String,
    },
}

/// Trait for format-specific extraction rules
trait ExtractionStrategy {
    /// Determine if an element should have its text content extracted
    fn is_translatable_element(&self, element_name: &str) -> bool;

    /// Determine if text content should be translated
    fn is_translatable_text(&self, text: &str) -> bool;

    /// Get the file extension for this format
    fn file_extension(&self) -> &str;
}

#[cfg(feature = "mjml")]
struct MjmlStrategy;

#[cfg(feature = "mjml")]
impl ExtractionStrategy for MjmlStrategy {
    fn is_translatable_element(&self, element_name: &str) -> bool {
        matches!(element_name, "mj-title" | "mj-text" | "mj-button")
    }

    fn is_translatable_text(&self, text: &str) -> bool {
        is_translatable_text_common(text)
    }

    fn file_extension(&self) -> &str {
        ".mjml"
    }
}

#[cfg(feature = "html")]
struct HtmlStrategy;

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
}

fn is_translatable_text_common(text: &str) -> bool {
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

fn get_strategy(format: &str) -> Result<Box<dyn ExtractionStrategy>> {
    match format.to_lowercase().as_str() {
        #[cfg(feature = "mjml")]
        "mjml" => Ok(Box::new(MjmlStrategy)),
        #[cfg(feature = "html")]
        "html" => Ok(Box::new(HtmlStrategy)),
        _ => {
            #[cfg(not(any(feature = "mjml", feature = "html")))]
            {
                anyhow::bail!("No format features enabled. Enable 'mjml' or 'html' feature.");
            }
            #[cfg(any(feature = "mjml", feature = "html"))]
            {
                anyhow::bail!(
                    "Unsupported format: {}. Available formats: {}",
                    format,
                    get_available_formats()
                );
            }
        }
    }
}

fn get_available_formats() -> String {
    let mut formats = Vec::new();
    #[cfg(feature = "mjml")]
    formats.push("mjml");
    #[cfg(feature = "html")]
    formats.push("html");
    formats.join(", ")
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Extract {
            input,
            output,
            format,
        } => extract_command(input, output.as_deref(), format, cli.verbose),
        Commands::Compile {
            input,
            po,
            output,
            format,
        } => compile_command(input, po, output.as_deref(), format, cli.verbose),
    }
}

fn extract_command(
    input_files: &[String],
    output_file: Option<&str>,
    format: &str,
    verbose: bool,
) -> Result<()> {
    if input_files.is_empty() {
        anyhow::bail!("At least one input file is required");
    }

    let strategy = get_strategy(format)?;

    let output_path = match output_file {
        Some(path) => path.to_string(),
        None => {
            let first_input = &input_files[0];
            first_input
                .trim_end_matches(strategy.file_extension())
                .to_string()
                + ".po"
        }
    };

    if verbose {
        println!(
            "Processing {} {} files...",
            input_files.len(),
            format.to_uppercase()
        );
    }

    let mut messages: HashMap<String, Vec<String>> = HashMap::new();

    for input_path in input_files {
        if verbose {
            println!("  Extracting from: {}", input_path);
        }
        extract_messages_from_file(input_path, &mut messages, strategy.as_ref(), verbose)?;
    }

    if verbose {
        println!("Writing PO file with {} unique strings...", messages.len());
    }

    write_po_file(&messages, &output_path, verbose)?;

    if verbose {
        println!("✅ PO file created: {}", output_path);
    } else {
        println!("PO file created: {}", output_path);
    }

    Ok(())
}

fn compile_command(
    input_file: &str,
    po_file: &str,
    output_file: Option<&str>,
    format: &str,
    verbose: bool,
) -> Result<()> {
    let strategy = get_strategy(format)?;

    if verbose {
        println!("Loading translations from: {}", po_file);
    }

    // Load translations from PO file using rspolib
    let po = pofile(po_file).with_context(|| format!("Failed to read PO file: {}", po_file))?;

    // Convert PO entries to translation map
    let mut translations = HashMap::new();
    let mut translation_count = 0;

    for entry in &po.entries {
        if !entry.msgid.is_empty() {
            if let Some(msgstr) = &entry.msgstr {
                if !msgstr.trim().is_empty() {
                    translations.insert(entry.msgid.clone(), msgstr.clone());
                    translation_count += 1;
                }
            }
        }
    }

    if verbose {
        println!("Loaded {} translations from PO file", translation_count);
        println!("Reading {} file: {}", format.to_uppercase(), input_file);
    }

    // Read input file
    let mut content = String::new();
    let mut file = File::open(input_file)
        .with_context(|| format!("Failed to open input file: {}", input_file))?;
    file.read_to_string(&mut content)?;

    if verbose {
        println!("Applying translations...");
    }

    // Apply translations
    let translated = apply_translations(&content, &translations, strategy.as_ref(), verbose)?;

    // Write output
    match output_file {
        Some(path) => {
            let mut output = File::create(path)
                .with_context(|| format!("Failed to create output file: {}", path))?;
            write!(output, "{}", translated)?;
            if verbose {
                println!(
                    "✅ Translated {} file created: {}",
                    format.to_uppercase(),
                    path
                );
            } else {
                println!(
                    "Translated {} file created: {}",
                    format.to_uppercase(),
                    path
                );
            }
        }
        None => {
            print!("{}", translated);
        }
    }

    Ok(())
}

fn extract_messages_from_file(
    file_path: &str,
    messages: &mut HashMap<String, Vec<String>>,
    strategy: &dyn ExtractionStrategy,
    verbose: bool,
) -> Result<()> {
    let file =
        File::open(file_path).with_context(|| format!("Failed to open file: {}", file_path))?;
    let mut reader = Reader::from_reader(BufReader::new(file));

    let mut buf = Vec::new();
    let mut current_element = String::new();
    let mut extracted_count = 0;

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
                    let reference = format!("{}:{}", file_path, line);
                    messages
                        .entry(trimmed_text.to_string())
                        .or_insert_with(Vec::new)
                        .push(reference);
                    extracted_count += 1;
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
                    let reference = format!("{}:{}", file_path, line);
                    messages
                        .entry(trimmed_text.to_string())
                        .or_insert_with(Vec::new)
                        .push(reference);
                    extracted_count += 1;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.into()),
            _ => (),
        }
        buf.clear();
    }

    if verbose && extracted_count > 0 {
        println!("    Extracted {} strings", extracted_count);
    }

    Ok(())
}

fn write_po_file(
    messages: &HashMap<String, Vec<String>>,
    output_path: &str,
    verbose: bool,
) -> Result<()> {
    if verbose {
        println!("  Creating PO file with {} entries...", messages.len());
    }

    // Create options for new PO file
    let opts = FileOptions {
        path_or_content: "".into(),
        wrapwidth: 76,
        byte_content: None,
    };

    let mut po_file = POFile::new(opts);

    // Add header
    let header = rspolib::POEntry {
        msgid: String::new(),
        msgstr: Some(format!(
            "Project-Id-Version: \\n\n\
             Report-Msgid-Bugs-To: \\n\n\
             POT-Creation-Date: {}\\n\n\
             PO-Revision-Date: \\n\n\
             Last-Translator: \\n\n\
             Language-Team: \\n\n\
             Language: \\n\n\
             MIME-Version: 1.0\\n\n\
             Content-Type: text/plain; charset=UTF-8\\n\n\
             Content-Transfer-Encoding: 8bit\\n\n\
             Plural-Forms: nplurals=2; plural=(n != 1);\\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M%z")
        )),
        ..Default::default()
    };
    po_file.entries.push(header);

    // Convert HashMap to sorted vector by first reference
    let mut sorted_messages: Vec<(&String, &Vec<String>)> = messages.iter().collect();
    sorted_messages.sort_by_key(|(_, refs)| {
        refs.first()
            .and_then(|r| r.split(':').last())
            .and_then(|line| line.parse::<usize>().ok())
            .unwrap_or(0)
    });

    // Add each message
    for (msgid, references) in sorted_messages {
        let entry = rspolib::POEntry {
            msgid: msgid.clone(),
            msgstr: Some(String::new()), // Empty translation to be filled
            comment: Some(format!("#: {}", references.join(" "))),
            ..Default::default()
        };
        po_file.entries.push(entry);
    }

    // Save the PO file
    po_file.save(output_path);

    Ok(())
}

fn apply_translations(
    content: &str,
    translations: &HashMap<String, String>,
    strategy: &dyn ExtractionStrategy,
    verbose: bool,
) -> Result<String> {
    let mut reader = Reader::from_str(content);
    let mut writer = quick_xml::Writer::new(Vec::new());

    let mut buf = Vec::new();
    let mut current_element = String::new();
    let mut text_buffer = String::new();
    let mut inside_text_content = false;
    let mut element_stack = Vec::new();
    let mut applied_translations = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                element_stack.push(name.clone());
                current_element = name;
                writer.write_event(&Event::Start(e.clone()))?;

                // Check if this element contains translatable text
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
                    writer.write_event(&Event::Text(e))?;
                }
            }
            Ok(Event::End(ref e)) => {
                if inside_text_content && !element_stack.is_empty() {
                    let element_name = &element_stack[element_stack.len() - 1];
                    if strategy.is_translatable_element(element_name) {
                        // Process the accumulated text
                        let trimmed_text = text_buffer.trim();
                        if !trimmed_text.is_empty() && strategy.is_translatable_text(trimmed_text) {
                            if let Some(translation) = translations.get(trimmed_text) {
                                // Write translated text with preserved formatting
                                let translated_with_padding =
                                    restore_text_padding(translation, &text_buffer);
                                writer.write_event(&Event::Text(
                                    quick_xml::events::BytesText::from_escaped(
                                        &translated_with_padding,
                                    ),
                                ))?;
                                applied_translations += 1;
                            } else {
                                // Write original text
                                writer.write_event(&Event::Text(
                                    quick_xml::events::BytesText::from_escaped(&text_buffer),
                                ))?;
                            }
                        } else {
                            // Write original text
                            writer.write_event(&Event::Text(
                                quick_xml::events::BytesText::from_escaped(&text_buffer),
                            ))?;
                        }
                    } else {
                        // Write original text for non-translatable elements
                        writer.write_event(&Event::Text(
                            quick_xml::events::BytesText::from_escaped(&text_buffer),
                        ))?;
                    }
                    inside_text_content = false;
                    text_buffer.clear();
                }
                writer.write_event(&Event::End(e.clone()))?;

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
                    writer.write_event(&Event::CData(e))?;
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

    if verbose && applied_translations > 0 {
        println!("  Applied {} translations", applied_translations);
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
