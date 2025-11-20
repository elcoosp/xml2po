use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use quick_xml::Writer;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};

/// MJML to PO converter and reverse
#[derive(Parser)]
#[clap(author, version, about)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract translatable strings from MJML files to create PO files
    Extract {
        /// Input MJML files
        #[clap(required = true)]
        input: Vec<String>,

        /// Output PO file
        #[clap(short, long)]
        output: Option<String>,
    },

    /// Compile PO file back into MJML with translated content
    Compile {
        /// Input MJML file
        #[clap(short, long)]
        mjml: String,

        /// Input PO file
        #[clap(short, long)]
        po: String,

        /// Output MJML file (stdout if not specified)
        #[clap(short, long)]
        output: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Extract { input, output } => extract_command(input, output.as_deref()),
        Commands::Compile { mjml, po, output } => compile_command(mjml, po, output.as_deref()),
    }
}

fn extract_command(input_files: &[String], output_file: Option<&str>) -> Result<()> {
    if input_files.is_empty() {
        eprintln!("At least one input file is required");
        std::process::exit(1);
    }

    let output_path = match output_file {
        Some(path) => path.to_string(),
        None => {
            let first_input = &input_files[0];
            first_input.trim_end_matches(".mjml").to_string() + ".po"
        }
    };

    let mut messages: HashMap<String, (usize, Vec<String>)> = HashMap::new();

    for input_path in input_files {
        extract_messages_from_file(input_path, &mut messages)?;
    }

    write_po_file(&messages, &output_path)?;

    println!("PO file created: {}", output_path);
    Ok(())
}

fn compile_command(mjml_file: &str, po_file: &str, output_file: Option<&str>) -> Result<()> {
    // Load translations from PO file
    let translations = load_po_translations(po_file)?;

    // Read MJML file
    let mut mjml_content = String::new();
    let mut file = File::open(mjml_file)
        .with_context(|| format!("Failed to open MJML file: {}", mjml_file))?;
    file.read_to_string(&mut mjml_content)?;

    // Apply translations
    let translated_mjml = apply_translations(&mjml_content, &translations)?;

    // Write output
    match output_file {
        Some(path) => {
            let mut output = File::create(path)
                .with_context(|| format!("Failed to create output file: {}", path))?;
            write!(output, "{}", translated_mjml)?;
            println!("Translated MJML file created: {}", path);
        }
        None => {
            print!("{}", translated_mjml);
        }
    }

    Ok(())
}

fn extract_messages_from_file(
    file_path: &str,
    messages: &mut HashMap<String, (usize, Vec<String>)>,
) -> Result<()> {
    let file =
        File::open(file_path).with_context(|| format!("Failed to open file: {}", file_path))?;
    let mut reader = Reader::from_reader(BufReader::new(file));

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

                // Only extract text from specific elements that contain user-visible content
                // and exclude URLs and other non-translatable content
                if !trimmed_text.is_empty()
                    && is_user_visible_element(&current_element)
                    && is_translatable_text(trimmed_text)
                {
                    let line = reader.buffer_position();
                    let reference = format!("{}:{}", file_path, line);
                    let entry = messages
                        .entry(trimmed_text.to_string())
                        .or_insert_with(|| (line, Vec::new()));
                    entry.1.push(reference);
                }
            }
            Ok(Event::CData(e)) => {
                let text = String::from_utf8(e.into_inner().to_vec())?;
                let trimmed_text = text.trim();

                // Only extract text from specific elements that contain user-visible content
                // and exclude URLs and other non-translatable content
                if !trimmed_text.is_empty()
                    && is_user_visible_element(&current_element)
                    && is_translatable_text(trimmed_text)
                {
                    let line = reader.buffer_position();
                    let reference = format!("{}:{}", file_path, line);
                    let entry = messages
                        .entry(trimmed_text.to_string())
                        .or_insert_with(|| (line, Vec::new()));
                    entry.1.push(reference);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.into()),
            _ => (),
        }
        buf.clear();
    }

    Ok(())
}

fn is_user_visible_element(element_name: &str) -> bool {
    match element_name {
        "mj-title" | "mj-text" | "mj-button" => true,
        _ => false,
    }
}

fn is_translatable_text(text: &str) -> bool {
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

    true
}

fn write_po_file(
    messages: &HashMap<String, (usize, Vec<String>)>,
    output_path: &str,
) -> Result<()> {
    let mut file = File::create(output_path)
        .with_context(|| format!("Failed to create output file: {}", output_path))?;

    // Write PO header
    writeln!(
        file,
        r#"msgid ""
msgstr ""
"Project-Id-Version: \n"
"Report-Msgid-Bugs-To: \n"
"POT-Creation-Date: {}\n"
"PO-Revision-Date: \n"
"Last-Translator: \n"
"Language-Team: \n"
"Language: \n"
"MIME-Version: 1.0\n"
"Content-Type: text/plain; charset=UTF-8\n"
"Content-Transfer-Encoding: 8bit\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"
"#,
        chrono::Utc::now().format("%Y-%m-%d %H:%M%z")
    )?;

    // Convert HashMap to sorted vector by first line occurrence
    let mut sorted_messages: Vec<(&String, &(usize, Vec<String>))> = messages.iter().collect();
    sorted_messages.sort_by_key(|(_, (first_line, _))| *first_line);

    // Write each message
    for (msgid, (_, references)) in sorted_messages {
        writeln!(file, "\n#: {}", references.join(" "))?;
        writeln!(file, "msgid \"{}\"", escape_po_string(msgid))?;
        writeln!(file, "msgstr \"\"")?;
    }

    Ok(())
}

fn load_po_translations(po_file: &str) -> Result<HashMap<String, String>> {
    let mut translations = HashMap::new();
    let content = std::fs::read_to_string(po_file)
        .with_context(|| format!("Failed to read PO file: {}", po_file))?;

    // Simple PO parser using regex
    let msgid_re = regex::Regex::new(r#"msgid\s+"([^"]*)""#)?;
    let msgstr_re = regex::Regex::new(r#"msgstr\s+"([^"]*)""#)?;

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        if line.starts_with("msgid ") {
            // Extract msgid
            let msgid_cap = msgid_re.captures(line);
            if let Some(caps) = msgid_cap {
                let mut msgid = caps[1].to_string();

                // Handle multi-line strings
                let mut j = i + 1;
                while j < lines.len()
                    && lines[j].trim().starts_with('"')
                    && lines[j].trim().ends_with('"')
                {
                    if let Some(content) = lines[j]
                        .trim()
                        .strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                    {
                        msgid.push_str(content);
                    }
                    j += 1;
                }

                // Look for msgstr
                let mut k = i + 1;
                while k < lines.len()
                    && !lines[k].trim().starts_with("msgstr")
                    && !lines[k].trim().starts_with("msgid")
                {
                    k += 1;
                }

                if k < lines.len() && lines[k].trim().starts_with("msgstr") {
                    let msgstr_cap = msgstr_re.captures(lines[k].trim());
                    if let Some(str_caps) = msgstr_cap {
                        let mut msgstr = str_caps[1].to_string();

                        // Handle multi-line strings for msgstr
                        let mut l = k + 1;
                        while l < lines.len()
                            && lines[l].trim().starts_with('"')
                            && lines[l].trim().ends_with('"')
                        {
                            if let Some(content) = lines[l]
                                .trim()
                                .strip_prefix('"')
                                .and_then(|s| s.strip_suffix('"'))
                            {
                                msgstr.push_str(content);
                            }
                            l += 1;
                        }

                        // Only add translation if msgstr is not empty
                        if !msgstr.is_empty() {
                            let unescaped_msgid = unescape_po_string(&msgid);
                            let unescaped_msgstr = unescape_po_string(&msgstr);
                            translations
                                .insert(unescaped_msgid.trim().to_string(), unescaped_msgstr);
                        }
                    }
                }

                i = j.max(k + 1);
                continue;
            }
        }
        i += 1;
    }

    Ok(translations)
}

fn unescape_po_string(s: &str) -> String {
    s.replace("\\\"", "\"")
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
        .replace("\\\\", "\\")
}

fn apply_translations(
    mjml_content: &str,
    translations: &HashMap<String, String>,
) -> Result<String> {
    let mut reader = Reader::from_str(mjml_content);

    let mut writer = Writer::new(Vec::new());

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
                writer.write_event(&Event::Start(e.clone()))?;

                // Check if this element contains translatable text
                if is_user_visible_element(&current_element) {
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
                    if is_user_visible_element(element_name) {
                        // Process the accumulated text
                        let trimmed_text = text_buffer.trim();
                        if !trimmed_text.is_empty() && is_translatable_text(trimmed_text) {
                            if let Some(translation) = translations.get(trimmed_text) {
                                // Write translated text with preserved formatting
                                let translated_with_padding =
                                    restore_text_padding(translation, &text_buffer);
                                writer.write_event(&Event::Text(
                                    quick_xml::events::BytesText::from_escaped(
                                        &translated_with_padding,
                                    ),
                                ))?;
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

fn escape_po_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}
