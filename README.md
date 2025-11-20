# xml2po

A Rust library and CLI tool for extracting translatable strings from XML/HTML files (including MJML) into PO (Portable Object) files and compiling translated PO files back into the original format.

## Features

- **Multiple Format Support**: Extract and compile translations for MJML and HTML files
- **Smart Text Detection**: Automatically filters out non-translatable content (URLs, emails, etc.)
- **PO File Integration**: Uses standard PO file format for translation workflows
- **Whitespace Preservation**: Maintains original formatting when applying translations
- **WASM Support**: Can be compiled to WebAssembly for browser usage
- **Flexible CLI**: Easy-to-use command-line interface for batch processing

## Installation

### CLI Tool

```bash
cargo install xml2po
```

### Library

Add to your `Cargo.toml`:

```toml
[dependencies]
xml2po = "0.1.0"

# Enable specific formats you need
features = ["mjml", "html", "wasm-bindgen"]
```

## Supported Formats

- **MJML** (enable with `mjml` feature)
- **HTML** (enable with `html` feature)

## Usage

### CLI

#### Extract translatable strings to PO file

```bash
# Extract from MJML files
xml2po extract --format mjml input1.mjml input2.mjml --output translations.po

# Extract from HTML files  
xml2po extract --format html *.html

# With verbose output
xml2po extract -v --format mjml template.mjml
```

#### Compile translations back to source format

```bash
# Compile MJML with translations
xml2po compile --input template.mjml --po translations.po --output translated.mjml

# Compile HTML and output to stdout
xml2po compile --input page.html --po translations.po --format html
```

### Library

#### Extract messages from content

```rust
use xml2po::{extract_messages, get_strategy};

let content = r#"<mjml><mj-text>Hello World</mj-text></mjml>"#;
let strategy = get_strategy("mjml")?;
let messages = extract_messages(content, "template.mjml", strategy.as_ref())?;

for message in messages {
    println!("Found: {} at {:?}", message.msgid, message.references);
}
```

#### Apply translations

```rust
use xml2po::{apply_translations, parse_po_to_translations, get_strategy};
use std::collections::HashMap;

let content = "<p>Hello World</p>";
let po_content = r#"
msgid "Hello World"
msgstr "Hola Mundo"
"#;

let strategy = get_strategy("html")?;
let translations = parse_po_to_translations(po_content)?;
let translated = apply_translations(content, &translations, strategy.as_ref())?;

println!("{}", translated); // <p>Hola Mundo</p>
```

#### Generate PO file content

```rust
use xml2po::{extract_messages, messages_to_po, get_strategy};

let content = "<h1>Welcome</h1><p>Hello World</p>";
let strategy = get_strategy("html")?;
let messages = extract_messages(content, "page.html", strategy.as_ref())?;
let po_content = messages_to_po(&messages);

println!("{}", po_content);
```

### WebAssembly

The library can be compiled to WebAssembly for use in web applications:

```javascript
// Extract messages to PO format
const poContent = await extractToPo(mjmlContent, "mjml", "template.mjml");

// Compile with translations
const translated = await compileWithPo(htmlContent, poContent, "html");
```

## Translatable Elements

### MJML
- `mj-title`
- `mj-text` 
- `mj-button`

### HTML
- `p`, `h1`-`h6`, `span`, `div`, `a`, `button`, `label`
- `li`, `td`, `th`, `title`, `option`
- `strong`, `em`, `b`, `i`

## Text Filtering

The tool automatically excludes:
- URLs (`http://`, `https://`, etc.)
- Email addresses (unless they contain placeholders)
- Pure whitespace
- Text without alphabetic characters
- File paths and protocol-based strings

## Features

- **mjml**: Enable MJML format support
- **html**: Enable HTML format support  
- **wasm-bindgen**: Enable WebAssembly compilation
- **chrono**: Enable timestamps in PO file headers (enabled by default)
- **rspolib**: Use rspolib for PO file handling in CLI

## Project Structure

- `lib.rs`: Core library functionality with extraction strategies
- `main.rs`: CLI application using the library
- WASM bindings for browser usage
- Comprehensive test suite

## Development

```bash
# Run tests
cargo test

# Build with specific features
cargo build --features mjml,html

# Test WASM build
cargo build --target wasm32-unknown-unknown --features wasm-bindgen
```

## License

MIT License
