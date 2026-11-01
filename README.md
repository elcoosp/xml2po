<div align="center">
  <img src="docs/logo.png" alt="xml2po Logo" width="300"/>
  <p>
    <strong>Extract and compile translations for XML, HTML, and MJML</strong><br/>
    A high-performance Rust library and CLI tool for seamless PO (Portable Object) file integration.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Rust-1.94%20%7C%202024-000000?style=flat-square&logo=rust" alt="Rust"/>
    <img src="https://img.shields.io/crates/v/xml2po?style=flat-square" alt="Crates.io"/>
    <img src="https://img.shields.io/badge/License-MIT-blue?style=flat-square" alt="License MIT"/>
    <img src="https://img.shields.io/badge/Target-Native%20%2B%20WASM-6F4E37?style=flat-square" alt="WASM Support"/>
  </p>
</div>

---

`xml2po` streamlines the localization workflow by extracting translatable strings from XML, HTML, and MJML files into standard PO files, and compiling translated PO files back into the original format while perfectly preserving whitespace and structure.

## Features

- **Multi-Format Support**: Native extraction and compilation for MJML and HTML.
- **Smart Text Detection**: Automatically filters out non-translatable content (URLs, emails, file paths, pure whitespace).
- **Standard PO Integration**: Fully compatible with standard PO file workflows and tools (e.g., gettext, Crowdin, Weblate).
- **Whitespace Preservation**: Maintains original formatting and indentation when applying translations.
- **WebAssembly Ready**: Compiles to WASM for seamless in-browser localization workflows.
- **Flexible CLI**: Intuitive command-line interface designed for batch processing and CI/CD pipelines.

## Installation

### CLI Tool
Install the binary directly via Cargo:
```bash
cargo install xml2po
```

### Library
Add `xml2po` to your `Cargo.toml`, enabling only the features you need:
```toml
[dependencies]
xml2po = { version = "0.1.0", features = ["mjml", "html"] }
```

## Usage

### Command-Line Interface

**Extract translatable strings to a PO file:**
```bash
# Extract from MJML files
xml2po extract --format mjml input1.mjml input2.mjml --output translations.po

# Extract from multiple HTML files
xml2po extract --format html *.html

# Enable verbose output for debugging
xml2po extract -v --format mjml template.mjml
```

**Compile translations back to the source format:**
```bash
# Compile MJML with translations to a new file
xml2po compile --input template.mjml --po translations.po --output translated.mjml

# Compile HTML and output directly to stdout
xml2po compile --input page.html --po translations.po --format html
```

**CI drift gate and coverage reporting:**
```bash
# Fail (exit 1) when the catalog is out of sync with the sources
xml2po check translations.po src/*.mjml

# Show translation coverage
xml2po stats translations.po

# List the formats compiled into this build
xml2po formats
```

**POT templates and wrapping control:**
```bash
# Extract a .pot template with the conventional PACKAGE VERSION header
xml2po extract --template --output template.pot src/*.mjml

# Control PO wrapping (0 = no wrapping, gettext --no-wrap semantics)
xml2po extract --output-width 0 src/*.mjml
```

### Rust Library

**Extract messages from content:**
```rust
use xml2po::{extract_messages, get_strategy};

let content = r#"<mjml><mj-text>Hello World</mj-text></mjml>"#;
let strategy = get_strategy("mjml")?;
let messages = extract_messages(content, "template.mjml", strategy.as_ref())?;

for message in messages {
    println!("Found: {} at {:?}", message.msgid, message.references);
}
```

**Apply translations to content:**
```rust
use xml2po::{apply_translations, parse_po_to_translations, get_strategy};

let content = "<p>Hello World</p>";
let po_content = r#"
msgid "Hello World"
msgstr "Hola Mundo"
"#;

let strategy = get_strategy("html")?;
let translations = parse_po_to_translations(po_content)?;
let translated = apply_translations(content, &translations, strategy.as_ref())?;

assert_eq!(translated, "<p>Hola Mundo</p>");
```

> [!TIP]
> **WebAssembly Usage**  
> The library can be compiled to WebAssembly (`wasm32-unknown-unknown`) for direct use in web applications via `wasm-bindgen`, enabling client-side extraction and compilation.
> ```javascript
> const poContent = await extractToPo(mjmlContent, "mjml", "template.mjml");
> const translated = await compileWithPo(htmlContent, poContent, "html");
> ```

## Supported Formats & Elements

`xml2po` targets specific, semantically translatable elements to avoid breaking markup or extracting boilerplate.

| Format | Translatable Elements |
| :--- | :--- |
| **MJML** | `mj-title`, `mj-text`, `mj-button` |
| **HTML** | `p`, `h1`–`h6`, `span`, `div`, `a`, `button`, `label`, `li`, `td`, `th`, `title`, `option`, `strong`, `em`, `b`, `i` |
| **XML** | Everything (generic fallback for arbitrary XML documents) |

> [!NOTE]
> **Smart Filtering**  
> The extraction engine automatically excludes:
> - URLs (`http://`, `https://`, etc.)
> - Email addresses (unless they contain translation placeholders)
> - Pure whitespace or text without alphabetic characters
> - File paths and protocol-based strings

## Cargo Features

| Feature | Description | Default |
| :--- | :--- | :---: |
| `mjml` | Enable MJML format extraction and compilation | ✅ |
| `html` | Enable HTML format extraction and compilation | ✅ |
| `xml` | Enable the generic XML fallback strategy | ✅ |
| `chrono` | Enable timestamps in PO file headers | ✅ |
| `serde` | `Serialize`/`Deserialize` for messages, options and catalogs | ❌ |
| `cli` | The command-line interface binary | ✅ |
| `parallel` | Rayon-parallel extraction of multiple input files | ✅ |
| `cli-encoding` | UTF-16 (BOM) input support in the CLI | ✅ |

## Development

Clone the repository and ensure you have the Rust toolchain installed.

```bash
# Run the comprehensive test suite
cargo test --all-features

# Build with specific format support
cargo build --no-default-features --features mjml,html

# Test WebAssembly build
cargo build --target wasm32-unknown-unknown

# Lint with the same pedantic configuration as CI (no allow escapes)
cargo clippy --all-features --all-targets -- -W clippy::pedantic --deny warnings

# Measure coverage (requires cargo-llvm-cov)
cargo llvm-cov --all-features
```

> [!IMPORTANT]
> When contributing new format support, implement the [`ExtractionStrategy`](src/strategy.rs) trait (elements, ignored elements, translatable attributes) and add a corresponding variant to `Format`. Refer to the existing strategies for implementation patterns.
