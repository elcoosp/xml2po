# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-11-04

### Added

- **Inline markup placeholders** (flagship): mixed content folds into single
  messages with `<tag>` placeholders; compilation re-inserts the original
  tags with their attributes and rejects markup-mismatched translations
  (`CompileReport::skipped_invalid`). Enabled by default; opt out with
  `ExtractionOptions::inline_placeholders(false)` / CLI `--no-inline`.
- ITS-lite semantics: HTML5 `translate="yes|no"` and `its:translate`
  attributes are honored by default (`respect_translate_attribute`);
  non-translatable inline content is kept verbatim inside msgids.
- Lossless PO layer: full plural round-trip (`msgstr[N]` →
  `PoEntry::msgstrs_plural`), `#, flags` parsing/serialization with fuzzy
  handling (`PoCatalog::translations_with`, `PoEntry::is_fuzzy`),
  `#|` previous msgids, `#~` obsolete entries preserved across merges with
  resurrection of returning strings, typed `PoMetadata` header access,
  `PoCatalog::parse_strict` duplicate detection, `from_key`/`get_mut`/
  `set_translation`/`from_messages` conveniences, `set_language`.
- XLIFF 1.2 export/import (`catalog_to_xliff` / `catalog_from_xliff`,
  feature `xliff`, on by default) with `bpt`/`ept`/`ph` inline codes,
  `resname` contexts, reference context-groups and translator notes.
- CLI: `update` subcommand (extract + merge), `--pot` alias, `--no-inline`,
  `--no-translate-attr`, `--use-fuzzy` (compile), `--json` (stats, check),
  `--min-coverage` coverage gate (check), directory inputs with
  `--recursive` and `--exclude` globs.
- Errors: XML failures now carry `line`/`column`; new
  `InvalidTranslation`, `FormatInference` and `DuplicateKey` variants.
- Text filter: rejects lone entity references (`&nbsp;`) and file paths.

### Changed

- **Breaking**: `ExtractionOptions::default()` now enables inline
  placeholders and translate-attribute awareness; msgids in inline mode
  are XML-escaped fragments (`&` → `&amp;`), matching itstool conventions.
- **Breaking**: `Error::Xml` gained `line`/`column` fields;
  `CompileReport` gained `skipped_invalid`; `PoEntry` gained
  `msgstrs_plural`, `flags` and `previous_msgid`; `PoCatalog` gained
  `obsolete`.
- **Breaking**: fuzzy entries are excluded from `PoCatalog::translations()`
  by default (standard gettext behavior).
- CLI: format inference failures use `Error::FormatInference` instead of a
  repurposed `PoParse` error.

### Fixed

- Unclosed HTML void elements (`<img>`, `<br>` without slash) no longer
  poison the ignore state of following siblings during extraction and
  compilation.
- PO merging no longer drops translations of strings that were renamed or
  temporarily removed (obsolete resurrection).

## [0.2.0] - 2026-11-01

### Added

- Working extract and compile commands

- Use rspolib

- Add verbose

- Split lib add ExtractionStrategy trait

- Messages_to_po take existing_po_path


### Changed

- Modularize lib, modernize deps and harden tooling


### Documentation

- Rm old doc


### Fixed

- Warns

- Missing strong translatable mjml


### Internal

- Init

- Up readme

- Run clippy

- Readme

