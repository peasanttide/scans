//! Integration tests for the ten checks.
//!
//! Every check gets a fixture that **triggers** it and a fixture that **passes** it. A check
//! with only a passing test is not tested at all — it would still pass if the check were
//! deleted.
//!
//! Fixtures are written into a tempdir rather than run against the real archive: the migrated
//! layout does not exist yet, and a test that depends on gigabytes of Git LFS content is not a
//! test.

use std::path::{Path, PathBuf};

use scans::load::{Archive, Diagnostic, Severity, load_archive};
use scans::validate::{self, Options, Report};

// ---------------------------------------------------------------------------------------
// Fixture helper
// ---------------------------------------------------------------------------------------

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Write a file under the archive root, creating parent directories.
    fn write(&self, rel: &str, contents: &str) -> &Self {
        self.write_bytes(rel, contents.as_bytes())
    }

    fn write_bytes(&self, rel: &str, contents: &[u8]) -> &Self {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("mkdir");
        std::fs::write(&path, contents).expect("write");
        self
    }

    /// An empty stand-in for a graphic or container, so `E701` stays quiet.
    fn touch(&self, rel: &str) -> &Self {
        self.write_bytes(rel, b"")
    }

    fn archive(&self) -> Archive {
        load_archive(self.root()).expect("archive loads")
    }

    fn report(&self) -> Report {
        let archive = self.archive();
        validate::validate(&archive, &Options::default())
    }

    fn report_with(&self, options: &Options) -> Report {
        let archive = self.archive();
        validate::validate(&archive, options)
    }
}

// ---------------------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------------------

fn render(report: &Report) -> String {
    report
        .findings
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[track_caller]
fn assert_has(report: &Report, code: &str) -> Diagnostic {
    let found: Vec<&Diagnostic> = report.with_code(code).collect();
    assert!(
        !found.is_empty(),
        "expected finding {code}, got:\n{}",
        render(report)
    );
    found[0].clone()
}

#[track_caller]
fn assert_lacks(report: &Report, code: &str) {
    assert!(
        !report.has_code(code),
        "expected no {code}, got:\n{}",
        render(report)
    );
}

#[track_caller]
fn assert_count(report: &Report, code: &str, expected: usize) {
    let actual = report.with_code(code).count();
    assert_eq!(
        actual,
        expected,
        "expected {expected} x {code}, got {actual}:\n{}",
        render(report)
    );
}

/// No findings at all — the bar a well-formed fixture must clear.
#[track_caller]
fn assert_clean(report: &Report) {
    assert!(
        report.findings.is_empty(),
        "expected no findings, got:\n{}",
        render(report)
    );
}

// ---------------------------------------------------------------------------------------
// Reusable fixture bodies
// ---------------------------------------------------------------------------------------

const SOURCE: &str = r#"
layer     = "source"
type      = "newspaper"
title     = "Journal de Paris"
language  = "fr"
place     = "Paris"

[rights]
work = "PD-old-100-expired"
"#;

/// A copy with a 100-page container covering the first half of 1789.
fn copy_toml(name: &str) -> String {
    format!(
        r#"
layer  = "copy"
of     = "journal-de-paris"
type   = "volume"
title  = "Journal de Paris, 1789, volume 1"
covers = "1789-01-01/1789-06-30"

[scan]
file  = "{name}.pdf"
count = 100
by    = "Google Books"
"#
    )
}

/// Source + copy + container, with the copy's PDF actually present on disk.
fn journal() -> Fixture {
    let f = Fixture::new();
    f.write("source/journal-de-paris/journal-de-paris.toml", SOURCE);
    f.write("source/journal-de-paris/1789/vol1.toml", &copy_toml("vol1"));
    f.touch("source/journal-de-paris/1789/vol1.pdf");
    f
}

/// A terse issue inside `vol1`.
fn issue(no: i64, date: &str, from: i64, to: i64) -> String {
    format!(
        r#"
layer = "document"
of    = "vol1"
type  = "issue"
no    = {no}
date  = "{date}"
pages = {{ from = {from}, to = {to} }}
"#
    )
}

/// The Turgot shape: one source, no copy layer, pages inline with standalone images.
fn atlas() -> Fixture {
    let f = Fixture::new();
    f.write(
        "source/turgot/turgot-1739.toml",
        r#"
layer = "source"
type  = "map"
title = "Plan de Paris"
date  = "1739"

[[page]]
n     = 0
title = "key sheet"
[[page.graphic]]
file   = "turgot-00.jp2"
width  = 23964
height = 16934

[[page]]
n = 1
[[page.graphic]]
file   = "turgot-01.jp2"
width  = 23926
height = 16926
"#,
    );
    f.touch("source/turgot/turgot-00.jp2");
    f.touch("source/turgot/turgot-01.jp2");
    f
}

// ---------------------------------------------------------------------------------------
// Baseline: a well-formed archive is silent
// ---------------------------------------------------------------------------------------

#[test]
fn a_well_formed_journal_is_silent() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/jdp-1789-01-03.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    assert_clean(&f.report());
}

#[test]
fn a_well_formed_atlas_is_silent() {
    assert_clean(&atlas().report());
}

#[test]
fn a_clean_archive_exits_zero() {
    let report = atlas().report();
    assert_eq!(report.exit_code(false), 0);
    assert_eq!(report.exit_code(true), 0);
}

// ---------------------------------------------------------------------------------------
// Pre-checks: W014, W015, E402, E403
// ---------------------------------------------------------------------------------------

#[test]
fn w014_triggers_on_an_unknown_text_kind() {
    let f = atlas();
    f.write(
        "source/turgot/note.toml",
        r#"
layer = "document"
of    = "turgot-1739"
title = "A note"

[[text]]
file = "note.txt"
kind = "guesswork"
"#,
    );
    let report = f.report();
    let finding = assert_has(&report, "W014");
    assert!(
        finding.message.contains("guesswork") && finding.message.contains("ocr"),
        "message must name the bad value and the vocabulary: {}",
        finding.message
    );
}

#[test]
fn w014_passes_on_a_known_text_kind() {
    let f = atlas();
    f.write(
        "source/turgot/note.toml",
        r#"
layer = "document"
of    = "turgot-1739"
title = "A note"

[[text]]
file = "note.txt"
kind = "transcription"
"#,
    );
    assert_lacks(&f.report(), "W014");
}

#[test]
fn w015_triggers_on_a_duplicate_zone_id_within_one_page() {
    let f = Fixture::new();
    f.write(
        "source/engravings/serment.toml",
        r#"
layer = "document"
title = "Le Serment du Jeu de Paume"

[[page]]
n = 1
[[page.graphic]]
file   = "serment.jpg"
width  = 4000
height = 2800
[[page.zone]]
id  = "signature"
ulx = 10
uly = 10
lrx = 100
lry = 100
[[page.zone]]
id  = "signature"
ulx = 200
uly = 200
lrx = 300
lry = 300
"#,
    );
    f.touch("source/engravings/serment.jpg");
    let finding = assert_has(&f.report(), "W015");
    assert!(finding.message.contains("signature"), "{}", finding.message);
}

#[test]
fn w015_passes_when_zone_ids_are_distinct() {
    let f = Fixture::new();
    f.write(
        "source/engravings/serment.toml",
        r#"
layer = "document"
title = "Le Serment du Jeu de Paume"

[[page]]
n = 1
[[page.graphic]]
file   = "serment.jpg"
width  = 4000
height = 2800
[[page.zone]]
id  = "signature"
ulx = 10
uly = 10
lrx = 100
lry = 100
[[page.zone]]
id  = "caption"
ulx = 200
uly = 200
lrx = 300
lry = 300
"#,
    );
    f.touch("source/engravings/serment.jpg");
    assert_clean(&f.report());
}

fn zone_fixture(zone: &str) -> Fixture {
    let f = Fixture::new();
    f.write(
        "source/engravings/serment.toml",
        &format!(
            r#"
layer = "document"
title = "Le Serment du Jeu de Paume"

[[page]]
n = 1
[[page.graphic]]
file   = "serment.jpg"
width  = 4000
height = 2800
[[page.zone]]
{zone}
"#
        ),
    );
    f.touch("source/engravings/serment.jpg");
    f
}

#[test]
fn e402_triggers_on_a_rectangle_with_no_width() {
    let f = zone_fixture("ulx = 300\nuly = 10\nlrx = 100\nlry = 100");
    let finding = assert_has(&f.report(), "E402");
    assert!(
        finding.message.contains("300") && finding.message.contains("100"),
        "message must name both coordinates: {}",
        finding.message
    );
}

#[test]
fn e402_triggers_on_a_rectangle_with_no_height() {
    let f = zone_fixture("ulx = 10\nuly = 300\nlrx = 100\nlry = 100");
    assert_has(&f.report(), "E402");
}

#[test]
fn e402_triggers_on_a_negative_coordinate() {
    let f = zone_fixture("ulx = -5\nuly = 10\nlrx = 100\nlry = 100");
    let finding = assert_has(&f.report(), "E402");
    assert!(finding.message.contains("-5"), "{}", finding.message);
}

#[test]
fn e402_passes_on_a_sane_rectangle() {
    let f = zone_fixture("ulx = 10\nuly = 10\nlrx = 100\nlry = 100");
    assert_clean(&f.report());
}

#[test]
fn e403_triggers_when_a_zone_leaves_the_primary_graphic() {
    let f = zone_fixture("ulx = 10\nuly = 10\nlrx = 5000\nlry = 100");
    let finding = assert_has(&f.report(), "E403");
    assert!(
        finding.message.contains("5000") && finding.message.contains("4000"),
        "message must name the edge and the extent it left: {}",
        finding.message
    );
}

#[test]
fn e403_passes_when_a_zone_is_inside_the_primary_graphic() {
    let f = zone_fixture("ulx = 0\nuly = 0\nlrx = 4000\nlry = 2800");
    assert_clean(&f.report());
}

// ---------------------------------------------------------------------------------------
// Check 1 — identity warnings (the errors belong to `load`)
// ---------------------------------------------------------------------------------------

#[test]
fn w105_triggers_on_an_id_outside_house_style() {
    let f = Fixture::new();
    f.write(
        "source/turgot/turgot_00.toml",
        "layer = \"source\"\ntitle = \"Sheet\"\n",
    );
    let finding = assert_has(&f.report(), "W105");
    assert!(
        finding.message.contains("turgot_00") && finding.message.contains("filename"),
        "message must name the id and say where it came from: {}",
        finding.message
    );
}

#[test]
fn w105_passes_on_a_hyphenated_lowercase_id() {
    let f = Fixture::new();
    f.write(
        "source/turgot/turgot-1739.toml",
        "layer = \"source\"\ntitle = \"Atlas\"\n",
    );
    assert_lacks(&f.report(), "W105");
}

#[test]
fn w106_triggers_on_ids_that_differ_only_in_case() {
    let f = Fixture::new();
    f.write(
        "source/a/turgot.toml",
        "layer = \"source\"\ntitle = \"One\"\n",
    );
    f.write(
        "source/b/x.toml",
        "layer = \"source\"\nid = \"Turgot\"\ntitle = \"Two\"\n",
    );
    let finding = assert_has(&f.report(), "W106");
    assert!(
        finding.message.contains("Turgot") && finding.message.contains("turgot"),
        "message must name both ids: {}",
        finding.message
    );
    assert!(finding.also.is_some(), "the other file must be named");
    // One finding for the pair, not one per member.
    assert_count(&f.report(), "W106", 1);
}

#[test]
fn w106_passes_on_genuinely_distinct_ids() {
    let f = Fixture::new();
    f.write(
        "source/a/turgot.toml",
        "layer = \"source\"\ntitle = \"One\"\n",
    );
    f.write(
        "source/b/verniquet.toml",
        "layer = \"source\"\ntitle = \"Two\"\n",
    );
    assert_lacks(&f.report(), "W106");
}

#[test]
fn w107_triggers_on_a_copy_with_no_of() {
    let f = Fixture::new();
    f.write(
        "source/orphan/vol1.toml",
        "layer = \"copy\"\ntitle = \"A volume from nowhere\"\n",
    );
    let finding = assert_has(&f.report(), "W107");
    assert!(finding.message.contains("copy"), "{}", finding.message);
}

#[test]
fn w107_passes_on_a_copy_that_declares_of() {
    let f = journal();
    assert_lacks(&f.report(), "W107");
}

#[test]
fn w107_does_not_fire_on_a_source() {
    let f = atlas();
    assert_lacks(&f.report(), "W107");
}

#[test]
fn w107_does_not_fire_on_a_standalone_document() {
    // The spec's one-off engraving has no parent at all and is called "the shape most of the
    // archive will take". Warning on the majority shape would teach people to ignore the tool.
    let f = Fixture::new();
    f.write(
        "source/engravings/serment.toml",
        r#"
layer = "document"
type  = "engraving"
title = "Le Serment du Jeu de Paume"
date  = "1791"
"#,
    );
    assert_clean(&f.report());
}

// ---------------------------------------------------------------------------------------
// Check 2 — child layer at or below parent's
// ---------------------------------------------------------------------------------------

#[test]
fn e201_triggers_when_a_source_is_a_child_of_a_document() {
    let f = Fixture::new();
    f.write(
        "source/x/doc.toml",
        "layer = \"document\"\ntitle = \"A document\"\n",
    );
    f.write(
        "source/x/series.toml",
        "layer = \"source\"\nof = \"doc\"\ntitle = \"A source under a document\"\n",
    );
    let finding = assert_has(&f.report(), "E201");
    assert!(
        finding.message.contains("source")
            && finding.message.contains("document")
            && finding.message.contains("doc"),
        "message must name both layers and the parent: {}",
        finding.message
    );
    assert_eq!(finding.also.as_deref(), Some("source/x/doc.toml"));
}

#[test]
fn e201_triggers_when_a_copy_is_a_child_of_a_document() {
    let f = Fixture::new();
    f.write(
        "source/x/doc.toml",
        "layer = \"document\"\ntitle = \"A document\"\n",
    );
    f.write(
        "source/x/vol.toml",
        "layer = \"copy\"\nof = \"doc\"\ntitle = \"A volume\"\n",
    );
    assert_has(&f.report(), "E201");
}

#[test]
fn e201_passes_on_the_normal_descent() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/jdp-1789-01-03.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    assert_lacks(&f.report(), "E201");
}

#[test]
fn e201_allows_equal_ranks_because_the_rule_is_at_or_below() {
    // A supplement hanging off an issue, and a series hanging off a source.
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/jdp-1789-01-03.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    f.write(
        "source/journal-de-paris/1789/01/03/jdp-1789-01-03-supplement.toml",
        r#"
layer = "document"
of    = "jdp-1789-01-03"
type  = "supplement"
date  = "1789-01-03"
pages = { from = 17, to = 18 }
"#,
    );
    assert_lacks(&f.report(), "E201");
}

// ---------------------------------------------------------------------------------------
// Check 4 — sibling documents' page ranges do not overlap within a copy
// ---------------------------------------------------------------------------------------

#[test]
fn e401_triggers_when_two_issues_claim_the_same_container_page() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    f.write(
        "source/journal-de-paris/1789/01/04/b.toml",
        &issue(4, "1789-01-04", 15, 18),
    );
    let report = f.report();
    let finding = assert_has(&report, "E401");
    assert!(
        finding.message.contains("15") && finding.message.contains("16"),
        "message must list the shared pages: {}",
        finding.message
    );
    assert!(
        finding.message.contains("vol1.pdf"),
        "message must name the container: {}",
        finding.message
    );
    // Reported once for the pair, on the lexicographically-first path.
    assert_count(&report, "E401", 1);
    assert_eq!(finding.path, "source/journal-de-paris/1789/01/03/a.toml");
    assert_eq!(
        finding.also.as_deref(),
        Some("source/journal-de-paris/1789/01/04/b.toml")
    );
}

#[test]
fn e401_passes_on_adjacent_ranges() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    f.write(
        "source/journal-de-paris/1789/01/04/b.toml",
        &issue(4, "1789-01-04", 17, 20),
    );
    assert_clean(&f.report());
}

#[test]
fn e401_truncates_a_long_list_of_shared_pages() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-01-03", 1, 20),
    );
    f.write(
        "source/journal-de-paris/1789/01/04/b.toml",
        &issue(4, "1789-01-04", 1, 20),
    );
    let finding = assert_has(&f.report(), "E401");
    assert!(
        finding.message.contains("… and 15 more"),
        "long lists must be truncated: {}",
        finding.message
    );
}

#[test]
fn e401_is_skipped_silently_when_there_is_no_copy_layer() {
    // Turgot has no copy, so there is nothing to be a sibling within. Two documents that both
    // claim page 1 of the same standalone image must not be reported.
    let f = Fixture::new();
    f.write(
        "source/x/src.toml",
        "layer = \"source\"\ntitle = \"An atlas\"\n",
    );
    for name in ["a", "b"] {
        f.write(
            &format!("source/x/{name}.toml"),
            r#"
layer = "document"
of    = "src"
title = "A sheet"

[[page]]
n = 1
[[page.graphic]]
file   = "sheet.jp2"
page   = 1
width  = 100
height = 100
"#,
        );
    }
    f.touch("source/x/sheet.jp2");
    assert_lacks(&f.report(), "E401");
}

// ---------------------------------------------------------------------------------------
// Check 5 — every page range fits inside the copy's scan.count
// ---------------------------------------------------------------------------------------

#[test]
fn e501_triggers_when_a_range_runs_past_the_container() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/12/31/last.toml",
        &issue(365, "1789-06-30", 99, 102),
    );
    let report = f.report();
    let finding = assert_has(&report, "E501");
    assert!(
        finding.message.contains("101") && finding.message.contains("100"),
        "message must name the offending page and the count: {}",
        finding.message
    );
    assert!(
        finding.message.contains("vol1"),
        "message must name the copy: {}",
        finding.message
    );
    // Pages 101 and 102 are both past the end.
    assert_count(&report, "E501", 2);
}

#[test]
fn e501_passes_when_the_range_fits() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/06/30/last.toml",
        &issue(181, "1789-06-30", 97, 100),
    );
    assert_lacks(&f.report(), "E501");
}

#[test]
fn e502_triggers_on_a_graphic_page_below_one() {
    let f = journal();
    // Declared explicitly, in the copy's own directory so the path resolves to the same PDF.
    f.write(
        "source/journal-de-paris/1789/odd.toml",
        r#"
layer = "document"
of    = "vol1"
type  = "issue"
date  = "1789-01-03"

[[page]]
n = 1
[[page.graphic]]
file = "vol1.pdf"
page = 0
"#,
    );
    let finding = assert_has(&f.report(), "E502");
    assert!(finding.message.contains("1-based"), "{}", finding.message);
}

#[test]
fn e501_ignores_a_graphic_that_is_not_the_copys_container() {
    // A page number inside some other file is not constrained by the copy's scan.count.
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/insert.toml",
        r#"
layer = "document"
of    = "vol1"
type  = "insert"
date  = "1789-01-03"

[[page]]
n = 1
[[page.graphic]]
file   = "elsewhere.pdf"
page   = 900
width  = 10
height = 10
"#,
    );
    f.touch("source/journal-de-paris/1789/elsewhere.pdf");
    assert_lacks(&f.report(), "E501");
}

// ---------------------------------------------------------------------------------------
// Check 6 — EDTF parses, and a document's date falls inside its copy's covers
// ---------------------------------------------------------------------------------------

#[test]
fn e601_triggers_on_a_date_outside_the_supported_subset() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-21", 13, 16),
    );
    let finding = assert_has(&f.report(), "E601");
    assert!(
        finding.message.contains("date") && finding.message.contains("1789-21"),
        "message must name the field and the value: {}",
        finding.message
    );
}

#[test]
fn e601_triggers_on_a_bad_founded_even_though_nothing_compares_against_it() {
    let f = Fixture::new();
    f.write(
        "source/x/src.toml",
        "layer = \"source\"\ntitle = \"A paper\"\nfounded = \"1777-01-03T12:00\"\n",
    );
    let finding = assert_has(&f.report(), "E601");
    assert!(finding.message.contains("founded"), "{}", finding.message);
}

#[test]
fn e601_passes_on_every_form_the_subset_accepts() {
    let f = Fixture::new();
    for (index, date) in [
        "1789-01-03",
        "1789-01",
        "1739",
        "1791/1799",
        "1795~",
        "178X",
    ]
    .iter()
    .enumerate()
    {
        f.write(
            &format!("source/x/d{index}.toml"),
            &format!("layer = \"document\"\ntitle = \"A thing\"\ndate = \"{date}\"\n"),
        );
    }
    assert_lacks(&f.report(), "E601");
}

#[test]
fn e604_triggers_when_covers_is_a_bare_date() {
    let f = Fixture::new();
    f.write(
        "source/x/src.toml",
        "layer = \"source\"\ntitle = \"A paper\"\n",
    );
    f.write(
        "source/x/vol1.toml",
        "layer = \"copy\"\nof = \"src\"\ntitle = \"A volume\"\ncovers = \"1789\"\n",
    );
    let finding = assert_has(&f.report(), "E604");
    assert!(
        finding.message.contains('/'),
        "message must show the interval form: {}",
        finding.message
    );
}

#[test]
fn e604_passes_on_a_real_interval() {
    assert_lacks(&journal().report(), "E604");
}

#[test]
fn e605_triggers_on_a_backwards_interval() {
    let f = Fixture::new();
    f.write(
        "source/x/d.toml",
        "layer = \"document\"\ntitle = \"A thing\"\ndate = \"1799/1789\"\n",
    );
    let finding = assert_has(&f.report(), "E605");
    assert!(finding.message.contains("1799/1789"), "{}", finding.message);
}

#[test]
fn e605_passes_on_a_forwards_interval() {
    let f = Fixture::new();
    f.write(
        "source/x/d.toml",
        "layer = \"document\"\ntitle = \"A thing\"\ndate = \"1789/1799\"\n",
    );
    assert_lacks(&f.report(), "E605");
}

#[test]
fn e602_triggers_on_an_issue_filed_under_the_wrong_volume() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/08/01/a.toml",
        &issue(213, "1789-08-01", 13, 16),
    );
    let finding = assert_has(&f.report(), "E602");
    assert!(
        finding.message.contains("1789-08-01")
            && finding.message.contains("1789-01-01/1789-06-30")
            && finding.message.contains("vol1"),
        "message must name the date, the covers and the copy: {}",
        finding.message
    );
    assert_eq!(finding.severity, Severity::Error);
}

#[test]
fn e602_passes_on_an_issue_inside_its_volume() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    assert_lacks(&f.report(), "E602");
}

#[test]
fn w603_degrades_an_imprecise_date_to_a_warning() {
    // "1789" is not contained in 1789-01-01/1789-06-30, but it is not misfiled either.
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789", 13, 16),
    );
    let report = f.report();
    let finding = assert_has(&report, "W603");
    assert_eq!(finding.severity, Severity::Warning);
    assert_lacks(&report, "E602");
    assert_eq!(
        report.exit_code(false),
        0,
        "a warning must not fail the run"
    );
    assert_eq!(report.exit_code(true), 1, "--strict must fail it");
}

#[test]
fn check_6_is_skipped_silently_without_a_copy_layer() {
    // The atlas dates itself 1739 and has no copy to be compared against.
    let report = atlas().report();
    assert_lacks(&report, "E602");
    assert_lacks(&report, "W603");
}

// ---------------------------------------------------------------------------------------
// Check 7 — files on disk
// ---------------------------------------------------------------------------------------

#[test]
fn e701_triggers_on_a_graphic_that_is_not_there() {
    let f = atlas();
    std::fs::remove_file(f.root().join("source/turgot/turgot-01.jp2")).expect("remove");
    let finding = assert_has(&f.report(), "E701");
    assert!(
        finding.message.contains("turgot-01.jp2"),
        "message must name the file: {}",
        finding.message
    );
    assert_eq!(finding.path, "source/turgot/turgot-1739.toml");
}

#[test]
fn e701_passes_when_every_graphic_is_present() {
    assert_lacks(&atlas().report(), "E701");
}

#[test]
fn e701_reports_a_missing_container_once_on_the_file_that_declared_it() {
    // Two issues inherit one `scan.file`. A missing PDF is one edit, so it is one finding —
    // and it belongs on the copy, not on the issues that merely inherited the value.
    let f = Fixture::new();
    f.write("source/journal-de-paris/journal-de-paris.toml", SOURCE);
    f.write("source/journal-de-paris/1789/vol1.toml", &copy_toml("vol1"));
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    f.write(
        "source/journal-de-paris/1789/01/04/b.toml",
        &issue(4, "1789-01-04", 17, 20),
    );
    let report = f.report();
    assert_count(&report, "E701", 1);
    let finding = assert_has(&report, "E701");
    assert_eq!(finding.path, "source/journal-de-paris/1789/vol1.toml");
}

#[test]
fn w703_triggers_on_a_declared_graphic_with_no_dimensions() {
    let f = Fixture::new();
    f.write(
        "source/engravings/serment.toml",
        r#"
layer = "document"
title = "Le Serment du Jeu de Paume"

[[page]]
n = 1
[[page.graphic]]
file = "serment.jpg"
"#,
    );
    f.touch("source/engravings/serment.jpg");
    let finding = assert_has(&f.report(), "W703");
    assert!(
        finding.message.contains("width or height"),
        "{}",
        finding.message
    );
}

#[test]
fn w703_passes_when_dimensions_are_declared() {
    assert_lacks(&atlas().report(), "W703");
}

#[test]
fn w703_stays_quiet_about_graphics_synthesised_from_a_range() {
    // A terse issue indexes into a PDF; it could not state pixel dimensions even in principle.
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    assert_lacks(&f.report(), "W703");
}

// ---------------------------------------------------------------------------------------
// Check 8 — cross-references resolve
// ---------------------------------------------------------------------------------------

fn supplement(target: &str) -> String {
    format!(
        r#"
layer = "document"
of    = "vol1"
type  = "supplement"
date  = "1789-01-03"
supplement_to = "{target}"
pages = {{ from = 17, to = 18 }}
"#
    )
}

#[test]
fn e801_triggers_on_a_supplement_to_that_names_nothing() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    f.write(
        "source/journal-de-paris/1789/01/03/s.toml",
        &supplement("jdp-1789-01-99"),
    );
    let finding = assert_has(&f.report(), "E801");
    assert!(
        finding.message.contains("supplement_to") && finding.message.contains("jdp-1789-01-99"),
        "message must name the field and the dangling value: {}",
        finding.message
    );
}

#[test]
fn e802_triggers_when_supplement_to_names_a_copy() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/s.toml",
        &supplement("vol1"),
    );
    let finding = assert_has(&f.report(), "E802");
    assert!(
        finding.message.contains("copy") && finding.message.contains("document"),
        "message must name the layer found and the layer required: {}",
        finding.message
    );
    assert_eq!(
        finding.also.as_deref(),
        Some("source/journal-de-paris/1789/vol1.toml")
    );
}

#[test]
fn check_8_passes_when_a_supplement_names_its_issue() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    f.write(
        "source/journal-de-paris/1789/01/03/s.toml",
        &supplement("a"),
    );
    let report = f.report();
    assert_lacks(&report, "E801");
    assert_lacks(&report, "E802");
    assert_lacks(&report, "W803");
}

#[test]
fn w803_triggers_when_a_supplement_points_into_another_copy() {
    let f = journal();
    let vol2 = copy_toml("vol2").replace("1789-01-01/1789-06-30", "1789-07-01/1789-12-31");
    f.write("source/journal-de-paris/1789/vol2.toml", &vol2);
    f.touch("source/journal-de-paris/1789/vol2.pdf");
    f.write(
        "source/journal-de-paris/1789/07/01/other.toml",
        r#"
layer = "document"
of    = "vol2"
type  = "issue"
no    = 182
date  = "1789-07-01"
pages = { from = 1, to = 4 }
"#,
    );
    f.write(
        "source/journal-de-paris/1789/01/03/s.toml",
        &supplement("other"),
    );
    let finding = assert_has(&f.report(), "W803");
    assert!(
        finding.message.contains("vol1") && finding.message.contains("vol2"),
        "message must name both copies: {}",
        finding.message
    );
    assert_eq!(finding.severity, Severity::Warning);
}

// ---------------------------------------------------------------------------------------
// Check 9 — counting expansions fit. Owned by `load`; these confirm it reaches the report
// exactly once, which is the failure mode the two-module split invites.
// ---------------------------------------------------------------------------------------

#[test]
fn e903_triggers_when_a_range_and_an_explicit_list_disagree() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        r#"
layer = "document"
of    = "vol1"
date  = "1789-01-03"
pages = { from = 13, to = 16 }

[[page]]
n = 1
[[page]]
n = 2
"#,
    );
    let report = f.report();
    let finding = assert_has(&report, "E903");
    assert!(
        finding.message.contains('4') && finding.message.contains('2'),
        "message must give both counts: {}",
        finding.message
    );
    assert_count(&report, "E903", 1);
}

#[test]
fn e301_from_load_appears_exactly_once() {
    let f = Fixture::new();
    f.write(
        "source/x/d.toml",
        r#"
layer = "document"
title = "A thing"

[[page]]
n = 1
[[page.graphic]]
file   = "a.jpg"
width  = 1
height = 1
[[page]]
n = 1
[[page.graphic]]
file   = "b.jpg"
width  = 1
height = 1
"#,
    );
    f.touch("source/x/a.jpg");
    f.touch("source/x/b.jpg");
    assert_count(&f.report(), "E301", 1);
}

#[test]
fn e903_passes_when_the_counts_agree() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-01-03", 13, 16),
    );
    assert_lacks(&f.report(), "E903");
}

// ---------------------------------------------------------------------------------------
// Check 10 — gap report
// ---------------------------------------------------------------------------------------

/// A copy holding issues numbered `numbers`, each on its own four pages.
fn serial(numbers: &[i64]) -> Fixture {
    let f = journal();
    for (index, no) in numbers.iter().enumerate() {
        let from = 1 + index as i64 * 4;
        f.write(
            &format!("source/journal-de-paris/1789/i{no:03}.toml"),
            &issue(*no, "1789-01-03", from, from + 3),
        );
    }
    f
}

#[test]
fn w1001_triggers_on_a_hole_in_the_run() {
    let f = serial(&[1, 2, 4, 5]);
    let report = f.report();
    let finding = assert_has(&report, "W1001");
    assert!(
        finding.message.contains("no = 3") && finding.message.contains("1..5"),
        "message must name the missing number and the observed run: {}",
        finding.message
    );
    assert_eq!(finding.severity, Severity::Warning);
    // Reported against the copy, which is the file that anchors the group.
    assert_eq!(finding.path, "source/journal-de-paris/1789/vol1.toml");
}

#[test]
fn w1001_collapses_a_consecutive_run_of_missing_numbers() {
    let f = serial(&[1, 5, 6]);
    let report = f.report();
    assert_count(&report, "W1001", 1);
    let finding = assert_has(&report, "W1001");
    assert!(
        finding.message.contains("no = 2..4"),
        "consecutive gaps must collapse: {}",
        finding.message
    );
}

#[test]
fn w1001_reports_each_separate_gap() {
    let f = serial(&[1, 3, 5]);
    assert_count(&f.report(), "W1001", 2);
}

#[test]
fn w1001_passes_on_a_complete_run() {
    let f = serial(&[1, 2, 3, 4]);
    assert_lacks(&f.report(), "W1001");
}

#[test]
fn w1001_says_nothing_about_a_group_too_small_to_have_a_run() {
    // Two issues cannot evidence a run, so 1 and 4 is not three missing issues.
    let f = serial(&[1, 4]);
    assert_lacks(&f.report(), "W1001");
}

#[test]
fn w1001_never_fails_the_build_on_its_own() {
    let report = serial(&[1, 2, 4, 5]).report();
    assert_eq!(report.errors(), 0);
    assert_eq!(report.exit_code(false), 0);
}

// ---------------------------------------------------------------------------------------
// Reporting: ordering, selection, exit codes
// ---------------------------------------------------------------------------------------

#[test]
fn findings_are_sorted_by_path_then_code() {
    let f = Fixture::new();
    f.write("source/b/vol.toml", "layer = \"copy\"\ntitle = \"B\"\n");
    f.write(
        "source/a/vol.toml",
        "layer = \"copy\"\nid = \"a\"\ntitle = \"A\"\n",
    );
    let report = f.report();
    let keys: Vec<(String, &str)> = report
        .findings
        .iter()
        .map(|f| (f.path.clone(), f.code))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "findings must come out in a fixed order");
}

#[test]
fn select_narrows_reporting_without_changing_the_verdict() {
    let f = journal();
    // Two problems, in two different directories.
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-21", 13, 16),
    );
    f.write(
        "source/journal-de-paris/1789/01/04/b.toml",
        &issue(4, "1789-31", 17, 20),
    );
    assert_count(&f.report(), "E601", 2);

    let narrowed = f.report_with(&Options {
        select: vec![PathBuf::from("source/journal-de-paris/1789/01/03")],
        ..Options::default()
    });
    assert_count(&narrowed, "E601", 1);
    assert_eq!(
        narrowed.findings[0].path,
        "source/journal-de-paris/1789/01/03/a.toml"
    );
}

#[test]
fn select_accepts_a_single_file() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-21", 13, 16),
    );
    f.write(
        "source/journal-de-paris/1789/01/04/b.toml",
        &issue(4, "1789-31", 17, 20),
    );
    let narrowed = f.report_with(&Options {
        select: vec![PathBuf::from("source/journal-de-paris/1789/01/04/b.toml")],
        ..Options::default()
    });
    assert_count(&narrowed, "E601", 1);
}

#[test]
fn select_does_not_match_a_partial_directory_name() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-21", 13, 16),
    );
    let narrowed = f.report_with(&Options {
        // "source/journal-de-paris/1789/01/0" is a prefix of the string but not of the path.
        select: vec![PathBuf::from("source/journal-de-paris/1789/01/0")],
        ..Options::default()
    });
    assert_count(&narrowed, "E601", 0);
}

#[test]
fn any_error_makes_the_exit_code_nonzero() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/03/a.toml",
        &issue(3, "1789-21", 13, 16),
    );
    let report = f.report();
    assert!(report.errors() > 0);
    assert_eq!(report.exit_code(false), 1);
    assert_eq!(report.exit_code(true), 1);
}

#[test]
fn a_warning_alone_fails_only_under_strict() {
    let f = Fixture::new();
    f.write(
        "source/x/turgot_00.toml",
        "layer = \"source\"\ntitle = \"Sheet\"\n",
    );
    let report = f.report();
    assert_eq!(report.errors(), 0);
    assert!(report.warnings() > 0);
    assert_eq!(report.exit_code(false), 0);
    assert_eq!(report.exit_code(true), 1);
}

// ---------------------------------------------------------------------------------------
// Check 7, probe half. Compiled only with `--features probe`.
// ---------------------------------------------------------------------------------------

#[cfg(feature = "probe")]
mod probe {
    use super::*;

    fn probing() -> Options {
        Options {
            probe: true,
            ..Options::default()
        }
    }

    /// A PNG header. Only the IHDR is read, so the pixel data is not needed.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        out.extend_from_slice(&13u32.to_be_bytes());
        out.extend_from_slice(b"IHDR");
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&[8, 2, 0, 0, 0]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    /// A JP2 header: the signature box, then a `jp2h`/`ihdr` stating height and width.
    fn jp2(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x0C, 0x6A, 0x50, 0x20, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
        ]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x14]);
        out.extend_from_slice(b"ftypjp2 ");
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(b"jp2 ");
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x16]);
        out.extend_from_slice(b"ihdr");
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&[0x00, 0x03, 0x07, 0x00, 0x00, 0x00]);
        out
    }

    /// A structurally valid PDF with `pages` empty pages, offsets computed so the xref is real.
    fn pdf(pages: usize) -> Vec<u8> {
        let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", i + 3)).collect();
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {pages} >>",
                kids.join(" ")
            ),
        ];
        objects.extend(std::iter::repeat_n(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
            pages,
        ));

        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
        out.push_str("0000000000 65535 f \n");
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    fn sheet(file: &str, width: i64, height: i64) -> String {
        format!(
            r#"
layer = "source"
type  = "map"
title = "An atlas"

[[page]]
n = 0
[[page.graphic]]
file   = "{file}"
width  = {width}
height = {height}
"#
        )
    }

    #[test]
    fn e702_triggers_when_the_declared_size_is_not_the_real_size() {
        let f = Fixture::new();
        f.write("source/x/atlas.toml", &sheet("sheet.png", 100, 200));
        f.write_bytes("source/x/sheet.png", &png(640, 480));
        let report = f.report_with(&probing());
        let finding = assert_has(&report, "E702");
        assert!(
            finding.message.contains("640x480") && finding.message.contains("100x200"),
            "message must give both sizes: {}",
            finding.message
        );
    }

    #[test]
    fn e702_passes_when_the_declared_size_is_right() {
        let f = Fixture::new();
        f.write("source/x/atlas.toml", &sheet("sheet.png", 640, 480));
        f.write_bytes("source/x/sheet.png", &png(640, 480));
        assert_clean(&f.report_with(&probing()));
    }

    #[test]
    fn e702_reads_jpeg_2000_which_is_what_the_archive_actually_holds() {
        let f = Fixture::new();
        f.write("source/x/atlas.toml", &sheet("sheet.jp2", 23964, 16934));
        f.write_bytes("source/x/sheet.jp2", &jp2(23964, 16934));
        assert_clean(&f.report_with(&probing()));

        let g = Fixture::new();
        g.write("source/x/atlas.toml", &sheet("sheet.jp2", 1, 1));
        g.write_bytes("source/x/sheet.jp2", &jp2(23964, 16934));
        assert_has(&g.report_with(&probing()), "E702");
    }

    #[test]
    fn w705_reports_an_unfetched_lfs_pointer_rather_than_a_wrong_size() {
        let f = Fixture::new();
        f.write("source/x/atlas.toml", &sheet("sheet.jp2", 23964, 16934));
        f.write(
            "source/x/sheet.jp2",
            "version https://git-lfs.github.com/spec/v1\noid sha256:0\nsize 1\n",
        );
        let report = f.report_with(&probing());
        let finding = assert_has(&report, "W705");
        assert!(finding.message.contains("LFS"), "{}", finding.message);
        assert_lacks(&report, "E702");
        assert_lacks(&report, "E701");
        assert_eq!(
            report.errors(),
            0,
            "an unverifiable size is not a wrong one"
        );
    }

    #[test]
    fn e704_triggers_when_scan_count_disagrees_with_the_container() {
        let f = Fixture::new();
        f.write("source/journal-de-paris/journal-de-paris.toml", SOURCE);
        f.write("source/journal-de-paris/1789/vol1.toml", &copy_toml("vol1"));
        f.write_bytes("source/journal-de-paris/1789/vol1.pdf", &pdf(7));
        let finding = assert_has(&f.report_with(&probing()), "E704");
        assert!(
            finding.message.contains('7') && finding.message.contains("100"),
            "message must give both counts: {}",
            finding.message
        );
    }

    #[test]
    fn e704_passes_when_scan_count_is_right() {
        let f = Fixture::new();
        f.write("source/journal-de-paris/journal-de-paris.toml", SOURCE);
        f.write(
            "source/journal-de-paris/1789/vol1.toml",
            &copy_toml("vol1").replace("count = 100", "count = 7"),
        );
        f.write_bytes("source/journal-de-paris/1789/vol1.pdf", &pdf(7));
        assert_lacks(&f.report_with(&probing()), "E704");
    }

    #[test]
    fn e706_triggers_on_a_text_file_that_is_not_there() {
        let f = Fixture::new();
        f.write(
            "source/x/d.toml",
            r#"
layer = "document"
title = "A thing"

[[text]]
file = "d.p1.txt"
kind = "ocr"
"#,
        );
        let finding = assert_has(&f.report_with(&probing()), "E706");
        assert!(finding.message.contains("d.p1.txt"), "{}", finding.message);
    }

    #[test]
    fn e706_passes_when_the_text_file_is_there() {
        let f = Fixture::new();
        f.write(
            "source/x/d.toml",
            r#"
layer = "document"
title = "A thing"

[[text]]
file = "d.p1.txt"
kind = "ocr"
"#,
        );
        f.write("source/x/d.p1.txt", "Le Journal de Paris.\n");
        assert_clean(&f.report_with(&probing()));
    }

    /// Only meaningful on a case-insensitive filesystem: elsewhere the file simply is not
    /// there and `E701` covers it.
    #[cfg(windows)]
    #[test]
    fn e707_triggers_when_the_case_does_not_match_the_disk() {
        let f = Fixture::new();
        f.write("source/x/atlas.toml", &sheet("Sheet.PNG", 640, 480));
        f.write_bytes("source/x/sheet.png", &png(640, 480));
        let report = f.report_with(&probing());
        let finding = assert_has(&report, "E707");
        assert!(
            finding.message.contains("Sheet.PNG") && finding.message.contains("sheet.png"),
            "message must give both spellings: {}",
            finding.message
        );
    }

    #[test]
    fn e707_passes_when_the_case_matches() {
        let f = Fixture::new();
        f.write("source/x/atlas.toml", &sheet("sheet.png", 640, 480));
        f.write_bytes("source/x/sheet.png", &png(640, 480));
        assert_lacks(&f.report_with(&probing()), "E707");
    }

    #[test]
    fn nothing_is_opened_unless_probe_is_asked_for() {
        // The same fixture that trips E702 under --probe must be silent without it.
        let f = Fixture::new();
        f.write("source/x/atlas.toml", &sheet("sheet.png", 100, 200));
        f.write_bytes("source/x/sheet.png", &png(640, 480));
        assert_clean(&f.report());
    }
}
