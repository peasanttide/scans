//! Migration tests, on fixtures that mirror the shapes of the real archive.
//!
//! Every fixture is written into a tempdir. The real repository is never migrated here: the
//! next phase does that, and a test that moved 3.5 GB of LFS content would not be a test.
//!
//! The three properties under test, in order of importance:
//!
//! 1. **Losslessness.** Every legacy key gets a disposition, and a key with no rule is an
//!    error rather than a silent drop.
//! 2. **Dry run writes nothing.**
//! 3. **The migrated tree loads clean**, which is the only end-to-end proof that the emitted
//!    TOML says what the migration meant.

use std::path::{Path, PathBuf};

use scans::load::load_archive;
use scans::migrate::{Action, Disposition, Options, Plan, apply, build_plan, plan};
use scans::model::{Layer, Record};

// ---------------------------------------------------------------------------------------
// Fixture helper
// ---------------------------------------------------------------------------------------

struct Fixture {
    /// Held only so the directory outlives the test; everything reads `root`.
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    /// The canonical root. `build_plan` canonicalises, and on Windows that adds a `\\?\`
    /// prefix — so a fixture that kept the uncanonical path could not strip it back off.
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonical tempdir");
        Fixture { dir, root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn write(&self, rel: &str, contents: &str) -> &Self {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("mkdir");
        std::fs::write(&path, contents.trim_start()).expect("write");
        self
    }

    /// An empty stand-in for a binary asset. The migration only ever checks that these exist.
    fn touch(&self, rel: &str) -> &Self {
        self.write(rel, "")
    }

    fn plan(&self) -> Plan {
        build_plan(self.root()).expect("the plan builds")
    }

    fn apply(&self) -> Plan {
        let plan = self.plan();
        assert!(
            !plan.has_errors(),
            "the plan has errors:\n{}",
            diagnostics(&plan)
        );
        apply(self.root(), &plan, &Options { dry_run: false }).expect("apply");
        plan
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root().join(rel))
            .unwrap_or_else(|e| panic!("reading {rel}: {e}"))
    }

    fn exists(&self, rel: &str) -> bool {
        self.root().join(rel).exists()
    }

    fn record(&self, rel: &str) -> Record {
        toml::from_str(&self.read(rel)).unwrap_or_else(|e| panic!("{rel} is not a record: {e}"))
    }
}

fn diagnostics(plan: &Plan) -> String {
    plan.diagnostics
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn codes(plan: &Plan) -> Vec<&str> {
    plan.diagnostics.iter().map(|d| d.code).collect()
}

fn assert_no_errors(plan: &Plan) {
    assert!(
        !plan.has_errors(),
        "expected no error diagnostics, got:\n{}",
        diagnostics(plan)
    );
}

/// Every file the plan would write, keyed by its repo-relative path.
fn writes(plan: &Plan, root: &Path) -> Vec<(String, String)> {
    plan.actions
        .iter()
        .filter_map(|a| match a {
            Action::Write { path, contents } => Some((rel(root, path), contents.clone())),
            _ => None,
        })
        .collect()
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn moves(plan: &Plan, root: &Path) -> Vec<(String, String)> {
    plan.actions
        .iter()
        .filter_map(|a| match a {
            Action::Move { from, to } => Some((rel(root, from), rel(root, to))),
            _ => None,
        })
        .collect()
}

fn deletes(plan: &Plan, root: &Path) -> Vec<String> {
    plan.actions
        .iter()
        .filter_map(|a| match a {
            Action::Delete { path } => Some(rel(root, path)),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Fixtures mirroring the real archive
// ---------------------------------------------------------------------------------------

/// Journal de Paris: a source, one copy with a PDF and an insert, an issue and its supplement.
///
/// The issue and supplement do not exist in the real repository — the 516 files the spec's
/// migration section describes were never created. They are synthesised here so migration
/// steps 6 and 7 have inputs to be tested against.
fn journal_fixture() -> Fixture {
    let f = Fixture::new();
    f.write(
        "journal-de-paris/journal-de-paris.toml",
        r#"
kind = "newspaper"
title = "Journal de Paris"
language = "fr"
place = "Paris"
country = "France"
founded = "1777-01-01"
frequency = "daily"
note = "First daily newspaper published in France."
index = "https://gazetier-revolutionnaire.gazettes18e.fr/periodique/journal-de-paris-1799"
licence = "PD-old-100-expired"

# Issue number equals day of year: no. 1 = 1 January, no. 365 = 31 December.
# 1789 is not a leap year.
held = "1789"
"#,
    );
    f.write(
        "journal-de-paris/1789/journal-de-paris-1789-vol1.toml",
        r#"
kind = "volume"
of = "journal-de-paris"
title = "Journal de Paris, annee 1789, volume 1 (janvier–juin)"
date = "1789"
year = 1789
volume = 1
file = "journal-de-paris-1789-vol1.pdf"
pages = 888

google_books_id = "wjkTAAAAQAAJ"
url = "http://books.google.fr/books?id=wjkTAAAAQAAJ&printsec=frontcover&hl=fr"

holding = "Bibliothèque cantonale et universitaire, Lausanne"
shelfmark = "1094184846"
licence = "PD-old-100-expired"
attribution = "Digitised by Google Books."

# The Google-generated front matter page was removed from the PDF;
# page numbers in this tree are 1-indexed into the trimmed file.
google_frontmatter_removed = 1

[[insert]]
title = "Etat general des revenus et des depenses"
pages = "801-850"
pagination = "roman v-xxxvj"
"#,
    );
    f.touch("journal-de-paris/1789/journal-de-paris-1789-vol1.pdf");
    f.write(
        "journal-de-paris/1789/01/03/journal-de-paris-1789-01-03.toml",
        r#"
kind = "issue"
of = "journal-de-paris-1789-vol1"
no = 3
date = "1789-01-03"
source = "journal-de-paris-1789-vol1.pdf"
pages = "13-16"
licence = "PD-old-100-expired"
"#,
    );
    f.write(
        "journal-de-paris/1789/01/03/journal-de-paris-1789-01-03-supplement.toml",
        r#"
kind = "supplement"
of = "journal-de-paris-1789-vol1"
date = "1789-01-03"
supplement_to = 3
source = "journal-de-paris-1789-vol1.pdf"
pages = "17-20"
"#,
    );
    f
}

/// Turgot: a source whose copy and document layers are collapsed in, and three sheet files
/// that fold into its `[[page]]` array and are then deleted.
fn turgot_fixture() -> Fixture {
    let f = Fixture::new();
    f.write(
        "turgot/turgot-1739.toml",
        r#"
kind = "map"
title = "Plan de Turgot"
short_title = "Turgot"
author = "Louis Bretez (survey and drawing); Claude Lucas (engraving); Aubin (lettering)"
date = "1739"
place = "Paris"
sheets = 3
url = "https://www.davidrumsey.com/luna/servlet/detail/RUMSEY~8~1~287243~90059509"
licence = "PD-old-100-expired"
attribution = "Digitised by the David Rumsey Map Collection."
"#,
    );
    for (n, extra) in [(0, " (key sheet)"), (1, ""), (2, "")] {
        f.write(
            &format!("turgot/turgot_{n:02}.toml"),
            &format!(
                r#"
title = "Plan de Turgot, sheet {n:02}{extra}"
of = "turgot-1739"
sheet = {n}
width = {}
height = 16934
author = "Louis Bretez (survey and drawing); Claude Lucas (engraving); Aubin (lettering)"
date = "1739"
url = "https://www.davidrumsey.com/luna/servlet/detail/RUMSEY~8~1~28722{n}~9005950{n}"
fetch = "https://www.davidrumsey.com/rumsey/download.pl?image=/166/1005900{n}.jp2"
licence = "PD-old-100-expired"
attribution = "Digitised by the David Rumsey Map Collection."
"#,
                23964 + n
            ),
        );
        f.touch(&format!("turgot/turgot_{n:02}.jp2"));
    }
    f
}

// ---------------------------------------------------------------------------------------
// Dry run writes nothing
// ---------------------------------------------------------------------------------------

#[test]
fn a_dry_run_touches_nothing() {
    let f = journal_fixture();
    let before = tree(f.root());

    let plan = plan(f.root(), &Options { dry_run: true }).expect("plans");
    assert!(!plan.is_empty(), "there is work to do");

    assert_eq!(before, tree(f.root()), "the dry run changed the tree");
    assert!(!f.exists("source"), "the dry run created source/");
}

#[test]
fn apply_refuses_when_the_plan_has_errors() {
    let f = Fixture::new();
    f.write(
        "odd/odd.toml",
        r#"
kind = "map"
title = "Something"
mystery_field = "no rule for this"
"#,
    );
    let plan = f.plan();
    assert!(plan.has_errors());
    let err = apply(f.root(), &plan, &Options { dry_run: false })
        .expect_err("apply must refuse a plan with errors");
    assert!(
        format!("{err:#}").contains("refusing to apply"),
        "unexpected error: {err:#}"
    );
    assert!(!f.exists("source"), "nothing may be written");
}

#[test]
fn apply_refuses_to_run_as_a_dry_run() {
    let f = turgot_fixture();
    let plan = f.plan();
    assert!(apply(f.root(), &plan, &Options { dry_run: true }).is_err());
}

/// Every path under `root`, sorted. Enough to prove a dry run is inert.
fn tree(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------------------
// Losslessness: a key with no rule is an error, never a drop
// ---------------------------------------------------------------------------------------

#[test]
fn an_unknown_key_is_an_error_and_not_a_silent_drop() {
    let f = Fixture::new();
    f.write(
        "thing/thing.toml",
        r#"
kind = "map"
title = "A map"
licence = "PD-old-100-expired"
hand_measured_offset = 17
"#,
    );
    let plan = f.plan();
    assert!(codes(&plan).contains(&"M001"), "{}", diagnostics(&plan));
    assert!(
        diagnostics(&plan).contains("hand_measured_offset"),
        "the message must name the key: {}",
        diagnostics(&plan)
    );
    // And it is nowhere in the ledger, because it was never given a disposition.
    assert!(
        plan.ledger
            .fate("thing/thing.toml", "hand_measured_offset")
            .is_none()
    );
}

#[test]
fn an_unknown_key_on_a_sheet_is_an_error_too() {
    let f = turgot_fixture();
    f.write(
        "turgot/turgot_01.toml",
        r#"
title = "Plan de Turgot, sheet 01"
of = "turgot-1739"
sheet = 1
width = 1
height = 1
plate_state = "second state"
"#,
    );
    let plan = f.plan();
    assert!(codes(&plan).contains(&"M001"), "{}", diagnostics(&plan));
    assert!(diagnostics(&plan).contains("plate_state"));
}

#[test]
fn an_unknown_kind_cannot_be_classified() {
    let f = Fixture::new();
    f.write("x/x.toml", "kind = \"phonograph\"\ntitle = \"?\"\n");
    let plan = f.plan();
    assert!(codes(&plan).contains(&"M002"), "{}", diagnostics(&plan));
}

#[test]
fn an_unknown_author_string_asks_for_a_rule_rather_than_guessing() {
    let f = Fixture::new();
    f.write(
        "x/x.toml",
        r#"
kind = "map"
title = "A map"
author = "Someone Nobody Has Written A Rule For"
"#,
    );
    let plan = f.plan();
    assert!(codes(&plan).contains(&"M003"), "{}", diagnostics(&plan));
}

#[test]
fn every_legacy_key_gets_exactly_one_of_the_three_dispositions() {
    for f in [journal_fixture(), turgot_fixture()] {
        let plan = f.plan();
        assert_no_errors(&plan);

        // Walk the fixtures again and check each file's keys against the ledger.
        for entry in walkdir::WalkDir::new(f.root()).into_iter().filter_map(Result::ok) {
            if entry.path().extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let file = rel(f.root(), entry.path());
            let text = std::fs::read_to_string(entry.path()).expect("read");
            let doc: toml_edit::DocumentMut = text.parse().expect("parse");
            for (key, _) in doc.iter() {
                assert!(
                    plan.ledger.fate(&file, key).is_some(),
                    "{file}: the key {key:?} has no disposition in the ledger"
                );
            }
        }
    }
}

#[test]
fn the_ledger_names_the_target_of_every_carried_key() {
    let f = journal_fixture();
    let plan = f.plan();
    let copy = "journal-de-paris/1789/journal-de-paris-1789-vol1.toml";

    // A copy's `pages` is a page count; a document's is a range. Same key, two conversions.
    assert!(matches!(
        plan.ledger.fate(copy, "pages"),
        Some(Disposition::Carried { to }) if to == "scan.count"
    ));
    let issue = "journal-de-paris/1789/01/03/journal-de-paris-1789-01-03.toml";
    assert!(matches!(
        plan.ledger.fate(issue, "pages"),
        Some(Disposition::Carried { to }) if to == "pages = { from = 13, to = 16 }"
    ));

    // Step 6: the document's `source` is dropped because the copy's `scan.file` is inherited.
    assert!(matches!(
        plan.ledger.fate(issue, "source"),
        Some(Disposition::DroppedDerivable { .. })
    ));
    // Step 3: `licence` restating the ancestor's is dropped; `attribution` differing is kept.
    assert!(matches!(
        plan.ledger.fate(issue, "licence"),
        Some(Disposition::DroppedDerivable { .. })
    ));
    assert!(matches!(
        plan.ledger.fate(copy, "attribution"),
        Some(Disposition::Carried { to }) if to == "rights.attribution"
    ));
}

#[test]
fn sheet_titles_are_formulaic_except_for_their_parenthetical() {
    let f = turgot_fixture();
    let plan = f.plan();
    assert_no_errors(&plan);

    // Sheet 00 says "(key sheet)", which is the only part that is not the formula.
    assert!(matches!(
        plan.ledger.fate("turgot/turgot_00.toml", "title"),
        Some(Disposition::Carried { to }) if to.contains("\"key sheet\"")
    ));
    // Sheets 01 and 02 say nothing the formula does not.
    for n in [1, 2] {
        assert!(
            matches!(
                plan.ledger.fate(&format!("turgot/turgot_{n:02}.toml"), "title"),
                Some(Disposition::DroppedFormulaic { .. })
            ),
            "sheet {n}'s title should be formulaic"
        );
    }
}

#[test]
fn a_sheet_title_that_breaks_the_formula_is_carried_in_full() {
    let f = turgot_fixture();
    f.write(
        "turgot/turgot_02.toml",
        r#"
title = "An entirely different title"
of = "turgot-1739"
sheet = 2
width = 1
height = 1
"#,
    );
    let plan = f.plan();
    assert!(matches!(
        plan.ledger.fate("turgot/turgot_02.toml", "title"),
        Some(Disposition::Carried { .. })
    ));
}

#[test]
fn a_derivable_claim_is_verified_and_not_assumed() {
    // `sheets` may only be dropped once that many sheets have really been folded in.
    let f = turgot_fixture();
    f.write(
        "turgot/turgot-1739.toml",
        r#"
kind = "map"
title = "Plan de Turgot"
date = "1739"
sheets = 99
licence = "PD-old-100-expired"
"#,
    );
    let plan = f.plan();
    assert!(codes(&plan).contains(&"M010"), "{}", diagnostics(&plan));

    // A sheet value that differs from its source's has nowhere to go, so it is an error
    // rather than a drop.
    let g = turgot_fixture();
    g.write(
        "turgot/turgot_01.toml",
        r#"
title = "Plan de Turgot, sheet 01"
of = "turgot-1739"
sheet = 1
width = 1
height = 1
date = "1740"
"#,
    );
    let plan = g.plan();
    assert!(codes(&plan).contains(&"M005"), "{}", diagnostics(&plan));
    assert!(diagnostics(&plan).contains("1740"));
}

#[test]
fn a_hand_measured_number_with_no_slot_survives_as_prose_and_says_so() {
    let f = journal_fixture();
    let plan = f.plan();
    assert!(codes(&plan).contains(&"M008"), "{}", diagnostics(&plan));

    let (_, contents) = writes(&plan, f.root())
        .into_iter()
        .find(|(p, _)| p.ends_with("journal-de-paris-1789-vol1.toml"))
        .expect("the copy is written");
    assert!(
        contents.contains("front-matter"),
        "google_frontmatter_removed must survive in scan.note:\n{contents}"
    );
}

#[test]
fn an_orphan_comment_is_carried_into_note_and_a_superseded_one_is_not() {
    let f = journal_fixture();
    let plan = f.plan();
    assert_no_errors(&plan);

    let source: Record = toml::from_str(
        &writes(&plan, f.root())
            .into_iter()
            .find(|(p, _)| p.ends_with("journal-de-paris.toml"))
            .expect("the source is written")
            .1,
    )
    .expect("parses");
    let note = source.note().expect("a note");
    assert!(
        note.contains("First daily newspaper"),
        "the original note survives: {note}"
    );
    assert!(
        note.contains("Issue number equals day of year"),
        "the day-of-year rule was only ever a comment and must survive: {note}"
    );

    // The front-matter comment restates what the generated scan.note already says, so it is
    // dropped rather than duplicated.
    let copy: Record = toml::from_str(
        &writes(&plan, f.root())
            .into_iter()
            .find(|(p, _)| p.ends_with("journal-de-paris-1789-vol1.toml"))
            .expect("the copy is written")
            .1,
    )
    .expect("parses");
    assert!(
        copy.note().is_none(),
        "the copy's only comment is superseded, so it should have no note: {:?}",
        copy.note()
    );
}

// ---------------------------------------------------------------------------------------
// The transformation itself
// ---------------------------------------------------------------------------------------

#[test]
fn everything_moves_under_source_and_keeps_its_relative_place() {
    let f = journal_fixture();
    let plan = f.plan();
    assert_no_errors(&plan);
    for (from, to) in moves(&plan, f.root()) {
        assert_eq!(to, format!("source/{from}"), "{from} moved somewhere odd");
    }
    // The PDF moves with its copy.
    assert!(moves(&plan, f.root()).iter().any(|(from, _)| from
        == "journal-de-paris/1789/journal-de-paris-1789-vol1.pdf"));
}

#[test]
fn the_journal_migrates_to_the_shape_the_spec_prints() {
    let f = journal_fixture();
    let plan = f.apply();
    assert_no_errors(&plan);

    let Record::Source(source) = f.record("source/journal-de-paris/journal-de-paris.toml") else {
        panic!("expected a source")
    };
    assert_eq!(source.id.as_deref(), Some("journal-de-paris"));
    assert_eq!(source.r#type.as_deref(), Some("newspaper"));
    assert_eq!(source.language.as_deref(), Some("fr"));
    assert_eq!(source.founded.as_deref(), Some("1777-01-01"));
    // `held = "1789"` becomes the interval it already denoted, because check 6 compares
    // `covers` as an interval.
    assert_eq!(source.covers.as_deref(), Some("1789-01-01/1789-12-31"));
    assert_eq!(
        source.rights.as_ref().and_then(|r| r.work.as_deref()),
        Some("PD-old-100-expired")
    );
    // `index` retires into a [[link]].
    assert_eq!(source.link.len(), 1);
    assert_eq!(source.link[0].rel, "index");

    let Record::Copy(copy) = f.record("source/journal-de-paris/1789/journal-de-paris-1789-vol1.toml")
    else {
        panic!("expected a copy")
    };
    assert_eq!(copy.r#type.as_deref(), Some("volume"));
    let scan = copy.scan.as_ref().expect("a scan");
    assert_eq!(scan.file.as_deref(), Some("journal-de-paris-1789-vol1.pdf"));
    assert_eq!(scan.count, Some(888));
    assert_eq!(scan.by.as_deref(), Some("Google Books"));
    assert_eq!(
        copy.holding.as_ref().and_then(|h| h.shelfmark.as_deref()),
        Some("1094184846"),
        "a shelfmark is always a string"
    );
    assert_eq!(
        copy.identifier
            .as_ref()
            .and_then(|i| i.get("google_books"))
            .map(String::as_str),
        Some("wjkTAAAAQAAJ")
    );
    // The copy restates the source's licence, so it does not repeat rights.work.
    assert_eq!(copy.rights.as_ref().and_then(|r| r.work.as_deref()), None);
    assert_eq!(
        copy.rights.as_ref().and_then(|r| r.attribution.as_deref()),
        Some("Digitised by Google Books.")
    );
    assert_eq!(copy.covers.as_deref(), Some("1789-01-01/1789-06-30"));

    let Record::Document(issue) =
        f.record("source/journal-de-paris/1789/01/03/journal-de-paris-1789-01-03.toml")
    else {
        panic!("expected a document")
    };
    assert_eq!(issue.r#type.as_deref(), Some("issue"));
    assert_eq!(issue.no, Some(3));
    assert_eq!(issue.date.as_deref(), Some("1789-01-03"));
    let pages = issue.pages.expect("a page range");
    assert_eq!((pages.from, pages.to), (13, 16));
    assert!(issue.scan.is_none(), "scan.file is inherited, not restated");
    assert!(issue.rights.is_none(), "the licence is inherited");
}

#[test]
fn a_supplement_points_at_an_id_rather_than_a_bare_integer() {
    let f = journal_fixture();
    let plan = f.apply();
    assert_no_errors(&plan);

    let Record::Document(supp) = f.record(
        "source/journal-de-paris/1789/01/03/journal-de-paris-1789-01-03-supplement.toml",
    ) else {
        panic!("expected a document")
    };
    assert_eq!(supp.r#type.as_deref(), Some("supplement"));
    assert_eq!(
        supp.supplement_to.as_deref(),
        Some("journal-de-paris-1789-01-03"),
        "the legacy `supplement_to = 3` must resolve to the sibling's id"
    );
    let pages = supp.pages.expect("a page range");
    assert_eq!((pages.from, pages.to), (17, 20));
}

#[test]
fn a_supplement_pointing_at_nothing_is_an_error() {
    let f = journal_fixture();
    f.write(
        "journal-de-paris/1789/01/03/journal-de-paris-1789-01-03-supplement.toml",
        r#"
kind = "supplement"
of = "journal-de-paris-1789-vol1"
date = "1789-01-03"
supplement_to = 999
pages = "17-20"
"#,
    );
    let plan = f.plan();
    assert!(codes(&plan).contains(&"M006"), "{}", diagnostics(&plan));
}

#[test]
fn an_insert_becomes_a_document_of_its_own_so_check_4_can_see_it() {
    let f = journal_fixture();
    let plan = f.apply();
    assert_no_errors(&plan);

    let Record::Document(insert) = f.record(
        "source/journal-de-paris/1789/journal-de-paris-1789-vol1-insert-1.toml",
    ) else {
        panic!("expected a document")
    };
    assert_eq!(insert.r#type.as_deref(), Some("insert"));
    assert_eq!(insert.of.as_deref(), Some("journal-de-paris-1789-vol1"));
    let pages = insert.pages.expect("a page range");
    assert_eq!((pages.from, pages.to), (801, 850));
    // `pagination` describes a run shorter than the page run, so it cannot become page.label.
    assert!(
        insert.note.as_deref().is_some_and(|n| n.contains("roman v-xxxvj")),
        "the hand-researched pagination must survive: {:?}",
        insert.note
    );
}

#[test]
fn the_sheets_fold_into_one_page_array_and_the_files_are_deleted() {
    let f = turgot_fixture();
    let plan = f.apply();
    assert_no_errors(&plan);

    // The three sheet records are gone; the three images moved and were not renamed.
    for n in 0..3 {
        assert!(!f.exists(&format!("turgot/turgot_{n:02}.toml")));
        assert!(f.exists(&format!("source/turgot/turgot_{n:02}.jp2")));
    }
    assert_eq!(deletes(&plan, f.root()).len(), 3);

    let Record::Source(atlas) = f.record("source/turgot/turgot-1739.toml") else {
        panic!("expected a source")
    };
    assert_eq!(atlas.page.len(), 3);
    // Ordered by n, and Turgot numbers from zero.
    assert_eq!(
        atlas.page.iter().map(|p| p.n).collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2)]
    );
    assert_eq!(atlas.page[0].title.as_deref(), Some("key sheet"));
    assert_eq!(atlas.page[1].title, None, "a formulaic title is dropped");

    // Each sheet's own landing page survives: they are all distinct and none is derivable.
    let urls: Vec<&str> = atlas
        .page
        .iter()
        .map(|p| p.url.as_deref().expect("a page url"))
        .collect();
    assert_eq!(urls.len(), 3);
    assert!(urls[0] != urls[1] && urls[1] != urls[2]);

    let graphic = &atlas.page[0].graphic[0];
    assert_eq!(graphic.file.as_deref(), Some("turgot_00.jp2"));
    assert_eq!(graphic.width, Some(23964));
    assert_eq!(graphic.height, Some(16934));
    assert!(
        graphic.url.as_deref().is_some_and(|u| u.contains("download.pl")),
        "the old `fetch` becomes graphic.url"
    );

    // The author string became parseable records, stated once.
    let resp = atlas.resp.as_ref().expect("resp");
    assert_eq!(resp.len(), 3);
    assert_eq!(resp[0].name, "Louis Bretez");
    assert_eq!(resp[0].roles(), vec!["surveyor", "draughtsman"]);
}

#[test]
fn a_sheet_with_no_image_beside_it_is_an_error() {
    let f = turgot_fixture();
    std::fs::remove_file(f.root().join("turgot/turgot_01.jp2")).expect("remove");
    let plan = f.plan();
    assert!(codes(&plan).contains(&"M007"), "{}", diagnostics(&plan));
}

// ---------------------------------------------------------------------------------------
// The migrated tree loads clean — the only end-to-end proof the emitted TOML is right
// ---------------------------------------------------------------------------------------

#[test]
fn the_migrated_journal_loads_with_no_findings_at_all() {
    let f = journal_fixture();
    f.apply();
    let archive = load_archive(f.root()).expect("loads");
    let found: Vec<String> = archive
        .diagnostics
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    assert!(found.is_empty(), "expected a clean load, got:\n{}", found.join("\n"));

    // Inheritance now does the work the legacy files did by hand.
    let issue = archive
        .by_id("journal-de-paris-1789-01-03")
        .expect("the issue");
    assert_eq!(issue.layer(), Layer::Document);
    assert_eq!(
        issue.resolved.rights.work.as_ref().map(|p| p.value.as_str()),
        Some("PD-old-100-expired"),
        "rights.work is inherited from the source two layers up"
    );
    assert_eq!(
        issue
            .resolved
            .scan_file
            .as_ref()
            .map(|p| p.value.as_str()),
        Some("journal-de-paris-1789-vol1.pdf")
    );
    // `scan.file` resolves against the directory of the copy that declared it, not the
    // issue's own directory three levels down.
    assert_eq!(
        issue.resolved.scan_file_path(),
        Some(
            std::fs::canonicalize(f.root())
                .expect("canonical")
                .join("source/journal-de-paris/1789/journal-de-paris-1789-vol1.pdf")
        )
    );
    // The terse range expanded by counting.
    assert_eq!(issue.pages.len(), 4);
    assert_eq!(issue.pages[0].n, 1);
    assert_eq!(issue.pages[0].graphics[0].page, Some(13));
    assert_eq!(issue.pages[3].graphics[0].page, Some(16));
}

#[test]
fn the_migrated_atlas_loads_and_addresses_from_zero() {
    let f = turgot_fixture();
    f.apply();
    let archive = load_archive(f.root()).expect("loads");
    let found: Vec<String> = archive
        .diagnostics
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    assert!(found.is_empty(), "expected a clean load, got:\n{}", found.join("\n"));

    let atlas = archive.by_id("turgot-1739").expect("the atlas");
    assert_eq!(atlas.pages.len(), 3);
    assert_eq!(atlas.pages[0].address(), "turgot-1739.p0");
    // A page never inherits, and a standalone image has no page index inside a container.
    assert_eq!(atlas.pages[0].graphics[0].page, None);
    assert!(archive.resolve_reference("turgot-1739.p0").is_ok());
    assert!(archive.resolve_reference("turgot-1739.p3").is_err());
}

/// Git does not track directories, so an emptied legacy folder leaves no trace in
/// `git status` and would survive the migration unnoticed — right next to its replacement
/// under `source/`, which is where the next hand-written record gets filed by mistake.
#[test]
fn the_emptied_legacy_directories_are_gone_afterwards() {
    let f = journal_fixture();
    // A non-record file inside the migrated tree: it travels under `source/` with everything
    // else, so its old directory is emptied too and must go.
    f.write("journal-de-paris/1789/02/notes.md", "not a record\n");
    // A top-level directory holding no records at all. The migration has no business here.
    f.write("docs/reading.md", "unrelated\n");
    f.apply();

    assert!(!f.exists("journal-de-paris/1789/01/03"), "the day is emptied");
    assert!(!f.exists("journal-de-paris/1789/01"), "the month is emptied");
    assert!(!f.exists("journal-de-paris/1789/02"), "and so is the other");
    assert!(
        !f.exists("journal-de-paris"),
        "nothing is left of the legacy tree"
    );

    // Emptied, never lost: every file is on the other side.
    assert!(f.exists("source/journal-de-paris/1789/02/notes.md"));
    assert!(f.exists("source/journal-de-paris/1789/01/03/journal-de-paris-1789-01-03.toml"));

    // A directory the migration never touched is none of its business.
    assert!(f.exists("docs/reading.md"), "an unrelated tree is untouched");
}

#[test]
fn a_second_run_over_a_migrated_tree_has_nothing_left_to_do() {
    let f = turgot_fixture();
    f.apply();
    let again = f.plan();
    assert!(
        again.is_empty(),
        "the migration is not idempotent; it still wants to:\n{:?}",
        again.actions
    );
}

// ---------------------------------------------------------------------------------------
// Emitted formatting — this archive is maintained by hand, so the shape of the file matters
// ---------------------------------------------------------------------------------------

#[test]
fn the_emitted_toml_is_aligned_ordered_and_sectioned() {
    let f = turgot_fixture();
    let plan = f.plan();
    let (_, atlas) = writes(&plan, f.root())
        .into_iter()
        .find(|(p, _)| p.ends_with("turgot-1739.toml"))
        .expect("the atlas is written");

    // The schema directive is line 1 — it is the only thing that gets the record validated
    // in an editor — and then `id` and `layer`, so the discriminator is visible without
    // scrolling.
    let lines: Vec<&str> = atlas.lines().collect();
    assert!(
        lines[0].starts_with("#:schema ") && lines[0].ends_with("schemas/source.json"),
        "first line was {:?}",
        lines[0]
    );
    assert!(lines[1].starts_with("id "), "second line was {:?}", lines[1]);
    assert!(
        lines[2].starts_with("layer "),
        "third line was {:?}",
        lines[2]
    );

    // The `=` of the scalar block is aligned.
    let equals: Vec<usize> = lines
        .iter()
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.find('='))
        .collect();
    assert!(equals.len() > 3);
    assert!(
        equals.windows(2).all(|w| w[0] == w[1]),
        "the scalar block is not aligned:\n{atlas}"
    );

    // Sections are separated by a blank line, and a page sits with its own graphic.
    assert!(atlas.contains("\n\n[[resp]]\n"));
    assert!(atlas.contains("\n\n[rights]\n"));
    assert!(atlas.contains("\n\n[[page]]\n"));
    assert!(
        atlas.contains("\n[[page.graphic]]\n") && !atlas.contains("\n\n[[page.graphic]]\n"),
        "a graphic belongs to the page above it, with no gap:\n{atlas}"
    );
    // A single role is a string; several are an array. Both spellings are legal and the
    // migration uses whichever the data calls for.
    assert!(atlas.contains(r#"role = ["surveyor", "draughtsman"]"#));
    assert!(atlas.contains(r#"role = "engraver""#));
    assert!(atlas.ends_with('\n'));
}

/// The directive is a *relative* path, so a wrong `../` count is silent: the editor finds no
/// schema and reports nothing, which looks exactly like a clean file. So check that the path
/// each record states actually lands on `schemas/source.json` from where that record lives.
#[test]
fn every_written_record_points_at_the_schema_from_its_own_depth() {
    for f in [journal_fixture(), turgot_fixture()] {
        let plan = f.plan();
        let written = writes(&plan, f.root());
        assert!(!written.is_empty());
        for (path, contents) in written {
            let first = contents.lines().next().expect("a first line");
            let stated = first
                .strip_prefix("#:schema ")
                .unwrap_or_else(|| panic!("{path} does not open with a #:schema line: {first:?}"));

            // Walk the stated path from the record's own directory and see where it lands.
            let mut at: Vec<&str> = path.split('/').collect();
            at.pop();
            for segment in stated.split('/') {
                if segment == ".." {
                    assert!(at.pop().is_some(), "{path}: {stated} escapes the repo root");
                } else {
                    at.push(segment);
                }
            }
            assert_eq!(
                at.join("/"),
                "schemas/source.json",
                "{path} states {stated}, which resolves to {}",
                at.join("/")
            );
        }
    }
}

#[test]
fn the_emitted_toml_is_byte_for_byte_stable() {
    let f = turgot_fixture();
    let first = writes(&f.plan(), f.root());
    let second = writes(&f.plan(), f.root());
    assert_eq!(first, second, "two plans over one tree must agree exactly");
}

#[test]
fn a_terse_page_range_is_an_inline_table() {
    let f = journal_fixture();
    let plan = f.plan();
    let (_, issue) = writes(&plan, f.root())
        .into_iter()
        .find(|(p, _)| p.ends_with("journal-de-paris-1789-01-03.toml"))
        .expect("the issue is written");
    assert!(
        issue.contains("pages = { from = 13, to = 16 }"),
        "the range must read as the spec prints it:\n{issue}"
    );
}

/// Not an assertion — a way to eyeball the emitted files. `cargo test -- --ignored --nocapture`
#[test]
#[ignore]
fn show_emitted_files() {
    for f in [journal_fixture(), turgot_fixture()] {
        for (path, contents) in writes(&f.plan(), f.root()) {
            println!("\n===== {path} =====\n{contents}");
        }
    }
}
