//! `kg doc --check` over its fixtures: a guide and a documented source,
//! each block's verdict and each failure's place in the file.

use kg_base::{LineIndex, Utf8PathBuf};
use kg_config::Settings;
use kg_doc::check::{Checker, FileReport, Select, Verdict};

fn fixtures() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn passed(context: &str) -> Verdict {
    Verdict::Passed { context: context.to_owned(), mini_base: false }
}

fn check(file: &str) -> (FileReport, String) {
    let dir = fixtures();
    // No import directory, so no base library (`Parser/IO.hs:73-74`): the
    // fixtures are self-contained.
    let mut checker = Checker::new(Settings::fallback(&dir), Select::Kanagawa);
    let report = checker.check_file(&dir.join(file)).expect("the fixture reads");
    (report, std::fs::read_to_string(dir.join(file)).unwrap())
}

/// `(line, column)`, 1-based, of each failure's first diagnostic.
fn failures(report: &FileReport, text: &str) -> Vec<(u32, u32, String)> {
    let index = LineIndex::new(text);
    report
        .blocks
        .iter()
        .filter(|b| b.verdict == Verdict::Failed)
        .map(|b| {
            let first = &b.diagnostics[0];
            assert_eq!(first.range.file, report.file, "in the fixture itself: {first:?}");
            let at = index.line_col(first.range.range.start());
            (at.line + 1, at.col + 1, first.message.clone())
        })
        .collect()
}

#[test]
fn the_guide_passes_but_for_its_mistake() {
    let (report, text) = check("guide.md");
    let verdicts: Vec<(u32, Verdict)> = report.blocks.iter().map(|b| (b.line, b.verdict.clone())).collect();
    assert_eq!(
        verdicts,
        [
            (5, passed("emptyfile.k")),
            (14, passed("emptyfunction.k")),
            (28, Verdict::Skipped),
            (35, passed("in-class.k")),
            (41, passed("emptyfunction.k")),
            (47, Verdict::Failed),
        ],
        "neither the `sh` block nor the untagged one is Kanagawa"
    );
    assert_eq!(failures(&report, &text), [(50, 15, "Symbol name not in scope: `undefined_name`".to_owned())]);
    assert_eq!(report.untagged, 1);
}

#[test]
fn the_source_s_kanagawa_blocks_are_checked_where_they_stand() {
    let (report, text) = check("documented.k");
    let verdicts: Vec<(u32, Verdict)> = report.blocks.iter().map(|b| (b.line, b.verdict.clone())).collect();
    assert_eq!(verdicts, [(8, passed("emptyfile.k")), (27, Verdict::Failed)], "the untagged key is not");
    assert_eq!(failures(&report, &text), [(30, 30, "Invalid number of arguments, expected 1".to_owned())]);
}
