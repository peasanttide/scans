//! ADVERSARIAL AUDIT — temporary. Each test constructs an archive that SHOULD fail a check
//! and prints what the real validator actually said.

use std::path::Path;

use scans::load::{Archive, load_archive};
use scans::validate::{self, Options, Report};

struct F {
    dir: tempfile::TempDir,
}

impl F {
    fn new() -> Self {
        F {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn write(&self, rel: &str, contents: &str) -> &Self {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        self
    }
    fn touch(&self, rel: &str) -> &Self {
        self.write(rel, "")
    }
    fn archive(&self) -> Archive {
        load_archive(self.root()).expect("loads")
    }
    fn report(&self) -> Report {
        validate::validate(&self.archive(), &Options::default())
    }
}

fn dump(label: &str, r: &Report) {
    println!("---- {label} ---- errors={} warnings={}", r.errors(), r.warnings());
    if r.findings.is_empty() {
        println!("   (NO FINDINGS AT ALL)");
    }
    for f in &r.findings {
        println!("   {f}");
    }
}

const SRC: &str = r#"
layer = "source"
type  = "newspaper"
title = "Journal de Paris"
"#;

fn copy_toml(name: &str, covers: &str) -> String {
    format!(
        r#"
layer  = "copy"
of     = "journal-de-paris"
title  = "Vol"
covers = "{covers}"

[scan]
file  = "{name}.pdf"
count = 100
"#
    )
}

fn journal() -> F {
    let f = F::new();
    f.write("source/journal-de-paris/journal-de-paris.toml", SRC);
    f.write(
        "source/journal-de-paris/1789/vol1.toml",
        &copy_toml("vol1", "1789-01-01/1789-06-30"),
    );
    f.touch("source/journal-de-paris/1789/vol1.pdf");
    f
}

fn issue(of: &str, date: &str, from: i64, to: i64) -> String {
    format!(
        r#"
layer = "document"
of    = "{of}"
type  = "issue"
date  = "{date}"
pages = {{ from = {from}, to = {to} }}
"#
    )
}

// =========================================================================================
// CHECK 4 — sibling page-range overlap
// =========================================================================================

#[test]
fn c4_identical_ranges() {
    let f = journal();
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-01-03", 13, 16));
    f.write("source/journal-de-paris/1789/b.toml", &issue("vol1", "1789-01-04", 13, 16));
    dump("c4 identical ranges", &f.report());
}

#[test]
fn c4_one_contains_another() {
    let f = journal();
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-01-03", 10, 20));
    f.write("source/journal-de-paris/1789/b.toml", &issue("vol1", "1789-01-04", 12, 14));
    dump("c4 containment", &f.report());
}

#[test]
fn c4_adjacent_is_clean() {
    let f = journal();
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-01-03", 13, 16));
    f.write("source/journal-de-paris/1789/b.toml", &issue("vol1", "1789-01-04", 17, 20));
    dump("c4 adjacent (should be clean)", &f.report());
}

#[test]
fn c4_supplement_overlapping_its_own_issue() {
    let f = journal();
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-01-03", 13, 16));
    f.write(
        "source/journal-de-paris/1789/s.toml",
        r#"
layer = "document"
of    = "a"
type  = "supplement"
supplement_to = "a"
date  = "1789-01-03"
pages = { from = 15, to = 18 }
"#,
    );
    dump("c4 supplement overlaps its issue", &f.report());
}

#[test]
fn c4_reversed_range() {
    let f = journal();
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-01-03", 16, 13));
    dump("c4 reversed from>to", &f.report());
}

/// HOLE CANDIDATE: no copy layer (Turgot shape, copy collapsed into the source).
/// Two documents claim the same pages of the same container. Check 4 is skipped entirely.
#[test]
fn c4_collapsed_copy_overlap() {
    let f = F::new();
    f.write(
        "source/atlas/atlas-1739.toml",
        r#"
layer = "source"
type  = "map"
title = "An atlas"

[scan]
file  = "atlas.pdf"
count = 21
"#,
    );
    f.touch("source/atlas/atlas.pdf");
    f.write("source/atlas/a.toml", &issue("atlas-1739", "1739", 1, 4));
    f.write("source/atlas/b.toml", &issue("atlas-1739", "1739", 3, 6));
    dump("c4 collapsed-copy overlap (E401 expected)", &f.report());
}

/// HOLE CANDIDATE: the grouping key is (copy id, file). Two documents under *different*
/// copies that both index into the SAME physical PDF never compare.
#[test]
fn c4_same_container_two_copies() {
    let f = F::new();
    f.write("source/journal-de-paris/journal-de-paris.toml", SRC);
    f.write(
        "source/journal-de-paris/1789/vol1.toml",
        &copy_toml("shared", "1789-01-01/1789-06-30"),
    );
    f.write(
        "source/journal-de-paris/1789/vol2.toml",
        &copy_toml("shared", "1789-01-01/1789-06-30").replace("of     = \"journal-de-paris\"", "of     = \"journal-de-paris\"\nid     = \"vol2\""),
    );
    f.touch("source/journal-de-paris/1789/shared.pdf");
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-01-03", 13, 16));
    f.write("source/journal-de-paris/1789/b.toml", &issue("vol2", "1789-01-04", 13, 16));
    dump("c4 same container, two copies (E401 expected)", &f.report());
}

/// HOLE CANDIDATE: a copy declaring its own [[page]] that collides with a document's range.
#[test]
fn c4_copy_pages_vs_document_pages() {
    let f = F::new();
    f.write("source/journal-de-paris/journal-de-paris.toml", SRC);
    f.write(
        "source/journal-de-paris/1789/vol1.toml",
        &format!(
            "{}\n[[page]]\nn = 1\n[[page.graphic]]\nfile = \"vol1.pdf\"\npage = 13\n",
            copy_toml("vol1", "1789-01-01/1789-06-30")
        ),
    );
    f.touch("source/journal-de-paris/1789/vol1.pdf");
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-01-03", 13, 16));
    dump("c4 copy [[page]] vs document range", &f.report());
}

// =========================================================================================
// CHECK 5 — range fits scan.count
// =========================================================================================

#[test]
fn c5_exceeds_by_exactly_one() {
    let f = journal();
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-01-03", 98, 101));
    dump("c5 exceeds by exactly one (page 101 > 100)", &f.report());
}

#[test]
fn c5_exactly_at_the_limit_is_clean() {
    let f = journal();
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-01-03", 97, 100));
    dump("c5 exactly at limit (should be clean)", &f.report());
}

/// HOLE CANDIDATE: scan.count on a SOURCE (collapsed copy). Check 5 uses nearest_copy only.
#[test]
fn c5_collapsed_copy_count_ignored() {
    let f = F::new();
    f.write(
        "source/atlas/atlas-1739.toml",
        r#"
layer = "source"
type  = "map"
title = "An atlas"

[scan]
file  = "atlas.pdf"
count = 21
"#,
    );
    f.touch("source/atlas/atlas.pdf");
    f.write("source/atlas/a.toml", &issue("atlas-1739", "1739", 900, 903));
    dump("c5 collapsed-copy scan.count (E501 expected)", &f.report());
}

/// HOLE CANDIDATE: document restates scan.file with a different spelling of the same file.
#[test]
fn c5_relative_path_spelling_dodges_the_container_match() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/01/a.toml",
        r#"
layer = "document"
of    = "vol1"
type  = "issue"
date  = "1789-01-03"
pages = { from = 900, to = 903 }

[scan]
file = "../vol1.pdf"
"#,
    );
    dump("c5 same file, different spelling (E501 expected)", &f.report());
}

// =========================================================================================
// CHECK 6 — dates
// =========================================================================================

#[test]
fn c6_date_outside_covers() {
    let f = journal();
    f.write("source/journal-de-paris/1789/a.toml", &issue("vol1", "1789-08-01", 13, 16));
    dump("c6 date outside covers", &f.report());
}

/// HOLE CANDIDATE: `covers` on a SOURCE with a collapsed copy is never compared against.
#[test]
fn c6_covers_on_a_source_is_ignored() {
    let f = F::new();
    f.write(
        "source/x/src.toml",
        r#"
layer  = "source"
title  = "A paper"
covers = "1789-01-01/1789-06-30"
"#,
    );
    f.write(
        "source/x/a.toml",
        "layer = \"document\"\nof = \"src\"\ntitle = \"t\"\ndate = \"1850-01-01\"\n",
    );
    dump("c6 covers on source, wildly wrong date (E602 expected)", &f.report());
}

/// HOLE CANDIDATE: a document `of` another document whose own `covers` is wrong.
#[test]
fn c6_document_covers_not_compared() {
    let f = F::new();
    f.write(
        "source/x/parent.toml",
        r#"
layer  = "document"
title  = "A run of letters"
covers = "1789-01-01/1789-06-30"
"#,
    );
    f.write(
        "source/x/child.toml",
        "layer = \"document\"\nof = \"parent\"\ntitle = \"t\"\ndate = \"1850-01-01\"\n",
    );
    dump("c6 document covers ignored", &f.report());
}

#[test]
fn c6_edtf_forms_that_should_parse() {
    let f = F::new();
    for (i, date) in [
        "1789-01-03",
        "1789-01",
        "1739",
        "1791/1799",
        "1795~",
        "1789-01?",
        "178X",
        "../1789-06-30",
        "1789-01-01/..",
        "1789%",
        "XXXX",
        "1789-XX-XX",
        "0000",
        "1789-02-29",
    ]
    .iter()
    .enumerate()
    {
        f.write(
            &format!("source/x/d{i}.toml"),
            &format!("layer = \"document\"\ntitle = \"t\"\ndate = \"{date}\"\n"),
        );
    }
    dump("c6 forms that SHOULD parse (any E601 = false reject)", &f.report());
}

#[test]
fn c6_edtf_forms_that_should_be_rejected() {
    let f = F::new();
    for (i, date) in [
        "1789-02-30",
        "1789-02-31",
        "1789-04-31",
        "1789-06-31",
        "1900-02-29",
        "0000-00-00",
        "1789-01-03/1789-01-02",
        "1789-1-3",
    ]
    .iter()
    .enumerate()
    {
        f.write(
            &format!("source/x/d{i}.toml"),
            &format!("layer = \"document\"\ntitle = \"t\"\ndate = \"{date}\"\n"),
        );
    }
    dump("c6 forms that should be REJECTED", &f.report());
    for n in f.archive().iter() {
        println!(
            "   {} date={:?} parsed={:?}",
            n.rel_path,
            n.record.date(),
            n.record.date().map(|d| scans::edtf::parse(d).map(|e| format!("{:?}", e.bounds())))
        );
    }
}

// =========================================================================================
// CHECK 1 — ids, of-cycles
// =========================================================================================

#[test]
fn c1_self_reference() {
    let f = F::new();
    f.write(
        "source/x/a.toml",
        "layer = \"source\"\nof = \"a\"\ntitle = \"t\"\n",
    );
    dump("c1 self-referencing of", &f.report());
}

#[test]
fn c1_two_cycle() {
    let f = F::new();
    f.write("source/x/a.toml", "layer = \"source\"\nof = \"b\"\ntitle = \"t\"\n");
    f.write("source/x/b.toml", "layer = \"source\"\nof = \"a\"\ntitle = \"t\"\n");
    dump("c1 a->b->a cycle", &f.report());
}

#[test]
fn c1_three_cycle_with_a_tail() {
    let f = F::new();
    f.write("source/x/t.toml", "layer = \"source\"\nof = \"a\"\ntitle = \"t\"\n");
    f.write("source/x/a.toml", "layer = \"source\"\nof = \"b\"\ntitle = \"t\"\n");
    f.write("source/x/b.toml", "layer = \"source\"\nof = \"c\"\ntitle = \"t\"\n");
    f.write("source/x/c.toml", "layer = \"source\"\nof = \"a\"\ntitle = \"t\"\n");
    dump("c1 tail into a 3-cycle", &f.report());
}

#[test]
fn c1_duplicate_ids_in_different_directories() {
    let f = F::new();
    f.write("source/a/dup.toml", "layer = \"source\"\ntitle = \"one\"\n");
    f.write("source/b/dup.toml", "layer = \"source\"\ntitle = \"two\"\n");
    dump("c1 duplicate id, two directories", &f.report());
}

/// HOLE CANDIDATE: duplicate ids where THREE files collide.
#[test]
fn c1_triplicate_ids() {
    let f = F::new();
    f.write("source/a/dup.toml", "layer = \"source\"\ntitle = \"one\"\n");
    f.write("source/b/dup.toml", "layer = \"source\"\ntitle = \"two\"\n");
    f.write("source/c/dup.toml", "layer = \"source\"\ntitle = \"three\"\n");
    dump("c1 triplicate id", &f.report());
}

// =========================================================================================
// .pN resolution
// =========================================================================================

#[test]
fn pn_reference_to_a_missing_page() {
    let f = F::new();
    f.write(
        "source/x/d.toml",
        r#"
layer = "document"
title = "t"

[[page]]
n = 1
[[page.graphic]]
file   = "a.jpg"
width  = 1
height = 1
"#,
    );
    f.touch("source/x/a.jpg");
    let a = f.archive();
    for r in ["d", "d.p1", "d.p2", "d.p999", "d.p0", "nope.p1", "d.P1", "d.p1.g0"] {
        println!("   {r:>12} => {:?}", a.resolve_reference(r).map(|t| match t {
            scans::load::RefTarget::Document(n) => format!("document {}", n.id),
            scans::load::RefTarget::Page { page, .. } => format!("page n={}", page.n),
        }));
    }
}

// =========================================================================================
// Robustness: panics, overflow, blowup
// =========================================================================================

#[test]
fn robustness_huge_range_shape_d_overflow() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/a.toml",
        r#"
layer = "document"
of    = "vol1"
date  = "1789-01-03"
pages = { from = -9223372036854775808, to = 9223372036854775807 }

[[page]]
n = 1
"#,
    );
    dump("robustness: i64 range overflow, shape D", &f.report());
}

#[test]
fn robustness_huge_range_shape_b() {
    let f = journal();
    f.write(
        "source/journal-de-paris/1789/a.toml",
        r#"
layer = "document"
of    = "vol1"
date  = "1789-01-03"
pages = { from = 1, to = 20000000 }
"#,
    );
    let r = f.report();
    println!("   findings = {}", r.findings.len());
    dump("robustness: 20M page range", &r);
}

// =========================================================================================
// Exit code vs --select
// =========================================================================================

#[test]
fn select_hides_errors_from_the_exit_code() {
    let f = journal();
    f.write("source/journal-de-paris/1789/01/a.toml", &issue("vol1", "1789-21", 13, 16));
    f.write("source/journal-de-paris/1789/02/b.toml", "layer = \"document\"\nof = \"vol1\"\ntitle = \"t\"\n");
    let whole = f.report();
    println!("   whole-archive exit code = {}", whole.exit_code(false));
    let narrowed = validate::validate(
        &f.archive(),
        &Options {
            select: vec![std::path::PathBuf::from("source/journal-de-paris/1789/02")],
            ..Options::default()
        },
    );
    println!("   selected exit code      = {}", narrowed.exit_code(false));
    dump("select narrowing", &narrowed);
}
