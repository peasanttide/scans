//! Recover issue documents from a bound volume's PDF text layer.
//!
//! A serial arrives as one PDF per bound volume, and the issues inside it are not recorded
//! anywhere — only the volume is. This module reads the text layer, finds where each issue
//! begins, and writes one `layer = "document"` record per issue.
//!
//! # Why this can be trusted
//!
//! OCR of an eighteenth-century paper is not reliable enough to be believed on its own: the
//! Journal de Paris volume 1 header for issue 5 reads `NUMERO 25`, and one page yields
//! `NUMERO 807179`. Reading the number off the page and writing it down would corrupt the
//! archive quietly.
//!
//! So nothing here rests on a single reading. Four independent signals say what an issue is:
//!
//! 1. the number OCR'd from its header, `NUMÉRO 3`;
//! 2. the date OCR'd from the same header, `Samedi 3 JANVIER 1789`;
//! 3. its position in the page grid — 168 of 190 spans in volume 1 are exactly four pages;
//! 4. the serial's own rule, recorded in `journal-de-paris.toml`: the issue number is the
//!    day of the year, so no. 3 *must* be 3 January.
//!
//! A record is emitted as certain when at least two agree, and flagged in its `note` when
//! they do not. [`Report::uncertain`] is the worklist to check against the scan.
//!
//! # Feature `ingest`
//!
//! Off by default: this reads PDF bytes, and the default `validate` path must never do that.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::load::Archive;
use crate::model::Layer;

/// A calendar date. Local to this module: [`crate::edtf`] models EDTF strings, which are a
/// wider language than the single days this arithmetic needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub y: i32,
    pub m: u32,
    pub d: u32,
}

impl Date {
    fn leap(y: i32) -> bool {
        (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
    }

    fn days_in(y: i32, m: u32) -> u32 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if Self::leap(y) => 29,
            2 => 28,
            _ => 0,
        }
    }

    /// 1 January is day 1.
    pub fn day_of_year(self) -> i64 {
        let mut n = self.d as i64;
        for m in 1..self.m {
            n += Self::days_in(self.y, m) as i64;
        }
        n
    }

    /// Inverse of [`Self::day_of_year`]. Returns `None` when the day falls outside the year.
    pub fn from_day_of_year(y: i32, mut doy: i64) -> Option<Self> {
        if doy < 1 {
            return None;
        }
        for m in 1..=12u32 {
            let len = Self::days_in(y, m) as i64;
            if doy <= len {
                return Some(Date { y, m, d: doy as u32 });
            }
            doy -= len;
        }
        None
    }

    pub fn edtf(self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }
}

const MONTHS: [(&str, u32); 12] = [
    ("JANVIER", 1),
    ("FEVRIER", 2),
    ("MARS", 3),
    ("AVRIL", 4),
    ("MAI", 5),
    ("JUIN", 6),
    ("JUILLET", 7),
    ("AOUT", 8),
    ("SEPTEMBRE", 9),
    ("OCTOBRE", 10),
    ("NOVEMBRE", 11),
    ("DECEMBRE", 12),
];

/// Strip accents and upper-case, so `Décembre`, `DÉCEMBRE` and a mangled `DECEMBRE` all match.
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'E',
            'à' | 'â' | 'ä' | 'À' | 'Â' | 'Ä' => 'A',
            'û' | 'ù' | 'ü' | 'Û' | 'Ù' | 'Ü' => 'U',
            'î' | 'ï' | 'Î' | 'Ï' => 'I',
            'ô' | 'ö' | 'Ô' | 'Ö' => 'O',
            'ç' | 'Ç' => 'C',
            'ſ' => 'S',
            // The OCR reaches for Greek and Cyrillic lookalikes: volume 1 page 749 reads
            // `NUMERο 154` with a Greek omicron, page 741 `NUMER • 152` with a bullet.
            'ο' | 'Ο' | 'о' | 'О' => 'O',
            'ε' | 'е' | 'Е' => 'E',
            'α' | 'а' | 'А' => 'A',
            'ρ' | 'р' | 'Р' => 'P',
            c => c.to_ascii_uppercase(),
        })
        .collect()
}

/// Parse an EDTF closed day interval, `1789-01-01/1789-06-30`.
///
/// Deliberately narrow: ingest needs two concrete days to count between, so an approximate
/// or open-ended interval is rejected rather than guessed at.
fn parse_interval(s: &str) -> Option<(Date, Date)> {
    let (a, b) = s.split_once('/')?;
    Some((parse_day(a)?, parse_day(b)?))
}

fn parse_day(s: &str) -> Option<Date> {
    let mut it = s.trim().splitn(3, '-');
    let y = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || d < 1 || d > Date::days_in(y, m) {
        return None;
    }
    Some(Date { y, m, d })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Issue,
    Supplement,
}

/// A header found in the text layer, before any reconciliation.
#[derive(Debug, Clone)]
struct Head {
    /// 1-based index into the PDF.
    page: i64,
    kind: Kind,
    ocr_no: Option<i64>,
    ocr_date: Option<Date>,
}

/// Read the first `LOOK` characters of a page and decide whether an issue starts there.
const LOOK: usize = 160;

fn find_head(text: &str, year: i32) -> Option<(Kind, Option<i64>, Option<Date>)> {
    let folded = fold(text);
    let head: String = folded.chars().take(LOOK).collect();

    // The masthead is the discriminator, and it is what makes this safe to automate.
    // `NUMERO` alone is not enough: a continuation page carrying a figure like `25250` in
    // running text matches it, and three consecutive false heads in volume 1 turned into
    // one-page issues that stole their real issue's number. Every genuine header — issue or
    // supplement — reprints the masthead. Whitespace is dropped because the OCR runs words
    // together as readily as it splits them.
    // `JOURNAL` only, not the full `JOURNAL DE PARIS`. Requiring `PARIS` too looks safer and
    // is not: the supplement to no. 76 is OCR'd as `SUPPLEMENT AU Nº. 76 DU JOURNAL DE
    // Mardi 17 Mars 1789`, with the city dropped. Rejecting it merged that supplement into
    // issue 76's span, and the grid then split the 8 pages into two issues, one of which
    // collided with the real no. 77 on the next page.
    let squashed: String = head.chars().filter(|c| !c.is_whitespace()).collect();
    if !squashed.contains("JOURNAL") {
        return None;
    }

    // `NUMER`, not `NUMERO`: the final letter is the one the OCR loses, to a bullet or a
    // Greek omicron. The stem is safe to match on because the masthead test above has
    // already ruled out running text.
    let kind = if head.contains("SUPPLEMENT") {
        Kind::Supplement
    } else if head.contains("NUMER") {
        Kind::Issue
    } else {
        return None;
    };

    // The number after NUMERO / AU N°. The OCR loses spaces freely, so take the first run of
    // digits after the keyword.
    let anchor = if kind == Kind::Supplement {
        head.find("SUPPLEMENT").map(|i| i + "SUPPLEMENT".len())
    } else {
        head.find("NUMER").map(|i| i + "NUMER".len())
    };
    let ocr_no = anchor.and_then(|start| {
        let rest = &head[start..];
        let digits: String = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<i64>().ok()
    });

    // `Samedi3 JANVIER1789` — day, month name, year, with spacing anywhere or nowhere.
    let mut ocr_date = None;
    for (name, m) in MONTHS {
        let Some(at) = head.find(name) else { continue };
        let before: String = head[..at].chars().rev().take(12).collect();
        let day: String = before
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let day: String = day.chars().rev().collect();
        if let Ok(d) = day.parse::<u32>()
            && (1..=31).contains(&d)
        {
            ocr_date = Some(Date { y: year, m, d });
        }
        break;
    }

    Some((kind, ocr_no, ocr_date))
}

/// What the reconciler concluded about one issue, and how sure it is.
#[derive(Debug, Clone)]
pub struct Issue {
    pub no: i64,
    pub date: Date,
    pub kind: Kind,
    /// `supplement_to` for a supplement.
    pub parent: Option<i64>,
    pub from: i64,
    pub to: i64,
    /// Signals that agreed. See the module docs.
    pub by_number: bool,
    pub by_date: bool,
    pub by_grid: bool,
    /// Set when the record had to be inferred; becomes the `note` and the worklist entry.
    pub inferred: Option<String>,
}

impl Issue {
    pub fn certain(&self) -> bool {
        [self.by_number, self.by_date, self.by_grid]
            .iter()
            .filter(|b| **b)
            .count()
            >= 2
    }

    fn id(&self, stem: &str) -> String {
        let base = format!("{stem}-{:04}-{:02}-{:02}", self.date.y, self.date.m, self.date.d);
        match self.kind {
            Kind::Issue => base,
            Kind::Supplement => format!("{base}-supplement"),
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub written: Vec<PathBuf>,
    pub issues: Vec<Issue>,
    pub uncertain: Vec<Issue>,
    pub notes: Vec<String>,
    /// How many issues the copy's `covers` interval implies. A shortfall against it is the
    /// operator's cue that a header went unread.
    ///
    /// There is deliberately no expected-supplement count. The legacy `supplements = 24` on
    /// the volume was dropped in migration as "derivable", which was wrong - it is a count of
    /// what was physically bound in and follows from nothing else - and it has no slot in the
    /// schema to be read back from. Until it has one, this cross-check cannot be made.
    pub expected_issues: Option<i64>,
}

pub struct Options {
    pub copy_id: String,
    pub apply: bool,
    /// The regular span of one issue. Volume 1 of the Journal de Paris is four pages, 168
    /// spans out of 190.
    pub pages_per_issue: i64,
}

/// Extract the text layer, one entry per PDF page, 1-based.
#[cfg(feature = "ingest")]
fn page_texts(pdf: &Path) -> Result<Vec<String>> {
    let doc = lopdf::Document::load(pdf)
        .with_context(|| format!("reading the text layer of {}", pdf.display()))?;
    let mut pages: Vec<(u32, _)> = doc.get_pages().into_iter().collect();
    pages.sort_by_key(|(n, _)| *n);
    let mut out = Vec::with_capacity(pages.len());
    for (n, _) in pages {
        out.push(doc.extract_text(&[n]).unwrap_or_default());
    }
    Ok(out)
}

#[cfg(not(feature = "ingest"))]
fn page_texts(_pdf: &Path) -> Result<Vec<String>> {
    bail!("this build has no PDF support; rebuild with --features ingest")
}

pub fn ingest(root: &Path, archive: &Archive, opts: &Options) -> Result<Report> {
    let node = archive
        .by_id(&opts.copy_id)
        .with_context(|| format!("no record with id {:?}", opts.copy_id))?;

    if node.layer() != Layer::Copy {
        bail!(
            "{} is a {:?}, and issues are ingested from a copy — the physical object whose \
             pages they sit on",
            opts.copy_id,
            node.layer()
        );
    }

    let scan_file = node
        .resolved
        .scan_file
        .as_ref()
        .with_context(|| format!("{} declares no scan.file to read", opts.copy_id))?;
    // `scan.file` is relative to the file that declared it, which may be an ancestor.
    let pdf = scan_file.dir().join(&scan_file.value);

    let covers = node
        .record
        .covers()
        .with_context(|| format!("{} declares no covers interval", opts.copy_id))?;
    let (first, last) = parse_interval(covers)
        .with_context(|| format!("{} has covers = {covers:?}, which is not a day interval", opts.copy_id))?;
    let year = first.y;
    let lo = first.day_of_year();
    let hi = last.day_of_year();

    let texts = page_texts(&pdf)?;
    let mut report = Report {
        expected_issues: Some(hi - lo + 1),
        ..Report::default()
    };
    report.notes.push(format!(
        "{}: {} pdf pages, expecting issues {}..{} ({} to {})",
        pdf.display(),
        texts.len(),
        lo,
        hi,
        first.edtf(),
        last.edtf()
    ));

    // ---- 1. detect -------------------------------------------------------------------
    let mut heads: Vec<Head> = Vec::new();
    for (i, t) in texts.iter().enumerate() {
        if let Some((kind, ocr_no, ocr_date)) = find_head(t, year) {
            heads.push(Head { page: i as i64 + 1, kind, ocr_no, ocr_date });
        }
    }
    report
        .notes
        .push(format!("detected {} headers in the text layer", heads.len()));

    if heads.is_empty() {
        bail!(
            "no issue headers found in {} — the PDF may have no text layer",
            pdf.display()
        );
    }

    // ---- 2. resolve each issue head to a number --------------------------------------
    // Take the OCR number only when it lands in range; otherwise fall back to the date, then
    // to sequence. Every fallback is recorded so it reaches the worklist.
    let mut resolved: Vec<Issue> = Vec::new();
    let mut prev_no: Option<i64> = None;

    for (idx, h) in heads.iter().enumerate() {
        let end = heads
            .get(idx + 1)
            .map(|n| n.page - 1)
            .unwrap_or(texts.len() as i64);

        if h.kind == Kind::Supplement {
            let parent = prev_no;
            let date = h
                .ocr_date
                .or_else(|| parent.and_then(|p| Date::from_day_of_year(year, p)));
            let Some(date) = date else { continue };
            resolved.push(Issue {
                no: parent.unwrap_or(0),
                date,
                kind: Kind::Supplement,
                parent,
                from: h.page,
                to: end,
                by_number: h.ocr_no.is_some_and(|n| Some(n) == parent),
                by_date: h.ocr_date.is_some(),
                by_grid: true,
                inferred: None,
            });
            continue;
        }

        let from_date = h.ocr_date.map(|d| d.day_of_year());
        let in_range = |n: i64| (lo..=hi).contains(&n);
        // Issues are bound in the order they were published, one per day. Once the sequence
        // is anchored it is the most reliable signal there is — far more so than a number
        // OCR'd off a worn eighteenth-century masthead, which reads `5` as `25` and `11` as
        // `49`. So the expected number leads, and the OCR only ever confirms it.
        let expected = prev_no.map(|p| p + 1).unwrap_or(lo);

        let (no, by_number, by_date, why) = match from_date {
            // A printed date outranks even the sequence: it is a long, redundant string, and
            // the serial's day-of-year rule ties it directly to the issue number. If it
            // disagrees with the sequence, an issue is genuinely missing from the volume.
            Some(b) if in_range(b) => {
                let why = (b != expected).then(|| {
                    format!(
                        "the printed date gives issue {b}, but {expected} was expected here; \
                         an issue may be missing from the volume"
                    )
                });
                (b, h.ocr_no == Some(b), true, why)
            }
            _ => {
                let why = match h.ocr_no {
                    Some(a) if a != expected => Some(format!(
                        "header number OCR'd as {a}, which does not fit the sequence and has \
                         no printed date to confirm it; took {expected} from the preceding issue"
                    )),
                    None => Some(format!(
                        "no header number and no printed date could be read; took {expected} \
                         from the preceding issue"
                    )),
                    _ => None,
                };
                (expected, h.ocr_no == Some(expected), false, why)
            }
        };

        if !in_range(no) {
            continue;
        }
        let by_grid = no == expected;

        let Some(date) = Date::from_day_of_year(year, no) else {
            continue;
        };
        prev_no = Some(no);
        resolved.push(Issue {
            no,
            date,
            kind: Kind::Issue,
            parent: None,
            from: h.page,
            to: end,
            by_number,
            by_date,
            by_grid,
            inferred: why,
        });
    }

    // ---- 3. fill the gaps using the page grid ----------------------------------------
    // A span several times the regular length is a header the OCR failed to read, not a
    // genuinely long issue. Split it.
    let per = opts.pages_per_issue;
    let mut filled: Vec<Issue> = Vec::new();
    for iss in resolved.iter() {
        let span = iss.to - iss.from + 1;
        if iss.kind == Kind::Issue && per > 0 && span > per && span % per == 0 {
            let parts = span / per;
            for k in 0..parts {
                let no = iss.no + k;
                // The grid must not invent an issue the volume does not contain. Volume 1
                // ends at 181; without this its trailing span produced a 1 July issue.
                if !(lo..=hi).contains(&no) {
                    continue;
                }
                let Some(date) = Date::from_day_of_year(year, no) else {
                    continue;
                };
                if k == 0 {
                    let mut first_part = iss.clone();
                    first_part.to = iss.from + per - 1;
                    filled.push(first_part);
                } else {
                    filled.push(Issue {
                        no,
                        date,
                        kind: Kind::Issue,
                        parent: None,
                        from: iss.from + k * per,
                        to: iss.from + (k + 1) * per - 1,
                        by_number: false,
                        by_date: false,
                        by_grid: true,
                        inferred: Some(format!(
                            "no header was read on this page; recovered from the {per}-page \
                             grid, between issues {} and {}",
                            iss.no,
                            iss.no + parts - 1
                        )),
                    });
                }
            }
        } else {
            filled.push(iss.clone());
        }
    }

    // The fill inserts issues after numbering, so an inserted record can land on the number
    // the next real header already took. Re-derive the sequence over the filled list: a
    // date-confirmed record is an anchor and keeps its number, everything else follows on
    // from whatever precedes it.
    let mut expected: Option<i64> = None;
    for iss in filled.iter_mut() {
        if iss.kind != Kind::Issue {
            continue;
        }
        let no = match (iss.by_date, expected) {
            (true, _) => iss.no,
            (false, Some(prev)) => prev + 1,
            (false, None) => iss.no,
        };
        if no != iss.no
            && (lo..=hi).contains(&no)
            && let Some(date) = Date::from_day_of_year(year, no)
        {
            iss.no = no;
            iss.date = date;
            iss.by_number = false;
        }
        expected = Some(iss.no);
    }

    // Renumber supplements onto whatever issue actually precedes them now.
    let mut last_issue: Option<i64> = None;
    for iss in filled.iter_mut() {
        match iss.kind {
            Kind::Issue => last_issue = Some(iss.no),
            Kind::Supplement => {
                if let Some(p) = last_issue {
                    iss.parent = Some(p);
                    iss.no = p;
                }
            }
        }
    }

    // Two issues cannot share a number. Where it happens one of them is a misread, and
    // neither can be trusted until a human says which — so both go on the worklist.
    let mut counts: BTreeMap<(i64, bool), usize> = BTreeMap::new();
    for iss in &filled {
        *counts
            .entry((iss.no, iss.kind == Kind::Supplement))
            .or_default() += 1;
    }
    for iss in filled.iter_mut() {
        if counts
            .get(&(iss.no, iss.kind == Kind::Supplement))
            .copied()
            .unwrap_or(0)
            > 1
        {
            let msg = format!("number {} is claimed by more than one record", iss.no);
            iss.inferred = Some(match iss.inferred.take() {
                Some(existing) => format!("{existing}; {msg}"),
                None => msg,
            });
            iss.by_number = false;
            iss.by_grid = false;
        }
    }

    for iss in &filled {
        report.issues.push(iss.clone());
        if !iss.certain() {
            report.uncertain.push(iss.clone());
        }
    }

    // ---- 4. emit ---------------------------------------------------------------------
    // Documents are named after the serial, not the volume: the 3 January issue is
    // `journal-de-paris-1789-01-03`, not `…-vol1-01-03`.
    let stem = node
        .record
        .of()
        .map(str::to_owned)
        .unwrap_or_else(|| opts.copy_id.clone());
    let dir = Path::new(&node.rel_path)
        .parent()
        .unwrap_or(Path::new(""))
        .to_path_buf();

    let mut seen: BTreeMap<String, i64> = BTreeMap::new();
    for iss in &filled {
        let mut id = iss.id(&stem);
        // Two supplements to one issue happen; keep both rather than overwrite.
        let n = seen.entry(id.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            id = format!("{id}-{n}");
        }

        let rel = dir
            .join(format!("{:02}", iss.date.m))
            .join(format!("{:02}", iss.date.d))
            .join(format!("{id}.toml"));

        let text = render(&id, iss, &opts.copy_id, &stem, &rel);
        if opts.apply {
            let abs = root.join(&rel);
            if let Some(p) = abs.parent() {
                std::fs::create_dir_all(p)
                    .with_context(|| format!("creating {}", p.display()))?;
            }
            std::fs::write(&abs, text).with_context(|| format!("writing {}", abs.display()))?;
        }
        report.written.push(rel);
    }

    Ok(report)
}

/// One document record. Written by hand rather than through `toml_edit` so the column
/// alignment matches the records the migration produced.
fn render(id: &str, iss: &Issue, copy_id: &str, stem: &str, rel: &Path) -> String {
    let depth = rel.components().count().saturating_sub(1);
    let up = "../".repeat(depth);
    let mut s = String::new();
    let _ = writeln!(s, "#:schema {up}schemas/source.json");
    let _ = writeln!(s, "id    = \"{id}\"");
    let _ = writeln!(s, "layer = \"document\"");
    let _ = writeln!(s, "of    = \"{copy_id}\"");
    match iss.kind {
        Kind::Issue => {
            let _ = writeln!(s, "type  = \"issue\"");
            let _ = writeln!(s, "no    = {}", iss.no);
        }
        Kind::Supplement => {
            let _ = writeln!(s, "type  = \"supplement\"");
            if let Some(p) = iss.parent
                && let Some(d) = Date::from_day_of_year(iss.date.y, p)
            {
                // Points at an id, never a bare integer.
                let _ = writeln!(
                    s,
                    "supplement_to = \"{stem}-{:04}-{:02}-{:02}\"",
                    d.y, d.m, d.d
                );
            }
        }
    }
    let _ = writeln!(s, "date  = \"{}\"", iss.date.edtf());
    let _ = writeln!(
        s,
        "pages = {{ from = {}, to = {} }}",
        iss.from, iss.to
    );
    if let Some(why) = &iss.inferred {
        let _ = writeln!(s, "note  = \"INFERRED: {}. Verify against the scan.\"", why);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_of_year_round_trips_across_a_non_leap_year() {
        for doy in 1..=365 {
            let d = Date::from_day_of_year(1789, doy).unwrap();
            assert_eq!(d.day_of_year(), doy, "{d:?}");
        }
        assert!(Date::from_day_of_year(1789, 366).is_none());
    }

    #[test]
    fn day_of_year_handles_leap_years() {
        assert!(Date::from_day_of_year(1788, 366).is_some());
        assert_eq!(
            Date::from_day_of_year(1788, 60).unwrap(),
            Date { y: 1788, m: 2, d: 29 }
        );
    }

    #[test]
    fn reads_a_real_issue_header() {
        let t = "NUMÉRO 3 . JOURNAL DE PARIS . Samedi3 JANVIER1789 , de la Lunele8.";
        let (kind, no, date) = find_head(t, 1789).unwrap();
        assert_eq!(kind, Kind::Issue);
        assert_eq!(no, Some(3));
        assert_eq!(date, Some(Date { y: 1789, m: 1, d: 3 }));
    }

    #[test]
    fn reads_a_real_supplement_header() {
        let t = "13 SUPPLÉMENT AU N °.3 DU JOURNAL DE PARIS. Samedi3 Janvier1789. ADMINISTRATION";
        let (kind, no, date) = find_head(t, 1789).unwrap();
        assert_eq!(kind, Kind::Supplement);
        assert_eq!(no, Some(3));
        assert_eq!(date, Some(Date { y: 1789, m: 1, d: 3 }));
    }

    #[test]
    fn reads_the_ordinal_first_issue() {
        let t = "NUMERO 1er : JOURNAL DE PARIS . Jeudi1\" JANVIER1789 , de laLune le6.";
        let (kind, no, date) = find_head(t, 1789).unwrap();
        assert_eq!(kind, Kind::Issue);
        assert_eq!(no, Some(1));
        assert_eq!(date, Some(Date { y: 1789, m: 1, d: 1 }));
    }

    #[test]
    fn a_continuation_page_is_not_a_header() {
        let t = "10 portion,d'aprèsdesconnoiſſancescertaines, ilſeroitévidemmentdéraisonnable";
        assert!(find_head(t, 1789).is_none());
    }

    #[test]
    fn a_number_in_running_text_is_not_a_header() {
        // Volume 1 produced three of these in a row. Without the masthead test each became a
        // one-page issue and stole the number of the real issue that followed.
        let t = "NUMERO 25250 de la souscription, & les fonds qui en proviennent ſeront verſés";
        assert!(find_head(t, 1789).is_none());
    }

    #[test]
    fn a_supplement_header_survives_the_ocr_dropping_paris() {
        // Volume 1 page 353, verbatim. Requiring the full `JOURNAL DE PARIS` rejected this
        // real header and corrupted three records downstream.
        let t = "351 SUPPLEMENT AU Nº. 76 DU JOURNAL DE Mardi 17 Mars 1789. LIMPOT ABONNÉ";
        let (kind, no, date) = find_head(t, 1789).unwrap();
        assert_eq!(kind, Kind::Supplement);
        assert_eq!(no, Some(76));
        assert_eq!(date, Some(Date { y: 1789, m: 3, d: 17 }));
    }

    #[test]
    fn the_masthead_alone_is_not_a_header_either() {
        let t = "JOURNAL DE PARIS. ſuite de la ſéance, où l'on a beaucoup diſcuté la queſtion";
        assert!(find_head(t, 1789).is_none());
    }

    #[test]
    fn two_agreeing_signals_are_certain_and_one_is_not() {
        let mut i = Issue {
            no: 3,
            date: Date { y: 1789, m: 1, d: 3 },
            kind: Kind::Issue,
            parent: None,
            from: 13,
            to: 16,
            by_number: true,
            by_date: true,
            by_grid: false,
            inferred: None,
        };
        assert!(i.certain());
        i.by_date = false;
        assert!(!i.certain());
    }
}
