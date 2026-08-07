//! Siméon-Prosper Hardy's *Mes loisirs*, from Gallica.
//!
//! Eight manuscript volumes in the Bibliothèque nationale de France, Département des
//! Manuscrits, Français 6680–6687: a Paris bookseller's day-by-day journal of events as they
//! reached him, from 1764 to the autumn of 1789. It is one of the very few sustained
//! first-hand accounts of the Revolution's approach written by someone who was neither a
//! politician nor a memoirist working from hindsight.
//!
//! ## Why images and not text
//!
//! It is a manuscript. There is no OCR worth having and Gallica publishes none; what exists
//! is 4,230 photographed pages at around 4500x7000. So this source is page images, and the
//! archive holds them as JPEG exactly as Gallica serves them rather than converting to
//! anything.
//!
//! ## The shape it takes in the archive
//!
//! The journal is one `source`; each bound volume is a `copy` of it. That is the layer model
//! used as intended rather than collapsed — unlike the frc pamphlets, where one item really
//! is one work, Hardy is a single work that exists as eight physical objects, and both the
//! whole and the parts need to be citable:
//!
//! ```text
//! hardy            the journal
//! hardy-vol1       Français 6680, 1764-1771
//! hardy-vol1.p27   one page of it
//! ```
//!
//! ## Rights, which are not the same as the rest of the archive
//!
//! The *work* is public domain — Hardy died in 1806 and the manuscript is of the 1760s to
//! 1780s. The *digitisation* is not offered on the same terms as the Internet Archive
//! material elsewhere in this repository: Gallica's conditions allow free reuse for
//! non-commercial purposes and require a licence from BnF for commercial reuse. That
//! distinction is recorded per record rather than glossed, because getting it wrong would
//! mean this archive asserting a freedom it was not given.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Directory under the archive root that this source lives in.
pub const DIR: &str = "sources/hardy";

/// Id of the journal as a whole.
pub const SOURCE_ID: &str = "hardy";

/// Gallica's required attribution formula.
pub const ATTRIBUTION: &str = "Source gallica.bnf.fr / Bibliothèque nationale de France.";

/// Gallica's terms on the digitisation, which are not public domain.
pub const SCAN_RIGHTS: &str = "BnF-Gallica-noncommercial";

/// One bound volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    /// 1-based volume number, as the archive numbers them.
    pub n: u32,
    /// Gallica ark identifier, without the `ark:/12148/` prefix.
    pub ark: &'static str,
    /// BnF shelfmark within Français.
    pub cote: &'static str,
    /// The span the volume covers, as an EDTF interval.
    pub covers: &'static str,
}

impl Volume {
    /// Record id, e.g. `hardy-vol1`.
    pub fn id(&self) -> String {
        format!("{SOURCE_ID}-vol{}", self.n)
    }

    /// Directory the volume's files live in, relative to the archive root.
    pub fn dir(&self) -> PathBuf {
        Path::new(DIR).join(format!("vol{}", self.n))
    }

    /// Filename of one page's image.
    ///
    /// Three digits: the longest volume has 658 views, and a fixed width means the files sort
    /// in reading order in any tool that sorts them as strings.
    pub fn page_file(&self, view: u32) -> String {
        format!("{}-p{view:03}.jpg", self.id())
    }

    /// Gallica's landing page for the volume.
    pub fn url(&self) -> String {
        format!("https://gallica.bnf.fr/ark:/12148/{}", self.ark)
    }

    /// IIIF image URL for one view at native resolution.
    ///
    /// `full/full/0/native.jpg` is IIIF Image API 1.1 syntax. Gallica also answers the 2.0
    /// spelling, `default.jpg`, with the identical bytes; the older spelling is used because
    /// it is what Gallica's own documentation and viewer emit.
    pub fn image_url(&self, view: u32) -> String {
        format!(
            "https://gallica.bnf.fr/iiif/ark:/12148/{}/f{view}/full/full/0/native.jpg",
            self.ark
        )
    }

    /// The IIIF manifest, which carries per-view dimensions.
    pub fn manifest_url(&self) -> String {
        format!("https://gallica.bnf.fr/iiif/ark:/12148/{}/manifest.json", self.ark)
    }
}

/// The eight volumes, in order.
///
/// The arks are stated here rather than discovered, because Gallica has no search API that
/// reliably returns exactly these eight and nothing else, and a wrong ark would quietly
/// download the wrong manuscript. Each was checked against its shelfmark in the IIIF
/// manifest's metadata before being written down.
pub const VOLUMES: &[Volume] = &[
    Volume { n: 1, ark: "btv1b9060740w", cote: "Français 6680", covers: "1764/1771" },
    Volume { n: 2, ark: "btv1b90607397", cote: "Français 6681", covers: "1772/1774" },
    Volume { n: 3, ark: "btv1b9060738t", cote: "Français 6682", covers: "1775/1778" },
    Volume { n: 4, ark: "btv1b9060732b", cote: "Français 6683", covers: "1778/1781" },
    Volume { n: 5, ark: "btv1b9060737d", cote: "Français 6684", covers: "1781/1784" },
    Volume { n: 6, ark: "btv1b90607360", cote: "Français 6685", covers: "1784/1787" },
    Volume { n: 7, ark: "btv1b9060735k", cote: "Français 6686", covers: "1787/1788" },
    Volume { n: 8, ark: "btv1b90607345", cote: "Français 6687", covers: "1788/1789" },
];

/// Look a volume up by number.
pub fn volume(n: u32) -> Option<&'static Volume> {
    VOLUMES.iter().find(|v| v.n == n)
}

// ---------------------------------------------------------------------------------------
// The IIIF manifest
// ---------------------------------------------------------------------------------------

/// One view's dimensions, taken from the manifest rather than by opening the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    /// 1-based view number, which is what `f{n}` in the image URL means.
    pub n: u32,
    pub width: i64,
    pub height: i64,
}

/// Pull the per-view dimensions out of a IIIF presentation manifest.
///
/// Pure, so the parsing is tested against a fixture rather than against Gallica. Canvases are
/// taken in sequence order and numbered from 1, which is the correspondence Gallica's own
/// `f{n}` image URLs use.
pub fn views_from_manifest(manifest: &serde_json::Value) -> Result<Vec<View>> {
    let canvases = manifest
        .get("sequences")
        .and_then(|s| s.get(0))
        .and_then(|s| s.get("canvases"))
        .and_then(|c| c.as_array())
        .context("manifest has no sequences[0].canvases")?;

    Ok(canvases
        .iter()
        .enumerate()
        .map(|(i, c)| View {
            n: i as u32 + 1,
            width: c.get("width").and_then(|v| v.as_i64()).unwrap_or(0),
            height: c.get("height").and_then(|v| v.as_i64()).unwrap_or(0),
        })
        .collect())
}

/// A named field from the manifest's `metadata` array, e.g. `Shelfmark` or `Title`.
pub fn manifest_field(manifest: &serde_json::Value, label: &str) -> Option<String> {
    let entries = manifest.get("metadata")?.as_array()?;
    for entry in entries {
        if entry.get("label").and_then(|l| l.as_str()) != Some(label) {
            continue;
        }
        return match entry.get("value") {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            // Gallica sometimes gives a language-tagged array.
            Some(serde_json::Value::Array(vs)) => vs.first().and_then(|v| {
                v.get("@value")
                    .and_then(|x| x.as_str())
                    .or_else(|| v.as_str())
                    .map(str::to_string)
            }),
            _ => None,
        };
    }
    None
}

// ---------------------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------------------

/// The journal as a whole: `layer = "source"`.
///
/// Everything the eight volumes share is declared here once and inherited — the author, the
/// language, the repository, the rights. A volume record then states only what is true of
/// that volume: its shelfmark, its ark, its span, and its pages.
pub fn source_record() -> crate::model::Source {
    use crate::model::{Holding, Resp, Rights, Roles, Scan};

    let mut identifier = crate::model::Identifier::new();
    identifier.insert("bnf_cote".into(), "Français 6680-6687".into());

    crate::model::Source {
        id: Some(SOURCE_ID.into()),
        r#type: Some("diary".into()),
        title: "Mes loisirs, ou Journal d'événemens tels qu'ils parviennent à ma connoissance"
            .into(),
        short_title: Some("Hardy, Mes loisirs".into()),
        language: Some("fr".into()),
        place: Some("Paris".into()),
        country: Some("France".into()),
        covers: Some("1764/1789".into()),
        note: Some(
            "A Paris bookseller's day-by-day journal of events as they reached him, kept from \
             1764 until the autumn of 1789 and running to eight manuscript volumes. Hardy was \
             neither a politician nor a memoirist writing from hindsight, which is what makes \
             the last two volumes unusual: they record the approach of the Revolution as it \
             looked from a shop in the rue Saint-Jacques, day by day, without knowing how it \
             ended. Unpublished in his lifetime; the manuscript is autograph throughout."
                .into(),
        ),
        url: Some("https://gallica.bnf.fr/ark:/12148/btv1b9060740w".into()),
        resp: Some(vec![Resp {
            name: "Siméon-Prosper Hardy".into(),
            role: Some(Roles::One("author".into())),
            note: Some("1729-1806, bookseller in Paris.".into()),
        }]),
        rights: Some(Rights {
            // Hardy died in 1806 and the manuscript is of the 1760s to 1780s.
            work: Some("PD-old-100-expired".into()),
            // NOT public domain, unlike the Internet Archive material elsewhere here.
            scan: Some(SCAN_RIGHTS.into()),
            attribution: Some(ATTRIBUTION.into()),
            note: Some(
                "The work is out of copyright; the digitisation is not offered on the same \
                 terms. Gallica's conditions permit free reuse of these images for \
                 non-commercial purposes and require a licence from the Bibliothèque \
                 nationale de France for commercial reuse. See \
                 https://gallica.bnf.fr/edit/und/conditions-dutilisation-des-contenus-de-gallica"
                    .into(),
            ),
        }),
        holding: Some(Holding {
            repository: Some("Bibliothèque nationale de France".into()),
            collection: Some("Département des Manuscrits, Français".into()),
            shelfmark: Some("Français 6680-6687".into()),
            note: None,
        }),
        identifier: Some(identifier),
        scan: Some(Scan {
            by: Some("Bibliothèque nationale de France".into()),
            url: Some("https://gallica.bnf.fr/".into()),
            note: Some(
                "Photographed from the bound manuscript and served by Gallica through the \
                 IIIF Image API. Held here at the native resolution Gallica returns for \
                 `full/full`, around 4500x7000, as the JPEG it serves rather than converted."
                    .into(),
            ),
            ..Scan::default()
        }),
        ..crate::model::Source::default()
    }
}

/// One bound volume: `layer = "copy"`, with its pages inline.
pub fn volume_record(v: &Volume, views: &[View]) -> crate::model::CopyRecord {
    use crate::model::{Graphic, Holding, Page, Scan};

    let mut identifier = crate::model::Identifier::new();
    identifier.insert("ark".into(), format!("ark:/12148/{}", v.ark));
    identifier.insert("gallica".into(), v.ark.into());

    let pages = views
        .iter()
        .map(|view| Page {
            n: Some(view.n as i64),
            graphic: vec![Graphic {
                file: Some(v.page_file(view.n)),
                // Taken from the IIIF manifest rather than by opening 4,230 JPEGs.
                width: (view.width > 0).then_some(view.width),
                height: (view.height > 0).then_some(view.height),
                url: Some(v.image_url(view.n)),
                mimetype: Some("image/jpeg".into()),
                ..Graphic::default()
            }],
            ..Page::default()
        })
        .collect();

    crate::model::CopyRecord {
        id: Some(v.id()),
        of: Some(SOURCE_ID.into()),
        r#type: Some("volume".into()),
        title: format!("Mes loisirs, volume {}", v.n),
        covers: Some(v.covers.into()),
        url: Some(v.url()),
        holding: Some(Holding {
            shelfmark: Some(v.cote.into()),
            ..Holding::default()
        }),
        identifier: Some(identifier),
        scan: Some(Scan {
            count: Some(views.len() as i64),
            ..Scan::default()
        }),
        page: pages,
        ..crate::model::CopyRecord::default()
    }
}

/// Render a record as TOML with the `#:schema` directive that gets it validated in an editor.
///
/// `depth` is how many directories below the repository root the file sits.
pub fn render(record: &crate::model::Record, depth: usize) -> Result<String> {
    let body = toml::to_string_pretty(record).context("serialising the record")?;
    let up = "../".repeat(depth);
    Ok(format!("#:schema {up}{}\n{body}", crate::SCHEMA_PATH))
}

// ---------------------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------------------

/// What fetching one page did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Fetched { bytes: u64 },
    /// Already on disk.
    Skipped,
    Failed(String),
}

#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub fetched: usize,
    pub bytes: u64,
    pub skipped: usize,
    pub failed: Vec<(String, String)>,
}

/// Smallest plausible page image.
///
/// Gallica occasionally answers with a short error document carrying a 200. A JPEG of a
/// 4500x7000 manuscript page is megabytes; anything under this is not one, and accepting it
/// would leave a file that the resume check then treats as done.
#[cfg(feature = "fetch")]
const MIN_IMAGE_BYTES: u64 = 8 * 1024;

/// Download the page images of some volumes.
#[cfg(feature = "fetch")]
pub fn fetch(
    root: &Path,
    volumes: &[&Volume],
    interval: std::time::Duration,
    force: bool,
    mut progress: impl FnMut(&Volume, u32, u32, &Outcome),
) -> Result<Summary> {
    use crate::frc::limiter::Limiter;

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(crate::frc::fetch::USER_AGENT)
        .timeout_global(Some(std::time::Duration::from_secs(300)))
        .build()
        .into();

    let mut limiter = Limiter::new(interval);
    let mut summary = Summary::default();

    for v in volumes {
        let dir = root.join(v.dir());
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;

        let views = fetch_views(&agent, &mut limiter, v)?;
        let total = views.len() as u32;

        // The record is written before the images, so an interrupted run still leaves a
        // volume that describes itself; `validate` then reports the missing files as E701
        // rather than the volume simply not existing.
        let record = crate::model::Record::Copy(volume_record(v, &views));
        std::fs::write(dir.join(format!("{}.toml", v.id())), render(&record, 3)?)
            .with_context(|| format!("writing the record for {}", v.id()))?;

        for view in &views {
            let dest = dir.join(v.page_file(view.n));
            if !force
                && std::fs::metadata(&dest).is_ok_and(|m| m.len() >= MIN_IMAGE_BYTES)
            {
                summary.skipped += 1;
                progress(v, view.n, total, &Outcome::Skipped);
                continue;
            }

            let url = v.image_url(view.n);
            let what = format!("{} f{}", v.id(), view.n);
            let outcome = match with_retry(&mut limiter, &what, || {
                download(&agent, &url, &dest)
            }) {
                Ok(bytes) => {
                    summary.fetched += 1;
                    summary.bytes += bytes;
                    Outcome::Fetched { bytes }
                }
                Err(e) => {
                    let why = format!("{e:#}");
                    summary.failed.push((what.clone(), why.clone()));
                    Outcome::Failed(why)
                }
            };
            progress(v, view.n, total, &outcome);
        }
    }

    Ok(summary)
}

/// How long to wait between requests to Gallica, by default.
///
/// Two seconds, not the one used for archive.org. Gallica throttles considerably harder: a
/// run at one request a second collected an HTTP 429 within five requests. This is somebody
/// else's public service and the polite rate is whatever they will accept without complaint.
pub const DEFAULT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// How many times a request is retried before the volume is given up on.
#[cfg(feature = "fetch")]
const MAX_ATTEMPTS: u32 = 5;

/// Run a request, backing off and retrying when Gallica asks us to.
///
/// 429 is the case this exists for. `ureq` surfaces it as `Error::StatusCode`, and the
/// response to it is to wait longer and try again rather than to hammer through — the
/// alternative is being blocked outright, which would end the run for everyone using this
/// address.
#[cfg(feature = "fetch")]
fn with_retry<T>(
    limiter: &mut crate::frc::limiter::Limiter,
    what: &str,
    mut attempt: impl FnMut() -> Result<T>,
) -> Result<T> {
    let mut last: Option<anyhow::Error> = None;
    for n in 1..=MAX_ATTEMPTS {
        limiter.wait();
        match attempt() {
            Ok(v) => {
                limiter.succeeded();
                return Ok(v);
            }
            Err(e) => {
                let throttled = matches!(
                    e.downcast_ref::<ureq::Error>(),
                    Some(ureq::Error::StatusCode(429))
                );
                // A 429 is Gallica saying "slower", so the penalty is grown from a floor
                // rather than from nothing: doubling 2s four times is under a minute, and a
                // throttle usually wants more than that.
                limiter.failed(throttled.then(|| std::time::Duration::from_secs(30 * n as u64)));
                if throttled {
                    eprintln!(
                        "\rscans: {what}: throttled by Gallica, waiting {}s",
                        limiter.penalty().as_secs()
                    );
                }
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("{what}: gave up")))
}

/// The per-view dimensions of one volume, from its IIIF manifest.
#[cfg(feature = "fetch")]
pub fn fetch_views(
    agent: &ureq::Agent,
    limiter: &mut crate::frc::limiter::Limiter,
    v: &Volume,
) -> Result<Vec<View>> {
    let url = v.manifest_url();
    let body = with_retry(limiter, &format!("{} manifest", v.id()), || {
        Ok(agent
            .get(&url)
            .call()?
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading {url}"))?)
    })?;
    let manifest: serde_json::Value =
        serde_json::from_str(&body).with_context(|| format!("parsing {url}"))?;
    views_from_manifest(&manifest)
}

/// Download one image, verify it is actually a JPEG, then move it into place.
///
/// Written beside the destination and renamed, never in place: a run killed mid-download must
/// not leave a partial file that the next run's resume check mistakes for a finished one.
#[cfg(feature = "fetch")]
fn download(agent: &ureq::Agent, url: &str, dest: &Path) -> Result<u64> {
    use std::io::{Read, Write};

    let tmp = dest.with_extension("jpg.part");
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("requesting {url}"))?;

    let mut file =
        std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let mut reader = response.body_mut().as_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    let mut head = Vec::new();

    loop {
        let n = reader.read(&mut buf).with_context(|| format!("reading {url}"))?;
        if n == 0 {
            break;
        }
        if head.len() < 3 {
            head.extend_from_slice(&buf[..n.min(3)]);
        }
        file.write_all(&buf[..n])
            .with_context(|| format!("writing {}", tmp.display()))?;
        total += n as u64;
    }
    drop(file);

    let reject = |why: String| -> anyhow::Error {
        let _ = std::fs::remove_file(&tmp);
        anyhow::anyhow!(why)
    };

    // Gallica answers some failures with a short document and a 200.
    if total < MIN_IMAGE_BYTES {
        return Err(reject(format!("only {total} bytes; not a page image")));
    }
    if head.first_chunk::<3>() != Some(&[0xff, 0xd8, 0xff]) {
        return Err(reject(format!(
            "not a JPEG — starts with {:02x?}",
            &head[..head.len().min(3)]
        )));
    }

    std::fs::rename(&tmp, dest)
        .with_context(|| format!("moving {} into place", tmp.display()))?;
    Ok(total)
}

/// Write the source record for the journal as a whole.
pub fn write_source_record(root: &Path) -> Result<PathBuf> {
    let dir = root.join(DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{SOURCE_ID}.toml"));
    let record = crate::model::Record::Source(source_record());
    std::fs::write(&path, render(&record, 2)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_volume_has_a_distinct_ark_and_shelfmark() {
        let mut arks: Vec<&str> = VOLUMES.iter().map(|v| v.ark).collect();
        arks.sort_unstable();
        let before = arks.len();
        arks.dedup();
        assert_eq!(arks.len(), before, "two volumes share an ark");

        let mut cotes: Vec<&str> = VOLUMES.iter().map(|v| v.cote).collect();
        cotes.sort_unstable();
        let before = cotes.len();
        cotes.dedup();
        assert_eq!(cotes.len(), before, "two volumes share a shelfmark");
    }

    #[test]
    fn volumes_are_numbered_one_to_eight_in_order() {
        assert_eq!(
            VOLUMES.iter().map(|v| v.n).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    /// Every `covers` interval must be valid EDTF, or check 6 will report all eight records.
    #[test]
    fn covers_intervals_are_edtf() {
        for v in VOLUMES {
            crate::edtf::parse(v.covers)
                .unwrap_or_else(|e| panic!("vol {} covers {:?}: {e}", v.n, v.covers));
        }
    }

    /// The volumes run consecutively: each begins in the year the previous one ends, because
    /// the diary is continuous and the binding is arbitrary.
    #[test]
    fn the_volumes_cover_a_continuous_span() {
        let year = |s: &str, i: usize| -> i32 {
            s.split('/').nth(i).unwrap().parse().unwrap()
        };
        for pair in VOLUMES.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            // The next volume either takes up in the year the last one ended — the binder
            // split mid-year — or in the year after it. A larger jump would mean a volume is
            // missing from the table.
            let gap = year(b.covers, 0) - year(a.covers, 1);
            assert!(
                (0..=1).contains(&gap),
                "vol {} ends {} but vol {} starts {}",
                a.n,
                a.covers,
                b.n,
                b.covers
            );
        }
    }

    #[test]
    fn ids_and_paths_follow_the_documented_layout() {
        let v = volume(1).unwrap();
        assert_eq!(v.id(), "hardy-vol1");
        assert_eq!(v.dir(), Path::new("sources/hardy/vol1"));
        assert_eq!(v.page_file(1), "hardy-vol1-p001.jpg");
        assert_eq!(v.page_file(658), "hardy-vol1-p658.jpg");
    }

    /// Fixed-width numbering is what makes the files sort in reading order.
    #[test]
    fn page_files_sort_in_reading_order() {
        let v = volume(1).unwrap();
        let mut names: Vec<String> = [1u32, 2, 10, 99, 100, 658].iter().map(|n| v.page_file(*n)).collect();
        let expected = names.clone();
        names.sort();
        assert_eq!(names, expected);
    }

    #[test]
    fn image_urls_are_iiif_native_resolution() {
        assert_eq!(
            volume(8).unwrap().image_url(10),
            "https://gallica.bnf.fr/iiif/ark:/12148/btv1b90607345/f10/full/full/0/native.jpg"
        );
    }

    #[test]
    fn views_are_numbered_from_one_with_their_dimensions() {
        let manifest = json!({
            "sequences": [{ "canvases": [
                {"label": "NP", "width": 4563, "height": 7028},
                {"label": "NP", "width": 4570, "height": 7030},
            ]}]
        });
        let views = views_from_manifest(&manifest).unwrap();
        assert_eq!(views.len(), 2);
        assert_eq!(views[0], View { n: 1, width: 4563, height: 7028 });
        assert_eq!(views[1].n, 2);
    }

    /// A canvas missing its dimensions must not abort the volume; the record simply omits
    /// them, which `validate` reports as W703 rather than as a failed ingest.
    #[test]
    fn a_canvas_without_dimensions_yields_zeroes_rather_than_an_error() {
        let manifest = json!({ "sequences": [{ "canvases": [ {"label": "NP"} ]}]});
        let views = views_from_manifest(&manifest).unwrap();
        assert_eq!(views[0], View { n: 1, width: 0, height: 0 });
    }

    #[test]
    fn a_manifest_with_no_canvases_is_an_error() {
        assert!(views_from_manifest(&json!({})).is_err());
    }

    #[test]
    fn metadata_fields_are_read_in_both_shapes_gallica_uses() {
        let manifest = json!({ "metadata": [
            {"label": "Shelfmark", "value": "BnF. Manuscrits. Français 6687"},
            {"label": "Title", "value": [{"@value": "Mes loisirs", "@language": "fr"}]},
        ]});
        assert_eq!(
            manifest_field(&manifest, "Shelfmark").as_deref(),
            Some("BnF. Manuscrits. Français 6687")
        );
        assert_eq!(manifest_field(&manifest, "Title").as_deref(), Some("Mes loisirs"));
        assert_eq!(manifest_field(&manifest, "Absent"), None);
    }
}
