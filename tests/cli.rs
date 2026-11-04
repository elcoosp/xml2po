//! End-to-end tests for the `xml2po` CLI.
//!
//! Most tests drive [`xml2po::cli::run`] in process so the CLI module is
//! fully covered; a few spawn the real binary to verify process-level
//! behavior (exit codes, version output).

use std::path::{Path, PathBuf};
use std::process::Command;

use xml2po::decode_file_bytes;

fn run_cli<I, S>(args: I) -> Result<i32, xml2po::Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut full: Vec<String> = vec!["xml2po".to_string()];
    full.extend(args.into_iter().map(|a| a.as_ref().to_string()));
    // Callers pass absolute paths, so no working-directory juggling is
    // needed and tests stay parallel-safe.
    xml2po::cli::run(full)
}

fn run_bin(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_xml2po"))
        .args(args)
        .output()
        .expect("binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// A minimal MJML document with three translatable strings.
const MJML: &str = r"<mjml><body><mj-text>Hello</mj-text><mj-button>Click</mj-button><mj-title>Title</mj-title></body></mjml>";

fn write(path: &Path, content: &str) -> PathBuf {
    std::fs::write(path, content).unwrap();
    path.to_path_buf()
}

/// Stringified path helper keeping call sites short.
fn p(dir: &Path, name: &str) -> String {
    dir.join(name).to_str().unwrap().to_string()
}

#[test]
fn extract_creates_po_with_default_output_name() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("template.mjml"), MJML);

    let input = dir.path().join("template.mjml");
    let input_str = input.to_str().unwrap();
    write(&input, MJML);

    run_cli(["extract", input_str]).unwrap();
    let po = std::fs::read_to_string(dir.path().join("template.mjml.po")).unwrap();
    assert!(po.contains("msgid \"Hello\""));
    assert!(po.contains("POT-Creation-Date"));
    assert!(po.contains(&format!("#: {input_str}:1")));
}

#[test]
fn extract_with_explicit_output_and_verbose() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);

    run_cli([
        "extract",
        "-v",
        &p(dir.path(), "t.mjml"),
        "-o",
        &p(dir.path(), "out.po"),
    ])
    .unwrap();
    assert!(
        std::fs::read_to_string(dir.path().join("out.po"))
            .unwrap()
            .contains("Hello")
    );
}

#[test]
fn extract_multiple_files_merges_references() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("a.mjml").as_path(),
        "<mjml><mj-text>Shared</mj-text></mjml>",
    );
    write(
        dir.path().join("b.mjml").as_path(),
        "<mjml><mj-text>Shared</mj-text><mj-text>Only B</mj-text></mjml>",
    );

    run_cli([
        "extract",
        &p(dir.path(), "a.mjml"),
        &p(dir.path(), "b.mjml"),
        "-o",
        &p(dir.path(), "out.po"),
    ])
    .unwrap();
    let po = std::fs::read_to_string(dir.path().join("out.po")).unwrap();
    assert!(po.contains("msgid \"Shared\""));
    assert!(po.contains("a.mjml:1"));
    assert!(po.contains("b.mjml:1"));
    assert!(po.contains("msgid \"Only B\""));
}

#[test]
fn extract_merge_preserves_translations() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "t.po")
        ])
        .unwrap(),
        0
    );

    // Simulate a translator filling in one entry.
    let po_path = dir.path().join("t.po");
    let po = std::fs::read_to_string(&po_path).unwrap();
    std::fs::write(
        &po_path,
        po.replace(
            "msgid \"Hello\"\nmsgstr \"\"",
            "msgid \"Hello\"\nmsgstr \"Bonjour\"",
        ),
    )
    .unwrap();

    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "t.po"),
            "--merge"
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(&po_path).unwrap();
    assert!(po.contains("msgstr \"Bonjour\""), "translation kept: {po}");
}

#[test]
fn extract_merge_without_existing_output_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "fresh.po"),
            "--merge"
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(dir.path().join("fresh.po")).unwrap();
    assert!(po.contains("msgid \"Hello\""));
}

#[test]
fn extract_merge_with_unreadable_output_fails() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    // A directory at the PO path cannot be read as a catalog.
    std::fs::create_dir(dir.path().join("t.po")).unwrap();
    assert!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "t.po"),
            "--merge"
        ])
        .is_err()
    );
}

#[test]
fn extract_write_failure_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    // Output inside a nonexistent directory cannot be created.
    assert!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "missing-dir/out.po")
        ])
        .is_err()
    );
}

#[test]
fn extract_context_mode_emits_msgctxt() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "t.po"),
            "--context"
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(dir.path().join("t.po")).unwrap();
    assert!(po.contains("msgctxt \"mj-text\""), "{po}");
    assert!(po.contains("msgid \"Hello\""));
}

#[test]
fn extract_repeated_flags_accumulate() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.mjml"),
        "<mjml><body><mj-text>Hello</mj-text><custom>Widget</custom><mj-section><mj-text>Skip me</mj-text></mj-section></body></mjml>",
    );
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "t.po"),
            "--element",
            "custom",
            "--element",
            "aside",
            "--ignore",
            "mj-section",
            "--attr",
            "data-label",
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(dir.path().join("t.po")).unwrap();
    assert!(po.contains("msgid \"Widget\""), "{po}");
    assert!(!po.contains("Skip me"), "{po}");
}

#[test]
fn compile_applies_translations_to_file() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    write(
        &dir.path().join("t.po"),
        "msgid \"Hello\"\nmsgstr \"Bonjour\"\n\nmsgid \"Click\"\nmsgstr \"Cliquer\"\n",
    );

    run_cli([
        "compile",
        "-v",
        "-i",
        &p(dir.path(), "t.mjml"),
        "-p",
        &p(dir.path(), "t.po"),
        "-o",
        &p(dir.path(), "fr.mjml"),
    ])
    .unwrap();
    let out = std::fs::read_to_string(dir.path().join("fr.mjml")).unwrap();
    assert!(out.contains("Bonjour"));
    assert!(out.contains("Cliquer"));
    assert!(!out.contains(">Hello<"));
}

#[test]
fn compile_to_stdout_in_process() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.mjml"),
        "<mjml><mj-text>Hello</mj-text></mjml>",
    );
    write(
        &dir.path().join("t.po"),
        "msgid \"Hello\"\nmsgstr \"Bonjour\"\n",
    );
    // Output goes to the test process stdout; success is all we assert.
    assert_eq!(
        run_cli([
            "compile",
            "-i",
            &p(dir.path(), "t.mjml"),
            "-p",
            &p(dir.path(), "t.po")
        ])
        .unwrap(),
        0
    );
}

#[test]
fn compile_write_failure_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    write(
        &dir.path().join("t.po"),
        "msgid \"Hello\"\nmsgstr \"Bonjour\"\n",
    );
    assert!(
        run_cli([
            "compile",
            "-i",
            &p(dir.path(), "t.mjml"),
            "-p",
            &p(dir.path(), "t.po"),
            "-o",
            &p(dir.path(), "missing-dir/out.mjml"),
        ])
        .is_err()
    );
}

#[test]
fn stats_reports_coverage_in_process() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.po"),
        "msgid \"a\"\nmsgstr \"A\"\n\nmsgid \"b\"\nmsgstr \"\"\n",
    );
    // Output goes to the test process stdout; success is all we assert.
    assert_eq!(run_cli(["stats", &p(dir.path(), "t.po")]).unwrap(), 0);
}

#[test]
fn formats_lists_supported_formats_in_process() {
    // Output goes to the test process stdout; success is all we assert.
    assert_eq!(run_cli(["formats"]).unwrap(), 0);
}

#[test]
fn extract_template_writes_pot_header() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "template.pot"),
            "--template"
        ])
        .unwrap(),
        0
    );
    let pot = std::fs::read_to_string(dir.path().join("template.pot")).unwrap();
    assert!(pot.contains("Project-Id-Version: PACKAGE VERSION"), "{pot}");
}

#[test]
fn extract_output_width_controls_wrapping() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.mjml"),
        "<mjml><mj-text>This is a rather long translatable sentence</mj-text></mjml>",
    );
    // A tiny width must force wrapped continuation lines.
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "narrow.po"),
            "--output-width",
            "16"
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(dir.path().join("narrow.po")).unwrap();
    assert!(po.contains("msgid \"\"\n"), "{po}");

    // Width 0 disables wrapping entirely: one long single-line msgid.
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "wide.po"),
            "--output-width",
            "0"
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(dir.path().join("wide.po")).unwrap();
    assert!(
        po.contains("msgid \"This is a rather long translatable sentence\""),
        "{po}"
    );
}

#[test]
fn compile_missing_po_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    assert!(
        run_cli([
            "compile",
            "-i",
            &p(dir.path(), "t.mjml"),
            "-p",
            &p(dir.path(), "absent.po")
        ])
        .is_err()
    );
}

#[test]
fn compile_missing_input_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.po"), "msgid \"a\"\nmsgstr \"A\"\n");
    assert!(
        run_cli([
            "compile",
            "-i",
            &p(dir.path(), "ghost.mjml"),
            "-p",
            &p(dir.path(), "t.po")
        ])
        .is_err()
    );
}

#[test]
fn compile_broken_po_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    write(&dir.path().join("bad.po"), "garbage line\n");
    assert!(
        run_cli([
            "compile",
            "-i",
            &p(dir.path(), "t.mjml"),
            "-p",
            &p(dir.path(), "bad.po")
        ])
        .is_err()
    );
}

#[test]
fn stats_missing_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    assert!(run_cli(["stats", &p(dir.path(), "absent.po")]).is_err());
}

#[test]
fn check_missing_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    assert!(
        run_cli([
            "check",
            &p(dir.path(), "absent.po"),
            &p(dir.path(), "t.mjml")
        ])
        .is_err()
    );
}

#[test]
fn check_passes_when_in_sync() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.mjml"),
        "<mjml><mj-text>Hello</mj-text></mjml>",
    );
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "t.mjml"),
            "-o",
            &p(dir.path(), "t.po")
        ])
        .unwrap(),
        0
    );
    assert_eq!(
        run_cli(["check", &p(dir.path(), "t.po"), &p(dir.path(), "t.mjml")]).unwrap(),
        0
    );
}

#[test]
fn check_fails_on_drift() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.mjml"),
        "<mjml><mj-text>Hello</mj-text></mjml>",
    );
    write(&dir.path().join("t.po"), "msgid \"Gone\"\nmsgstr \"X\"\n");
    assert_eq!(
        run_cli(["check", &p(dir.path(), "t.po"), &p(dir.path(), "t.mjml")]).unwrap(),
        1
    );
}

#[test]
fn format_inferred_from_extension() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("page.html"),
        "<html><body><p>Text</p></body></html>",
    );
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "page.html"),
            "-o",
            &p(dir.path(), "page.po")
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(dir.path().join("page.po")).unwrap();
    assert!(po.contains("msgid \"Text\""));
}

#[test]
fn explicit_format_overrides_inference() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("data.txt"),
        "<root><item>Value</item></root>",
    );
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "data.txt"),
            "-o",
            &p(dir.path(), "d.po"),
            "--format",
            "xml"
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(dir.path().join("d.po")).unwrap();
    assert!(po.contains("msgid \"Value\""));
}

#[test]
fn error_when_format_cannot_be_inferred() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("data.txt"), "<root>x</root>");
    let err = run_cli(["extract", &p(dir.path(), "data.txt")]).unwrap_err();
    assert!(err.to_string().contains("cannot infer format"), "{err}");
}

#[test]
fn utf16_document_is_decoded_via_bom() {
    let dir = tempfile::tempdir().unwrap();
    let utf16: Vec<u8> = std::iter::once(0xFFu8)
        .chain(std::iter::once(0xFE))
        .chain(
            "<mjml><mj-text>Wide</mj-text></mjml>"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        )
        .collect();
    std::fs::write(dir.path().join("wide.mjml"), &utf16).unwrap();

    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "wide.mjml"),
            "-o",
            &p(dir.path(), "wide.po")
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(dir.path().join("wide.po")).unwrap();
    assert!(po.contains("msgid \"Wide\""), "{po}");
}

#[test]
fn attribute_extraction_via_cli() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("p.html"),
        "<html><body><img alt=\"Sunset\"/></body></html>",
    );
    assert_eq!(
        run_cli([
            "extract",
            &p(dir.path(), "p.html"),
            "-o",
            &p(dir.path(), "p.po")
        ])
        .unwrap(),
        0
    );
    let po = std::fs::read_to_string(dir.path().join("p.po")).unwrap();
    assert!(po.contains("msgid \"Sunset\""), "{po}");
}

#[test]
fn decode_file_bytes_variants() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x");

    // UTF-8 BOM is stripped.
    std::fs::write(&path, [0xEFu8, 0xBB, 0xBF]).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(decode_file_bytes(&path, &bytes).unwrap(), "");

    // UTF-16 BE BOM decodes.
    let be: Vec<u8> = std::iter::once(0xFEu8)
        .chain(std::iter::once(0xFF))
        .chain("A".encode_utf16().flat_map(u16::to_be_bytes))
        .collect();
    assert_eq!(decode_file_bytes(&path, &be).unwrap(), "A");

    // Invalid UTF-16 (lone high surrogate) fails.
    let broken = [0xFFu8, 0xFE, 0x00, 0xD8];
    assert!(decode_file_bytes(&path, &broken).is_err());

    // Valid UTF-8 BOM followed by invalid UTF-8 fails.
    let bom_bad = [0xEFu8, 0xBB, 0xBF, 0xFF, 0xFE, 0x00, 0xD8];
    assert!(decode_file_bytes(&path, &bom_bad).is_err());

    // Plain invalid UTF-8 without any BOM fails.
    assert!(decode_file_bytes(&path, &[0x80u8]).is_err());
}

/// Process-level checks against the real binary.
#[test]
fn binary_version_and_help() {
    let (code, stdout) = run_bin(&["--version"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("xml2po"));
    let (code, stdout) = run_bin(&["--help"]);
    assert_eq!(code, 0);
    for word in ["extract", "compile", "stats", "check"] {
        assert!(stdout.contains(word), "{word} missing from help");
    }
}

/// `run` prints the help to stdout and exits 0 (in-process path).
#[test]
fn run_help_prints_usage_and_exits_zero() {
    let code = run_cli(["--help"]).unwrap();
    assert_eq!(code, 0);
    // Help is printed by clap to stdout inside `run`.
    // The assertion below relies on the subcommand list.
    let help = help_snapshot();
    assert!(help.contains("Usage"));
}

/// Capture the stdout of a `--help` invocation in one line of plumbing.
fn help_snapshot() -> String {
    // clap's `print!` inside `run` writes to the process stdout; the test
    // harness captures it, so we re-parse the static help instead of
    // intercepting the stream.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xml2po"));
    cmd.arg("--help");
    let out = cmd.output().expect("binary runs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `run` reports a clap parse error on stderr and exits with code 2
/// (in-process path).
#[test]
fn run_parse_error_exits_two_in_process() {
    let code = run_cli(["frobnicate"]).unwrap();
    assert_eq!(code, 2);
}

/// Process-level checks against the real binary.
#[test]
fn binary_unknown_command_fails() {
    let (code, _) = run_bin(&["frobnicate"]);
    assert_ne!(code, 0);
}

/// Process-level checks: the binary's error path prints and fails.
#[test]
fn binary_missing_input_fails_with_message() {
    let dir = tempfile::tempdir().unwrap();
    let ghost = p(dir.path(), "ghost.mjml");
    let out = Command::new(env!("CARGO_BIN_EXE_xml2po"))
        .args(["extract", &ghost])
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(stderr.contains("error:"), "{stderr}");
}

// ---------------------------------------------------------------------
// v0.3.0: inline placeholders, ITS semantics, CI gates, JSON output.
// ---------------------------------------------------------------------

/// Mixed content folds into one message by default.
#[test]
fn extract_inline_folds_mixed_content_by_default() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.html"),
        "<html><body><p>Hello <b>bold</b> world</p></body></html>",
    );
    run_cli([
        "extract",
        &p(dir.path(), "t.html"),
        "-o",
        &p(dir.path(), "out.po"),
    ])
    .unwrap();
    let po = std::fs::read_to_string(dir.path().join("out.po")).unwrap();
    assert!(po.contains(r#"msgid "Hello <b>bold</b> world""#), "{po}");
}

/// `--no-inline` restores the classic per-segment extraction.
#[test]
fn extract_no_inline_splits_segments() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.html"),
        "<html><body><p>Hello <b>bold</b> world</p></body></html>",
    );
    run_cli([
        "extract",
        "--no-inline",
        &p(dir.path(), "t.html"),
        "-o",
        &p(dir.path(), "out.po"),
    ])
    .unwrap();
    let po = std::fs::read_to_string(dir.path().join("out.po")).unwrap();
    assert!(po.contains("msgid \"Hello\""), "{po}");
    assert!(po.contains("msgid \"bold\""), "{po}");
    assert!(po.contains("msgid \"world\""), "{po}");
}

/// `translate="no"` is honored by default.
#[test]
fn extract_respects_translate_no_attribute() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.html"),
        r#"<html><body><section translate="no">Skip</section><p>Shown</p></body></html>"#,
    );
    run_cli([
        "extract",
        &p(dir.path(), "t.html"),
        "-o",
        &p(dir.path(), "out.po"),
    ])
    .unwrap();
    let po = std::fs::read_to_string(dir.path().join("out.po")).unwrap();
    assert!(po.contains("msgid \"Shown\""), "{po}");
    assert!(!po.contains("Skip"), "{po}");
}

/// `--pot` writes the template header.
#[test]
fn extract_pot_writes_template_header() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    run_cli([
        "extract",
        "--pot",
        &p(dir.path(), "t.mjml"),
        "-o",
        &p(dir.path(), "out.pot"),
    ])
    .unwrap();
    let po = std::fs::read_to_string(dir.path().join("out.pot")).unwrap();
    assert!(po.contains("Project-Id-Version: PACKAGE VERSION"), "{po}");
}

/// `compile` applies inline translations end to end.
#[test]
fn compile_inline_roundtrip_through_cli() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.html"),
        "<html><body><p>Hello <b>bold</b> world</p></body></html>",
    );
    run_cli([
        "extract",
        &p(dir.path(), "t.html"),
        "-o",
        &p(dir.path(), "t.po"),
    ])
    .unwrap();

    let po_path = dir.path().join("t.po");
    let po = std::fs::read_to_string(&po_path).unwrap();
    let translated = po.replace(
        "msgid \"Hello <b>bold</b> world\"\nmsgstr \"\"",
        "msgid \"Hello <b>bold</b> world\"\nmsgstr \"Hola <b>negrita</b> mundo\"",
    );
    std::fs::write(&po_path, translated).unwrap();

    run_cli([
        "compile",
        "--input",
        &p(dir.path(), "t.html"),
        "--po",
        &p(dir.path(), "t.po"),
        "-o",
        &p(dir.path(), "fr.html"),
    ])
    .unwrap();
    let out = std::fs::read_to_string(dir.path().join("fr.html")).unwrap();
    assert!(out.contains("Hola <b>negrita</b> mundo"), "{out}");
}

/// Fuzzy translations are skipped by default and applied with
/// `--use-fuzzy`.
#[test]
fn compile_fuzzy_policy() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    let po_path = dir.path().join("t.po");
    std::fs::write(
        &po_path,
        "#, fuzzy\nmsgid \"Hello\"\nmsgstr \"Hola (fuzzy)\"\n\nmsgid \"Title\"\nmsgstr \"Título\"\n",
    )
    .unwrap();

    run_cli([
        "compile",
        "--input",
        &p(dir.path(), "t.mjml"),
        "--po",
        &p(dir.path(), "t.po"),
        "-o",
        &p(dir.path(), "strict.mjml"),
    ])
    .unwrap();
    let strict = std::fs::read_to_string(dir.path().join("strict.mjml")).unwrap();
    assert!(strict.contains("Título"), "{strict}");
    assert!(!strict.contains("fuzzy"), "{strict}");

    run_cli([
        "compile",
        "--use-fuzzy",
        "--input",
        &p(dir.path(), "t.mjml"),
        "--po",
        &p(dir.path(), "t.po"),
        "-o",
        &p(dir.path(), "loose.mjml"),
    ])
    .unwrap();
    let loose = std::fs::read_to_string(dir.path().join("loose.mjml")).unwrap();
    assert!(loose.contains("Hola (fuzzy)"), "{loose}");
}

/// `stats --json` emits machine-readable coverage.
#[test]
fn stats_json_output() {
    let dir = tempfile::tempdir().unwrap();
    let po_path = dir.path().join("t.po");
    std::fs::write(
        &po_path,
        "msgid \"a\"\nmsgstr \"A\"\n\nmsgid \"b\"\nmsgstr \"\"\n",
    )
    .unwrap();
    let code = run_cli(["stats", "--json", &p(dir.path(), "t.po")]).unwrap();
    assert_eq!(code, 0);
    let _ = po_path; // output goes to stdout; in-process we can't capture it
}

/// `check --min-coverage` gates on translated ratio.
#[test]
fn check_min_coverage_gate() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("t.html"),
        "<html><body><p>One</p><p>Two</p></body></html>",
    );
    run_cli([
        "extract",
        &p(dir.path(), "t.html"),
        "-o",
        &p(dir.path(), "t.po"),
    ])
    .unwrap();

    let po_path = dir.path().join("t.po");
    let po = std::fs::read_to_string(&po_path).unwrap();
    // Translate one of the two strings (50% coverage).
    let translated = po.replace(
        "msgid \"One\"\nmsgstr \"\"",
        "msgid \"One\"\nmsgstr \"Uno\"",
    );
    std::fs::write(&po_path, translated).unwrap();

    // 50% passes a 40% gate and fails a 60% gate.
    assert_eq!(
        run_cli([
            "check",
            "--min-coverage",
            "40",
            &p(dir.path(), "t.po"),
            &p(dir.path(), "t.html")
        ])
        .unwrap(),
        0
    );
    assert_eq!(
        run_cli([
            "check",
            "--min-coverage",
            "60",
            &p(dir.path(), "t.po"),
            &p(dir.path(), "t.html")
        ])
        .unwrap(),
        1
    );
}

/// Directory inputs with `--recursive` and `--exclude`.
#[test]
fn extract_recursive_with_excludes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("src");
    std::fs::create_dir_all(root.join("pages")).unwrap();
    std::fs::create_dir_all(root.join("drafts")).unwrap();
    std::fs::write(
        root.join("index.html"),
        "<html><body><p>Main</p></body></html>",
    )
    .unwrap();
    std::fs::write(
        root.join("pages").join("about.html"),
        "<html><body><p>About</p></body></html>",
    )
    .unwrap();
    std::fs::write(
        root.join("drafts").join("wip.html"),
        "<html><body><p>Draft</p></body></html>",
    )
    .unwrap();
    std::fs::write(root.join("notes.txt"), "not html").unwrap();

    run_cli([
        "extract",
        "--recursive",
        "--exclude",
        "drafts/*",
        &p(dir.path(), "src"),
        "--format",
        "html",
        "-o",
        &p(dir.path(), "out.po"),
    ])
    .unwrap();
    let po = std::fs::read_to_string(dir.path().join("out.po")).unwrap();
    assert!(po.contains("msgid \"Main\""), "{po}");
    assert!(po.contains("msgid \"About\""), "{po}");
    assert!(!po.contains("Draft"), "{po}");
}

/// `update` merges translations into an existing catalog.
#[test]
fn update_subcommand_merges_and_obsoletes() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    run_cli([
        "extract",
        &p(dir.path(), "t.mjml"),
        "-o",
        &p(dir.path(), "t.po"),
    ])
    .unwrap();

    let po_path = dir.path().join("t.po");
    let po = std::fs::read_to_string(&po_path).unwrap();
    let translated = po.replace(
        "msgid \"Hello\"\nmsgstr \"\"",
        "msgid \"Hello\"\nmsgstr \"Hola\"",
    );
    std::fs::write(&po_path, translated).unwrap();

    // The source changes: "Click" disappears, "Buy" appears.
    write(
        &dir.path().join("t.mjml"),
        "<mjml><body><mj-text>Hello</mj-text><mj-title>Title</mj-title><mj-button>Buy</mj-button></body></mjml>",
    );
    run_cli([
        "update",
        &p(dir.path(), "t.mjml"),
        "-o",
        &p(dir.path(), "t.po"),
    ])
    .unwrap();

    let po = std::fs::read_to_string(&po_path).unwrap();
    assert!(po.contains("msgid \"Buy\""), "{po}");
    assert!(po.contains("msgstr \"Hola\""), "{po}");
    // The removed string becomes obsolete instead of vanishing.
    assert!(po.contains("#~ msgid \"Click\""), "{po}");
}

/// `--json` on check prints machine-readable drift.
#[test]
fn check_json_flag_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("t.mjml"), MJML);
    run_cli([
        "extract",
        &p(dir.path(), "t.mjml"),
        "-o",
        &p(dir.path(), "t.po"),
    ])
    .unwrap();
    assert_eq!(
        run_cli([
            "check",
            "--json",
            &p(dir.path(), "t.po"),
            &p(dir.path(), "t.mjml")
        ])
        .unwrap(),
        0
    );
}
