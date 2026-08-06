//! The addresses this repository actually promises.
//!
//! Every other test file runs on a fixture. This one runs on the real archive, because the
//! product of the repository is stable citable addresses and a test on a fixture cannot tell
//! you whether *this* archive still serves them. `turgot-1739.p0` is printed in the design
//! spec as an address a consumer may cite; if it ever stops resolving to the key sheet, that
//! is a broken citation in the consuming repository, and it should break the build here first.
//!
//! Nothing here opens an image: it reads TOML and checks what the loader made of it.

use std::path::{Path, PathBuf};
use std::process::Command;

use scans::load::{RefTarget, load_archive};

/// The repository root — the crate lives in `tools/`, so it is one level up.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tools/ has a parent")
        .to_path_buf()
}

#[test]
fn the_real_archive_loads_without_a_single_diagnostic() {
    let archive = load_archive(repo_root()).expect("the archive loads");
    let found: Vec<String> = archive
        .diagnostics
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    assert!(
        found.is_empty(),
        "the archive is supposed to be clean:\n{}",
        found.join("\n")
    );
    assert!(
        archive.iter().count() >= 7,
        "the archive lost records: {} loaded",
        archive.iter().count()
    );
}

/// The address the spec prints for the atlas key sheet, and the file it must land on.
#[test]
fn turgot_1739_p0_is_the_key_sheet_and_its_graphic_is_turgot_00_jp2() {
    let archive = load_archive(repo_root()).expect("the archive loads");

    let RefTarget::Page {
        node,
        page,
        graphic,
    } = archive
        .resolve_reference("turgot-1739.p0")
        .expect("turgot-1739.p0 resolves")
    else {
        panic!("turgot-1739.p0 must resolve to a page, not to a record");
    };

    assert_eq!(node.id, "turgot-1739");
    assert_eq!(page.n, 0);
    assert_eq!(page.title.as_deref(), Some("key sheet"));

    let graphic = graphic.expect("the key sheet has a graphic");
    assert_eq!(graphic.file_raw, "turgot_00.jp2");
    // A standalone image, not a page inside a container.
    assert_eq!(graphic.page, None);
    assert_eq!((graphic.width, graphic.height), (Some(23964), Some(16934)));

    // The path is resolved against the declaring file's directory, and the image is there.
    // Compared as a suffix: the loader canonicalises, which on Windows adds a `\\?\` prefix.
    assert!(
        graphic.file.ends_with("source/turgot/turgot_00.jp2"),
        "expected the key sheet under source/turgot/, got {}",
        graphic.file.display()
    );
    assert!(
        graphic.file.is_file(),
        "{} is not on disk",
        graphic.file.display()
    );
}

/// The sheets are the whole point of folding 94 files into two, so check the run is intact
/// at both ends and that each address lands on its own image.
#[test]
fn every_atlas_sheet_addresses_its_own_image() {
    let archive = load_archive(repo_root()).expect("the archive loads");

    for (id, count, last_file) in [
        ("turgot-1739", 21, "turgot_20.jp2"),
        ("verniquet-1795", 73, "verniquet_72.jp2"),
    ] {
        let node = archive.by_id(id).unwrap_or_else(|| panic!("{id} exists"));
        assert_eq!(node.pages.len(), count, "{id} lost sheets");

        let mut files = Vec::new();
        for i in 0..count {
            let address = format!("{id}.p{i}");
            let RefTarget::Page { page, graphic, .. } = archive
                .resolve_reference(&address)
                .unwrap_or_else(|e| panic!("{address} does not resolve: {e}"))
            else {
                panic!("{address} must resolve to a page");
            };
            assert_eq!(page.n, i as i64);
            let graphic = graphic.unwrap_or_else(|| panic!("{address} has no graphic"));
            assert!(
                graphic.file.is_file(),
                "{address}: {} is not on disk",
                graphic.file.display()
            );
            files.push(graphic.file_raw.clone());
        }

        assert_eq!(files.last().map(String::as_str), Some(last_file));
        let mut distinct = files.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), files.len(), "{id}: two sheets share an image");

        // One past the end must not resolve, or `.pN` would silently accept anything.
        assert!(archive.resolve_reference(&format!("{id}.p{count}")).is_err());
    }
}

/// The two volumes are the only records that index into a container, and the terse
/// `pages = { from, to }` form on an issue is meaningless without them.
#[test]
fn both_journal_volumes_name_a_pdf_that_exists() {
    let archive = load_archive(repo_root()).expect("the archive loads");

    for (id, count, pdf) in [
        (
            "journal-de-paris-1789-vol1",
            888,
            "journal-de-paris-1789-vol1.pdf",
        ),
        (
            "journal-de-paris-1789-vol2",
            1346,
            "journal-de-paris-1789-vol2.pdf",
        ),
    ] {
        let node = archive.by_id(id).unwrap_or_else(|| panic!("{id} exists"));
        let scan_file = node
            .resolved
            .scan_file_path()
            .unwrap_or_else(|| panic!("{id} has no scan.file"));
        assert_eq!(scan_file.file_name().unwrap().to_string_lossy(), pdf);
        assert!(scan_file.is_file(), "{} is not on disk", scan_file.display());
        assert_eq!(node.resolved.scan_count, Some(count));
    }
}

// ---------------------------------------------------------------------------------------
// The command line
// ---------------------------------------------------------------------------------------

fn scans(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_scans"))
        .args(args)
        .arg("--root")
        .arg(repo_root())
        .output()
        .expect("the scans binary runs");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

/// `scans resolve` is how a human checks a citation, so its output has to name the file and
/// the page — an address that resolves silently is no better than one that does not resolve.
#[test]
fn resolve_names_the_record_the_page_and_the_image() {
    let (ok, out) = scans(&["resolve", "turgot-1739.p0"]);
    assert!(ok, "resolve failed:\n{out}");
    assert!(out.contains("source/turgot/turgot-1739.toml"), "{out}");
    assert!(out.contains("n = 0"), "{out}");
    assert!(out.contains("key sheet"), "{out}");
    assert!(out.contains("source/turgot/turgot_00.jp2"), "{out}");
    assert!(out.contains("23964x16934"), "{out}");
    // Repo-relative, not the canonicalised `\?\D:\...` spelling.
    assert!(!out.contains("?\\"), "paths are not repo-relative:\n{out}");
}

/// An address that does not resolve must fail loudly. A citation tool that exits 0 on a dead
/// reference is worse than none.
#[test]
fn resolve_fails_on_an_address_that_does_not_exist() {
    let (ok, out) = scans(&["resolve", "turgot-1739.p21"]);
    assert!(!ok, "a dead address must exit non-zero:\n{out}");
    assert!(out.contains("turgot-1739.p21"), "{out}");
    assert!(out.contains("no page"), "{out}");

    let (ok, out) = scans(&["resolve", "no-such-record"]);
    assert!(!ok, "an unknown id must exit non-zero:\n{out}");
    assert!(out.contains("unknown id"), "{out}");
}

/// The archive is the thing this tool exists to keep clean, so its own repository has to pass
/// the tool's strictest default: warnings included.
#[test]
fn the_command_line_reports_the_archive_clean_even_under_strict() {
    let (ok, out) = scans(&["validate", "--strict"]);
    assert!(ok, "the archive does not validate:\n{out}");
    assert!(out.contains("0 error(s), 0 warning(s)"), "{out}");
}
