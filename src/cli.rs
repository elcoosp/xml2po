//! Command-line interface.
//!
//! Kept as a library module so integration tests can drive the same code
//! path as the binary.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::error::{Error, Result};
use crate::extract::extract_messages_with;
use crate::message::{Message, SourceRef};
use crate::options::ExtractionOptions;
use crate::po::PoCatalog;
use crate::strategy::get_strategy;

/// Extract and compile translations for XML, HTML and MJML documents.
#[derive(Debug, Parser)]
#[command(name = "xml2po", version, about, propagate_version = true)]
pub struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,

    /// Print progress information to stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

/// CLI subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Extract translatable strings from documents into a PO file.
    Extract {
        /// Input documents (or directories with `--recursive`).
        #[arg(required = true)]
        input: Vec<PathBuf>,

        /// Output PO file (default: first input with `.po` extension).
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Source format (inferred from the first input extension when omitted).
        #[arg(short, long)]
        format: Option<String>,

        /// Merge into the output file, preserving existing translations.
        #[arg(short = 'm', long)]
        merge: bool,

        /// Emit gettext contexts (element name / element@attribute).
        #[arg(long)]
        context: bool,

        /// Extra element whose text is translatable (repeatable).
        #[arg(long = "element")]
        elements: Vec<String>,

        /// Element to ignore completely (repeatable).
        #[arg(long = "ignore")]
        ignored: Vec<String>,

        /// Extra translatable attribute name (repeatable).
        #[arg(long = "attr")]
        attributes: Vec<String>,

        /// Write a POT template header (Project-Id-Version: PACKAGE VERSION)
        /// instead of a PO header.
        #[arg(long)]
        template: bool,

        /// Alias for `--template` (gettext convention).
        #[arg(long)]
        pot: bool,

        /// Disable inline placeholders: split mixed content into separate
        /// strings (v0.2 behavior).
        #[arg(long)]
        no_inline: bool,

        /// Ignore the HTML5/ITS `translate="yes|no"` attribute.
        #[arg(long)]
        no_translate_attr: bool,

        /// Recurse into directories instead of failing on them.
        #[arg(long)]
        recursive: bool,

        /// Skip paths matching this glob (`*`, `?`; repeatable).
        #[arg(long = "exclude")]
        excludes: Vec<String>,

        /// Wrap PO output at this many columns (0 disables wrapping).
        #[arg(long, default_value_t = 79)]
        output_width: usize,
    },

    /// Compile a document by applying translations from a PO file.
    Compile {
        /// Document to translate.
        #[arg(short, long)]
        input: PathBuf,

        /// PO file with translations.
        #[arg(short, long)]
        po: PathBuf,

        /// Output file (stdout when omitted).
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Source format (inferred from the input extension when omitted).
        #[arg(short, long)]
        format: Option<String>,

        /// Expect gettext contexts (element name / element@attribute).
        #[arg(long)]
        context: bool,

        /// Also apply translations flagged `#, fuzzy`.
        #[arg(long)]
        use_fuzzy: bool,

        /// Disable inline placeholders: match v0.2-style split messages.
        #[arg(long)]
        no_inline: bool,

        /// Ignore the HTML5/ITS `translate="yes|no"` attribute.
        #[arg(long)]
        no_translate_attr: bool,

        /// Extra translatable attribute name (repeatable).
        #[arg(long = "attr")]
        attributes: Vec<String>,

        /// Extra element whose text is translatable (repeatable).
        #[arg(long = "element")]
        elements: Vec<String>,

        /// Element to ignore completely (repeatable).
        #[arg(long = "ignore")]
        ignored: Vec<String>,
    },

    /// Show translation coverage of a PO file.
    Stats {
        /// PO file to inspect.
        po: PathBuf,

        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// List the source formats compiled into this build.
    Formats,

    /// Refresh an existing PO catalog: extract, merge, and obsolete
    /// removed strings. Equivalent to `extract --merge`.
    Update {
        /// Input documents (or directories with `--recursive`).
        #[arg(required = true)]
        input: Vec<PathBuf>,

        /// PO file to update (default: first input with `.po` extension).
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Source format (inferred from the first input extension when omitted).
        #[arg(short, long)]
        format: Option<String>,

        /// Emit gettext contexts (element name / element@attribute).
        #[arg(long)]
        context: bool,

        /// Extra element whose text is translatable (repeatable).
        #[arg(long = "element")]
        elements: Vec<String>,

        /// Element to ignore completely (repeatable).
        #[arg(long = "ignore")]
        ignored: Vec<String>,

        /// Extra translatable attribute name (repeatable).
        #[arg(long = "attr")]
        attributes: Vec<String>,

        /// Disable inline placeholders.
        #[arg(long)]
        no_inline: bool,

        /// Ignore the HTML5/ITS `translate="yes|no"` attribute.
        #[arg(long)]
        no_translate_attr: bool,

        /// Recurse into directories.
        #[arg(long)]
        recursive: bool,

        /// Skip paths matching this glob (repeatable).
        #[arg(long = "exclude")]
        excludes: Vec<String>,
    },

    /// Compare a PO file against the current sources (CI drift gate).
    ///
    /// Exits with code 1 when the sources contain strings missing from the
    /// catalog, the catalog contains strings no longer in the sources, or
    /// coverage is below `--min-coverage`.
    Check {
        /// PO file to compare.
        po: PathBuf,

        /// Documents the catalog was generated from.
        #[arg(required = true)]
        input: Vec<PathBuf>,

        /// Source format (inferred from the first input extension when omitted).
        #[arg(short, long)]
        format: Option<String>,

        /// Expect gettext contexts (element name / element@attribute).
        #[arg(long)]
        context: bool,

        /// Extra translatable attribute name (repeatable).
        #[arg(long = "attr")]
        attributes: Vec<String>,

        /// Extra element whose text is translatable (repeatable).
        #[arg(long = "element")]
        elements: Vec<String>,

        /// Element to ignore completely (repeatable).
        #[arg(long = "ignore")]
        ignored: Vec<String>,

        /// Disable inline placeholders.
        #[arg(long)]
        no_inline: bool,

        /// Ignore the HTML5/ITS `translate="yes|no"` attribute.
        #[arg(long)]
        no_translate_attr: bool,

        /// Fail (exit 1) when translated coverage is below this
        /// percentage (0-100).
        #[arg(long)]
        min_coverage: Option<f64>,

        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

/// Runs the CLI with the given arguments (including the program name).
///
/// Returns the process exit code, `1` for `check` drift and `0` otherwise.
///
/// # Errors
///
/// Returns an [`Error`] when a subcommand fails (file IO, parsing, ...).
pub fn run<I, T>(args: I) -> Result<i32>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    // clap reports `--help`/`--version` as errors: honor their exit codes.
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            let text = err.to_string();
            if err.use_stderr() {
                eprint!("{text}");
                return Ok(2);
            }
            print!("{text}");
            return Ok(0);
        }
    };
    dispatch(cli)
}

/// Shared flag bundle for the source-processing subcommands.
struct SourceFlags<'a> {
    context: bool,
    elements: &'a [String],
    ignored: &'a [String],
    attributes: &'a [String],
    no_inline: bool,
    no_translate_attr: bool,
}

impl SourceFlags<'_> {
    fn to_options(&self) -> ExtractionOptions {
        options(
            self.context,
            self.elements,
            self.ignored,
            self.attributes,
            self.no_inline,
            self.no_translate_attr,
        )
    }
}

/// Routes a parsed CLI invocation to its subcommand.
///
/// The body is intentionally one block per subcommand: each block only
/// unwraps the clap variant into its handler, and splitting them would
/// scatter the field wiring.
#[allow(clippy::too_many_lines)]
fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Extract {
            input,
            output,
            format,
            merge,
            context,
            elements,
            ignored,
            attributes,
            no_inline,
            no_translate_attr,
            recursive,
            excludes,
            output_width,
            ..
        } => {
            let flags = SourceFlags {
                context,
                elements: &elements,
                ignored: &ignored,
                attributes: &attributes,
                no_inline,
                no_translate_attr,
            };
            extract_command(&ExtractRequest {
                inputs: &input,
                output: output.as_deref(),
                format: format.as_deref(),
                merge,
                output_width,
                recursive,
                excludes: &excludes,
                options: &flags.to_options(),
                verbose: cli.verbose,
            })
        }
        Command::Update {
            input,
            output,
            format,
            context,
            elements,
            ignored,
            attributes,
            no_inline,
            no_translate_attr,
            recursive,
            excludes,
            ..
        } => {
            let flags = SourceFlags {
                context,
                elements: &elements,
                ignored: &ignored,
                attributes: &attributes,
                no_inline,
                no_translate_attr,
            };
            extract_command(&ExtractRequest {
                inputs: &input,
                output: output.as_deref(),
                format: format.as_deref(),
                merge: true,
                output_width: 79,
                recursive,
                excludes: &excludes,
                options: &flags.to_options(),
                verbose: cli.verbose,
            })
        }
        Command::Compile {
            input,
            po,
            output,
            format,
            context,
            use_fuzzy,
            no_inline,
            no_translate_attr,
            attributes,
            elements,
            ignored,
        } => {
            let flags = SourceFlags {
                context,
                elements: &elements,
                ignored: &ignored,
                attributes: &attributes,
                no_inline,
                no_translate_attr,
            };
            compile_command(
                &input,
                &po,
                output.as_deref(),
                format.as_deref(),
                &flags.to_options(),
                use_fuzzy,
                cli.verbose,
            )
        }
        Command::Stats { po, json } => stats_command(&po, json),
        Command::Formats => Ok(formats_command()),
        Command::Check {
            po,
            input,
            format,
            context,
            attributes,
            elements,
            ignored,
            no_inline,
            no_translate_attr,
            min_coverage,
            json,
        } => {
            let flags = SourceFlags {
                context,
                elements: &elements,
                ignored: &ignored,
                attributes: &attributes,
                no_inline,
                no_translate_attr,
            };
            check_command(
                &po,
                &input,
                format.as_deref(),
                &flags.to_options(),
                min_coverage,
                json,
                cli.verbose,
            )
        }
    }
}

/// Builds [`ExtractionOptions`] from repeated CLI flags.
fn options(
    context: bool,
    elements: &[String],
    ignored: &[String],
    attributes: &[String],
    no_inline: bool,
    no_translate_attr: bool,
) -> ExtractionOptions {
    let mut builder = ExtractionOptions::builder()
        .with_context(context)
        .inline_placeholders(!no_inline)
        .respect_translate_attribute(!no_translate_attr);
    for e in elements {
        builder = builder.extra_translatable_element(e.clone());
    }
    for e in ignored {
        builder = builder.ignored_element(e.clone());
    }
    for a in attributes {
        builder = builder.translatable_attribute(a.clone());
    }
    builder.build()
}

/// Expands directory inputs into their files (recursive walk), filtering
/// by the format's known extensions and the exclusion globs. Files are
/// returned in sorted order for deterministic catalogs.
fn expand_inputs(
    inputs: &[PathBuf],
    format: &str,
    recursive: bool,
    excludes: &[String],
) -> Result<Vec<PathBuf>> {
    let extensions: &[&str] = match format {
        "mjml" => &["mjml"],
        "html" => &["html", "htm", "xhtml"],
        "xml" => &["xml", "svg", "xsl"],
        _ => &[], // unknown formats: take every file
    };
    let mut files = Vec::new();
    for input in inputs {
        if input.is_dir() {
            if !recursive {
                return Err(Error::FormatInference {
                    path: input.clone(),
                });
            }
            collect_directory(input, input, extensions, excludes, &mut files)?;
        } else {
            files.push(input.clone());
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Recursive directory walk collecting files with matching extensions.
/// Exclusion globs are matched against the file name, the path relative
/// to the walked root, and the full path.
fn collect_directory(
    dir: &Path,
    root: &Path,
    extensions: &[&str],
    excludes: &[String],
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let display = path.display().to_string();
        if excludes.iter().any(|pattern| {
            glob_match(pattern, &name)
                || glob_match(pattern, &relative)
                || glob_match(pattern, &display)
        }) {
            continue;
        }
        if path.is_dir() {
            collect_directory(&path, root, extensions, excludes, files)?;
        } else if extensions.is_empty()
            || path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.contains(&e.to_ascii_lowercase().as_str()))
        {
            files.push(path);
        }
    }
    Ok(())
}

/// Minimal glob matching supporting `*` and `?` over path strings.
fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_inner(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob_match_inner(&pattern[1..], text)
                || (!text.is_empty() && glob_match_inner(pattern, &text[1..]))
        }
        (Some(b'?'), Some(_)) => glob_match_inner(&pattern[1..], &text[1..]),
        (Some(p), Some(t)) if p == t => glob_match_inner(&pattern[1..], &text[1..]),
        _ => false,
    }
}

/// Escapes a string for embedding in hand-rolled JSON output.
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Infers the format from a file extension, falling back to `default`.
fn format_from_extension(path: &Path, default: Option<&str>) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase());
    match ext.as_deref() {
        Some("mjml") => Ok("mjml".to_string()),
        Some("html" | "htm" | "xhtml") => Ok("html".to_string()),
        Some("xml" | "svg" | "xsl") => Ok("xml".to_string()),
        _ => default.map_or_else(
            || {
                Err(Error::FormatInference {
                    path: path.to_path_buf(),
                })
            },
            |f| Ok(f.to_string()),
        ),
    }
}

/// Reads a source document as UTF-8 text (BOM-aware when available).
fn read_document(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    #[cfg(feature = "cli-encoding")]
    {
        crate::decode_file_bytes(path, &bytes)
    }
    #[cfg(not(feature = "cli-encoding"))]
    {
        let _ = path;
        String::from_utf8(bytes).map_err(|_| Error::NotUtf8 {
            path: path.to_path_buf(),
        })
    }
}

/// Merges per-file extractions into one deterministic message list.
fn merge_messages(per_file: Vec<Vec<Message>>) -> Vec<Message> {
    let mut grouped: BTreeMap<(Option<String>, String), Vec<SourceRef>> = BTreeMap::new();
    for messages in per_file {
        for message in messages {
            grouped
                .entry((message.msgctxt, message.msgid))
                .or_default()
                .extend(message.references);
        }
    }
    grouped
        .into_iter()
        .map(|((msgctxt, msgid), mut references)| {
            references.sort();
            references.dedup();
            Message {
                msgid,
                msgctxt,
                references,
            }
        })
        .collect()
}

/// Everything the `extract` subcommand needs, bundled to keep the
/// function signature at two arguments.
struct ExtractRequest<'a> {
    inputs: &'a [PathBuf],
    output: Option<&'a Path>,
    format: Option<&'a str>,
    /// Merge with an existing catalog and write a plain (non-template)
    /// header.
    merge: bool,
    /// Column at which to wrap PO values (0 disables wrapping).
    output_width: usize,
    /// Recurse into directories.
    recursive: bool,
    /// Glob patterns to skip.
    excludes: &'a [String],
    options: &'a ExtractionOptions,
    verbose: bool,
}

/// `extract` subcommand.
fn extract_command(request: &ExtractRequest<'_>) -> Result<i32> {
    let ExtractRequest {
        inputs,
        output,
        format,
        merge,
        output_width,
        recursive,
        excludes,
        options,
        verbose,
    } = *request;
    // `update`/`--merge` always write a real PO header; plain `extract`
    // honors `--template`.
    let template = !merge;
    // clap marks the inputs as required, so `inputs` is never empty here.
    let format = format_from_extension(&inputs[0], format)?;
    let strategy = get_strategy(&format)?;
    let files = expand_inputs(inputs, &format, recursive, excludes)?;
    if files.is_empty() {
        return Err(Error::FormatInference {
            path: inputs[0].clone(),
        });
    }

    let output_path = output.map_or_else(
        || {
            let mut path = inputs[0].clone().into_os_string();
            path.push(".po");
            PathBuf::from(path)
        },
        Path::to_path_buf,
    );

    if verbose {
        eprintln!(
            "extracting {} string(s) from {} file(s)",
            format.to_uppercase(),
            files.len()
        );
    }

    let per_file: Vec<Vec<Message>> = {
        #[cfg(feature = "parallel")]
        {
            use rayon::prelude::*;
            files
                .par_iter()
                .map(|path| extract_file(path, strategy, options, verbose))
                .collect::<Result<Vec<_>>>()?
        }
        #[cfg(not(feature = "parallel"))]
        {
            files
                .iter()
                .map(|path| extract_file(path, strategy, options, verbose))
                .collect::<Result<Vec<_>>>()?
        }
    };
    let messages = merge_messages(per_file);

    let existing = if merge {
        read_existing_catalog(&output_path)?
    } else {
        None
    };

    let mut catalog = PoCatalog::from_messages_with_merge(&messages, existing.as_ref());
    if existing.is_none() {
        // POT templates use the conventional placeholder project name.
        catalog.header = crate::po::default_header(template.then_some("PACKAGE VERSION"));
    }

    let po = catalog.to_po_string_with_width(output_width);
    std::fs::write(&output_path, po.as_bytes()).map_err(|source| Error::Io {
        path: output_path.clone(),
        source,
    })?;
    if verbose {
        eprintln!(
            "wrote {} entries to {}",
            messages.len(),
            output_path.display()
        );
    } else {
        println!("PO file created: {}", output_path.display());
    }
    Ok(0)
}

/// Loads the output PO file for `--merge`, if it exists.
fn read_existing_catalog(output_path: &Path) -> Result<Option<PoCatalog>> {
    match std::fs::read_to_string(output_path) {
        Ok(content) => Ok(Some(PoCatalog::parse(&content)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::Io {
            path: output_path.to_path_buf(),
            source: err,
        }),
    }
}

/// Extracts messages from one file, reporting IO/parse failures with path context.
fn extract_file(
    path: &Path,
    strategy: &dyn crate::ExtractionStrategy,
    options: &ExtractionOptions,
    verbose: bool,
) -> Result<Vec<Message>> {
    let content = read_document(path)?;
    let name = path.display().to_string();
    let messages = extract_messages_with(&content, &name, strategy, options)?;
    if verbose {
        eprintln!("  {}: {} string(s)", name, messages.len());
    }
    Ok(messages)
}

/// `formats` subcommand: one supported format per line.
fn formats_command() -> i32 {
    for format in crate::strategy::available_formats() {
        println!("{format}");
    }
    0
}

/// `compile` subcommand.
#[allow(clippy::too_many_arguments)]
fn compile_command(
    input: &Path,
    po_path: &Path,
    output: Option<&Path>,
    format: Option<&str>,
    options: &ExtractionOptions,
    use_fuzzy: bool,
    verbose: bool,
) -> Result<i32> {
    let format = format_from_extension(input, format)?;
    let strategy = get_strategy(&format)?;

    let po_content = std::fs::read_to_string(po_path).map_err(|source| Error::Io {
        path: po_path.to_path_buf(),
        source,
    })?;
    let catalog = PoCatalog::parse(&po_content)?;
    let translations = catalog.translations_with(use_fuzzy);
    if verbose {
        eprintln!("loaded {} translation(s)", translations.len());
    }

    let content = read_document(input)?;
    let report =
        crate::compile::apply_translations_with(&content, &translations, strategy, options)?;
    if verbose {
        eprintln!(
            "applied {} text and {} attribute translation(s)",
            report.applied_texts, report.applied_attributes
        );
    }

    if let Some(path) = output {
        std::fs::write(path, report.output.as_bytes()).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        println!("translated file created: {}", path.display());
    } else {
        let mut stdout = std::io::stdout();
        stdout
            .write_all(report.output.as_bytes())
            .map_err(|source| Error::Write { source })?;
        stdout.flush().map_err(|source| Error::Write { source })?;
    }
    Ok(0)
}

/// `stats` subcommand.
fn stats_command(po_path: &Path, json: bool) -> Result<i32> {
    let po_content = std::fs::read_to_string(po_path).map_err(|source| Error::Io {
        path: po_path.to_path_buf(),
        source,
    })?;
    let catalog = PoCatalog::parse(&po_content)?;
    let stats = catalog.stats();
    if json {
        println!(
            "{{\"total\":{},\"translated\":{},\"untranslated\":{},\"coverage\":{:.1}}}",
            stats.total,
            stats.translated,
            stats.untranslated,
            stats.coverage_percent()
        );
    } else {
        println!("entries:        {}", stats.total);
        println!("translated:     {}", stats.translated);
        println!("untranslated:   {}", stats.untranslated);
        println!("coverage:       {:.1}%", stats.coverage_percent());
    }
    Ok(0)
}

/// `check` subcommand.
#[allow(clippy::too_many_arguments)]
fn check_command(
    po_path: &Path,
    inputs: &[PathBuf],
    format: Option<&str>,
    options: &ExtractionOptions,
    min_coverage: Option<f64>,
    json: bool,
    verbose: bool,
) -> Result<i32> {
    let po_content = std::fs::read_to_string(po_path).map_err(|source| Error::Io {
        path: po_path.to_path_buf(),
        source,
    })?;
    let catalog = PoCatalog::parse(&po_content)?;

    // clap marks the inputs as required, so `inputs` is never empty here.
    let format = format_from_extension(&inputs[0], format)?;
    let strategy = get_strategy(&format)?;
    let files = expand_inputs(inputs, &format, false, &[])?;

    let per_file: Vec<Vec<Message>> = files
        .iter()
        .map(|path| extract_file(path, strategy, options, verbose))
        .collect::<Result<Vec<_>>>()?;
    let messages = merge_messages(per_file);

    let diff = catalog.diff_messages(&messages);
    let stats = catalog.stats();
    let coverage = stats.coverage_percent();
    let coverage_ok = min_coverage.is_none_or(|min| coverage + f64::EPSILON >= min);

    if json {
        print!(
            "{{\"clean\":{},\"coverage\":{:.1},\"added\":[",
            diff.is_clean(),
            coverage
        );
        for (i, key) in diff.added.iter().enumerate() {
            if i > 0 {
                print!(",");
            }
            print!(
                "{{\"msgid\":\"{}\",\"msgctxt\":{}}}",
                json_escape(&key.msgid),
                key.msgctxt
                    .as_ref()
                    .map_or("null".to_string(), |c| format!("\"{}\"", json_escape(c)))
            );
        }
        print!("],\"removed\":[");
        for (i, key) in diff.removed.iter().enumerate() {
            if i > 0 {
                print!(",");
            }
            print!(
                "{{\"msgid\":\"{}\",\"msgctxt\":{}}}",
                json_escape(&key.msgid),
                key.msgctxt
                    .as_ref()
                    .map_or("null".to_string(), |c| format!("\"{}\"", json_escape(c)))
            );
        }
        println!("]}}");
    } else if diff.is_clean() {
        println!("OK: catalog is in sync with the sources");
    } else {
        for key in &diff.added {
            println!("+ missing from catalog: {:?}", key.msgid);
        }
        for key in &diff.removed {
            println!("- stale in catalog: {:?}", key.msgid);
        }
        println!(
            "catalog drift: {} added, {} removed",
            diff.added.len(),
            diff.removed.len()
        );
    }

    if !diff.is_clean() {
        return Ok(1);
    }
    match min_coverage {
        Some(min) if !coverage_ok => {
            if !json {
                println!("coverage {coverage:.1}% is below the required {min:.1}%");
            }
            Ok(1)
        }
        _ => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(context: bool) -> ExtractionOptions {
        options(context, &[], &[], &[], false, false)
    }

    #[test]
    fn format_inference() {
        assert_eq!(
            format_from_extension(Path::new("a.mjml"), None).unwrap(),
            "mjml"
        );
        assert_eq!(
            format_from_extension(Path::new("a.HTML"), None).unwrap(),
            "html"
        );
        assert_eq!(
            format_from_extension(Path::new("a.htm"), None).unwrap(),
            "html"
        );
        assert_eq!(
            format_from_extension(Path::new("a.svg"), None).unwrap(),
            "xml"
        );
        assert_eq!(
            format_from_extension(Path::new("a.txt"), Some("xml")).unwrap(),
            "xml"
        );
        let err = format_from_extension(Path::new("a.txt"), None).unwrap_err();
        assert!(err.to_string().contains("cannot infer format"));
    }

    #[test]
    fn options_builder_from_flags() {
        let o = options(
            true,
            &["custom".to_string()],
            &["pre".to_string()],
            &["data-x".to_string()],
            false,
            false,
        );
        assert!(o.with_context);
        assert_eq!(o.extra_translatable_elements, vec!["custom"]);
        assert_eq!(o.ignored_elements, vec!["pre"]);
        assert_eq!(o.translatable_attributes, vec!["data-x"]);
        assert_eq!(opts(false), ExtractionOptions::default());
    }

    #[test]
    fn merge_messages_sorts_and_dedups() {
        let m = merge_messages(vec![
            vec![Message::new("b", vec![SourceRef::new("2.html", 5)])],
            vec![
                Message::new(
                    "b",
                    vec![SourceRef::new("1.html", 9), SourceRef::new("2.html", 5)],
                ),
                Message::new("a", vec![SourceRef::new("1.html", 1)]),
            ],
        ]);
        let ids: Vec<&str> = m.iter().map(|x| x.msgid.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(m[1].references.len(), 2);
        assert_eq!(
            m[1].references,
            vec![SourceRef::new("1.html", 9), SourceRef::new("2.html", 5)]
        );
    }

    #[test]
    fn merge_messages_keeps_contexts_apart() {
        let m = merge_messages(vec![
            vec![Message::with_context("x", "a", vec![])],
            vec![Message::with_context("x", "b", vec![])],
        ]);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn read_document_missing_file() {
        let err = read_document(Path::new("/nonexistent/file.mjml")).unwrap_err();
        assert!(matches!(err, Error::Io { .. }));
    }

    #[test]
    fn read_document_utf8_and_not_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ok.html");
        std::fs::write(&p, b"<p>ok</p>").unwrap();
        assert!(read_document(&p).unwrap().contains("ok"));

        let bad = dir.path().join("bad.html");
        std::fs::write(&bad, [0xFF, 0xFE, 0x00]).unwrap();
        let err = read_document(&bad).unwrap_err();
        assert!(matches!(err, Error::NotUtf8 { .. }));
    }

    #[test]
    fn cli_parse_rejects_unknown_command() {
        let err = Cli::try_parse_from(["xml2po", "bogus"]).unwrap_err();
        assert_ne!(err.to_string(), "");
    }
}
