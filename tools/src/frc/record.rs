//! Turning one Internet Archive `_meta.xml` into one archive record.
//!
//! Every judgement about what an IA field *means* lives here, and nowhere else. The input is
//! a [`Meta`], so this is testable without a file, a network, or a repository.
//!
//! ## The shape of the output
//!
//! One `layer = "source"` with the copy and document layers collapsed in, which is what
//! `pellet-1873` and `turgot-1739` already do. A pamphlet is a single work, digitised once;
//! three files apiece would mean 115,000 files to describe 38,377 pamphlets and would not say
//! anything the one file does not.
//!
//! ## What this does not do
//!
//! It does not invent. Where IA's metadata is absent, ambiguous, or self-contradictory the
//! field is left off and, when the fact is worth keeping in prose, it goes into `note`. A
//! record that quietly guesses a date is worse than one that has none, because the archive's
//! whole product is citations that can be trusted.

use crate::model::{Holding, Identifier, Link, Resp, Rights, Roles, Scan, Source};

use super::meta::Meta;

/// What one item's metadata mapped to, plus anything the mapping wants to report.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapped {
    pub record: Source,
    /// IA fields that were present and carried nowhere. Reported so that a field which starts
    /// mattering is noticed rather than silently dropped for 38,377 items.
    pub ignored: Vec<String>,
}

/// IA fields deliberately carried nowhere.
///
/// Almost all of them describe archive.org's own workflow — who scanned it, which invoice it
/// was on, what state the republishing pipeline reached — rather than the pamphlet. The few
/// that describe the item are folded into other fields and named here so the ignore list does
/// not report them.
const IGNORED: &[&str] = &[
    // Digitisation workflow and bookkeeping.
    "addeddate", "backup_location", "bookreader-defaults", "boxid", "camera", "ccnum",
    "collection", "contributor", "curation", "external-identifier", "foldoutcount",
    "identifier-access", "imagecount", "invoice", "lccn", "mediatype", "operator",
    "page-progression", "possible-copyright-status", "ppi", "publicdate", "repub_seconds",
    "repub_state", "republisher", "republisher_date", "republisher_operator", "scandate",
    "scanfee", "scanner", "scanningcenter", "shiptracking", "sponsor", "sponsordate",
    "updatedate", "updater", "uploader", "worldcat_source_edition",
    // Folded into other fields by the mapping below.
    "call_number", "citation", "creator", "date", "description", "dfate", "identifier",
    "identifier-ark", "language", "notes", "openlibrary_edition", "openlibrary_work",
    "physical_description", "publisher", "subject", "title",
];

/// Map one item.
pub fn map(id: &str, meta: &Meta) -> Mapped {
    let mut notes: Vec<String> = Vec::new();

    let (place, publisher) = split_publisher(meta.get("publisher"));

    let record = Source {
        id: Some(id.to_string()),
        r#type: Some(genre(meta).to_string()),
        title: meta.get("title").unwrap_or("Untitled").to_string(),
        language: meta.get("language").and_then(language).map(str::to_string),
        place,
        country: None,
        // `dfate` is a real typo in real records, and for some it is the only date present.
        date: meta.first_of(&["date", "dfate"]).and_then(|d| edtf(d, &mut notes)),
        subject: meta.unique("subject").into_iter().map(str::to_string).collect(),
        extent: meta.get("physical_description").map(str::to_string),
        url: Some(format!("https://archive.org/details/{id}")),

        resp: resp(meta, publisher.as_deref()),
        rights: Some(rights(meta)),
        holding: Some(holding(meta)),
        identifier: Some(identifier(id, meta)),
        scan: Some(scan(id, meta)),
        link: links(meta),

        ..Source::default()
    };

    // Everything the cataloguer wrote in prose, in a stable order, as one note. These are
    // genuinely useful — attributions, references to Martin & Walter, and the scanner's own
    // account of the damage to the copy — and none of them fits a field.
    notes.extend(meta.unique("description").into_iter().map(str::to_string));
    if let Some(c) = meta.get("citation") {
        notes.push(format!("Bibliography: {c}."));
    }
    if let Some(n) = meta.get("notes") {
        notes.push(format!("Scanner's note: {n}"));
    }

    let ignored = meta
        .names()
        .filter(|n| !IGNORED.contains(n))
        .map(str::to_string)
        .collect();

    let mut record = record;
    record.note = join_notes(notes);
    Mapped { record, ignored }
}

/// `pamphlet`, `book`, or `periodical`.
///
/// The collection is overwhelmingly pamphlets, and the printed extent is the only honest
/// signal of which is which. Fifty pages is the conventional bibliographic line between a
/// pamphlet and a book; an item whose extent cannot be read is called a pamphlet, because
/// that is what all but a few hundred of them are.
fn genre(meta: &Meta) -> &'static str {
    match printed_pages(meta.get("physical_description")) {
        Some(n) if n >= 50 => "book",
        _ => "pamphlet",
    }
}

/// The leading page count of `"46 p. ; 22 cm."`.
fn printed_pages(extent: Option<&str>) -> Option<i64> {
    let extent = extent?;
    let digits: String = extent
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// `"Paris : De l'impr. de Cailleau"` → place and publisher.
///
/// The ` : ` is the ISBD separator between place and publisher, and IA's records follow it
/// closely enough to rely on. Square brackets mean the cataloguer supplied the place rather
/// than reading it off the title page; the fact is the same either way, so they are dropped.
fn split_publisher(publisher: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(raw) = publisher else {
        return (None, None);
    };
    match raw.split_once(" : ") {
        Some((place, rest)) => {
            let place = place.trim().trim_matches(['[', ']']).trim();
            let rest = rest.trim().trim_end_matches([',', ';']).trim();
            (
                (!place.is_empty()).then(|| place.to_string()),
                (!rest.is_empty()).then(|| rest.to_string()),
            )
        }
        // No separator: it is all publisher, and the place is unknown rather than guessed.
        None => (None, Some(raw.trim().to_string())),
    }
}

/// ISO-639-2 to the BCP-47 code the archive uses.
///
/// A closed table rather than a general library: this collection is French with a thin margin
/// of Latin, English, German, Italian, Spanish and Dutch, and an unrecognised code should
/// leave `language` off rather than pass a three-letter code off as BCP-47.
fn language(code: &str) -> Option<&'static str> {
    Some(match code.trim().to_ascii_lowercase().as_str() {
        "fre" | "fra" | "fr" => "fr",
        "eng" | "en" => "en",
        "lat" | "la" => "la",
        "ger" | "deu" | "de" => "de",
        "ita" | "it" => "it",
        "spa" | "es" => "es",
        "dut" | "nld" | "nl" => "nl",
        _ => return None,
    })
}

/// An IA date as EDTF, or `None` with the raw value recorded as a note.
///
/// IA dates in this collection are mostly a bare year, sometimes a full date, and sometimes
/// prose. Only the forms that are already valid EDTF are passed through; anything else is
/// kept verbatim in the note, where a human can see it, rather than being coerced into a
/// date the record would then assert.
fn edtf(raw: &str, notes: &mut Vec<String>) -> Option<String> {
    let raw = raw.trim();
    let looks_like_edtf = matches!(raw.len(), 4 | 7 | 10)
        && raw
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == 'X' || c == '?' || c == '~');

    if looks_like_edtf && crate::edtf::parse(raw).is_ok() {
        return Some(raw.to_string());
    }

    // A four-digit year inside prose — "published in 1789" — is worth recovering, but the
    // prose is kept too, because it usually says something the year does not.
    let year = raw
        .as_bytes()
        .windows(4)
        .find(|w| w.iter().all(|c| c.is_ascii_digit()))
        .and_then(|w| std::str::from_utf8(w).ok())
        .filter(|y| ("1400".."2100").contains(y));

    match year {
        Some(y) => {
            notes.push(format!("Date given by the catalogue as {raw:?}."));
            Some(y.to_string())
        }
        None => {
            if !raw.is_empty() {
                notes.push(format!("Date given by the catalogue as {raw:?}, unreadable."));
            }
            None
        }
    }
}

/// Creators, then the publisher.
///
/// IA encodes a role inside the name string — `"Cailleau, André-Charles, 1731-1798, printer"`
/// — for printers and engravers. That trailing role is lifted into `role`, because a name
/// with a job title glued to the end is exactly the unparseable `author` string this schema
/// exists to replace.
fn resp(meta: &Meta, publisher: Option<&str>) -> Option<Vec<Resp>> {
    let mut out: Vec<Resp> = Vec::new();

    for raw in meta.unique("creator") {
        let (name, role) = split_role(raw);
        // The same person often appears twice, once with a trailing full stop. Match on the
        // name so the two do not become two people.
        let key = name.trim_end_matches('.').to_string();
        if out.iter().any(|r| r.name.trim_end_matches('.') == key) {
            continue;
        }
        out.push(Resp {
            name,
            role: Some(Roles::One(role.to_string())),
            note: None,
        });
    }

    if let Some(p) = publisher {
        out.push(Resp {
            name: p.to_string(),
            role: Some(Roles::One("publisher".into())),
            note: None,
        });
    }

    (!out.is_empty()).then_some(out)
}

/// Roles IA glues to the end of a name.
const TRAILING_ROLES: &[&str] = &[
    "printer", "publisher", "engraver", "bookseller", "editor", "translator", "illustrator",
    "author", "compiler", "lithographer", "cartographer",
];

/// `"Cailleau, André-Charles, 1731-1798, printer"` → `("Cailleau, André-Charles, 1731-1798", "printer")`.
fn split_role(raw: &str) -> (String, &'static str) {
    let trimmed = raw.trim().trim_end_matches('.');
    for role in TRAILING_ROLES {
        for sep in [", ", " "] {
            let suffix = format!("{sep}{role}");
            if let Some(name) = trimmed
                .strip_suffix(&suffix)
                .or_else(|| trimmed.to_ascii_lowercase().strip_suffix(&suffix).map(|_| {
                    &trimmed[..trimmed.len() - suffix.len()]
                }))
            {
                return (name.trim().trim_end_matches(',').to_string(), role);
            }
        }
    }
    (trimmed.to_string(), "author")
}

/// Rights, which are the same across the whole collection.
fn rights(meta: &Meta) -> Rights {
    Rights {
        // Every item is a French imprint of the 1780s and 1790s. Nothing here is in copyright
        // anywhere, and the collection's own IA records say so.
        work: Some("PD-old-100-expired".into()),
        scan: Some("PD".into()),
        attribution: Some(match meta.get("sponsor") {
            Some(s) => format!("Digitised by the Internet Archive, sponsored by {s}."),
            None => "Digitised by the Internet Archive.".into(),
        }),
        note: None,
    }
}

fn holding(meta: &Meta) -> Holding {
    Holding {
        repository: meta.get("contributor").map(str::to_string),
        shelfmark: meta.get("call_number").map(str::to_string),
        // `newberryfrenchpamphlets` is an archive.org bucket name, not a collection as a
        // reader would cite it. The other buckets an item sits in — `americana`, `newberry` —
        // say nothing about the item at all.
        collection: meta
            .all("collection")
            .contains(&"newberryfrenchpamphlets")
            .then(|| "French Revolution Collection".to_string()),
        note: None,
    }
}

fn identifier(id: &str, meta: &Meta) -> Identifier {
    let mut out = Identifier::new();
    out.insert("internet_archive".into(), id.to_string());
    for (key, field) in [
        ("ark", "identifier-ark"),
        ("openlibrary_edition", "openlibrary_edition"),
        ("openlibrary_work", "openlibrary_work"),
        ("lccn", "lccn"),
    ] {
        if let Some(v) = meta.get(field) {
            out.insert(key.into(), v.to_string());
        }
    }
    out
}

fn scan(id: &str, meta: &Meta) -> Scan {
    // Assembled rather than templated, so an item missing half of these does not end up with
    // a note full of holes.
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = meta.get("scanner") {
        parts.push(format!("Scanned on {v}"));
    }
    if let Some(v) = meta.get("camera") {
        parts.push(format!("with a {v}"));
    }
    if let Some(v) = meta.get("scanningcenter") {
        parts.push(format!("at the {v} centre"));
    }

    Scan {
        file: Some(format!("{id}.pdf")),
        // `imagecount` counts scanned images, covers and endpapers included, which is exactly
        // what `count` means: pages in the container, not printed pages.
        count: meta.get("imagecount").and_then(|v| v.parse().ok()),
        by: Some("Internet Archive".into()),
        url: Some(format!("https://archive.org/details/{id}")),
        ppi: meta.get("ppi").and_then(|v| v.parse().ok()),
        note: (!parts.is_empty()).then(|| format!("{}.", parts.join(" "))),
    }
}

fn links(meta: &Meta) -> Vec<Link> {
    let mut out = Vec::new();
    if let Some(url) = meta.get("link_to_catalog") {
        out.push(Link {
            rel: "catalogue".into(),
            url: url.to_string(),
            title: None,
            note: None,
        });
    }
    out
}

/// One note out of several sentences, or none.
fn join_notes(notes: Vec<String>) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for n in notes {
        let n = n.trim().to_string();
        if !n.is_empty() && !seen.contains(&n) {
            seen.push(n);
        }
    }
    if seen.is_empty() {
        return None;
    }
    // The cataloguer's own notes rarely end in a full stop; the sentences read as a paragraph
    // once they do.
    let joined = seen
        .iter()
        .map(|n| {
            if n.ends_with(['.', '!', '?']) {
                n.clone()
            } else {
                format!("{n}.")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Some(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(pairs: &[(&str, &str)]) -> Meta {
        Meta::from_pairs(pairs.iter().copied())
    }

    #[test]
    fn a_minimal_record_still_has_an_id_a_title_and_a_scan() {
        let m = map("x00unse", &meta(&[("title", "Sur les subsistances")]));
        assert_eq!(m.record.id.as_deref(), Some("x00unse"));
        assert_eq!(m.record.title, "Sur les subsistances");
        assert_eq!(
            m.record.scan.as_ref().unwrap().file.as_deref(),
            Some("x00unse.pdf")
        );
        assert_eq!(
            m.record.identifier.as_ref().unwrap()["internet_archive"],
            "x00unse"
        );
    }

    /// An item with no title at all must still produce a valid record, because `title` is
    /// required on a source and 38,377 items will contain a few.
    #[test]
    fn a_missing_title_does_not_produce_an_empty_one() {
        let m = map("x", &meta(&[]));
        assert_eq!(m.record.title, "Untitled");
    }

    // -- dates ---------------------------------------------------------------------------

    #[test]
    fn a_bare_year_passes_through() {
        let m = map("x", &meta(&[("date", "1789")]));
        assert_eq!(m.record.date.as_deref(), Some("1789"));
        assert_eq!(m.record.note, None);
    }

    #[test]
    fn a_full_date_passes_through() {
        let m = map("x", &meta(&[("date", "1789-05-04")]));
        assert_eq!(m.record.date.as_deref(), Some("1789-05-04"));
    }

    /// The typo is load-bearing: for some records it is the only date present.
    #[test]
    fn the_dfate_typo_is_read_as_a_date() {
        let m = map("x", &meta(&[("dfate", "1789")]));
        assert_eq!(m.record.date.as_deref(), Some("1789"));
    }

    #[test]
    fn a_real_date_wins_over_the_typo() {
        let m = map("x", &meta(&[("date", "1790"), ("dfate", "1789")]));
        assert_eq!(m.record.date.as_deref(), Some("1790"));
    }

    /// A year recovered from prose is kept, and so is the prose — it usually says something
    /// the year does not.
    #[test]
    fn a_year_inside_prose_is_recovered_and_the_prose_is_kept() {
        let m = map("x", &meta(&[("date", "l'an II de la liberté, 1790")]));
        assert_eq!(m.record.date.as_deref(), Some("1790"));
        assert!(
            m.record.note.as_deref().unwrap().contains("l'an II"),
            "{:?}",
            m.record.note
        );
    }

    /// Guessing here would put a false date on a citation, which is the one thing this
    /// archive must not do.
    #[test]
    fn an_unreadable_date_is_left_off_rather_than_guessed() {
        let m = map("x", &meta(&[("date", "n.d.")]));
        assert_eq!(m.record.date, None);
        assert!(m.record.note.as_deref().unwrap().contains("n.d."));
    }

    // -- publisher and place -------------------------------------------------------------

    #[test]
    fn the_isbd_separator_splits_place_from_publisher() {
        let m = map(
            "x",
            &meta(&[("publisher", "Paris : De l'impr. de Cailleau, rue Galande, no. 64")]),
        );
        assert_eq!(m.record.place.as_deref(), Some("Paris"));
        let resp = m.record.resp.as_ref().unwrap();
        assert!(
            resp.iter()
                .any(|r| r.roles() == ["publisher"] && r.name.starts_with("De l'impr.")),
            "{resp:?}"
        );
    }

    /// Brackets mean the cataloguer supplied the place. The place is the same fact either way.
    #[test]
    fn a_supplied_place_loses_its_brackets() {
        let m = map("x", &meta(&[("publisher", "[Paris] : Se vend chez les libraires")]));
        assert_eq!(m.record.place.as_deref(), Some("Paris"));
    }

    #[test]
    fn a_publisher_with_no_separator_yields_no_place() {
        let m = map("x", &meta(&[("publisher", "Chez Baudouin")]));
        assert_eq!(m.record.place, None);
        assert!(
            m.record
                .resp
                .as_ref()
                .unwrap()
                .iter()
                .any(|r| r.name == "Chez Baudouin")
        );
    }

    // -- responsibility ------------------------------------------------------------------

    /// The role glued to the end of a name is exactly the unparseable string this schema
    /// exists to replace.
    #[test]
    fn a_trailing_role_is_lifted_out_of_the_name() {
        let m = map(
            "x",
            &meta(&[("creator", "Cailleau, André-Charles, 1731-1798, printer")]),
        );
        let r = &m.record.resp.as_ref().unwrap()[0];
        assert_eq!(r.name, "Cailleau, André-Charles, 1731-1798");
        assert_eq!(r.roles(), ["printer"]);
    }

    #[test]
    fn a_creator_with_no_role_is_an_author() {
        let m = map("x", &meta(&[("creator", "Vidaillet")]));
        assert_eq!(m.record.resp.as_ref().unwrap()[0].roles(), ["author"]);
    }

    /// IA repeats a creator verbatim, and repeats it with a trailing full stop. Neither is a
    /// second person.
    #[test]
    fn a_creator_repeated_with_a_full_stop_is_one_person() {
        let m = map(
            "x",
            &meta(&[
                ("creator", "Lambert, Charles, 1734-1816."),
                ("creator", "Lambert, Charles, 1734-1816"),
            ]),
        );
        assert_eq!(m.record.resp.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn a_corporate_creator_survives_intact() {
        let m = map(
            "x",
            &meta(&[("creator", "France. Assemblée nationale constituante (1789-1791)")]),
        );
        let r = &m.record.resp.as_ref().unwrap()[0];
        assert_eq!(r.name, "France. Assemblée nationale constituante (1789-1791)");
        assert_eq!(r.roles(), ["author"]);
    }

    // -- the rest ------------------------------------------------------------------------

    #[test]
    fn the_printed_extent_decides_pamphlet_from_book() {
        assert_eq!(
            map("x", &meta(&[("physical_description", "7 p. ; 18 cm.")]))
                .record
                .r#type
                .as_deref(),
            Some("pamphlet")
        );
        assert_eq!(
            map("x", &meta(&[("physical_description", "312 p. ; 22 cm.")]))
                .record
                .r#type
                .as_deref(),
            Some("book")
        );
        // Unreadable extent: call it what all but a few hundred of them are.
        assert_eq!(
            map("x", &meta(&[("physical_description", "1 sheet")]))
                .record
                .r#type
                .as_deref(),
            Some("pamphlet")
        );
    }

    #[test]
    fn language_codes_become_bcp47_and_unknown_ones_are_left_off() {
        assert_eq!(
            map("x", &meta(&[("language", "fre")])).record.language.as_deref(),
            Some("fr")
        );
        assert_eq!(map("x", &meta(&[("language", "zzz")])).record.language, None);
    }

    #[test]
    fn subjects_and_extent_are_carried() {
        let m = map(
            "x",
            &meta(&[
                ("subject", "Nobility"),
                ("subject", "Titles of honor and nobility"),
                ("subject", "Nobility"),
                ("physical_description", "46 p. ; 22 cm."),
            ]),
        );
        assert_eq!(m.record.subject, ["Nobility", "Titles of honor and nobility"]);
        assert_eq!(m.record.extent.as_deref(), Some("46 p. ; 22 cm."));
    }

    #[test]
    fn the_scan_carries_its_resolution_and_image_count() {
        let m = map("x", &meta(&[("imagecount", "50"), ("ppi", "500")]));
        let s = m.record.scan.as_ref().unwrap();
        assert_eq!(s.count, Some(50));
        assert_eq!(s.ppi, Some(500));
    }

    #[test]
    fn the_newberry_bucket_becomes_a_collection_a_reader_would_cite() {
        let m = map(
            "x",
            &meta(&[
                ("collection", "newberryfrenchpamphlets"),
                ("collection", "americana"),
                ("contributor", "The Newberry Library"),
                ("call_number", "Case FRC 26109"),
            ]),
        );
        let h = m.record.holding.as_ref().unwrap();
        assert_eq!(h.collection.as_deref(), Some("French Revolution Collection"));
        assert_eq!(h.repository.as_deref(), Some("The Newberry Library"));
        assert_eq!(h.shelfmark.as_deref(), Some("Case FRC 26109"));
    }

    #[test]
    fn prose_fields_are_gathered_into_one_note() {
        let m = map(
            "x",
            &meta(&[
                ("description", "Signed: Vidaillet"),
                ("description", "Publisher statement from colophon"),
                ("citation", "Martin & Walter. Révolution française, IV:1, 33504"),
                ("notes", "No copyright page found"),
            ]),
        );
        let note = m.record.note.as_deref().unwrap();
        assert!(note.contains("Signed: Vidaillet."), "{note}");
        assert!(note.contains("Martin & Walter"), "{note}");
        assert!(note.contains("Scanner's note:"), "{note}");
    }

    /// A field that starts mattering must be noticed rather than silently dropped 38,377
    /// times.
    #[test]
    fn unmapped_fields_are_reported() {
        let m = map("x", &meta(&[("title", "t"), ("something_new", "v"), ("ppi", "500")]));
        assert_eq!(m.ignored, ["something_new"]);
    }
}
