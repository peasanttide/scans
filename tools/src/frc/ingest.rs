//! Walking a local frc-data checkout and writing the archive records and OCR sidecars.
//!
//! This half of the ingest touches no network. Everything it needs — the item list, the
//! catalogue metadata, and the word-level OCR — is in the frc-data repository, which is one
//! clone rather than 76,754 requests against archive.org for bytes that already exist in git.
//! Only the PDFs have to be fetched, and that is [`super::fetch`].
//!
//! ## Layout
//!
//! ```text
//! sources/frc/ab/abondance00unse/abondance00unse.toml       the record
//!                                abondance00unse.ocr.toml   the OCR
//!                                abondance00unse.pdf        fetched later
//! ```
//!
//! Two-character shards taken from the identifier: about 700 directories of 55 items rather
//! than one of 38,377. Addresses do not change if the sharding is ever redone, because ids
//! are flat and directories are cosmetic.
//!
//! ## Resumability
//!
//! An item is done when its record and sidecar both exist. That is the whole resume
//! condition, checked from the filesystem, so an interrupted run costs one `stat` per item to
//! pick up and there is no ledger to fall out of step with the tree it describes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::ocr;

use super::{meta, record};

/// Directory under the archive root that this corpus lives in.
pub const DIR: &str = "sources/frc";

/// What one item's ingest did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Record and sidecar written.
    Written,
    /// Both already existed.
    Skipped,
    /// Written, but the item has no OCR in frc-data.
    WrittenWithoutOcr,
    /// Nothing written; the reason is worth reporting rather than aborting the run.
    Failed(String),
}

/// Everything one run of [`ingest`] did.
#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub written: usize,
    pub skipped: usize,
    pub without_ocr: usize,
    pub failed: Vec<(String, String)>,
    /// IA field names that were present somewhere and mapped nowhere, with a count.
    pub ignored_fields: std::collections::BTreeMap<String, usize>,
}

/// The shard directory an identifier lives in.
///
/// The first two characters, lowercased, with anything that is not a letter or digit replaced
/// so the name is safe on every filesystem. IA identifiers are already `[a-z0-9]`-ish, but
/// 38,377 of them is enough that "already" is not a guarantee worth relying on.
pub fn shard(id: &str) -> String {
    let mut out = String::new();
    for c in id.chars().take(2) {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    // A one-character identifier still needs a two-character shard.
    while out.len() < 2 {
        out.push('_');
    }
    out
}

/// Where an item's files go, relative to the archive root.
pub fn item_dir(id: &str) -> PathBuf {
    Path::new(DIR).join(shard(id)).join(id)
}

/// Every identifier in a frc-data checkout, sorted.
///
/// Taken from `Metadata/`, which is the fuller list: some items have catalogue metadata and
/// no OCR, and they are still part of the collection.
pub fn identifiers(frc_data: &Path) -> Result<Vec<String>> {
    let dir = frc_data.join("Metadata");
    let entries = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {} — is this a frc-data checkout?", dir.display()))?;

    let mut ids: Vec<String> = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(id) = name.strip_suffix("_meta.xml") {
            ids.push(id.to_string());
        }
    }
    ids.sort();
    Ok(ids)
}

/// Ingest one item.
pub fn ingest_one(root: &Path, frc_data: &Path, id: &str, force: bool) -> Outcome {
    match try_ingest_one(root, frc_data, id, force) {
        Ok(outcome) => outcome,
        Err(e) => Outcome::Failed(format!("{e:#}")),
    }
}

fn try_ingest_one(root: &Path, frc_data: &Path, id: &str, force: bool) -> Result<Outcome> {
    let dir = root.join(item_dir(id));
    let record_path = dir.join(format!("{id}.toml"));
    // The OCR is now one file per page, so "already done" is asked of the first page. An
    // item whose page 1 is written was written in full: the loop below writes them in order
    // and a failure part-way aborts the item before its record is written.
    let ocr_path = dir.join(ocr::file_name(id, 1));
    let xml_path = frc_data.join("XML_for_OCR").join(format!("{id}_djvu.xml"));

    let has_ocr_source = xml_path.exists();
    if !force && record_path.exists() && (ocr_path.exists() || !has_ocr_source) {
        return Ok(Outcome::Skipped);
    }

    let meta_path = frc_data.join("Metadata").join(format!("{id}_meta.xml"));
    let meta_text = std::fs::read_to_string(&meta_path)
        .with_context(|| format!("reading {}", meta_path.display()))?;
    let parsed = meta::parse(&meta_text).with_context(|| format!("parsing {id}_meta.xml"))?;
    let mapped = record::map(id, &parsed);

    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    // The OCR is written first: a record that declares `[[text]]` pointing at a file that is
    // not there yet would be a broken archive for as long as the run takes.
    let mut text_entries: Vec<crate::model::Text> = Vec::new();
    if has_ocr_source {
        let xml = std::fs::read_to_string(&xml_path)
            .with_context(|| format!("reading {}", xml_path.display()))?;
        let pages =
            crate::djvu::parse(&xml, id).with_context(|| format!("parsing {id}_djvu.xml"))?;

        let engine = parsed.get("ocr").map(str::to_string);
        let schema_rel = format!("../../../../{}", ocr::SCHEMA_PATH);

        for mut page in pages {
            page.engine = engine.clone();
            page.lang = mapped.record.language.clone();

            let name = ocr::file_name(id, page.page);
            std::fs::write(dir.join(&name), ocr::to_markdown(&page, &schema_rel))
                .with_context(|| format!("writing {name}"))?;

            text_entries.push(crate::model::Text {
                file: name,
                kind: Some("ocr".into()),
                by: engine.clone(),
                lang: page.lang.clone(),
                note: None,
            });
        }
    }

    let mut source = mapped.record;
    source.text = text_entries;
    let rendered = render_record(&source)?;
    std::fs::write(&record_path, rendered)
        .with_context(|| format!("writing {}", record_path.display()))?;

    Ok(if has_ocr_source {
        Outcome::Written
    } else {
        Outcome::WrittenWithoutOcr
    })
}

/// A record as TOML, with the `#:schema` directive that gets it validated in an editor.
fn render_record(source: &crate::model::Source) -> Result<String> {
    let record = crate::model::Record::Source(source.clone());
    let body = toml::to_string_pretty(&record).context("serialising the record")?;
    Ok(format!(
        "#:schema ../../../../{}\n{body}",
        crate::SCHEMA_PATH
    ))
}

/// Ingest a whole corpus.
///
/// `progress` is called after each item so a caller can report without this module knowing
/// what reporting looks like.
pub fn ingest(
    root: &Path,
    frc_data: &Path,
    ids: &[String],
    force: bool,
    mut progress: impl FnMut(usize, &str, &Outcome),
) -> Summary {
    let mut summary = Summary::default();

    for (i, id) in ids.iter().enumerate() {
        // The ignored-field census is worth having across the whole corpus, and it costs one
        // extra parse of a 3 KB file only for items that are actually processed.
        let outcome = ingest_one(root, frc_data, id, force);
        match &outcome {
            Outcome::Written => summary.written += 1,
            Outcome::WrittenWithoutOcr => {
                summary.written += 1;
                summary.without_ocr += 1;
            }
            Outcome::Skipped => summary.skipped += 1,
            Outcome::Failed(why) => summary.failed.push((id.clone(), why.clone())),
        }
        progress(i, id, &outcome);
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shards_are_two_characters_of_the_identifier() {
        assert_eq!(shard("abondance00unse"), "ab");
        assert_eq!(shard("1789iemilseptcen00unse"), "17");
        assert_eq!(shard("ABC"), "ab");
    }

    /// A short or odd identifier must still produce a usable directory name.
    #[test]
    fn odd_identifiers_still_shard() {
        assert_eq!(shard("a"), "a_");
        assert_eq!(shard(""), "__");
        assert_eq!(shard("a.b"), "a_");
    }

    #[test]
    fn an_item_lands_two_directories_deep() {
        assert_eq!(
            item_dir("abondance00unse"),
            Path::new("sources/frc/ab/abondance00unse")
        );
    }

    /// The end-to-end shape, on a frc-data checkout built for the test.
    #[test]
    fn ingesting_an_item_writes_a_record_and_a_sidecar_that_agree() {
        let tmp = tempfile::tempdir().unwrap();
        let frc = tmp.path().join("frc-data");
        std::fs::create_dir_all(frc.join("Metadata")).unwrap();
        std::fs::create_dir_all(frc.join("XML_for_OCR")).unwrap();
        std::fs::write(
            frc.join("Metadata/x00unse_meta.xml"),
            r#"<metadata><title>Sur les subsistances</title><language>fre</language>
               <date>1789</date><ocr>ABBYY FineReader 11.0</ocr><ppi>500</ppi></metadata>"#,
        )
        .unwrap();
        std::fs::write(
            frc.join("XML_for_OCR/x00unse_djvu.xml"),
            r#"<DjVuXML><BODY><OBJECT width="100" height="200"><HIDDENTEXT>
               <PAGECOLUMN><REGION backgroundColor="123"><PARAGRAPH><LINE>
                 <WORD coords="1,2,3,4,5">Subsistances</WORD>
               </LINE></PARAGRAPH></REGION></PAGECOLUMN>
               </HIDDENTEXT></OBJECT></BODY></DjVuXML>"#,
        )
        .unwrap();

        let root = tmp.path().join("archive");
        assert_eq!(
            ingest_one(&root, &frc, "x00unse", false),
            Outcome::Written
        );

        let dir = root.join("sources/frc/x0/x00unse");
        let record = std::fs::read_to_string(dir.join("x00unse.toml")).unwrap();
        assert!(record.contains("layer = \"source\""), "{record}");
        assert!(record.contains("Sur les subsistances"), "{record}");
        // The record must point at the sidecar that was actually written.
        assert!(record.contains("x00unse.p1.ocr.md"), "{record}");

        let sidecar = std::fs::read_to_string(dir.join("x00unse.p1.ocr.md")).unwrap();
        let parsed: ocr::Ocr = ocr::from_markdown(&sidecar).unwrap();
        assert_eq!(parsed.of, "x00unse");
        assert_eq!(parsed.page, 1);
        assert_eq!(parsed.engine.as_deref(), Some("ABBYY FineReader 11.0"));
        assert_eq!(parsed.text, "Subsistances");

        // Second time round it is already done.
        assert_eq!(ingest_one(&root, &frc, "x00unse", false), Outcome::Skipped);
        assert_eq!(ingest_one(&root, &frc, "x00unse", true), Outcome::Written);
    }

    /// 4,000-odd items have catalogue metadata and no OCR. They are still part of the
    /// collection and must still get a record.
    #[test]
    fn an_item_with_no_ocr_still_gets_a_record() {
        let tmp = tempfile::tempdir().unwrap();
        let frc = tmp.path().join("frc-data");
        std::fs::create_dir_all(frc.join("Metadata")).unwrap();
        std::fs::create_dir_all(frc.join("XML_for_OCR")).unwrap();
        std::fs::write(
            frc.join("Metadata/y_meta.xml"),
            "<metadata><title>Sans OCR</title></metadata>",
        )
        .unwrap();

        let root = tmp.path().join("archive");
        assert_eq!(
            ingest_one(&root, &frc, "y", false),
            Outcome::WrittenWithoutOcr
        );

        let record = std::fs::read_to_string(root.join("sources/frc/y_/y/y.toml")).unwrap();
        assert!(!record.contains("[[text]]"), "no sidecar to point at: {record}");
        // And it stays done rather than being retried forever.
        assert_eq!(ingest_one(&root, &frc, "y", false), Outcome::Skipped);
    }

    #[test]
    fn identifiers_come_from_the_metadata_directory_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = tmp.path().join("Metadata");
        std::fs::create_dir_all(&meta).unwrap();
        for name in ["b_meta.xml", "a_meta.xml", "notmeta.txt"] {
            std::fs::write(meta.join(name), "<metadata/>").unwrap();
        }
        assert_eq!(identifiers(tmp.path()).unwrap(), ["a", "b"]);
    }

    /// A single bad item must not take the other 38,376 down with it.
    #[test]
    fn a_failure_is_reported_and_the_run_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let frc = tmp.path().join("frc-data");
        std::fs::create_dir_all(frc.join("Metadata")).unwrap();
        std::fs::write(frc.join("Metadata/good_meta.xml"), "<metadata><title>t</title></metadata>")
            .unwrap();
        // `bad` has no metadata file at all.
        let root = tmp.path().join("archive");

        let ids = vec!["bad".to_string(), "good".to_string()];
        let summary = ingest(&root, &frc, &ids, false, |_, _, _| {});
        assert_eq!(summary.written, 1);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].0, "bad");
    }
}
