# mjml2po

## Usage Examples:

1. **Single file input with auto-generated output name:**
   ```bash
   cargo run template.mjml
   # Creates template.po
   ```

2. **Multiple files with auto-generated output name:**
   ```bash
   cargo run template1.mjml template2.mjml
   # Creates template1.po (or template2.po if that's the first file)
   ```

3. **Multiple files with specific output:**
   ```bash
   cargo run template1.mjml template2.mjml translations.po
   # Creates translations.po
   ```

## Features:

1. **Multiple File Support**: Can process multiple MJML files and merge all strings into a single PO file
2. **Proper Position Tracking**: Records line numbers and file names for each extracted string
3. **Duplicate Handling**: Merges duplicate strings and combines their references
4. **Sorting**: Sorts entries by their first occurrence in the files
5. **Proper Escaping**: Correctly escapes special characters for PO file format
6. **Error Handling**: Uses `anyhow` for comprehensive error reporting
7. **Standard PO Format**: Generates valid PO files with proper headers

## How it works:

1. **XML Parsing**: Uses `quick-xml` to parse MJML files as XML
2. **Text Extraction**: Extracts all text nodes and CDATA sections
3. **Reference Tracking**: Records file names and line numbers for each string
4. **Duplicate Merging**: Combines references for identical strings across files
5. **PO Generation**: Creates a properly formatted PO file with all extracted strings
