//! Command-line interface.
//!
//! Kept as a library module so integration tests can drive the same code
//! path as the binary.

use std::collections::BTreeMap;
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
        /// Input documents.
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
    },

    /// List the source formats compiled into this build.
    Formats,

    /// Compare a PO file against the current sources (CI drift gate).
    ///
    /// Exits with code 1 when the sources contain strings missing from the
    /// catalog or the catalog contains strings no longer in the sources.
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
            template,
            output_width,
        } => extract_command(&ExtractRequest {
            inputs: &input,
            output: output.as_deref(),
            format: format.as_deref(),
            merge,
            template,
            output_width,
            options: &options(context, &elements, &ignored, &attributes),
            verbose: cli.verbose,
        }),
        Command::Compile {
            input,
            po,
            output,
            format,
            context,
            attributes,
            elements,
            ignored,
        } => compile_command(
            &input,
            &po,
            output.as_deref(),
            format.as_deref(),
            &options(context, &elements, &ignored, &attributes),
            cli.verbose,
        ),
        Command::Stats { po } => stats_command(&po),
        Command::Formats => Ok(formats_command()),
        Command::Check {
            po,
            input,
            format,
            context,
            attributes,
            elements,
            ignored,
        } => check_command(
            &po,
            &input,
            format.as_deref(),
            &options(context, &elements, &ignored, &attributes),
            cli.verbose,
        ),
    }
}

/// Builds [`ExtractionOptions`] from repeated CLI flags.
fn options(
    context: bool,
    elements: &[String],
    ignored: &[String],
    attributes: &[String],
) -> ExtractionOptions {
    let mut builder = ExtractionOptions::builder().with_context(context);
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
                Err(Error::PoParse {
                    line: 0,
                    message: format!(
                        "cannot infer format from {:?}; pass --format <mjml|html|xml>",
                        path.display()
                    ),
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
    merge: bool,
    /// Write a POT template header instead of a PO header.
    template: bool,
    /// Column at which to wrap PO values (0 disables wrapping).
    output_width: usize,
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
        template,
        output_width,
        options,
        verbose,
    } = *request;
    // clap marks the inputs as required, so `inputs` is never empty here.
    let format = format_from_extension(&inputs[0], format)?;
    let strategy = get_strategy(&format)?;

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
            inputs.len()
        );
    }

    let per_file: Vec<Vec<Message>> = {
        #[cfg(feature = "parallel")]
        {
            use rayon::prelude::*;
            inputs
                .par_iter()
                .map(|path| extract_file(path, strategy, options, verbose))
                .collect::<Result<Vec<_>>>()?
        }
        #[cfg(not(feature = "parallel"))]
        {
            inputs
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
fn compile_command(
    input: &Path,
    po_path: &Path,
    output: Option<&Path>,
    format: Option<&str>,
    options: &ExtractionOptions,
    verbose: bool,
) -> Result<i32> {
    let format = format_from_extension(input, format)?;
    let strategy = get_strategy(&format)?;

    let po_content = std::fs::read_to_string(po_path).map_err(|source| Error::Io {
        path: po_path.to_path_buf(),
        source,
    })?;
    let catalog = PoCatalog::parse(&po_content)?;
    let translations = catalog.translations();
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
fn stats_command(po_path: &Path) -> Result<i32> {
    let po_content = std::fs::read_to_string(po_path).map_err(|source| Error::Io {
        path: po_path.to_path_buf(),
        source,
    })?;
    let catalog = PoCatalog::parse(&po_content)?;
    let stats = catalog.stats();
    println!("entries:        {}", stats.total);
    println!("translated:     {}", stats.translated);
    println!("untranslated:   {}", stats.untranslated);
    println!("coverage:       {:.1}%", stats.coverage_percent());
    Ok(0)
}

/// `check` subcommand.
fn check_command(
    po_path: &Path,
    inputs: &[PathBuf],
    format: Option<&str>,
    options: &ExtractionOptions,
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

    let per_file: Vec<Vec<Message>> = inputs
        .iter()
        .map(|path| extract_file(path, strategy, options, verbose))
        .collect::<Result<Vec<_>>>()?;
    let messages = merge_messages(per_file);

    let diff = catalog.diff_messages(&messages);
    if diff.is_clean() {
        println!("OK: catalog is in sync with the sources");
        return Ok(0);
    }
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
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(context: bool) -> ExtractionOptions {
        options(context, &[], &[], &[])
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
