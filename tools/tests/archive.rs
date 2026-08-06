//! Integration tests for discovery, inheritance, page expansion and addressing.
//!
//! Every fixture is written into a tempdir. The real archive is deliberately not used: the
//! migrated layout does not exist yet, and a test that depends on 3.5 GB of LFS content is
//! not a test.

use std::path::Path;

use scans::load::{Archive, RefTarget, Reference, load_archive, parse_reference};
use scans::model::{Layer, Record};

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
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("mkdir");
        std::fs::write(&path, contents).expect("write");
        self
    }

    fn load(&self) -> Archive {
        load_archive(self.root()).expect("archive loads")
    }
}

fn codes(archive: &Archive) -> Vec<&str> {
    archive.diagnostics.iter().map(|d| d.code).collect()
}

fn has(archive: &Archive, code: &str) -> bool {
    codes(archive).contains(&code)
}

fn assert_clean(archive: &Archive) {
    let found: Vec<String> = archive
        .diagnostics
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    assert!(
        found.is_empty(),
        "expected no findings, got:\n{}",
        found.join("\n")
    );
}

/// The Journal de Paris shape: source, copy in a subdirectory, issue three directories deeper.
fn journal_fixture() -> Fixture {
    let f = Fixture::new();
    f.write(
        "source/journal-de-paris/journal-de-paris.toml",
        r#"
layer     = "source"
type      = "newspaper"
title     = "Journal de Paris"
language  = "fr"
place     = "Paris"
country   = "France"
founded   = "1777-01-01"
frequency = "daily"
covers    = "1789"

[rights]
work = "PD-old-100-expired"

[[resp]]
name = "Journal de Paris"
role = "publisher"

[[link]]
rel = "index"
url = "https://example.invalid/journal-de-paris"
"#,
    );
    f.write(
        "source/journal-de-paris/1789/journal-de-paris-1789-vol1.toml",
        r#"
layer  = "copy"
of     = "journal-de-paris"
type   = "volume"
title  = "Journal de Paris, annee 1789, volume 1 (janvier-juin)"
covers = "1789-01-01/1789-06-30"
url    = "http://books.google.invalid/books?id=wjkTAAAAQAAJ"

[scan]
file  = "journal-de-paris-1789-vol1.pdf"
count = 888
by    = "Google Books"
url   = "http://books.google.invalid/books?id=wjkTAAAAQAAJ"
note  = "Google front-matter leaf removed."

[holding]
repository = "Bibliotheque cantonale et universitaire, Lausanne"
shelfmark  = "1094184846"

[identifier]
google_books = "wjkTAAAAQAAJ"

[rights]
attribution = "Digitised by Google Books."
"#,
    );
    f.write(
        "source/journal-de-paris/1789/01/03/journal-de-paris-1789-01-03.toml",
        r#"
layer = "document"
of    = "journal-de-paris-1789-vol1"
type  = "issue"
no    = 3
date  = "1789-01-03"
pages = { from = 13, to = 16 }
"#,
    );
    f
}

/// The Turgot shape: one source with the copy and document layers collapsed in, pages inline,
/// numbered from zero.
fn turgot_fixture() -> Fixture {
    let f = Fixture::new();
    f.write(
        "source/turgot/turgot-1739.toml",
        r#"
layer       = "source"
type        = "map"
title       = "Plan de Paris"
short_title = "Plan de Turgot"
date        = "1739"
place       = "Paris"

[rights]
work        = "PD-old-100-expired"
attribution = "Digitised by the David Rumsey Map Collection."

[holding]
collection = "David Rumsey Map Collection"

[[resp]]
name = "Louis Bretez"
role = ["surveyor", "draughtsman"]
[[resp]]
name = "Claude Lucas"
role = "engraver"

[[page]]
n     = 0
title = "key sheet"
url   = "https://example.invalid/detail/287243"
[[page.graphic]]
file   = "turgot_00.jp2"
width  = 23964
height = 16934
url    = "https://example.invalid/download/10059022.jp2"

[[page]]
n = 1
[[page.graphic]]
file   = "turgot_01.jp2"
width  = 23926
height = 16926

[[page]]
[[page.graphic]]
file   = "turgot_02.jp2"
width  = 24026
height = 16942
"#,
    );
    f
}

// ---------------------------------------------------------------------------------------
// Discovery and the record enum
// ---------------------------------------------------------------------------------------

#[test]
fn loads_a_three_layer_archive() {
    let f = journal_fixture();
    let archive = f.load();
    assert_clean(&archive);
    assert_eq!(archive.nodes.len(), 3);

    let source = archive.by_id("journal-de-paris").expect("source");
    assert_eq!(source.layer(), Layer::Source);
    // The id was not declared in the file; it came from the filename stem.
    assert!(!source.id_declared);

    assert_eq!(
        archive.by_id("journal-de-paris-1789-vol1").unwrap().layer(),
        Layer::Copy
    );
    assert_eq!(
        archive
            .by_id("journal-de-paris-1789-01-03")
            .unwrap()
            .layer(),
        Layer::Document
    );
}

#[test]
fn the_layer_tag_selects_the_variant() {
    let f = Fixture::new();
    f.write("source/a/a.toml", "layer = \"source\"\ntitle = \"A\"\n");
    f.write(
        "source/a/b.toml",
        "layer = \"copy\"\ntitle = \"B\"\nof = \"a\"\n",
    );
    f.write("source/a/c.toml", "layer = \"document\"\nof = \"a\"\n");
    let archive = f.load();
    assert_clean(&archive);

    assert!(matches!(
        archive.by_id("a").unwrap().record,
        Record::Source(_)
    ));
    assert!(matches!(
        archive.by_id("b").unwrap().record,
        Record::Copy(_)
    ));
    assert!(matches!(
        archive.by_id("c").unwrap().record,
        Record::Document(_)
    ));
}

/// A document needs no title; a source and a copy do.
#[test]
fn title_is_required_only_where_the_spec_requires_it() {
    let f = Fixture::new();
    f.write("source/a/a.toml", "layer = \"source\"\n");
    let archive = f.load();
    assert!(has(&archive, "E011"), "codes were {:?}", codes(&archive));
}

// ---------------------------------------------------------------------------------------
// deny_unknown_fields
// ---------------------------------------------------------------------------------------

#[test]
fn a_typo_is_an_error_not_silence() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        "layer = \"source\"\ntitle = \"A\"\nplcae = \"Paris\"\n",
    );
    let archive = f.load();
    assert!(has(&archive, "E013"), "codes were {:?}", codes(&archive));
    let message = &archive.diagnostics[0].message;
    assert!(message.contains("plcae"), "message was {message:?}");
    // The record did not load, so nothing pretends the file was fine.
    assert_eq!(archive.nodes.len(), 0);
}

#[test]
fn typos_are_caught_inside_inline_tables_too() {
    for (fixture, needle) in [
        (
            "layer = \"source\"\ntitle = \"A\"\n[rights]\nwrok = \"PD\"\n",
            "wrok",
        ),
        (
            "layer = \"source\"\ntitle = \"A\"\n[holding]\nshelfmarc = \"1\"\n",
            "shelfmarc",
        ),
        (
            "layer = \"source\"\ntitle = \"A\"\n[scan]\ncont = 3\n",
            "cont",
        ),
        (
            "layer = \"document\"\npages = { from = 1, too = 2 }\n",
            "too",
        ),
    ] {
        let f = Fixture::new();
        f.write("source/a/a.toml", fixture);
        let archive = f.load();
        assert!(
            has(&archive, "E013") || has(&archive, "E011"),
            "{needle}: codes were {:?}",
            codes(&archive)
        );
    }
}

/// `layer = "page"` must be impossible on a file. Pages are inline only.
#[test]
fn a_page_may_not_be_a_file() {
    let f = Fixture::new();
    f.write("source/a/a.toml", "layer = \"page\"\nn = 1\n");
    let archive = f.load();
    assert!(has(&archive, "E015"), "codes were {:?}", codes(&archive));
}

#[test]
fn an_unknown_layer_is_an_error() {
    let f = Fixture::new();
    f.write("source/a/a.toml", "layer = \"witness\"\ntitle = \"A\"\n");
    let archive = f.load();
    assert!(has(&archive, "E014"), "codes were {:?}", codes(&archive));
}

/// A shelfmark with leading zeros must survive; an integer must be refused.
#[test]
fn shelfmark_is_always_a_string() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        "layer = \"copy\"\ntitle = \"A\"\n[holding]\nshelfmark = 1094184846\n",
    );
    let archive = f.load();
    assert!(has(&archive, "E012"), "codes were {:?}", codes(&archive));

    let g = Fixture::new();
    g.write(
        "source/a/a.toml",
        "layer = \"copy\"\ntitle = \"A\"\n[holding]\nshelfmark = \"0094184846\"\n",
    );
    let archive = g.load();
    assert_clean(&archive);
    assert_eq!(
        archive
            .by_id("a")
            .unwrap()
            .resolved
            .holding
            .shelfmark
            .as_deref(),
        Some(&"0094184846".to_string())
    );
}

// ---------------------------------------------------------------------------------------
// Ids and the of chain
// ---------------------------------------------------------------------------------------

#[test]
fn duplicate_ids_are_reported_against_both_files() {
    let f = Fixture::new();
    f.write("source/a/x.toml", "layer = \"source\"\ntitle = \"A\"\n");
    f.write(
        "source/b/y.toml",
        "layer = \"source\"\ntitle = \"B\"\nid = \"x\"\n",
    );
    let archive = f.load();
    let d = archive
        .diagnostics
        .iter()
        .find(|d| d.code == "E101")
        .expect("E101");
    assert!(d.also.is_some(), "the finding must name the other file");
}

#[test]
fn a_dangling_of_is_reported_once_on_the_file_that_declares_it() {
    let f = Fixture::new();
    f.write("source/a/a.toml", "layer = \"source\"\ntitle = \"A\"\n");
    f.write(
        "source/a/b.toml",
        "layer = \"copy\"\ntitle = \"B\"\nof = \"nonexistent\"\n",
    );
    f.write("source/a/c.toml", "layer = \"document\"\nof = \"b\"\n");
    let archive = f.load();
    let e102: Vec<_> = archive
        .diagnostics
        .iter()
        .filter(|d| d.code == "E102")
        .collect();
    assert_eq!(e102.len(), 1, "one broken pointer, one finding");
    assert!(e102[0].path.ends_with("b.toml"));
}

#[test]
fn a_cycle_is_caught_and_does_not_hang() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        "layer = \"source\"\ntitle = \"A\"\nof = \"b\"\n",
    );
    f.write(
        "source/a/b.toml",
        "layer = \"source\"\ntitle = \"B\"\nof = \"a\"\n",
    );
    let archive = f.load();
    assert!(has(&archive, "E103"), "codes were {:?}", codes(&archive));
}

#[test]
fn a_self_reference_is_a_cycle() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        "layer = \"source\"\ntitle = \"A\"\nof = \"a\"\n",
    );
    let archive = f.load();
    assert!(has(&archive, "E103"), "codes were {:?}", codes(&archive));
}

#[test]
fn an_id_with_a_reserved_character_is_an_error() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        "layer = \"source\"\ntitle = \"A\"\nid = \"a.b\"\n",
    );
    let archive = f.load();
    assert!(has(&archive, "E104"), "codes were {:?}", codes(&archive));
}

// ---------------------------------------------------------------------------------------
// Inheritance
// ---------------------------------------------------------------------------------------

#[test]
fn the_worked_example_resolves_exactly_as_specified() {
    let f = journal_fixture();
    let archive = f.load();
    assert_clean(&archive);

    let issue = archive.by_id("journal-de-paris-1789-01-03").expect("issue");
    let r = &issue.resolved;

    // Scalars from the source.
    assert_eq!(r.language.as_deref(), Some(&"fr".to_string()));
    assert_eq!(r.place.as_deref(), Some(&"Paris".to_string()));

    // `rights` merges key by key: `work` from the source, `attribution` from the copy.
    assert_eq!(
        r.rights.work.as_deref(),
        Some(&"PD-old-100-expired".to_string())
    );
    assert_eq!(
        r.rights.attribution.as_deref(),
        Some(&"Digitised by Google Books.".to_string())
    );
    assert_eq!(r.rights.work.as_ref().unwrap().node, "journal-de-paris");
    assert_eq!(
        r.rights.attribution.as_ref().unwrap().node,
        "journal-de-paris-1789-vol1"
    );

    // `holding` and `identifier` come from the copy.
    assert_eq!(
        r.holding.shelfmark.as_deref(),
        Some(&"1094184846".to_string())
    );
    assert_eq!(
        r.identifier.get("google_books").map(|p| p.value.as_str()),
        Some("wjkTAAAAQAAJ")
    );

    // `scan.file` inherits; `scan.count` and `scan.note` do not.
    assert_eq!(
        r.scan_file.as_deref(),
        Some(&"journal-de-paris-1789-vol1.pdf".to_string())
    );
    assert_eq!(r.scan_by.as_deref(), Some(&"Google Books".to_string()));
    assert_eq!(
        r.scan_count, None,
        "scan.count describes the container as the copy knows it and must not inherit"
    );
    assert_eq!(r.scan_note, None, "scan.note must not inherit");

    // The copy keeps its own count.
    let copy = archive.by_id("journal-de-paris-1789-vol1").unwrap();
    assert_eq!(copy.resolved.scan_count, Some(888));

    // Never-inherited fields stay absent on the issue.
    assert_eq!(issue.record.title(), None);
    assert_eq!(issue.record.url(), None);
    assert_eq!(issue.record.covers(), None);
    assert!(issue.record.link().is_empty(), "[[link]] never inherits");

    // check 6 reads `covers` from the nearest copy, not from the issue.
    let nearest = archive.nearest_copy(issue.index).expect("nearest copy");
    assert_eq!(nearest.id, "journal-de-paris-1789-vol1");
    assert_eq!(nearest.record.covers(), Some("1789-01-01/1789-06-30"));
}

/// `country` is a recognised field but is not on the inheritance allowlist. This is the
/// property that stops extension fields leaking downward.
#[test]
fn fields_outside_the_allowlist_do_not_inherit() {
    let f = journal_fixture();
    let archive = f.load();
    let issue = archive.by_id("journal-de-paris-1789-01-03").unwrap();
    assert_eq!(
        archive.by_id("journal-de-paris").unwrap().record.country(),
        Some("France")
    );
    assert_eq!(
        issue.record.country(),
        None,
        "country is not on the allowlist and must not reach the issue"
    );
}

/// A document whose `of` names the source directly, skipping the copy layer entirely.
#[test]
fn a_skipped_layer_inherits_from_the_nearest_declared_ancestor() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/standalone.toml",
        r#"
layer = "document"
of    = "journal-de-paris"
type  = "engraving"
title = "A plate bound in no volume"
date  = "1789"
"#,
    );
    let archive = f.load();
    assert_clean(&archive);

    let doc = archive.by_id("standalone").expect("standalone");
    assert_eq!(
        doc.chain.len(),
        2,
        "document then source, with no copy between"
    );

    // Source-level values still arrive.
    assert_eq!(doc.resolved.language.as_deref(), Some(&"fr".to_string()));
    assert_eq!(
        doc.resolved.rights.work.as_deref(),
        Some(&"PD-old-100-expired".to_string())
    );
    // Copy-level values do not, because no copy is in the chain.
    assert_eq!(doc.resolved.scan_file, None);
    assert_eq!(doc.resolved.holding.shelfmark, None);

    // Checks 4, 5 and 6 skip silently here.
    assert!(archive.nearest_copy(doc.index).is_none());
    assert!(archive.nearest_source(doc.index).is_some());
}

#[test]
fn resp_replaces_wholesale_and_an_empty_array_clears_it() {
    let f = journal_fixture();
    // A child that restates resp replaces the parent's list entirely.
    f.write(
        "source/journal-de-paris/1789/replaces.toml",
        r#"
layer = "document"
of    = "journal-de-paris"
[[resp]]
name = "A Correspondent"
role = "author"
"#,
    );
    // A child that writes `resp = []` clears it.
    f.write(
        "source/journal-de-paris/1789/clears.toml",
        r#"
layer = "document"
of    = "journal-de-paris"
resp  = []
"#,
    );
    let archive = f.load();
    assert_clean(&archive);

    let inherits = archive.by_id("journal-de-paris-1789-01-03").unwrap();
    assert_eq!(inherits.resolved.resp().len(), 1);
    assert_eq!(inherits.resolved.resp()[0].name, "Journal de Paris");

    let replaces = archive.by_id("replaces").unwrap();
    assert_eq!(replaces.resolved.resp().len(), 1);
    assert_eq!(replaces.resolved.resp()[0].name, "A Correspondent");

    let clears = archive.by_id("clears").unwrap();
    assert!(
        clears.resolved.resp().is_empty(),
        "resp = [] is the documented way to clear"
    );
    assert!(
        clears.resolved.resp.is_some(),
        "the empty list was declared, not merely absent"
    );
}

#[test]
fn a_role_may_be_one_string_or_a_list() {
    let f = turgot_fixture();
    let archive = f.load();
    assert_clean(&archive);
    let resp = archive.by_id("turgot-1739").unwrap().resolved.resp();
    assert_eq!(resp[0].roles(), vec!["surveyor", "draughtsman"]);
    assert_eq!(resp[1].roles(), vec!["engraver"]);
}

#[test]
fn a_child_scalar_wins_over_the_parent() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/latin.toml",
        r#"
layer    = "document"
of       = "journal-de-paris"
language = "la"
"#,
    );
    let archive = f.load();
    let doc = archive.by_id("latin").unwrap();
    assert_eq!(doc.resolved.language.as_deref(), Some(&"la".to_string()));
    assert_eq!(
        doc.resolved.language.as_ref().unwrap().node,
        "latin",
        "the child declared it, so provenance is the child"
    );
}

/// Declaring a key with an empty string stops the search. There is no "unset" sentinel.
#[test]
fn an_empty_string_is_a_declaration_not_an_absence() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/blank.toml",
        "layer = \"document\"\nof = \"journal-de-paris\"\nplace = \"\"\n",
    );
    let archive = f.load();
    assert_eq!(
        archive.by_id("blank").unwrap().resolved.place.as_deref(),
        Some(&String::new())
    );
}

// ---------------------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------------------

/// The load-bearing case. The issue lives in `1789/01/03/`, the PDF beside the copy in
/// `1789/`. If `scan.file` resolved against the inheriting file's directory instead of the
/// declaring file's, every issue would look for the PDF in the wrong place.
#[test]
fn an_inherited_file_path_resolves_against_the_declaring_files_directory() {
    let f = journal_fixture();
    let archive = f.load();
    let issue = archive.by_id("journal-de-paris-1789-01-03").unwrap();

    let expected = archive
        .root
        .join("source/journal-de-paris/1789/journal-de-paris-1789-vol1.pdf");
    assert_eq!(
        issue.resolved.scan_file_path().as_deref(),
        Some(expected.as_path())
    );

    for page in &issue.pages {
        assert_eq!(page.graphics[0].file, expected);
    }
}

#[test]
fn a_local_graphic_path_resolves_against_its_own_directory() {
    let f = turgot_fixture();
    let archive = f.load();
    let atlas = archive.by_id("turgot-1739").unwrap();
    assert_eq!(
        atlas.pages[0].graphics[0].file,
        archive.root.join("source/turgot/turgot_00.jp2")
    );
}

#[test]
fn a_path_escaping_the_root_is_refused() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        r#"
layer = "source"
title = "A"
[[page]]
n = 1
[[page.graphic]]
file = "../../../../etc/passwd"
"#,
    );
    let archive = f.load();
    assert!(has(&archive, "E016"), "codes were {:?}", codes(&archive));
}

#[test]
fn a_backslash_path_is_refused_on_every_platform() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        r#"
layer = "source"
title = "A"
[[page]]
n = 1
[[page.graphic]]
file = "sub\\image.jp2"
"#,
    );
    let archive = f.load();
    assert!(has(&archive, "E012"), "codes were {:?}", codes(&archive));
}

// ---------------------------------------------------------------------------------------
// Page expansion
// ---------------------------------------------------------------------------------------

/// Shape A: no pages at all. Legal, and not even a warning.
#[test]
fn a_record_with_no_pages_expands_to_nothing() {
    let f = journal_fixture();
    let archive = f.load();
    assert!(archive.by_id("journal-de-paris").unwrap().pages.is_empty());
    assert_clean(&archive);
}

/// Shape B: a terse range expands by counting, `n` starting at 1.
#[test]
fn a_range_expands_by_counting_from_one() {
    let f = journal_fixture();
    let archive = f.load();
    let issue = archive.by_id("journal-de-paris-1789-01-03").unwrap();

    assert_eq!(issue.pages.len(), 4);
    assert_eq!(
        issue.pages.iter().map(|p| p.n).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(
        issue
            .pages
            .iter()
            .map(|p| p.graphics[0].page.unwrap())
            .collect::<Vec<_>>(),
        vec![13, 14, 15, 16],
        "graphic pages index into the inherited PDF"
    );
    assert!(issue.pages.iter().all(|p| p.graphics[0].synthesised));
    assert_eq!(issue.pages[0].address(), "journal-de-paris-1789-01-03.p1");
}

/// Shape C: explicit pages. Turgot numbers from zero, and an omitted `n` continues the count.
#[test]
fn explicit_pages_count_on_from_the_previous_page() {
    let f = turgot_fixture();
    let archive = f.load();
    let atlas = archive.by_id("turgot-1739").unwrap();

    assert_eq!(
        atlas.pages.iter().map(|p| p.n).collect::<Vec<_>>(),
        vec![0, 1, 2],
        "n = 0 re-bases the count, so the third page (which omits n) is 2"
    );
    assert_eq!(atlas.pages[0].title.as_deref(), Some("key sheet"));
    assert_eq!(atlas.pages[1].title, None);
    // A standalone image has no page index inside a container.
    assert!(atlas.pages.iter().all(|p| p.graphics[0].page.is_none()));
    assert_eq!(atlas.pages[0].graphics[0].width, Some(23964));
    assert_eq!(atlas.pages[1].graphics[0].height, Some(16926));
    assert!(atlas.pages.iter().all(|p| !p.graphics[0].synthesised));
}

/// The counting default starts at 1 when the first page omits `n`.
#[test]
fn an_omitted_first_n_is_one() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        r#"
layer = "source"
title = "A"
[[page]]
[[page.graphic]]
file = "one.jpg"
[[page]]
[[page.graphic]]
file = "two.jpg"
"#,
    );
    let archive = f.load();
    let node = archive.by_id("a").unwrap();
    assert_eq!(
        node.pages.iter().map(|p| p.n).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

/// Shape D: the range supplies graphic page numbers, the array supplies everything else.
#[test]
fn a_range_and_an_explicit_list_combine() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/01/03/both.toml",
        r#"
layer = "document"
of    = "journal-de-paris-1789-vol1"
pages = { from = 20, to = 22 }

[[page]]
n     = 1
label = "xx"
[[page]]
n = 2
[[page]]
n = 3
"#,
    );
    let archive = f.load();
    assert_clean(&archive);
    let node = archive.by_id("both").unwrap();
    assert_eq!(
        node.pages
            .iter()
            .map(|p| (p.n, p.graphics[0].page.unwrap()))
            .collect::<Vec<_>>(),
        vec![(1, 20), (2, 21), (3, 22)]
    );
    assert_eq!(node.pages[0].label.as_deref(), Some("xx"));
}

#[test]
fn a_range_whose_length_disagrees_with_the_list_is_an_error() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/01/03/bad.toml",
        r#"
layer = "document"
of    = "journal-de-paris-1789-vol1"
pages = { from = 20, to = 25 }
[[page]]
n = 1
"#,
    );
    let archive = f.load();
    assert!(has(&archive, "E903"), "codes were {:?}", codes(&archive));
}

#[test]
fn a_graphic_page_disagreeing_with_its_range_position_is_an_error() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/01/03/bad.toml",
        r#"
layer = "document"
of    = "journal-de-paris-1789-vol1"
pages = { from = 20, to = 21 }
[[page]]
n = 1
[[page.graphic]]
page = 20
[[page]]
n = 2
[[page.graphic]]
page = 99
"#,
    );
    let archive = f.load();
    assert!(has(&archive, "E906"), "codes were {:?}", codes(&archive));
}

#[test]
fn a_backwards_range_is_an_error() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/01/03/bad.toml",
        "layer = \"document\"\nof = \"journal-de-paris-1789-vol1\"\npages = { from = 16, to = 13 }\n",
    );
    let archive = f.load();
    assert!(has(&archive, "E901"), "codes were {:?}", codes(&archive));
}

#[test]
fn a_range_starting_below_one_is_an_error() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/01/03/bad.toml",
        "layer = \"document\"\nof = \"journal-de-paris-1789-vol1\"\npages = { from = 0, to = 3 }\n",
    );
    let archive = f.load();
    assert!(has(&archive, "E902"), "codes were {:?}", codes(&archive));
}

/// A range needs a container to index into.
#[test]
fn a_range_with_no_inherited_scan_file_is_an_error() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        "layer = \"document\"\npages = { from = 1, to = 3 }\n",
    );
    let archive = f.load();
    assert!(has(&archive, "E904"), "codes were {:?}", codes(&archive));
}

#[test]
fn a_graphic_with_no_file_and_no_scan_file_is_an_error() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        r#"
layer = "source"
title = "A"
[[page]]
n = 1
[[page.graphic]]
width = 10
"#,
    );
    let archive = f.load();
    assert!(has(&archive, "E904"), "codes were {:?}", codes(&archive));
}

/// A graphic that omits `file` falls back to the resolved `scan.file`.
#[test]
fn a_graphic_without_a_file_falls_back_to_the_scan_file() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/01/03/fallback.toml",
        r#"
layer = "document"
of    = "journal-de-paris-1789-vol1"
[[page]]
n = 1
[[page.graphic]]
page = 30
"#,
    );
    let archive = f.load();
    assert_clean(&archive);
    let g = &archive.by_id("fallback").unwrap().pages[0].graphics[0];
    assert_eq!(
        g.file,
        archive
            .root
            .join("source/journal-de-paris/1789/journal-de-paris-1789-vol1.pdf")
    );
    assert_eq!(g.page, Some(30));
}

#[test]
fn a_duplicate_n_is_an_error() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        r#"
layer = "source"
title = "A"
[[page]]
n = 1
[[page.graphic]]
file = "one.jpg"
[[page]]
n = 1
[[page.graphic]]
file = "two.jpg"
"#,
    );
    let archive = f.load();
    assert!(has(&archive, "E301"), "codes were {:?}", codes(&archive));
}

/// The collision is between an explicit `n` and a counted one, which only resolved values
/// can catch.
#[test]
fn a_duplicate_n_between_an_explicit_and_a_counted_value_is_caught() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        r#"
layer = "source"
title = "A"
[[page]]
n = 1
[[page.graphic]]
file = "one.jpg"
[[page]]
[[page.graphic]]
file = "two.jpg"
[[page]]
n = 2
[[page.graphic]]
file = "three.jpg"
"#,
    );
    let archive = f.load();
    assert!(has(&archive, "E301"), "codes were {:?}", codes(&archive));
}

#[test]
fn a_page_with_no_graphic_is_a_warning_not_an_error() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        "layer = \"source\"\ntitle = \"A\"\n[[page]]\nn = 1\n",
    );
    let archive = f.load();
    assert!(has(&archive, "W904"), "codes were {:?}", codes(&archive));
    assert!(
        archive.diagnostics.iter().all(|d| !d.is_error()),
        "a text-only page is legal"
    );
}

/// A non-contiguous explicit sequence is legal; only uniqueness is enforced.
#[test]
fn a_gappy_n_sequence_is_legal() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        r#"
layer = "source"
title = "A"
[[page]]
n = 1
[[page.graphic]]
file = "one.jpg"
[[page]]
n = 7
[[page.graphic]]
file = "seven.jpg"
[[page]]
n = -3
[[page.graphic]]
file = "minus.jpg"
"#,
    );
    let archive = f.load();
    assert_clean(&archive);
    let node = archive.by_id("a").unwrap();
    assert_eq!(
        node.pages.iter().map(|p| p.n).collect::<Vec<_>>(),
        vec![1, 7, -3]
    );
}

#[test]
fn the_primary_graphic_is_the_first_in_declaration_order() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        r#"
layer = "source"
title = "A"
[[page]]
n = 1
[[page.graphic]]
file = "full.jp2"
[[page.graphic]]
file = "thumb.jpg"
"#,
    );
    let archive = f.load();
    let page = &archive.by_id("a").unwrap().pages[0];
    assert_eq!(page.graphics.len(), 2);
    assert_eq!(page.primary_graphic().unwrap().file_raw, "full.jp2");
}

// ---------------------------------------------------------------------------------------
// Reference parsing
// ---------------------------------------------------------------------------------------

#[test]
fn a_bare_id_is_a_document_reference() {
    assert_eq!(
        parse_reference("journal-de-paris-1789-01-03").unwrap(),
        Reference::Document("journal-de-paris-1789-01-03".into())
    );
}

#[test]
fn a_page_reference_splits_on_the_last_dot() {
    assert_eq!(
        parse_reference("turgot-1739.p0").unwrap(),
        Reference::Page("turgot-1739".into(), 0)
    );
    assert_eq!(
        parse_reference("journal-de-paris-1789-01-03.p12").unwrap(),
        Reference::Page("journal-de-paris-1789-01-03".into(), 12)
    );
    assert_eq!(
        parse_reference("a.p-3").unwrap(),
        Reference::Page("a".into(), -3)
    );
}

#[test]
fn malformed_references_are_rejected() {
    for bad in [
        "",        // empty
        "a.P1",    // uppercase P
        "a.p01",   // leading zero: two spellings of one address
        "a.p+1",   // leading plus
        "a.p",     // no number
        "a.px",    // not a number
        "a.p1.g0", // graphic selection is reserved, not implemented
        "a.p-0",   // zero has exactly one spelling
        "a b.p1",  // whitespace
        "a.p 1",   // whitespace
        ".p1",     // empty id
        "a.1",     // missing the p
    ] {
        let result = parse_reference(bad);
        assert!(
            result.is_err(),
            "{bad:?} should be rejected, got {result:?}"
        );
        assert_eq!(result.unwrap_err().code, "E110");
    }
}

// ---------------------------------------------------------------------------------------
// Reference resolution
// ---------------------------------------------------------------------------------------

#[test]
fn a_page_reference_resolves_to_a_page_and_its_primary_graphic() {
    let f = turgot_fixture();
    let archive = f.load();

    let RefTarget::Page {
        node,
        page,
        graphic,
    } = archive.resolve_reference("turgot-1739.p0").unwrap()
    else {
        panic!("expected a page");
    };
    assert_eq!(node.id, "turgot-1739");
    assert_eq!(page.n, 0);
    assert_eq!(page.title.as_deref(), Some("key sheet"));
    let graphic = graphic.expect("the key sheet has a graphic");
    assert_eq!(graphic.file_raw, "turgot_00.jp2");
    assert_eq!(graphic.width, Some(23964));
}

/// `.pN` matches the resolved `n`, not the array position. `p0` is the first entry only
/// because that entry says `n = 0`.
#[test]
fn page_references_match_n_not_array_position() {
    let f = turgot_fixture();
    let archive = f.load();

    let RefTarget::Page { page, .. } = archive.resolve_reference("turgot-1739.p1").unwrap() else {
        panic!("expected a page");
    };
    assert_eq!(page.graphics[0].file_raw, "turgot_01.jp2");

    // There is no page 3: the entries resolve to 0, 1, 2.
    assert_eq!(
        archive
            .resolve_reference("turgot-1739.p3")
            .unwrap_err()
            .code,
        "E112"
    );
}

#[test]
fn a_bare_id_resolves_to_the_record_itself() {
    let f = journal_fixture();
    let archive = f.load();
    let RefTarget::Document(node) = archive.resolve_reference("journal-de-paris").unwrap() else {
        panic!("expected a document target");
    };
    assert_eq!(node.id, "journal-de-paris");
}

#[test]
fn an_unknown_id_does_not_resolve() {
    let f = journal_fixture();
    let archive = f.load();
    assert_eq!(archive.resolve_reference("nope").unwrap_err().code, "E111");
    assert_eq!(
        archive.resolve_reference("nope.p1").unwrap_err().code,
        "E111"
    );
}

/// Resolution never walks the `of` chain looking for a page. The copy declares no pages, so
/// asking it for page 1 finds nothing even though its issue has one.
#[test]
fn page_resolution_never_walks_the_of_chain() {
    let f = journal_fixture();
    let archive = f.load();

    assert!(
        archive
            .resolve_reference("journal-de-paris-1789-01-03.p1")
            .is_ok()
    );
    assert_eq!(
        archive
            .resolve_reference("journal-de-paris-1789-vol1.p1")
            .unwrap_err()
            .code,
        "E112",
        "the copy has no pages of its own"
    );
}

/// `n` is document-local: two owners may both have a page 1 and they are different addresses.
#[test]
fn n_is_scoped_to_its_owner() {
    let f = journal_fixture();
    f.write(
        "source/journal-de-paris/1789/01/04/journal-de-paris-1789-01-04.toml",
        r#"
layer = "document"
of    = "journal-de-paris-1789-vol1"
pages = { from = 17, to = 20 }
"#,
    );
    let archive = f.load();
    assert_clean(&archive);

    let RefTarget::Page { page: a, .. } = archive
        .resolve_reference("journal-de-paris-1789-01-03.p1")
        .unwrap()
    else {
        panic!()
    };
    let RefTarget::Page { page: b, .. } = archive
        .resolve_reference("journal-de-paris-1789-01-04.p1")
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(a.n, b.n);
    assert_ne!(a.graphics[0].page, b.graphics[0].page);
    assert_eq!(a.graphics[0].page, Some(13));
    assert_eq!(b.graphics[0].page, Some(17));
}

// ---------------------------------------------------------------------------------------
// Discovery behaviour
// ---------------------------------------------------------------------------------------

#[test]
fn empty_directories_are_tolerated() {
    let f = journal_fixture();
    for month in 1..=12 {
        std::fs::create_dir_all(
            f.root()
                .join(format!("source/journal-de-paris/1789/{month:02}")),
        )
        .expect("mkdir");
    }
    let archive = f.load();
    assert_clean(&archive);
    assert_eq!(archive.nodes.len(), 3);
}

#[test]
fn docs_and_git_are_not_walked() {
    let f = journal_fixture();
    f.write("source/docs/notes.toml", "this is not = = valid toml");
    f.write("source/.git/config.toml", "nor = = is this");
    let archive = f.load();
    assert_clean(&archive);
}

/// Tool configuration is not archive content. Before the migration, discovery falls back to
/// walking the repository root, where `.taplo.toml` and `Cargo.toml` live.
#[test]
fn tool_configuration_is_not_mistaken_for_a_record() {
    let f = Fixture::new();
    // No source/ directory, so discovery falls back to walking the root.
    f.write("a.toml", "layer = \"source\"\ntitle = \"A\"\n");
    f.write(".taplo.toml", "[[rule]]\ninclude = [\"x\"]\n");
    f.write("Cargo.toml", "[package]\nname = \"x\"\n");
    let archive = f.load();
    assert_clean(&archive);
    assert_eq!(archive.nodes.len(), 1);
    assert!(archive.by_id("a").is_some());
}

/// A parse failure must lead with the reason, not with a source snippet.
#[test]
fn a_parse_error_message_leads_with_the_reason() {
    let f = Fixture::new();
    f.write(
        "source/a/a.toml",
        "kind = \"newspaper\"\ntitle = \"Legacy\"\n",
    );
    let archive = f.load();
    let d = &archive.diagnostics[0];
    assert_eq!(d.code, "E011");
    assert!(
        d.message.starts_with("missing field `layer`"),
        "message was {:?}",
        d.message
    );
    assert!(
        !d.message.contains('|'),
        "the snippet leaked: {:?}",
        d.message
    );
    assert!(
        d.message.contains("line 1"),
        "the location was dropped: {:?}",
        d.message
    );
}

#[test]
fn findings_are_emitted_in_a_deterministic_order() {
    let f = Fixture::new();
    f.write(
        "source/z/z.toml",
        "layer = \"source\"\ntitle = \"Z\"\nof = \"nope\"\n",
    );
    f.write(
        "source/a/a.toml",
        "layer = \"source\"\ntitle = \"A\"\nof = \"nope\"\n",
    );
    let first = f.load();
    let second = f.load();
    let keys: Vec<_> = first.diagnostics.iter().map(|d| d.to_string()).collect();
    let again: Vec<_> = second.diagnostics.iter().map(|d| d.to_string()).collect();
    assert_eq!(keys, again);
    assert!(keys[0] < keys[1], "sorted by path: {keys:?}");
}
