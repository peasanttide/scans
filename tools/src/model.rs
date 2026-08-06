//! The archive record model.
//!
//! This is the single definition that drives three things at once:
//!
//! 1. Rust type checking,
//! 2. TOML deserialisation (via serde),
//! 3. `schemas/source.json` (via schemars).
//!
//! The JSON Schema is **generated** from these types by `scans schema`. It must never be
//! hand-edited, or it will drift from the validator and start lying. `scans schema --check`
//! is the guard.
//!
//! Every record type carries `#[serde(deny_unknown_fields)]`. This archive is hand-maintained,
//! so a typo must be an error rather than a silently ignored key.
//!
//! ## EDTF-valued fields are typed `String` here, on purpose
//!
//! `date`, `covers` and `founded` are plain `String`. They are *not* parsed during
//! deserialisation, so a malformed date is a validation finding (`E601`) naming the file and
//! field, not an opaque TOML parse failure. It also keeps [`crate::edtf`] the single authority
//! on what EDTF means: a regex in the JSON Schema would be a second, drifting definition of
//! the same grammar.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------
// The record enum
// ---------------------------------------------------------------------------------------

/// One archive file.
///
/// Internally tagged on `layer`, which makes `layer` the keystone discriminator: serde uses it
/// to pick the variant, and schemars turns it into a `oneOf` whose branches each pin
/// `layer` to a `const`. That is what gives per-layer autocomplete and per-layer required
/// fields in the editor.
///
/// `layer = "page"` is deliberately **not** a variant. Pages are declared inline as
/// `[[page]]` and never exist as files (spec: "Page", settled decision 4). An attempt to
/// write one is rejected by serde as an unknown variant, which is finding `E015`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "layer", rename_all = "lowercase")]
pub enum Record {
    /// The publication or work, independent of any copy.
    Source(Source),
    /// One physical object that was digitised.
    ///
    /// The inner type is named `CopyRecord` rather than `Copy` so it cannot be confused with
    /// the `std::marker::Copy` trait in type position. The serde tag is taken from the
    /// *variant* name, so the TOML still reads `layer = "copy"`.
    Copy(CopyRecord),
    /// The citable intellectual unit.
    Document(Document),
}

/// Which layer a record is. Ordered `source` < `copy` < `document` < `page`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Source,
    Copy,
    Document,
    /// Never appears on a file; exists so layer-rank comparisons (check 2) can name it.
    Page,
}

impl Layer {
    /// Rank used by check 2. A child's layer must rank at or below its parent's.
    pub fn rank(self) -> u8 {
        match self {
            Layer::Source => 0,
            Layer::Copy => 1,
            Layer::Document => 2,
            Layer::Page => 3,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Source => "source",
            Layer::Copy => "copy",
            Layer::Document => "document",
            Layer::Page => "page",
        }
    }
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------------------
// Layer: source
// ---------------------------------------------------------------------------------------

/// `layer = "source"` — the publication or work.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Stable flat id. Defaults to the filename stem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Id of an ancestor. A source may be `of` another source (e.g. a series).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    /// Genre: `newspaper`, `map`, `diary`, … Open vocabulary, not validated.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_title: Option<String>,
    /// BCP-47 / ISO-639-1 code. Inherits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Place of publication. Inherits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// Does not inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// EDTF. Date the publication began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub founded: Option<String>,
    /// `daily`, `weekly`, … Free string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency: Option<String>,
    /// EDTF. Date of the work, for single-work sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// EDTF interval. The span this source covers. Does not inherit.
    ///
    /// This is where the legacy `held = "1789"` field lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covers: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Canonical landing page. Does not inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Terse page range. Legal but unusual on a source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<PageRange>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights: Option<Rights>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holding: Option<Holding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<Identifier>,
    /// Legal on a source when the copy layer is collapsed in (Turgot, Verniquet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<Scan>,

    /// Inherits wholesale. `Option` rather than a bare `Vec` because `resp = []` is the
    /// documented way to *clear* an inherited value, and that must be distinguishable from
    /// "not declared here".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resp: Option<Vec<Resp>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link: Vec<Link>,
    /// Legal on a source when copy and document are collapsed in (Turgot: 21 pages).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page: Vec<Page>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<Text>,
}

// ---------------------------------------------------------------------------------------
// Layer: copy
// ---------------------------------------------------------------------------------------

/// `layer = "copy"` — one physical object that was digitised.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Id of the source (or another copy). Absent = orphan copy; legal, warns `W107`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    /// Physical genre: `volume`, `roll`, `folder`. Open vocabulary.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// EDTF interval. The date span of material bound in this object. Used by check 6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covers: Option<String>,
    /// EDTF. Date of the object itself, if meaningful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// The normal home for `scan`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<Scan>,
    /// The normal home for `holding`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holding: Option<Holding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<Identifier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights: Option<Rights>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resp: Option<Vec<Resp>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page: Vec<Page>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<Text>,
}

// ---------------------------------------------------------------------------------------
// Layer: document
// ---------------------------------------------------------------------------------------

/// `layer = "document"` — the citable intellectual unit.
///
/// Note `title` is **not** required: a terse issue file has none.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Document {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Id of **any** ancestor — copy, source, or another document.
    /// Absent = standalone document (the one-off engraving case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    /// `issue`, `supplement`, `sheet`, `letter`, `play`, `engraving`, … Open vocabulary.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_title: Option<String>,
    /// Issue/serial number. Used by check 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no: Option<i64>,
    /// EDTF. Used by check 6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// EDTF interval, for documents spanning a range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covers: Option<String>,
    /// Id of another **document**. Check 8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supplement_to: Option<String>,
    /// Terse page range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<PageRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Inheritable scalar; present here so a document can override an ancestor's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Inheritable scalar; present here so a document can override an ancestor's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights: Option<Rights>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holding: Option<Holding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<Identifier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<Scan>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resp: Option<Vec<Resp>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page: Vec<Page>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<Text>,
}

// ---------------------------------------------------------------------------------------
// Inline structures
// ---------------------------------------------------------------------------------------

/// A terse page range, `pages = { from = 13, to = 16 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageRange {
    pub from: i64,
    pub to: i64,
}

impl PageRange {
    /// Number of pages the range covers. Meaningless unless `to >= from`.
    ///
    /// Saturating, not wrapping: these values come from a hand-edited file, so
    /// `from = i64::MIN, to = i64::MAX` is reachable by typo and must not overflow
    /// before [`crate::load`] gets the chance to reject it.
    pub fn len(self) -> i64 {
        self.to.saturating_sub(self.from).saturating_add(1)
    }

    pub fn is_empty(self) -> bool {
        self.to < self.from
    }
}

/// A statement of responsibility. Replaces the unparseable `author` string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Resp {
    /// Person or corporate name as written.
    pub name: String,
    /// One role or several. Both spellings are legal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Roles>,
    /// Qualifications such as "attributed".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Resp {
    /// Roles normalised to a list, which is how the resolved model treats them.
    pub fn roles(&self) -> Vec<&str> {
        match &self.role {
            None => Vec::new(),
            Some(Roles::One(r)) => vec![r.as_str()],
            Some(Roles::Many(rs)) => rs.iter().map(String::as_str).collect(),
        }
    }
}

/// `role = "engraver"` or `role = ["surveyor", "draughtsman"]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Roles {
    One(String),
    Many(Vec<String>),
}

/// Everything that is not the canonical `url`. Retires ad-hoc `index` and `fetch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// Relation type. Open vocabulary: `index`, `catalogue`, `viewer`, `download`, `iiif`,
    /// `thumbnail`, `about`, `worldcat`.
    pub rel: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Rights status. Inherits, merged key by key.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rights {
    /// Rights status of the intellectual work, e.g. `PD-old-100-expired`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
    /// Rights status of the digitisation itself, distinct from the work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<String>,
    /// Credit line, e.g. `Digitised by Google Books.`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Where the physical object lives. Inherits, merged key by key.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Holding {
    /// Institution, e.g. `Bibliothèque cantonale et universitaire, Lausanne`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Call number. **Always a string**, never an int — leading zeros and letters occur.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shelfmark: Option<String>,
    /// The named collection within the repository, e.g. `David Rumsey Map Collection`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An **open** table of catalogue-identifier keys to string values. Inherits, merged key by key.
///
/// Values are always strings so identifiers with leading zeros survive round-trip.
pub type Identifier = BTreeMap<String, String>;

/// The digitisation event and its container file.
///
/// This is the one table with **selective** key inheritance: `file`, `by` and `url` inherit;
/// `count` and `note` do not. See [`crate::load`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scan {
    /// Path to the container, relative to the directory of the file that **declared** it.
    /// Inherits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Number of pages in the container. Used by check 5. Does not inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    /// Digitising agent, e.g. `Google Books`. Inherits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Landing page for the digitisation, distinct from top-level `url`. Inherits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Caveats. Does not inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// OCR or transcription. Never inline content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Text {
    /// Path to the text file, resolved against the declaring file's directory.
    pub file: String,
    /// `ocr` or `transcription`. Defaults to `ocr` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Producing agent and version, e.g. `tesseract 5.3 fra`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Text {
    pub const DEFAULT_KIND: &'static str = "ocr";

    pub fn kind_or_default(&self) -> &str {
        self.kind.as_deref().unwrap_or(Self::DEFAULT_KIND)
    }
}

/// One scanned side, carrying graphics. **Declared inline only** — there is never a page file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Page {
    /// Page number **within its owning document**. This is what `.pN` matches.
    /// Omitted means "continue the count". May be `0` (Turgot) or negative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The printed page label when it differs from `n`, e.g. roman `xxxvj`.
    /// Purely descriptive: never parsed, never used for addressing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The page's own landing page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Zero or more. Zero is legal (a text-only page) and emits `W904`.
    /// The **first** entry is the primary graphic: the one `.pN` resolves to and the one
    /// whose pixel space zone coordinates are in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphic: Vec<Graphic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<Text>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zone: Vec<Zone>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link: Vec<Link>,
}

/// One image file, or one page inside a multi-page container.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Graphic {
    /// Image or container path. Omitted means "fall back to the resolved `scan.file`".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based page index **inside** `file`. Omitted when `file` is a standalone image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    /// Pixel width of **this page**, not of the container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    /// Pixel height of **this page**, not of the container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    /// Direct download URL for this exact image (the retired `fetch` field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Explicit media type; otherwise inferred from the extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mimetype: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A rectangle on a page, in the pixel space of that page's **primary graphic**.
///
/// Zones are not addressable and not citable; nothing in the address grammar reaches them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Zone {
    /// Page-local label. Not a global id, not addressable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Upper-left x, pixels, origin top-left.
    pub ulx: i64,
    /// Upper-left y.
    pub uly: i64,
    /// Lower-right x, exclusive.
    pub lrx: i64,
    /// Lower-right y, exclusive.
    pub lry: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------------------
// Uniform access across the three variants
// ---------------------------------------------------------------------------------------

/// Read a field that exists on all three variants.
macro_rules! all_variants {
    ($self:ident, $field:ident) => {
        match $self {
            Record::Source(r) => &r.$field,
            Record::Copy(r) => &r.$field,
            Record::Document(r) => &r.$field,
        }
    };
}

impl Record {
    pub fn layer(&self) -> Layer {
        match self {
            Record::Source(_) => Layer::Source,
            Record::Copy(_) => Layer::Copy,
            Record::Document(_) => Layer::Document,
        }
    }

    /// The explicitly declared `id`, if any. The effective id (falling back to the filename
    /// stem) lives on [`crate::load::Node`].
    pub fn declared_id(&self) -> Option<&str> {
        all_variants!(self, id).as_deref()
    }

    pub fn of(&self) -> Option<&str> {
        all_variants!(self, of).as_deref()
    }

    pub fn r#type(&self) -> Option<&str> {
        all_variants!(self, r#type).as_deref()
    }

    pub fn title(&self) -> Option<&str> {
        match self {
            Record::Source(r) => Some(r.title.as_str()),
            Record::Copy(r) => Some(r.title.as_str()),
            Record::Document(r) => r.title.as_deref(),
        }
    }

    pub fn short_title(&self) -> Option<&str> {
        all_variants!(self, short_title).as_deref()
    }

    pub fn language(&self) -> Option<&str> {
        all_variants!(self, language).as_deref()
    }

    pub fn place(&self) -> Option<&str> {
        all_variants!(self, place).as_deref()
    }

    pub fn country(&self) -> Option<&str> {
        all_variants!(self, country).as_deref()
    }

    pub fn note(&self) -> Option<&str> {
        all_variants!(self, note).as_deref()
    }

    pub fn url(&self) -> Option<&str> {
        all_variants!(self, url).as_deref()
    }

    pub fn date(&self) -> Option<&str> {
        all_variants!(self, date).as_deref()
    }

    pub fn covers(&self) -> Option<&str> {
        all_variants!(self, covers).as_deref()
    }

    /// Only a source has `founded`.
    pub fn founded(&self) -> Option<&str> {
        match self {
            Record::Source(r) => r.founded.as_deref(),
            _ => None,
        }
    }

    /// Only a document has `no`.
    pub fn no(&self) -> Option<i64> {
        match self {
            Record::Document(r) => r.no,
            _ => None,
        }
    }

    /// Only a document has `supplement_to`.
    pub fn supplement_to(&self) -> Option<&str> {
        match self {
            Record::Document(r) => r.supplement_to.as_deref(),
            _ => None,
        }
    }

    /// A terse page range. A copy never has one.
    pub fn pages(&self) -> Option<PageRange> {
        match self {
            Record::Source(r) => r.pages,
            Record::Copy(_) => None,
            Record::Document(r) => r.pages,
        }
    }

    pub fn rights(&self) -> Option<&Rights> {
        all_variants!(self, rights).as_ref()
    }

    pub fn holding(&self) -> Option<&Holding> {
        all_variants!(self, holding).as_ref()
    }

    pub fn identifier(&self) -> Option<&Identifier> {
        all_variants!(self, identifier).as_ref()
    }

    pub fn scan(&self) -> Option<&Scan> {
        all_variants!(self, scan).as_ref()
    }

    /// `None` = not declared here. `Some(&[])` = declared empty, which **clears** the
    /// inherited value.
    pub fn resp(&self) -> Option<&[Resp]> {
        all_variants!(self, resp).as_deref()
    }

    pub fn link(&self) -> &[Link] {
        all_variants!(self, link)
    }

    pub fn page(&self) -> &[Page] {
        all_variants!(self, page)
    }

    pub fn text(&self) -> &[Text] {
        all_variants!(self, text)
    }
}

// ---------------------------------------------------------------------------------------
// JSON Schema generation
// ---------------------------------------------------------------------------------------

/// Generate the JSON Schema for [`Record`].
///
/// This is the only place `schemas/source.json` comes from. `scans schema --check` compares
/// the file on disk against this, which is what stops the schema drifting from the validator.
pub fn json_schema() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12();
    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<Record>();
    let mut value = serde_json::to_value(schema).expect("schema serialises");

    strip_null_unions(&mut value);

    if let Some(obj) = value.as_object_mut() {
        obj.insert("title".into(), serde_json::json!("Scan archive record"));
        obj.insert(
            "description".into(),
            serde_json::json!(
                "One TOML file in the primary source and scan archive. Generated from the Rust \
                 types in tools/src/model.rs by `scans schema` - do not edit by hand."
            ),
        );
    }
    value
}

/// Remove every trace of `null` from the generated schema.
///
/// TOML has no null. An optional key is simply absent, and the schema already says so by
/// leaving the key out of `required`. Without this, an editor offers `null` as a valid
/// completion for every optional field — which would be wrong in every case.
///
/// `Option<T>` reaches us in two shapes, depending on whether `T` is inlined or referenced:
///
/// * `"type": ["string", "null"]` — collapsed to `"type": "string"`.
/// * `"anyOf": [{"$ref": …}, {"type": "null"}]` — the null branch is dropped and the single
///   surviving branch is merged into its parent, so sibling keys such as `description`
///   survive.
fn strip_null_unions(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(types)) = map.get("type") {
                let non_null: Vec<_> = types.iter().filter(|t| *t != "null").cloned().collect();
                if non_null.len() < types.len() {
                    let replacement = if non_null.len() == 1 {
                        non_null.into_iter().next().expect("length checked")
                    } else {
                        serde_json::Value::Array(non_null)
                    };
                    map.insert("type".into(), replacement);
                }
            }

            for combinator in ["anyOf", "oneOf"] {
                let Some(serde_json::Value::Array(branches)) = map.get(combinator) else {
                    continue;
                };
                let kept: Vec<_> = branches
                    .iter()
                    .filter(|b| !is_null_schema(b))
                    .cloned()
                    .collect();
                if kept.len() == branches.len() {
                    continue;
                }
                match kept.len() {
                    1 => {
                        map.remove(combinator);
                        if let Some(only) = kept.into_iter().next().and_then(|b| match b {
                            serde_json::Value::Object(o) => Some(o),
                            _ => None,
                        }) {
                            for (k, v) in only {
                                map.entry(k).or_insert(v);
                            }
                        }
                    }
                    _ => {
                        map.insert(combinator.into(), serde_json::Value::Array(kept));
                    }
                }
            }

            for v in map.values_mut() {
                strip_null_unions(v);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                strip_null_unions(v);
            }
        }
        _ => {}
    }
}

/// A schema branch that permits nothing but `null`.
fn is_null_schema(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.get("type").is_some_and(|t| t == "null"))
}

/// Serialise the schema exactly as `scans schema` writes it, so `--check` compares like for
/// like.
pub fn json_schema_text() -> String {
    let mut s = serde_json::to_string_pretty(&json_schema()).expect("schema serialises");
    s.push('\n');
    s
}
