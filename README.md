<div align="center">
  <img src="docs/logo.png" alt="xml2po Logo" width="300"/>
  <p>
    <strong>Markup-aware translation workflows for XML, HTML and MJML</strong><br/>
    Extract to PO, translate anywhere (gettext, Weblate, Crowdin, CAT tools via XLIFF), compile back — without ever losing a translation.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Rust-1.86%20%7C%202024-000000?style=flat-square&logo=rust" alt="Rust"/>
    <img src="https://img.shields.io/crates/v/xml2po?style=flat-square" alt="Crates.io"/>
    <img src="https://img.shields.io/badge/License-MIT-blue?style=flat-square" alt="License MIT"/>
    <img src="https://img.shields.io/badge/Target-Native%20%2B%20WASM-6F4E37?style=flat-square" alt="WASM Support"/>
  </p>
</div>

---

`xml2po` streamlines localization for XML-flavored templates: it extracts
translatable strings into standard PO files, merges translator work back
without data loss, and compiles translations into the original documents
while preserving whitespace, entities and markup byte-for-byte.

## Highlights

- **Inline markup placeholders** — `<p>Hello <b>bold</b> world</p>` is one
  msgid: `Hello <b>bold</b> world`. Sentences stay whole, translators
  reorder words freely, and compilation re-inserts the *original* tags
  (attributes included). Translations whose markup doesn't match are
  rejected safely and counted in the report.
- **ITS-lite semantics** — HTML5 `translate="no|yes"` and
  `its:translate` are honored by default. Non-translatable inline content
  (`<span translate="no">ACME</span>`) is kept verbatim inside the msgid,
  itstool-style.
- **Lossless PO layer** — full plural round-trip (`msgstr[N]`), `#, flags`
  (fuzzy translations are skipped unless you ask for them), `#|` previous
  msgids, `#~` obsolete entries preserved across merges, typed header
  metadata (`PoMetadata`), and strict duplicate detection.
- **CI gates** — `check` fails on catalog drift or low coverage;
  `stats --json` / `check --json` for machine-readable pipelines.
- **XLIFF 1.2 bridge** — exchange catalogs with Trados, memoQ, Lokalise,
  Transifex and friends (`catalog_to_xliff` / `catalog_from_xliff`).
- **WebAssembly ready** — compiles to `wasm32-unknown-unknown` for
  in-browser localization workflows.
- **Error messages with line:column** — XML failures point at the exact
  position, not a byte offset.

## Installation

### CLI

```bash
cargo install xml2po
```

### Library

```toml
[dependencies]
xml2po = "0.3"
```

## Quickstart

### Command line

```bash
# Extract translatable strings (inline placeholders on by default)
xml2po extract src/*.html --output messages.po

# Refresh a catalog after source changes: translations are kept,
# removed strings become #~ obsolete entries
xml2po update src/*.html --output messages.po

# Compile a translated document
xml2po compile --input page.html --po fr.po --output fr.html

# CI: fail when the catalog drifts or coverage drops below 90%
xml2po check --min-coverage 90 messages.po src/*.html
xml2po stats --json messages.po
```

### Library

```rust
use xml2po::{extract_messages, apply_translations, get_strategy};
use xml2po::po::PoCatalog;

let content = r#"<p>Hello <b>bold</b> world</p>"#;
let strategy = get_strategy("html")?;

// One message per sentence — inline tags folded in.
let messages = extract_messages(content, "page.html", strategy)?;
assert_eq!(messages[0].msgid, "Hello <b>bold</b> world");

// Translate and compile: the original <b> tag is re-inserted.
let mut catalog = PoCatalog::from_messages(&messages);
catalog.set_translation(
    xml2po::message::CatalogKey::new(None, "Hello <b>bold</b> world"),
    "Hola <b>negrita</b> mundo",
);
let translated = apply_translations(content, &catalog.translations(), strategy)?;
assert_eq!(translated, "<p>Hola <b>negrita</b> mundo</p>");
```

## How inline placeholders work

During **extraction**, inline elements (`b`, `em`, `span`, `a`, `strong`,
`br`, …) fold into the surrounding message as attribute-free tags, and
literal text is XML-escaped (`&` → `&amp;`, `<` → `&lt;`) so msgids stay
unambiguous:

```html
<p>Name: <span translate="no">ACME</span> inc.</p>
```

```po
msgid "Name: <span translate=\"no\">ACME</span> inc."
```

During **compilation** the tag *sequences* of msgid and msgstr must match
(same tags, same order). Text between tags may be reordered, split or
merged freely — word order can change between languages. Each translation
tag is replaced by the corresponding source tag with its original
attributes:

```html
<p>Name: <span translate="no">ACME</span> GmbH</p>
```

| Translation | Result |
| :--- | :--- |
| `Name: <span translate="no">ACME</span> Ltd.` | ✅ applied |
| `Name: <span>ACME</span> Ltd.` (missing attr) | ✅ tags match by name — applied, source form restored |
| `Name: ACME Ltd.` (tags dropped) | ❌ rejected, source kept, `skipped_invalid` +1 |
| `Name: </span><span>ACME` (reordered) | ❌ rejected |

Opt out any time: `ExtractionOptions::builder().inline_placeholders(false)`
or the CLI flag `--no-inline` restores classic per-segment extraction.

## Supported formats & elements

| Format | Translatable elements | Inline elements | Translatable attributes |
| :--- | :--- | :--- | :--- |
| **MJML** | `mj-title`, `mj-text`, `mj-button`, `strong`, `em` | `strong`, `em` | — |
| **HTML** | `p`, `h1`–`h6`, `span`, `div`, `a`, `button`, `label`, `li`, `td`, `th`, `title`, `option`, `strong`, `em`, `b`, `i` | the HTML phrasing set (`a`, `abbr`, `b`, `br`, `em`, `i`, `kbd`, `mark`, `q`, `small`, `span`, `strong`, `sub`, `sup`, `time`, `u`, `var`, …) | `alt`, `title`, `placeholder`, `aria-label` |
| **XML** | everything (generic fallback) | — (opt in via a custom strategy) | — |

> [!NOTE]
> **Smart filtering** — the extraction engine automatically excludes URLs,
> e-mail addresses, file paths, lone entities (`&nbsp;`), hex colors,
> pure numbers/symbols and placeholder-only strings (`%s`, `{{name}}`).

Custom formats: implement [`ExtractionStrategy`](src/strategy.rs) —
`is_translatable_element`, `is_ignored_element`,
`translatable_attributes` and (new in 0.3) `is_inline_element` — and
pass it to `extract_messages` / `apply_translations`.

## Smart merge: never lose a translation

`extract --merge` (or the `update` subcommand) implements gettext
`msgmerge` semantics:

- strings still present keep their translation, flags, comments and
  plural forms;
- strings removed from the sources become `#~` obsolete entries;
- strings that come back are **resurrected** with their old translation;
- fuzzy translations are kept but skipped during compilation unless
  `--use-fuzzy` (or `PoCatalog::translations_with(true)`) is set.

## CI integration

```yaml
- name: Check translations
  run: |
    xml2po update src/templates/*.html --output locales/messages.po
    xml2po check --min-coverage 90 locales/messages.po src/templates/*.html
```

- `check` exits `0` only when the catalog is in sync **and** coverage is
  at or above `--min-coverage`.
- `check --json` prints `{"clean":true,"coverage":95.0,"added":[],"removed":[]}`.
- `stats --json` prints `{"total":42,"translated":40,"untranslated":2,"coverage":95.2}`.
- `--recursive --exclude 'drafts/*'` extracts whole directory trees with
  glob exclusions.

> [!TIP]
> **WebAssembly usage** — compile to `wasm32-unknown-unknown` and call
> `extract_to_po` / `compile_with_po` from JavaScript via `wasm-bindgen`:
> ```javascript
> const po = extractToPo(mjmlContent, "mjml", "template.mjml");
> const translated = compileWithPo(htmlContent, po, "html");
> ```

## Cargo features

| Feature | Description | Default |
| :--- | :--- | :---: |
| `mjml` | MJML format extraction and compilation | ✅ |
| `html` | HTML format extraction and compilation | ✅ |
| `xml` | Generic XML fallback strategy | ✅ |
| `chrono` | Timestamps in generated PO headers | ✅ |
| `serde` | `Serialize`/`Deserialize` for messages, options and catalogs | ❌ |
| `xliff` | XLIFF 1.2 export/import | ✅ |
| `cli` | The command-line interface binary | ✅ |
| `parallel` | Rayon-parallel extraction of multiple input files | ✅ |
| `cli-encoding` | UTF-16 (BOM) input support in the CLI | ✅ |

## PO feature matrix

| PO construct | Extract | Parse | Serialize | Compile |
| :--- | :---: | :---: | :---: | :---: |
| `msgctxt` | ✅ (opt-in) | ✅ | ✅ | ✅ |
| `msgid_plural` + `msgstr[N]` | — | ✅ | ✅ | msgstr[0] |
| `#: references` (file:line) | ✅ | ✅ | ✅ | — |
| `#, flags` | — | ✅ | ✅ | fuzzy policy |
| `#|` previous msgids | — | ✅ | ✅ | — |
| `#~` obsolete entries | ✅ (merge) | ✅ | ✅ | skipped |
| Header metadata | ✅ | ✅ (`PoMetadata`) | ✅ | — |
| Wrapping at 79 cols / `--no-wrap` | ✅ | ✅ | ✅ | — |

## Development

```bash
# Run the test suite (unit + CLI integration + doc tests)
cargo test --all-features

# Lint with the pedantic configuration used by CI (warnings are errors)
cargo clippy --all-features --all-targets -- -W clippy::pedantic --deny warnings

# Check formatting
cargo fmt --all -- --check

# Test WebAssembly build
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown

# Measure coverage (requires cargo-llvm-cov)
cargo llvm-cov --all-features
```

A runnable library example lives in `examples/translate_flow.rs`:
`cargo run --example translate_flow`.

> [!IMPORTANT]
> When contributing a new format, implement the
> [`ExtractionStrategy`](src/strategy.rs) trait (elements, ignored
> elements, translatable attributes, inline elements) and add a variant
> to `Format`. Refer to the built-in strategies for the pattern.

## License

MIT — see [LICENSE](LICENSE).
