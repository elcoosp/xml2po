# mjml2po

A Rust utility to extract translatable strings from MJML email templates and generate PO (Portable Object) files for localization.

## Features

- 📧 Extracts text from MJML email templates
- 🌍 Generates standard PO files for translation workflows
- 📂 Processes multiple MJML files into a single PO file
- 🎯 Focuses on user-visible content only (`mj-title`, `mj-text`, `mj-button`)
- 🚫 Automatically excludes URLs, email addresses, and other non-translatable content
- 📍 Preserves source file references with line numbers
- 📋 Merges duplicate strings across multiple files
- 📝 Preserves placeholder variables like `{{name}}` and `{{resetToken}}`

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

### Single file processing:
```bash
mjml2po template.mjml
# Creates template.po
```

### Multiple files with auto-generated output:
```bash
mjml2po template1.mjml template2.mjml
# Creates template1.po (or template2.po)
```

### Multiple files with specific output:
```bash
mjml2po template1.mjml template2.mjml translations.po
# Creates translations.po
```

### Process all MJML files in a directory:
```bash
mjml2po emails/*.mjml translations.po
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

## Output Format

The generated PO files follow the standard format with:

- Proper headers with creation timestamp
- File and line number references (`#: filename:line`)
- Escaped special characters
- Sorted by order of appearance
- Merged duplicate strings with combined references

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
- Uses [anyhow](https://crates.io/crates/anyhow) for error handling
- Inspired by localization tools in the i18n ecosystem
