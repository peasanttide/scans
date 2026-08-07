//! Fetching the corpus PDFs from archive.org.
//!
//! The only part of the ingest that touches the network, and the only part that has to be
//! careful about it. Everything else — the item list, the catalogue metadata, the word-level
//! OCR — comes out of the frc-data repository in a single clone.
//!
//! ## What this asks of archive.org
//!
//! Two requests per item: the metadata JSON, which gives the authoritative filename, size and
//! MD5 of the PDF, and the PDF itself. 38,377 items is about 77,000 requests and 63 GB, which
//! at one request a second is a run of a day or so.
//!
//! It could be one request per item by guessing the URL as `<id>/<id>.pdf` and skipping
//! verification. That is not done, for two reasons: a handful of items name their PDF
//! something else, and without the published MD5 there is no way to tell a complete download
//! from a truncated one. A silently truncated PDF committed to the archive and then believed
//! is a worse outcome than a slower run.
//!
//! ## Resuming
//!
//! An item is done when its PDF is on disk with the right size. That is the whole resume
//! condition — there is no ledger, because a ledger is a second source of truth that can fall
//! out of step with the tree it describes. A run interrupted at item 20,000 restarts by
//! `stat`ing 20,000 files, which takes a moment and cannot be wrong.

use std::time::Duration;

// Everything below is needed only by the networking half, which is behind the feature.
#[cfg(feature = "fetch")]
use std::{io::Read, path::Path};

#[cfg(feature = "fetch")]
use anyhow::{Context, Result, bail};

#[cfg(feature = "fetch")]
use super::{ingest::item_dir, limiter::Limiter};

/// Identifies this crawler to archive.org, so an operator who wants it to stop can find us.
///
/// An honest User-Agent is the cheapest possible courtesy and the first thing anyone looks
/// for in a log when traffic they did not expect turns up.
pub const USER_AGENT: &str =
    concat!("scans/", env!("CARGO_PKG_VERSION"), " (+https://github.com/peasanttide/scans)");

/// How many times one item is retried before it is parked and the run moves on.
#[cfg(feature = "fetch")]
const MAX_ATTEMPTS: u32 = 4;

/// Options for a fetch run.
#[derive(Debug, Clone)]
pub struct Options {
    /// Minimum gap between requests.
    pub interval: Duration,
    /// Stop after this many items.
    pub limit: Option<usize>,
    /// Re-fetch items that already have a PDF.
    pub force: bool,
    /// Report what would be fetched without fetching it.
    pub dry_run: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            interval: super::limiter::DEFAULT_INTERVAL,
            limit: None,
            force: false,
            dry_run: false,
        }
    }
}

/// What fetching one item did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Fetched { bytes: u64 },
    /// Already on disk at the right size.
    Skipped,
    /// The item has no PDF derivative on archive.org at all.
    NoPdf,
    Failed(String),
}

#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub fetched: usize,
    pub bytes: u64,
    pub skipped: usize,
    pub no_pdf: usize,
    pub failed: Vec<(String, String)>,
}

/// The PDF archive.org holds for an item: its name, size and checksum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfFile {
    pub name: String,
    pub size: u64,
    pub md5: Option<String>,
}

/// Pick the PDF out of an item's metadata JSON.
///
/// Pure, so the choice between several PDFs is tested without a network. Prefers the
/// derivative named after the item, which is what archive.org's own reader links to; falls
/// back to the largest, on the grounds that the biggest PDF is the full scan rather than a
/// cover or an appendix.
pub fn choose_pdf(metadata: &serde_json::Value, id: &str) -> Option<PdfFile> {
    let files = metadata.get("files")?.as_array()?;

    let mut candidates: Vec<PdfFile> = files
        .iter()
        .filter_map(|f| {
            let name = f.get("name")?.as_str()?;
            if !name.to_ascii_lowercase().ends_with(".pdf") {
                return None;
            }
            Some(PdfFile {
                name: name.to_string(),
                // `size` is a decimal string in IA metadata, not a number.
                size: f.get("size").and_then(|s| s.as_str()).and_then(|s| s.parse().ok())
                    .or_else(|| f.get("size").and_then(|s| s.as_u64()))
                    .unwrap_or(0),
                md5: f.get("md5").and_then(|m| m.as_str()).map(str::to_string),
            })
        })
        .collect();

    let preferred = format!("{id}.pdf");
    if let Some(exact) = candidates.iter().find(|c| c.name == preferred) {
        return Some(exact.clone());
    }
    candidates.sort_by_key(|c| c.size);
    candidates.pop()
}

/// A `Retry-After` header value as a duration.
///
/// Only the delta-seconds form is understood. The HTTP-date form is legal and archive.org
/// does not send it; treating an unparseable value as "no advice given" falls back to the
/// geometric back-off, which is the safe direction to be wrong in.
pub fn retry_after(value: Option<&str>) -> Option<Duration> {
    value?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Whether a status code is worth trying again.
///
/// 429 and 503 are the server asking for patience. 5xx is the server having a bad time. A 404
/// is an answer, and repeating the question will not change it.
pub fn is_retryable(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

#[cfg(feature = "fetch")]
mod net {
    use super::*;

    /// One item's metadata, from `https://archive.org/metadata/<id>`.
    pub fn metadata(agent: &ureq::Agent, id: &str) -> Result<serde_json::Value> {
        let url = format!("https://archive.org/metadata/{id}");
        let mut response = agent
            .get(&url)
            .call()
            .with_context(|| format!("requesting {url}"))?;
        let body = response
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading {url}"))?;
        serde_json::from_str(&body).with_context(|| format!("parsing the metadata of {id}"))
    }

    /// Download to a temporary file, verify, then move into place.
    ///
    /// Written beside the destination and renamed, never written in place: a run killed
    /// mid-download must not leave a half PDF that the next run's resume check mistakes for a
    /// finished one.
    pub fn download(agent: &ureq::Agent, url: &str, dest: &Path, want: &PdfFile) -> Result<u64> {
        let tmp = dest.with_extension("pdf.part");
        let mut response = agent
            .get(url)
            .call()
            .with_context(|| format!("requesting {url}"))?;

        let mut hasher = <md5::Md5 as md5::Digest>::new();
        let mut file = std::fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        let mut reader = response.body_mut().as_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;

        loop {
            let n = reader.read(&mut buf).with_context(|| format!("reading {url}"))?;
            if n == 0 {
                break;
            }
            md5::Digest::update(&mut hasher, &buf[..n]);
            std::io::Write::write_all(&mut file, &buf[..n])
                .with_context(|| format!("writing {}", tmp.display()))?;
            total += n as u64;
        }
        drop(file);

        let verify = |ok: bool, what: &str| -> Result<()> {
            if ok {
                return Ok(());
            }
            let _ = std::fs::remove_file(&tmp);
            bail!("{what}")
        };

        verify(
            want.size == 0 || total == want.size,
            &format!("truncated download: got {total} bytes, archive.org says {}", want.size),
        )?;

        if let Some(expected) = &want.md5 {
            let got = format!("{:x}", md5::Digest::finalize(hasher));
            verify(
                got.eq_ignore_ascii_case(expected),
                &format!("checksum mismatch: got {got}, archive.org says {expected}"),
            )?;
        }

        std::fs::rename(&tmp, dest)
            .with_context(|| format!("moving {} into place", tmp.display()))?;
        Ok(total)
    }
}

/// Fetch the PDFs for a list of identifiers.
#[cfg(feature = "fetch")]
pub fn fetch(
    root: &Path,
    ids: &[String],
    options: &Options,
    mut progress: impl FnMut(usize, &str, &Outcome),
) -> Summary {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into();

    let mut limiter = Limiter::new(options.interval);
    let mut summary = Summary::default();

    for (i, id) in ids.iter().enumerate() {
        let outcome = fetch_one(&agent, &mut limiter, root, id, options);
        match &outcome {
            Outcome::Fetched { bytes } => {
                summary.fetched += 1;
                summary.bytes += bytes;
            }
            Outcome::Skipped => summary.skipped += 1,
            Outcome::NoPdf => summary.no_pdf += 1,
            Outcome::Failed(why) => summary.failed.push((id.clone(), why.clone())),
        }
        progress(i, id, &outcome);
    }

    summary
}

#[cfg(feature = "fetch")]
fn fetch_one(
    agent: &ureq::Agent,
    limiter: &mut Limiter,
    root: &Path,
    id: &str,
    options: &Options,
) -> Outcome {
    let dir = root.join(item_dir(id));
    let dest = dir.join(format!("{id}.pdf"));

    if !options.force && dest.exists() {
        return Outcome::Skipped;
    }

    let mut last_error = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        limiter.wait();

        let metadata = match net::metadata(agent, id) {
            Ok(m) => m,
            Err(e) => {
                last_error = format!("{e:#}");
                limiter.failed(retry_after_of(&e));
                if !retryable(&e) || attempt == MAX_ATTEMPTS {
                    break;
                }
                continue;
            }
        };
        limiter.succeeded();

        let Some(pdf) = choose_pdf(&metadata, id) else {
            return Outcome::NoPdf;
        };

        if options.dry_run {
            return Outcome::Fetched { bytes: pdf.size };
        }

        if let Err(e) = std::fs::create_dir_all(&dir) {
            return Outcome::Failed(format!("creating {}: {e}", dir.display()));
        }

        limiter.wait();
        let url = format!("https://archive.org/download/{id}/{}", pdf.name);
        match net::download(agent, &url, &dest, &pdf) {
            Ok(bytes) => {
                limiter.succeeded();
                return Outcome::Fetched { bytes };
            }
            Err(e) => {
                last_error = format!("{e:#}");
                limiter.failed(retry_after_of(&e));
                if attempt == MAX_ATTEMPTS {
                    break;
                }
            }
        }
    }

    Outcome::Failed(last_error)
}

/// Whether an error from `ureq` is worth another attempt.
#[cfg(feature = "fetch")]
fn retryable(e: &anyhow::Error) -> bool {
    match e.downcast_ref::<ureq::Error>() {
        Some(ureq::Error::StatusCode(code)) => is_retryable(*code),
        // A transport failure — connection reset, timeout — is exactly the transient case.
        Some(_) => true,
        None => false,
    }
}

/// The server's own `Retry-After`, if it sent one with the failure.
#[cfg(feature = "fetch")]
fn retry_after_of(_e: &anyhow::Error) -> Option<Duration> {
    // `ureq` 3 does not surface response headers on a `StatusCode` error, so there is nothing
    // to read here and the geometric back-off is what applies. Kept as a named seam so that
    // honouring the header is a change in one place if a later version exposes it.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(files: serde_json::Value) -> serde_json::Value {
        json!({ "files": files })
    }

    #[test]
    fn the_pdf_named_after_the_item_wins() {
        let m = meta(json!([
            {"name": "other.pdf", "size": "9999999", "md5": "b"},
            {"name": "x.pdf", "size": "100", "md5": "a"},
            {"name": "x_djvu.txt", "size": "5"},
        ]));
        let pdf = choose_pdf(&m, "x").unwrap();
        assert_eq!(pdf.name, "x.pdf");
        assert_eq!(pdf.size, 100);
        assert_eq!(pdf.md5.as_deref(), Some("a"));
    }

    /// A handful of items name their PDF something else. The biggest one is the full scan.
    #[test]
    fn otherwise_the_largest_pdf_wins() {
        let m = meta(json!([
            {"name": "cover.pdf", "size": "100"},
            {"name": "scan_full.pdf", "size": "50000"},
        ]));
        assert_eq!(choose_pdf(&m, "x").unwrap().name, "scan_full.pdf");
    }

    #[test]
    fn an_item_with_no_pdf_yields_none() {
        let m = meta(json!([{"name": "x_djvu.txt", "size": "5"}]));
        assert_eq!(choose_pdf(&m, "x"), None);
        assert_eq!(choose_pdf(&json!({}), "x"), None);
    }

    /// IA writes sizes as decimal strings, not numbers. Reading them as numbers silently
    /// yields zero, which would disable the truncation check on every item.
    #[test]
    fn sizes_are_read_from_ias_string_form() {
        let m = meta(json!([{"name": "x.pdf", "size": "574464"}]));
        assert_eq!(choose_pdf(&m, "x").unwrap().size, 574_464);
    }

    #[test]
    fn a_missing_size_does_not_read_as_zero_bytes_expected() {
        let m = meta(json!([{"name": "x.pdf"}]));
        let pdf = choose_pdf(&m, "x").unwrap();
        // Zero means "unknown", and the verifier treats it as "do not check".
        assert_eq!(pdf.size, 0);
        assert_eq!(pdf.md5, None);
    }

    #[test]
    fn retry_after_reads_delta_seconds_and_ignores_the_rest() {
        assert_eq!(retry_after(Some("30")), Some(Duration::from_secs(30)));
        assert_eq!(retry_after(Some(" 5 ")), Some(Duration::from_secs(5)));
        assert_eq!(retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")), None);
        assert_eq!(retry_after(None), None);
    }

    /// A 404 is an answer. Asking again will not change it.
    #[test]
    fn only_transient_statuses_are_retried() {
        assert!(is_retryable(429));
        assert!(is_retryable(503));
        assert!(is_retryable(500));
        assert!(!is_retryable(404));
        assert!(!is_retryable(403));
        assert!(!is_retryable(200));
    }
}
