//! The ten checks.
//!
//! # What runs where
//!
//! [`crate::load`] has already produced every *structural* finding — the ones that must be
//! settled before any other check can run — and [`Report::new`] seeds the report with them.
//! This module must never re-report those codes, or every one would appear twice:
//! `E010`–`E016`, `E101`–`E104`, `E108`, `E301`, `E901`–`E906`, `W903` and `W904`. That is
//! check 3 and check 9 in their entirety, and the error half of check 1.
//!
//! Everything else lives here:
//!
//! | check | codes | needs `--probe` |
//! |---|---|---|
//! | pre-checks | `W014`, `W015`, `E402`, `E403` | no |
//! | 1 — identity | `W105`, `W106`, `W107` | no |
//! | 2 — layer order | `E201` | no |
//! | 3 — page `n` unique | *(load: `E301`)* | no |
//! | 4 — sibling overlap | `E401` | no |
//! | 5 — range fits `scan.count` | `E501`, `E502` | no |
//! | 6 — EDTF and `covers` | `E601`, `E602`, `E604`, `E605`, `W603` | no |
//! | 7 — files on disk | `E701`, `W703` | `E702`, `E704`, `E706`, `E707`, `W705` |
//! | 8 — cross-references | `E801`, `E802`, `W803` | no |
//! | 9 — counting fits | *(load: `E901`–`E906`)* | no |
//! | 10 — gap report | `W1001` | no |
//!
//! # The probe rule
//!
//! Nothing outside the `probe` cargo feature may open a file's bytes or touch the network. The
//! single exception is `E701`, which is a bare existence check: a `stat` costs nothing, catches
//! the commonest typo, and never reads a byte of the several gigabytes of `.jp2` this
//! repository keeps in Git LFS.
//!
//! `--probe` on a binary built without the feature is a no-op here; `main.rs` prints the
//! explanation.
//!
//! # Severity
//!
//! [`Severity`] has two levels, so check 10 — which the spec calls a *report* rather than an
//! error — is emitted as a warning. It never fails the build unless `--strict` is given, which
//! is the intended behaviour: a missing issue number is a fact about the archive's
//! completeness, not a schema violation, and the archive is knowingly incomplete.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::edtf::{self, DateRelation, Edtf};
use crate::load::{
    Archive, Diagnostic, Node, NodeId, ResolvedGraphic, ResolvedPage, Severity, sort_diagnostics,
};
use crate::model::{Layer, Text};

/// The `text.kind` values the archive knows about. Anything else is `W014`, a warning, because
/// the vocabulary is open and a new kind should not stop a build.
const TEXT_KINDS: &[&str] = &["ocr", "transcription"];

/// How many shared page numbers `E401` lists before it truncates.
const MAX_SHARED_PAGES_SHOWN: usize = 5;

/// How many gap findings check 10 emits for one group before collapsing the remainder.
const MAX_GAP_FINDINGS: usize = 50;

// ---------------------------------------------------------------------------------------
// Options and report
// ---------------------------------------------------------------------------------------

/// How to run the checks.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Enable the checks that must read image and container bytes. Requires the `probe`
    /// cargo feature; without it these checks are skipped.
    pub probe: bool,
    /// Promote every warning to an error for the purposes of the exit code.
    pub strict: bool,
    /// Restrict *reporting* to findings under these paths. Loading is always whole-archive,
    /// because the id index and the sibling checks need every file. Empty means report
    /// everything.
    pub select: Vec<PathBuf>,
}

impl Options {
    /// True when the byte-reading checks should actually run: asked for *and* compiled in.
    fn probing(&self) -> bool {
        self.probe && cfg!(feature = "probe")
    }
}

/// The findings, in the documented deterministic order.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Diagnostic>,
}

impl Report {
    /// Start a report from the archive's load-time findings.
    pub fn new(archive: &Archive) -> Self {
        Report {
            findings: archive.diagnostics.clone(),
        }
    }

    pub fn push(&mut self, finding: Diagnostic) {
        self.findings.push(finding);
    }

    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|f| f.is_error()).count()
    }

    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    /// Every finding carrying this code, for tests and for callers that want to inspect one
    /// check in isolation.
    pub fn with_code<'a>(&'a self, code: &'a str) -> impl Iterator<Item = &'a Diagnostic> {
        self.findings.iter().filter(move |f| f.code == code)
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.with_code(code).next().is_some()
    }

    /// `0` when clean, `1` when anything counts as an error. `--strict` makes warnings count.
    pub fn exit_code(&self, strict: bool) -> i32 {
        let failing = if strict {
            !self.findings.is_empty()
        } else {
            self.errors() > 0
        };
        i32::from(failing)
    }

    /// Fix the output order: path, then code, then locator.
    pub fn sort(&mut self) {
        sort_diagnostics(&mut self.findings);
    }

    /// Drop findings outside the selected paths.
    ///
    /// Loading is always whole-archive — the id index, the sibling overlap check and the gap
    /// report all need every file — so narrowing happens here, at the reporting step, and never
    /// changes a verdict.
    fn retain_selected(&mut self, archive: &Archive, select: &[PathBuf]) {
        if select.is_empty() {
            return;
        }
        let prefixes: Vec<String> = select
            .iter()
            .map(|p| selection_prefix(archive, p))
            .collect();
        self.findings.retain(|f| {
            prefixes.iter().any(|prefix| {
                prefix.is_empty() || f.path == *prefix || f.path.starts_with(&format!("{prefix}/"))
            })
        });
    }
}

/// Turn a user-supplied path into the repo-relative prefix findings are matched against.
///
/// Accepts a path relative to the working directory (what someone types on the command line)
/// or relative to the archive root (what a test writes), and falls back to lexical handling
/// when neither exists on disk.
fn selection_prefix(archive: &Archive, path: &Path) -> String {
    let absolute = std::fs::canonicalize(path)
        .or_else(|_| std::fs::canonicalize(archive.root.join(path)))
        .unwrap_or_else(|_| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                archive.root.join(path)
            }
        });

    let relative = absolute.strip_prefix(&archive.root).unwrap_or(&absolute);
    relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_matches('/')
        .to_string()
}

// ---------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------

/// Run the checks over an already-loaded archive.
pub fn validate(archive: &Archive, options: &Options) -> Report {
    let mut report = Report::new(archive);

    pre_checks(archive, &mut report);
    check_1_identity(archive, &mut report);
    check_2_layer_order(archive, &mut report);
    // Check 3 (`E301`) and check 9 (`E901`–`E906`, `W903`, `W904`) are settled during page
    // expansion in `load`, and are already in the report via `Report::new`.
    check_4_sibling_overlap(archive, &mut report);
    check_5_scan_count(archive, &mut report);
    check_6_dates(archive, &mut report);
    check_7_files(archive, options, &mut report);
    check_8_crossrefs(archive, &mut report);
    check_10_gaps(archive, &mut report);
    check_11_ocr(archive, options, &mut report);

    report.retain_selected(archive, &options.select);
    report.sort();
    report
}

// ---------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------

/// Repo-relative path with forward slashes, for messages.
fn rel(archive: &Archive, path: &Path) -> String {
    path.strip_prefix(&archive.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Lexically normalise a path: resolve `.` and `..` without touching the filesystem.
///
/// Only `E706` needs this — graphic paths were already resolved during loading — so it is
/// compiled out with the rest of the probe half.
#[cfg(feature = "probe")]
fn normalise(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// One graphic, together with the page and record it came from.
struct GraphicRef<'a> {
    node: &'a Node,
    page: &'a ResolvedPage,
    index: usize,
    graphic: &'a ResolvedGraphic,
}

impl GraphicRef<'_> {
    /// `[[page]]#2 graphic#0`.
    fn locator(&self) -> String {
        format!("[[page]]#{} graphic#{}", self.page.source_index, self.index)
    }

    /// The citable address of the page this graphic hangs off.
    fn address(&self) -> String {
        self.page.address()
    }
}

/// Every graphic in the archive, in a stable order.
fn graphic_refs(archive: &Archive) -> impl Iterator<Item = GraphicRef<'_>> {
    archive.iter().flat_map(|node| {
        node.pages.iter().flat_map(move |page| {
            page.graphics
                .iter()
                .enumerate()
                .map(move |(index, graphic)| GraphicRef {
                    node,
                    page,
                    index,
                    graphic,
                })
        })
    })
}

/// The file a finding about this graphic should be reported against.
///
/// A synthesised graphic has no line of its own: its path came from `scan.file`, which is
/// usually declared on an ancestor. Reporting a missing PDF against 365 issue files that merely
/// inherited it would bury the one file that actually needs editing, so the finding is
/// attributed to the file that declared the value.
fn attributed_path(archive: &Archive, r: &GraphicRef<'_>) -> String {
    if r.graphic.synthesised
        && let Some(prov) = r.node.resolved.scan_file.as_ref()
        && r.node.resolved.scan_file_path().as_deref() == Some(r.graphic.file.as_path())
    {
        return rel(archive, &prov.file);
    }
    r.node.rel_path.clone()
}

/// Comma-separated, truncated after [`MAX_SHARED_PAGES_SHOWN`].
fn show_pages(pages: &BTreeSet<i64>) -> String {
    let shown: Vec<String> = pages
        .iter()
        .take(MAX_SHARED_PAGES_SHOWN)
        .map(i64::to_string)
        .collect();
    if pages.len() > MAX_SHARED_PAGES_SHOWN {
        format!(
            "{} … and {} more",
            shown.join(", "),
            pages.len() - MAX_SHARED_PAGES_SHOWN
        )
    } else {
        shown.join(", ")
    }
}

// ---------------------------------------------------------------------------------------
// Pre-checks: W014, W015, E402, E403
// ---------------------------------------------------------------------------------------

fn pre_checks(archive: &Archive, report: &mut Report) {
    for node in archive.iter() {
        for (index, text) in node.record.text().iter().enumerate() {
            check_text_kind(node, text, &format!("[[text]]#{index}"), report);
        }

        for page in &node.pages {
            for (index, text) in page.texts.iter().enumerate() {
                let locator = format!("[[page]]#{} text#{index}", page.source_index);
                check_text_kind(node, text, &locator, report);
            }
            check_zones(archive, node, page, report);
        }
    }
}

fn check_text_kind(node: &Node, text: &Text, locator: &str, report: &mut Report) {
    let Some(kind) = text.kind.as_deref() else {
        return;
    };
    if TEXT_KINDS.contains(&kind) {
        return;
    }
    report.push(
        Diagnostic::warning(
            node.rel_path.clone(),
            "W014",
            format!(
                "text.kind {kind:?} is outside the known vocabulary; expected one of {}",
                TEXT_KINDS.join(", ")
            ),
        )
        .at(locator),
    );
}

/// Zone geometry. Coordinates are in the pixel space of the page's **primary graphic**, so the
/// bounds check (`E403`) can only run when that graphic declares its dimensions.
fn check_zones(archive: &Archive, node: &Node, page: &ResolvedPage, report: &mut Report) {
    let mut seen_ids: BTreeMap<&str, usize> = BTreeMap::new();
    let primary = page.primary_graphic();

    for (index, zone) in page.zones.iter().enumerate() {
        let locator = format!("[[page]]#{} zone#{index}", page.source_index);
        let name = zone
            .id
            .as_deref()
            .map(|id| format!("{id:?}"))
            .unwrap_or_else(|| format!("#{index}"));

        if let Some(id) = zone.id.as_deref() {
            match seen_ids.get(id) {
                None => {
                    seen_ids.insert(id, index);
                }
                Some(first) => report.push(
                    Diagnostic::warning(
                        node.rel_path.clone(),
                        "W015",
                        format!(
                            "duplicate zone id {id:?} within page n = {}; also declared at \
                             zone#{first}",
                            page.n
                        ),
                    )
                    .at(&locator),
                ),
            }
        }

        // E402: the rectangle must be non-empty and on the page.
        let mut geometry_ok = true;
        for (label, value) in [
            ("ulx", zone.ulx),
            ("uly", zone.uly),
            ("lrx", zone.lrx),
            ("lry", zone.lry),
        ] {
            if value < 0 {
                report.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E402",
                        format!(
                            "zone {name} on page n = {}: {label} = {value} must not be negative",
                            page.n
                        ),
                    )
                    .at(&locator),
                );
                geometry_ok = false;
            }
        }
        if zone.ulx >= zone.lrx {
            report.push(
                Diagnostic::error(
                    node.rel_path.clone(),
                    "E402",
                    format!(
                        "zone {name} on page n = {}: ulx = {} is not left of lrx = {}, so the \
                         rectangle has no width",
                        page.n, zone.ulx, zone.lrx
                    ),
                )
                .at(&locator),
            );
            geometry_ok = false;
        }
        if zone.uly >= zone.lry {
            report.push(
                Diagnostic::error(
                    node.rel_path.clone(),
                    "E402",
                    format!(
                        "zone {name} on page n = {}: uly = {} is not above lry = {}, so the \
                         rectangle has no height",
                        page.n, zone.uly, zone.lry
                    ),
                )
                .at(&locator),
            );
            geometry_ok = false;
        }
        if !geometry_ok {
            continue;
        }

        // E403: only meaningful once the primary graphic has stated its pixel space.
        let Some(graphic) = primary else { continue };
        for (label, edge, extent, extent_label) in [
            ("lrx", zone.lrx, graphic.width, "width"),
            ("lry", zone.lry, graphic.height, "height"),
        ] {
            let Some(extent) = extent else { continue };
            if edge > extent {
                report.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E403",
                        format!(
                            "zone {name} on page n = {}: {label} = {edge} is outside the primary \
                             graphic {} ({extent_label} = {extent})",
                            page.n,
                            rel(archive, &graphic.file)
                        ),
                    )
                    .at(&locator),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Check 1 — identity. The errors (E101–E104, E108) belong to `load`; these are the warnings.
// ---------------------------------------------------------------------------------------

fn check_1_identity(archive: &Archive, report: &mut Report) {
    // W105 — house style.
    for node in archive.iter() {
        if crate::load::id_is_valid(&node.id) && !crate::load::id_is_preferred(&node.id) {
            report.push(Diagnostic::warning(
                node.rel_path.clone(),
                "W105",
                format!(
                    "id {:?} is outside the preferred form: lowercase [a-z0-9] groups separated \
                     by single hyphens{}",
                    node.id,
                    if node.id_declared {
                        ""
                    } else {
                        " (this id came from the filename; rename the file or set `id`)"
                    }
                ),
            ));
        }
    }

    // W106 — ids that differ only in case. Exact duplicates are `E101`, already reported by
    // `load`; this catches the pair that a case-insensitive filesystem will eventually confuse.
    let mut folded: BTreeMap<String, Vec<&Node>> = BTreeMap::new();
    for node in archive.iter() {
        folded.entry(node.id.to_lowercase()).or_default().push(node);
    }
    for group in folded.values() {
        let mut distinct: Vec<&&Node> = Vec::new();
        for node in group {
            if !distinct.iter().any(|other| other.id == node.id) {
                distinct.push(node);
            }
        }
        if distinct.len() < 2 {
            continue;
        }
        let mut ordered = distinct.clone();
        ordered.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        let first = ordered[0];
        for node in &ordered[1..] {
            report.push(
                Diagnostic::warning(
                    node.rel_path.clone(),
                    "W106",
                    format!(
                        "id {:?} collides case-insensitively with {:?}; the two are distinct ids \
                         but this repository is developed on a case-insensitive filesystem",
                        node.id, first.id
                    ),
                )
                .also(first.rel_path.clone()),
            );
        }
    }

    // W107 — an orphan copy.
    //
    // Deliberately *not* raised for a parentless document. The spec's third document example is
    // a one-off engraving with no parent at all, described as "the shape most of the archive
    // will take"; warning on the majority shape would train people to ignore the validator,
    // which is the failure this whole file exists to avoid. A copy is different: it is one
    // physical object *of* a work, so a copy of nothing is a genuine loose end.
    for node in archive.iter() {
        if node.layer() != Layer::Copy || node.record.of().is_some() {
            continue;
        }
        report.push(Diagnostic::warning(
            node.rel_path.clone(),
            "W107",
            "copy declares no 'of', so it is a copy of no source and will inherit nothing — no \
             rights, no holding, no language"
                .to_string(),
        ));
    }
}

// ---------------------------------------------------------------------------------------
// Check 2 — child layer at or below parent's
// ---------------------------------------------------------------------------------------

fn check_2_layer_order(archive: &Archive, report: &mut Report) {
    for node in archive.iter() {
        let Some(parent) = archive.parent(node.index) else {
            continue;
        };
        let (child_layer, parent_layer) = (node.layer(), parent.layer());
        // Equal ranks are legal: the spec says "at or below", which permits a source `of` a
        // source (a series) and a document `of` a document (a supplement under an issue).
        if child_layer.rank() >= parent_layer.rank() {
            continue;
        }
        report.push(
            Diagnostic::error(
                node.rel_path.clone(),
                "E201",
                format!(
                    "layer {:?} may not be a child of layer {:?}: of = {:?} resolves to a {} \
                     record; the order is source > copy > document > page",
                    child_layer.as_str(),
                    parent_layer.as_str(),
                    parent.id,
                    parent_layer
                ),
            )
            .also(parent.rel_path.clone()),
        );
    }
}

// ---------------------------------------------------------------------------------------
// Check 4 — sibling documents' page ranges do not overlap within a copy
// ---------------------------------------------------------------------------------------

/// Two documents collide only when they claim the same page of the same container, so the
/// grouping key is `(copy, container file)` rather than the copy alone: a copy may in principle
/// hold more than one container, and two documents in different containers cannot collide.
fn check_4_sibling_overlap(archive: &Archive, report: &mut Report) {
    let mut claims: BTreeMap<(&str, &Path), BTreeMap<i64, BTreeSet<NodeId>>> = BTreeMap::new();

    for r in graphic_refs(archive) {
        if r.node.layer() != Layer::Document {
            continue;
        }
        let Some(copy) = archive.nearest_copy(r.node.index) else {
            // No copy layer means no siblings to collide with. Turgot and Verniquet are
            // deliberately shaped this way, so this is skipped silently.
            continue;
        };
        let Some(page) = r.graphic.page else { continue };
        claims
            .entry((copy.id.as_str(), r.graphic.file.as_path()))
            .or_default()
            .entry(page)
            .or_default()
            .insert(r.node.index);
    }

    // Collapse to one finding per unordered pair per container, listing every shared page.
    let mut shared: BTreeMap<(NodeId, NodeId, &Path), BTreeSet<i64>> = BTreeMap::new();
    for ((_, file), by_page) in &claims {
        for (page, owners) in by_page {
            if owners.len() < 2 {
                continue;
            }
            let owners: Vec<NodeId> = owners.iter().copied().collect();
            for (i, a) in owners.iter().enumerate() {
                for b in &owners[i + 1..] {
                    shared.entry((*a, *b, *file)).or_default().insert(*page);
                }
            }
        }
    }

    for ((a, b, file), pages) in shared {
        let (left, right) = (archive.get(a), archive.get(b));
        // Reported on the lexicographically-first document's path.
        let (first, second) = if left.rel_path <= right.rel_path {
            (left, right)
        } else {
            (right, left)
        };
        report.push(
            Diagnostic::error(
                first.rel_path.clone(),
                "E401",
                format!(
                    "page range overlaps {:?} on {}: both claim graphic page(s) {}",
                    second.id,
                    rel(archive, file),
                    show_pages(&pages)
                ),
            )
            .also(second.rel_path.clone()),
        );
    }
}

// ---------------------------------------------------------------------------------------
// Check 5 — every page range fits inside the copy's scan.count
// ---------------------------------------------------------------------------------------

fn check_5_scan_count(archive: &Archive, report: &mut Report) {
    for r in graphic_refs(archive) {
        let Some(copy) = archive.nearest_copy(r.node.index) else {
            continue;
        };
        // `scan.count` is self-only, so it is read from the copy, never from the resolved value
        // of the record being checked: the count describes the container as the copy knows it.
        let Some(count) = copy.resolved.scan_count else {
            continue;
        };
        let Some(container) = copy.resolved.scan_file_path() else {
            continue;
        };
        // Only graphics that actually index into the copy's container are constrained by it.
        if r.graphic.file != container {
            continue;
        }
        let Some(page) = r.graphic.page else { continue };

        if page < 1 {
            report.push(
                Diagnostic::error(
                    r.node.rel_path.clone(),
                    "E502",
                    format!(
                        "{}: graphic page {page} is less than 1; container pages are 1-based",
                        r.address()
                    ),
                )
                .at(r.locator()),
            );
        } else if page > count {
            report.push(
                Diagnostic::error(
                    r.node.rel_path.clone(),
                    "E501",
                    format!(
                        "{}: graphic page {page} exceeds scan.count = {count} of copy {:?} ({})",
                        r.address(),
                        copy.id,
                        rel(archive, &container)
                    ),
                )
                .at(r.locator())
                .also(copy.rel_path.clone()),
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Check 6 — EDTF parses, and a document's date falls inside its copy's covers
// ---------------------------------------------------------------------------------------

fn check_6_dates(archive: &Archive, report: &mut Report) {
    for node in archive.iter() {
        // Parse every EDTF-typed field, so a bad `founded` is caught even though nothing
        // compares against it.
        let date = parse_edtf_field(node, "date", node.record.date(), report);
        let _ = parse_edtf_field(node, "founded", node.record.founded(), report);
        let covers = parse_edtf_field(node, "covers", node.record.covers(), report);

        if let Some(covers) = covers.as_ref()
            && !covers.is_interval()
        {
            report.push(Diagnostic::error(
                node.rel_path.clone(),
                "E604",
                format!(
                    "covers {:?} must be an EDTF interval containing '/', e.g. \
                     \"1789-01-01/1789-06-30\" — a bare date states a point, not a span",
                    covers.raw()
                ),
            ));
        }

        let Some(date) = date else { continue };
        let Some(copy) = archive.nearest_copy(node.index) else {
            // Skipped silently: an archive without a copy layer is a supported shape.
            continue;
        };
        // A copy is its own nearest copy; comparing its date against its own covers would be
        // circular, so only descendants are checked.
        if copy.index == node.index {
            continue;
        }
        let Some(raw_covers) = copy.record.covers() else {
            continue;
        };
        // A malformed `covers` is already `E601` on the copy; do not report it twice.
        let Ok(copy_covers) = edtf::parse(raw_covers) else {
            continue;
        };
        if !copy_covers.is_interval() {
            continue;
        }

        match edtf::relate(&date, &copy_covers) {
            DateRelation::Contained => {}
            DateRelation::Overlaps => report.push(
                Diagnostic::warning(
                    node.rel_path.clone(),
                    "W603",
                    format!(
                        "date {:?} is not fully contained in covers {:?} of copy {:?}; it \
                         overlaps, which is what an imprecise date looks like — state a more \
                         precise date if you can",
                        date.raw(),
                        copy_covers.raw(),
                        copy.id
                    ),
                )
                .also(copy.rel_path.clone()),
            ),
            DateRelation::Disjoint => report.push(
                Diagnostic::error(
                    node.rel_path.clone(),
                    "E602",
                    format!(
                        "date {:?} falls outside covers {:?} of copy {:?}; there is no day the \
                         two have in common, so this record is filed under the wrong copy",
                        date.raw(),
                        copy_covers.raw(),
                        copy.id
                    ),
                )
                .also(copy.rel_path.clone()),
            ),
        }
    }
}

/// Parse one EDTF-valued field, reporting `E601` on a bad value and `E605` on a backwards
/// interval. Returns `None` when the field is absent or unparseable.
fn parse_edtf_field(
    node: &Node,
    field: &str,
    value: Option<&str>,
    report: &mut Report,
) -> Option<Edtf> {
    let raw = value?;
    match edtf::parse(raw) {
        Err(e) => {
            report.push(Diagnostic::error(
                node.rel_path.clone(),
                "E601",
                format!("field '{field}' value {raw:?} is not valid EDTF: {e}"),
            ));
            None
        }
        Ok(parsed) => {
            if parsed.start_after_end() {
                report.push(Diagnostic::error(
                    node.rel_path.clone(),
                    "E605",
                    format!(
                        "field '{field}' interval {raw:?} has its start after its end; write the \
                         earlier endpoint first"
                    ),
                ));
            }
            Some(parsed)
        }
    }
}

// ---------------------------------------------------------------------------------------
// Check 7 — graphic files exist; dimensions match
// ---------------------------------------------------------------------------------------

fn check_7_files(archive: &Archive, options: &Options, report: &mut Report) {
    // `E701` is the one default-path check that touches the filesystem, and it is a bare
    // existence test. Deduplicated by (reporting file, resolved path) so that a copy whose PDF
    // is missing produces one finding rather than one per issue that inherited it.
    let mut reported: BTreeSet<(String, PathBuf)> = BTreeSet::new();

    for r in graphic_refs(archive) {
        let owner = attributed_path(archive, &r);

        if reported.insert((owner.clone(), r.graphic.file.clone())) && !r.graphic.file.exists() {
            let mut finding = Diagnostic::error(
                owner.clone(),
                "E701",
                format!(
                    "graphic file {:?} does not exist (resolved to {})",
                    r.graphic.file_raw,
                    rel(archive, &r.graphic.file)
                ),
            );
            if owner != r.node.rel_path {
                finding = finding.also(r.node.rel_path.clone());
            } else {
                finding = finding.at(r.locator());
            }
            report.push(finding);
        }

        // Only warn about a graphic somebody actually wrote out. A graphic synthesised from a
        // `pages` range indexes into a PDF and could not state pixel dimensions even in
        // principle, so warning about it would be noise on every terse issue in the archive.
        if !r.graphic.synthesised && (r.graphic.width.is_none() || r.graphic.height.is_none()) {
            report.push(
                Diagnostic::warning(
                    r.node.rel_path.clone(),
                    "W703",
                    format!(
                        "graphic {:?} declares no {}; crops and tiles cannot be planned without \
                         the pixel space",
                        r.graphic.file_raw,
                        match (r.graphic.width, r.graphic.height) {
                            (None, None) => "width or height",
                            (None, _) => "width",
                            _ => "height",
                        }
                    ),
                )
                .at(r.locator()),
            );
        }
    }

    if options.probing() {
        probe_checks(archive, report);
    }
}

// ---------------------------------------------------------------------------------------
// Check 8 — cross-references resolve
// ---------------------------------------------------------------------------------------

/// Every field whose value is an id pointing at another record, and the layer it must name.
///
/// The spec says "`supplement_to`, and similar". Adding the next one is a line here.
fn crossrefs(node: &Node) -> Vec<(&'static str, &str, Layer)> {
    let mut out = Vec::new();
    if let Some(target) = node.record.supplement_to() {
        out.push(("supplement_to", target, Layer::Document));
    }
    out
}

fn check_8_crossrefs(archive: &Archive, report: &mut Report) {
    for node in archive.iter() {
        for (field, target_id, expected) in crossrefs(node) {
            let Some(target) = archive.by_id(target_id) else {
                report.push(Diagnostic::error(
                    node.rel_path.clone(),
                    "E801",
                    format!(
                        "{field} = {target_id:?} does not resolve to any known id; ids are flat \
                         and default to the filename stem"
                    ),
                ));
                continue;
            };

            if target.layer() != expected {
                report.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E802",
                        format!(
                            "{field} = {target_id:?} resolves to layer {:?}; it must name a {}",
                            target.layer().as_str(),
                            expected
                        ),
                    )
                    .also(target.rel_path.clone()),
                );
                continue;
            }

            // A supplement and the issue it supplements are bound in the same volume unless
            // something has gone wrong, but "unless" is why this is a warning.
            let here = archive.nearest_copy(node.index).map(|c| c.id.as_str());
            let there = archive.nearest_copy(target.index).map(|c| c.id.as_str());
            if here != there && (here.is_some() || there.is_some()) {
                report.push(
                    Diagnostic::warning(
                        node.rel_path.clone(),
                        "W803",
                        format!(
                            "{field} = {target_id:?} is in a different copy ({} vs {})",
                            here.unwrap_or("none"),
                            there.unwrap_or("none")
                        ),
                    )
                    .also(target.rel_path.clone()),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Check 10 — gap report
// ---------------------------------------------------------------------------------------

/// Missing issue numbers in a serial's run.
///
/// The spec asks for gaps in a serial's *expected* run, but nothing in the schema declares an
/// expected run and inventing an `[expect]` block would be inventing schema. The run is
/// therefore inferred from what is present: `min(no) … max(no)` within a group, which finds a
/// hole between two issues and says nothing at all about the ends. Warning only — a missing
/// issue is a fact about the archive's completeness, not a schema violation.
fn check_10_gaps(archive: &Archive, report: &mut Report) {
    let mut groups: BTreeMap<NodeId, BTreeMap<i64, &Node>> = BTreeMap::new();

    for node in archive.iter() {
        if node.layer() != Layer::Document {
            continue;
        }
        let Some(no) = node.record.no() else { continue };
        let Some(anchor) = archive
            .nearest_copy(node.index)
            .or_else(|| archive.nearest_source(node.index))
        else {
            continue;
        };
        groups.entry(anchor.index).or_default().insert(no, node);
    }

    for (anchor, members) in groups {
        // Two issues say nothing about a run; three is the least that can have a hole in it.
        if members.len() < 3 {
            continue;
        }
        let anchor = archive.get(anchor);
        let (min, max) = match (members.keys().next(), members.keys().next_back()) {
            (Some(a), Some(b)) => (*a, *b),
            _ => continue,
        };

        let missing: Vec<i64> = (min..=max).filter(|n| !members.contains_key(n)).collect();
        let runs = collapse_runs(&missing);
        let shown = runs.len().min(MAX_GAP_FINDINGS);

        for (a, b) in runs.iter().take(shown) {
            let which = if a == b {
                format!("no = {a}")
            } else {
                format!("no = {a}..{b}")
            };
            report.push(Diagnostic::warning(
                anchor.rel_path.clone(),
                "W1001",
                format!(
                    "gap in {:?}: no document with {which} (observed run {min}..{max}, {} present)",
                    anchor.id,
                    members.len()
                ),
            ));
        }
        if runs.len() > shown {
            report.push(Diagnostic::warning(
                anchor.rel_path.clone(),
                "W1001",
                format!(
                    "gap in {:?}: … and {} more gap(s) in the observed run {min}..{max}",
                    anchor.id,
                    runs.len() - shown
                ),
            ));
        }
    }
}

/// `[3, 4, 5, 9]` becomes `[(3, 5), (9, 9)]`.
fn collapse_runs(values: &[i64]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for &v in values {
        match out.last_mut() {
            Some(last) if last.1 + 1 == v => last.1 = v,
            _ => out.push((v, v)),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Check 7, probe half
// ---------------------------------------------------------------------------------------

/// Without the `probe` feature the byte-reading checks do not exist at all — not as a disabled
/// branch, not as an unused import. A default build must compile with no decoder present.
#[cfg(not(feature = "probe"))]
fn probe_checks(_archive: &Archive, _report: &mut Report) {}

#[cfg(feature = "probe")]
fn probe_checks(archive: &Archive, report: &mut Report) {
    use probe::{ImageProbe, actual_name, image_dimensions, is_lfs_pointer, pdf_page_count};

    // -- E702 / W705 / E707 on graphics ---------------------------------------------------
    let mut examined: BTreeSet<PathBuf> = BTreeSet::new();

    for r in graphic_refs(archive) {
        let path = r.graphic.file.as_path();
        if !path.exists() {
            // Already `E701`.
            continue;
        }

        // E707 — the filesystem here is case-insensitive, so `turgot_00.JP2` opens a file named
        // `turgot_00.jp2` and would silently break on a case-sensitive checkout.
        if examined.insert(path.to_path_buf())
            && let Some(actual) = actual_name(path)
        {
            report.push(
                Diagnostic::error(
                    r.node.rel_path.clone(),
                    "E707",
                    format!(
                        "graphic file {:?} differs in case from the on-disk name {actual:?}; this \
                         resolves here but will not on a case-sensitive filesystem",
                        r.graphic.file_raw
                    ),
                )
                .at(r.locator()),
            );
        }

        let (Some(width), Some(height)) = (r.graphic.width, r.graphic.height) else {
            continue;
        };
        // A graphic that names a page inside a container describes that page's pixel space, not
        // the container's; verifying it needs a renderer, which is out of scope for a linter.
        if r.graphic.page.is_some() {
            continue;
        }

        match image_dimensions(path) {
            ImageProbe::Dimensions {
                width: aw,
                height: ah,
            } => {
                if aw != width || ah != height {
                    report.push(
                        Diagnostic::error(
                            r.node.rel_path.clone(),
                            "E702",
                            format!(
                                "graphic {:?} is {aw}x{ah}, but declares {width}x{height}",
                                r.graphic.file_raw
                            ),
                        )
                        .at(r.locator()),
                    );
                }
            }
            ImageProbe::LfsPointer => report.push(
                Diagnostic::warning(
                    r.node.rel_path.clone(),
                    "W705",
                    format!(
                        "graphic {:?} is an unfetched Git LFS pointer; dimensions not verified — \
                         run `git lfs pull` to check it",
                        r.graphic.file_raw
                    ),
                )
                .at(r.locator()),
            ),
            ImageProbe::Unsupported(reason) | ImageProbe::Failed(reason) => report.push(
                Diagnostic::warning(
                    r.node.rel_path.clone(),
                    "W705",
                    format!(
                        "graphic {:?}: dimensions not verified ({reason})",
                        r.graphic.file_raw
                    ),
                )
                .at(r.locator()),
            ),
        }
    }

    // -- E704: the container's real page count --------------------------------------------
    for node in archive.iter() {
        // `scan.count` never inherits, so this runs once per record that states one.
        let Some(declared) = node.resolved.scan_count else {
            continue;
        };
        let Some(path) = node.resolved.scan_file_path() else {
            continue;
        };
        if !path.exists() {
            continue;
        }
        if is_lfs_pointer(&path) {
            report.push(Diagnostic::warning(
                node.rel_path.clone(),
                "W705",
                format!(
                    "scan.file {:?} is an unfetched Git LFS pointer; scan.count not verified",
                    rel(archive, &path)
                ),
            ));
            continue;
        }
        match pdf_page_count(&path) {
            Ok(actual) if actual != declared => report.push(Diagnostic::error(
                node.rel_path.clone(),
                "E704",
                format!(
                    "scan.file {:?} has {actual} page(s), but scan.count = {declared}",
                    rel(archive, &path)
                ),
            )),
            Ok(_) => {}
            Err(reason) => report.push(Diagnostic::warning(
                node.rel_path.clone(),
                "W705",
                format!(
                    "scan.file {:?}: page count not verified ({reason})",
                    rel(archive, &path)
                ),
            )),
        }
    }

    // -- E706: text files ------------------------------------------------------------------
    for node in archive.iter() {
        let mut texts: Vec<(String, &Text)> = node
            .record
            .text()
            .iter()
            .enumerate()
            .map(|(i, t)| (format!("[[text]]#{i}"), t))
            .collect();
        for page in &node.pages {
            for (i, t) in page.texts.iter().enumerate() {
                texts.push((format!("[[page]]#{} text#{i}", page.source_index), t));
            }
        }

        for (locator, text) in texts {
            // Text paths are never inherited, so they resolve against this file's own directory.
            let path = normalise(&node.dir().join(&text.file));
            if !path.exists() {
                report.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E706",
                        format!(
                            "text file {:?} does not exist (resolved to {})",
                            text.file,
                            rel(archive, &path)
                        ),
                    )
                    .at(&locator),
                );
            } else if let Some(actual) = actual_name(&path) {
                report.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E707",
                        format!(
                            "text file {:?} differs in case from the on-disk name {actual:?}",
                            text.file
                        ),
                    )
                    .at(&locator),
                );
            }
        }
    }
}

/// Byte-reading helpers. Compiled only under the `probe` feature.
///
/// Image dimensions are read from file headers by hand rather than through a decoder crate.
/// The archive's images are JPEG 2000, which no mainstream Rust image crate reads, so a
/// dependency would add weight without covering the one format that actually matters — and a
/// header read is a few dozen bytes where a decode is a 24000x17000 raster.
#[cfg(feature = "probe")]
mod probe {
    use std::io::Read;
    use std::path::Path;

    /// Git LFS pointer files begin with this line.
    const LFS_MAGIC: &[u8] = b"version https://git-lfs.github.com/spec/v1";

    /// Enough to cover the header of every format below, and small enough that reading it from
    /// a 60 MB `.jp2` is free.
    const HEADER_BYTES: usize = 256 * 1024;

    /// Guard against reading a plausible-looking pair of bytes as a real image size.
    const MAX_PLAUSIBLE_PIXELS: i64 = 1_000_000;

    pub enum ImageProbe {
        Dimensions {
            width: i64,
            height: i64,
        },
        /// The file is present but is an unfetched LFS pointer, so its dimensions are
        /// unverifiable rather than wrong.
        LfsPointer,
        /// A format this reader does not know.
        Unsupported(String),
        /// Present and known, but the header did not yield a size.
        Failed(String),
    }

    fn read_prefix(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
        let file = std::fs::File::open(path)?;
        let mut buffer = Vec::new();
        std::io::BufReader::new(file)
            .take(limit as u64)
            .read_to_end(&mut buffer)?;
        Ok(buffer)
    }

    pub fn is_lfs_pointer(path: &Path) -> bool {
        read_prefix(path, LFS_MAGIC.len()).is_ok_and(|b| b == LFS_MAGIC)
    }

    /// The on-disk spelling of `path`'s file name, when it differs from what was written.
    ///
    /// Returns `None` when the case matches, when the parent cannot be read, or when the name
    /// is not there at all (which is `E701`'s business, not this function's).
    pub fn actual_name(path: &Path) -> Option<String> {
        let name = path.file_name()?.to_str()?;
        let parent = path.parent()?;
        let mut case_insensitive_match = None;
        for entry in std::fs::read_dir(parent).ok()? {
            let entry = entry.ok()?;
            let found = entry.file_name();
            let found = found.to_str()?;
            if found == name {
                return None;
            }
            if found.eq_ignore_ascii_case(name) {
                case_insensitive_match = Some(found.to_string());
            }
        }
        case_insensitive_match
    }

    pub fn image_dimensions(path: &Path) -> ImageProbe {
        let bytes = match read_prefix(path, HEADER_BYTES) {
            Ok(b) => b,
            Err(e) => return ImageProbe::Failed(format!("cannot read: {e}")),
        };
        if bytes.starts_with(LFS_MAGIC) {
            return ImageProbe::LfsPointer;
        }

        let found = jp2(&bytes)
            .or_else(|| j2k(&bytes))
            .or_else(|| png(&bytes))
            .or_else(|| gif(&bytes))
            .or_else(|| bmp(&bytes))
            .or_else(|| webp(&bytes))
            .or_else(|| jpeg(&bytes));

        match found {
            Some((width, height)) if plausible(width, height) => {
                ImageProbe::Dimensions { width, height }
            }
            Some((width, height)) => ImageProbe::Failed(format!(
                "header gave an implausible size of {width}x{height}"
            )),
            None => ImageProbe::Unsupported(format!(
                "no header reader for {}",
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| format!(".{e}"))
                    .unwrap_or_else(|| "this file".to_string())
            )),
        }
    }

    fn plausible(width: i64, height: i64) -> bool {
        (1..=MAX_PLAUSIBLE_PIXELS).contains(&width) && (1..=MAX_PLAUSIBLE_PIXELS).contains(&height)
    }

    fn be32(bytes: &[u8], at: usize) -> Option<i64> {
        let slice: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(slice) as i64)
    }

    fn be16(bytes: &[u8], at: usize) -> Option<i64> {
        let slice: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(u16::from_be_bytes(slice) as i64)
    }

    fn le16(bytes: &[u8], at: usize) -> Option<i64> {
        let slice: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(u16::from_le_bytes(slice) as i64)
    }

    fn le32(bytes: &[u8], at: usize) -> Option<i64> {
        let slice: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(i32::from_le_bytes(slice) as i64)
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// JP2, the boxed container format. The `ihdr` box states height then width, in that order.
    fn jp2(bytes: &[u8]) -> Option<(i64, i64)> {
        const SIGNATURE: &[u8] = &[0x00, 0x00, 0x00, 0x0C, 0x6A, 0x50, 0x20, 0x20];
        if !bytes.starts_with(SIGNATURE) {
            return None;
        }
        let at = find(bytes, b"ihdr")? + 4;
        let height = be32(bytes, at)?;
        let width = be32(bytes, at + 4)?;
        Some((width, height))
    }

    /// A raw JPEG 2000 codestream: SOC then SIZ, whose `Xsiz`/`XOsiz` pairs give the size.
    fn j2k(bytes: &[u8]) -> Option<(i64, i64)> {
        if !bytes.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
            return None;
        }
        // 4 marker bytes, then Lsiz (2) and Rsiz (2).
        let at = 8;
        let xsiz = be32(bytes, at)?;
        let ysiz = be32(bytes, at + 4)?;
        let x_offset = be32(bytes, at + 8)?;
        let y_offset = be32(bytes, at + 12)?;
        Some((xsiz - x_offset, ysiz - y_offset))
    }

    fn png(bytes: &[u8]) -> Option<(i64, i64)> {
        if !bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            return None;
        }
        Some((be32(bytes, 16)?, be32(bytes, 20)?))
    }

    fn gif(bytes: &[u8]) -> Option<(i64, i64)> {
        if !bytes.starts_with(b"GIF87a") && !bytes.starts_with(b"GIF89a") {
            return None;
        }
        Some((le16(bytes, 6)?, le16(bytes, 8)?))
    }

    fn bmp(bytes: &[u8]) -> Option<(i64, i64)> {
        if !bytes.starts_with(b"BM") {
            return None;
        }
        // Height is signed and negative for a top-down bitmap.
        Some((le32(bytes, 18)?, le32(bytes, 22)?.abs()))
    }

    fn webp(bytes: &[u8]) -> Option<(i64, i64)> {
        if !bytes.starts_with(b"RIFF") || bytes.get(8..12)? != b"WEBP" {
            return None;
        }
        match bytes.get(12..16)? {
            b"VP8 " => Some((le16(bytes, 26)? & 0x3FFF, le16(bytes, 28)? & 0x3FFF)),
            b"VP8L" => {
                let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
                Some((
                    ((bits & 0x3FFF) + 1) as i64,
                    (((bits >> 14) & 0x3FFF) + 1) as i64,
                ))
            }
            b"VP8X" => {
                // Two 24-bit little-endian values, each stored as "size minus one".
                let le24 = |at: usize| -> Option<i64> {
                    Some(
                        *bytes.get(at)? as i64
                            + ((*bytes.get(at + 1)? as i64) << 8)
                            + ((*bytes.get(at + 2)? as i64) << 16)
                            + 1,
                    )
                };
                Some((le24(24)?, le24(27)?))
            }
            _ => None,
        }
    }

    /// Walk the marker segments to the first start-of-frame, which carries height then width.
    fn jpeg(bytes: &[u8]) -> Option<(i64, i64)> {
        if !bytes.starts_with(&[0xFF, 0xD8]) {
            return None;
        }
        let mut at = 2;
        while at + 4 < bytes.len() {
            if bytes[at] != 0xFF {
                at += 1;
                continue;
            }
            let marker = bytes[at + 1];
            // Standalone markers carry no length.
            if matches!(marker, 0xD8 | 0xD9 | 0xFF | 0x01) || (0xD0..=0xD7).contains(&marker) {
                at += 2;
                continue;
            }
            let length = be16(bytes, at + 2)? as usize;
            // SOF0..SOF15, excluding the DHT/JPG/DAC markers that share the range.
            let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
            if is_sof {
                return Some((be16(bytes, at + 7)?, be16(bytes, at + 5)?));
            }
            at += 2 + length;
        }
        None
    }

    /// The container's real page count.
    ///
    /// `lopdf` rather than a scan for `/Type /Page`, because a PDF that stores its page objects
    /// in compressed object streams yields zero to that scan — turning an unreadable file into
    /// a confident, wrong error.
    pub fn pdf_page_count(path: &Path) -> Result<i64, String> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension != "pdf" {
            return Err(format!("no page-count reader for .{extension} containers"));
        }
        let document =
            lopdf::Document::load(path).map_err(|e| format!("cannot read the PDF: {e}"))?;
        Ok(document.get_pages().len() as i64)
    }
}

// ---------------------------------------------------------------------------------------
// Check 11 — the OCR sidecars.
//
// An `.ocr.toml` is archive content but not a record: it has no `layer`, so `load` skips it
// and it is reached instead through the `[[text]]` that points at it. That makes this the
// only place anything looks at one, and the checks are correspondingly structural.
//
// ## Why the content half is behind `--probe`
//
// `E708` is a `stat` and runs always. Everything below it has to parse the sidecar, and the
// frc corpus is 840,810 of them — parsing all of that turns `validate` from a six-second
// command into a much longer one. That is the same reason `--probe` exists for the image
// checks: the default path must stay fast enough to run on every save.
// ---------------------------------------------------------------------------------------

fn check_11_ocr(archive: &Archive, options: &Options, report: &mut Report) {
    for node in archive.iter() {
        for (i, text) in node.record.text().iter().enumerate() {
            if !text.file.ends_with(crate::ocr::SUFFIX) {
                continue;
            }
            let locator = format!("[[text]]#{}", i + 1);
            let dir = node.path.parent().unwrap_or(&archive.root);
            let path = dir.join(&text.file);

            if !path.exists() {
                report.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E708",
                        format!(
                            "OCR sidecar {:?} does not exist (resolved to {})",
                            text.file,
                            rel(archive, &path)
                        ),
                    )
                    .at(locator.clone()),
                );
                continue;
            }

            if !options.probe {
                continue;
            }

            let raw = match std::fs::read_to_string(&path) {
                Ok(raw) => raw,
                Err(e) => {
                    report.push(
                        Diagnostic::error(
                            node.rel_path.clone(),
                            "E708",
                            format!("OCR sidecar {:?} cannot be read: {e}", text.file),
                        )
                        .at(locator),
                    );
                    continue;
                }
            };

            let ocr: crate::ocr::Ocr = match crate::ocr::from_markdown(&raw) {
                Ok(ocr) => ocr,
                Err(e) => {
                    report.push(Diagnostic::error(
                        rel(archive, &path),
                        "E709",
                        format!("not a valid OCR sidecar: {e}"),
                    ));
                    continue;
                }
            };

            // `of` is what ties the sidecar back to the record. A sidecar pointing at some
            // other id is a file that has been moved or copied without being updated, and the
            // OCR it holds is then attached to the wrong pamphlet.
            if ocr.of != node.id {
                report.push(Diagnostic::error(
                    rel(archive, &path),
                    "E710",
                    format!(
                        "of = {:?}, but this sidecar is pointed at by {:?}",
                        ocr.of, node.id
                    ),
                ));
            }

            // The page number is in the filename and in the frontmatter, and a mismatch means
            // one of them is lying about which page this is — which silently misplaces every
            // coordinate on it.
            let expected = crate::ocr::file_name(&ocr.of, ocr.page);
            if !text.file.ends_with(&expected) {
                report.push(Diagnostic::error(
                    rel(archive, &path),
                    "E712",
                    format!(
                        "page = {}, which does not match the filename; expected {expected}",
                        ocr.page
                    ),
                ));
            }
        }
    }
}
