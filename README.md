# mjml2po

A Rust utility to extract translatable strings from MJML email templates and generate PO (Portable Object) files for localization.

## Features

- 📧 Extracts text from MJML email templates
- 🌍 Generates standard PO files for translation workflows using `rspolib`
- 📂 Processes multiple MJML files into a single PO file
- 🎯 Focuses on user-visible content only (`mj-title`, `mj-text`, `mj-button`)
- 🚫 Automatically excludes URLs, email addresses, and other non-translatable content
- 📍 Preserves source file references with line numbers
- 📋 Merges duplicate strings across multiple files
- 📝 Preserves placeholder variables like `{{name}}` and `{{resetToken}}`
- 🔁 Compile command to apply translations back to MJML templates

## Installation

```bash
cargo install mjml2po
```

Or build from source:

```bash
git clone https://github.com/your-username/mjml2po.git
cd mjml2po
cargo build --release
```

## Usage

### Extract command - Generate PO files from MJML:
```bash
# Single file processing
mjml2po extract template.mjml
# Creates template.po

# Multiple files with auto-generated output
mjml2po extract template1.mjml template2.mjml
# Creates template1.po (or template2.po)

# Multiple files with specific output
mjml2po extract template1.mjml template2.mjml translations.po
# Creates translations.po

# Process all MJML files in a directory
mjml2po extract emails/*.mjml translations.po
```

### Compile command - Apply translations to MJML:
```bash
# Compile PO file back into MJML with translated content
mjml2po compile --mjml template.mjml --po translations.po --output translated.mjml

# Output to stdout
mjml2po compile --mjml template.mjml --po translations.po
```

## What Gets Extracted

The tool extracts text from these MJML elements:

- `<mj-title>` - Email subject lines
- `<mj-text>` - Body text content
- `<mj-button>` - Button text labels

### Example MJML:
```xml
<mjml>
  <mj-head>
    <mj-title>Welcome to Our Service</mj-title>
  </mj-head>
  <mj-body>
    <mj-text>Hello {{name}},</mj-text>
    <mj-button href="https://example.com">Click Here</mj-button>
  </mj-body>
</mjml>
```

### Generated PO entry:
```po
#: template.mjml:3
msgid "Welcome to Our Service"
msgstr ""

#: template.mjml:6
msgid "Hello {{name}},"
msgstr ""

#: template.mjml:7
msgid "Click Here"
msgstr ""
```

## What Gets Excluded

- URLs (http://, https://, etc.)
- Email addresses
- Attribute values (href, src, etc.)
- Non-user-visible elements
- Whitespace-only content

## Workflow Example

1. **Extract strings for translation**:
   ```bash
   mjml2po extract welcome.mjml reset.mjml --output messages.po
   ```

2. **Translate the PO file** (manually or with translation tools like Poedit, Lokalise, etc.)

3. **Compile translated PO back to MJML**:
   ```bash
   mjml2po compile --mjml welcome.mjml --po messages.po --output welcome_es.mjml
   ```

## Output Format

The generated PO files follow the standard format with:

- Proper headers with creation timestamp
- File and line number references (`#: filename:line`)
- Sorted by order of appearance
- Merged duplicate strings with combined references
- Compatible with all standard PO file tools

## Requirements

- Rust 1.60 or later
- MJML files must be valid XML

## Contributing

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/AmazingFeature`)
3. Commit your changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to the branch (`git push origin feature/AmazingFeature`)
5. Open a Pull Request

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- Built with [quick-xml](https://crates.io/crates/quick-xml) for fast XML parsing
- Uses [rspolib](https://crates.io/crates/rspolib) for robust PO file handling
- Uses [anyhow](https://crates.io/crates/anyhow) for error handling
- Uses [clap](https://crates.io/crates/clap) for command-line argument parsing
- Inspired by localization tools in the i18n ecosystem

>>>
>>> readd verbose logging option
# mjml2po

A Rust utility to extract translatable strings from MJML email templates and generate PO (Portable Object) files for localization.

## Features

- 📧 Extracts text from MJML email templates
- 🌍 Generates standard PO files for translation workflows using `rspolib`
- 📂 Processes multiple MJML files into a single PO file
- 🎯 Focuses on user-visible content only (`mj-title`, `mj-text`, `mj-button`)
- 🚫 Automatically excludes URLs, email addresses, and other non-translatable content
- 📍 Preserves source file references with line numbers
- 📋 Merges duplicate strings across multiple files
- 📝 Preserves placeholder variables like `{{name}}` and `{{resetToken}}`
- 🔁 Compile command to apply translations back to MJML templates
- 📋 Verbose logging option for detailed output

## Installation

```bash
cargo install mjml2po
```

Or build from source:

```bash
git clone https://github.com/your-username/mjml2po.git
cd mjml2po
cargo build --release
```

## Usage

### Extract command - Generate PO files from MJML:
```bash
# Single file processing
mjml2po extract template.mjml
# Creates template.po

# Multiple files with auto-generated output
mjml2po extract template1.mjml template2.mjml
# Creates template1.po (or template2.po)

# Multiple files with specific output
mjml2po extract template1.mjml template2.mjml translations.po
# Creates translations.po

# Process all MJML files in a directory with verbose output
mjml2po extract emails/*.mjml translations.po --verbose
# Creates translations.po with detailed logging

# Process with verbose output
mjml2po extract template.mjml --verbose
```

### Compile command - Apply translations to MJML:
```bash
# Compile PO file back into MJML with translated content
mjml2po compile --mjml template.mjml --po translations.po --output translated.mjml

# Output to stdout with verbose logging
mjml2po compile --mjml template.mjml --po translations.po --verbose

# Save to file with verbose output
mjml2po compile --mjml template.mjml --po translations.po --output translated.mjml --verbose
```

## What Gets Extracted

The tool extracts text from these MJML elements:

- `<mj-title>` - Email subject lines
- `<mj-text>` - Body text content
- `<mj-button>` - Button text labels

### Example MJML:
```xml
<mjml>
  <mj-head>
    <mj-title>Welcome to Our Service</mj-title>
  </mj-head>
  <mj-body>
    <mj-text>Hello {{name}},</mj-text>
    <mj-button href="https://example.com">Click Here</mj-button>
  </mj-body>
</mjml>
```

### Generated PO entry:
```po
#: template.mjml:3
msgid "Welcome to Our Service"
msgstr ""

#: template.mjml:6
msgid "Hello {{name}},"
msgstr ""

#: template.mjml:7
msgid "Click Here"
msgstr ""
```

## What Gets Excluded

- URLs (http://, https://, etc.)
- Email addresses
- Attribute values (href, src, etc.)
- Non-user-visible elements
- Whitespace-only content

## Workflow Example

1. **Extract strings for translation**:
   ```bash
   mjml2po extract welcome.mjml reset.mjml --output messages.po --verbose
   ```

2. **Translate the PO file** (manually or with translation tools like Poedit, Lokalise, etc.)

3. **Compile translated PO back to MJML**:
   ```bash
   mjml2po compile --mjml welcome.mjml --po messages.po --output welcome_es.mjml --verbose
   ```

## Output Format

The generated PO files follow the standard format with:

- Proper headers with creation timestamp
- File and line number references (`#: filename:line`)
- Sorted by order of appearance
- Merged duplicate strings with combined references
- Compatible with all standard PO file tools

## Verbose Mode

When using the `--verbose` flag, you'll get detailed output including:
- Files being processed
- Number of strings extracted
- Processing status updates
- Error details if any occur

## Requirements

- Rust 1.60 or later
- MJML files must be valid XML

## Contributing

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/AmazingFeature`)
3. Commit your changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to the branch (`git push origin feature/AmazingFeature`)
5. Open a Pull Request

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- Built with [quick-xml](https://crates.io/crates/quick-xml) for fast XML parsing
- Uses [rspolib](https://crates.io/crates/rspolib) for robust PO file handling
- Uses [anyhow](https://crates.io/crates/anyhow) for error handling
- Uses [clap](https://crates.io/crates/clap) for command-line argument parsing
- Inspired by localization tools in the i18n ecosystem
