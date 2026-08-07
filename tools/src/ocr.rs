//! `<id>.pN.ocr.md` — one scanned page of OCR, as markdown with YAML frontmatter.
//!
//! ## One file, one page
//!
//! A page is the unit. The corpus holds 840,810 of them across 38,377 items. One file per
//! item put a 3,378-page bound run of nine volumes into a single 49 MB text file, which is
//! unreadable in an editor and unreviewable in a diff; per page, nothing is more than a few
//! kilobytes, and `<id>.pN` matches the address grammar the archive already uses.
//!
//! ## The shape
//!
//! ```markdown
//! ---
//! of: abondance00unse
//! page: 6
//! engine: ABBYY FineReader 11.0
//! lang: fr
//! w: 2597
//! h: 4418
//! dpi: 500
//! ---
//!
//! ABONDANCE NATIONALE,
//! ou, Découvertes d'artillerie
//! ```
//!
//! Frontmatter says which page of what, and how big the image is. The body is the page's
//! text. That is the whole format.
//!
//! ## What is deliberately not here
//!
//! Word coordinates, per-word confidence, baselines, and DjVu's four levels of nesting. An
//! earlier version of this format carried all of it, packed into parallel arrays, and was
//! losslessly convertible back to the `_djvu.xml` it came from. It is gone at the repository
//! owner's direction: the boxes were not being used, and they were seven eighths of the
//! bytes — 8.3 GB against about 1.1 GB for the text alone.
//!
//! Nothing is destroyed by that. The coordinates still exist in `XML_for_OCR/` in
//! [frc-data], which is one clone away, so a future version that wants them back can have
//! them without asking archive.org for anything.
//!
//! [frc-data]: https://github.com/NewberryDIS/frc-data
//!
//! ## Scalars are bare unless they would lie
//!
//! The frontmatter reads as plain YAML — `lang: fr`, not `lang: "fr"` — because that is what
//! it is for. But YAML's plain scalars are a minefield of values that mean something other
//! than themselves: `no` is the boolean false, and it is also the ISO code for Norwegian.
//! [`yaml_scalar`] emits a value bare when it round-trips as itself and quotes it when it
//! would not, so the common case looks like the example above and the pathological case is
//! still correct.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Path of the generated OCR schema, relative to the repository root.
pub const SCHEMA_PATH: &str = "schemas/ocr.json";

/// The suffix that marks an OCR sidecar.
pub const SUFFIX: &str = ".ocr.md";

// ---------------------------------------------------------------------------------------
// The file model
// ---------------------------------------------------------------------------------------

/// One `<id>.pN.ocr.md` — the OCR of a single scanned page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ocr {
    /// Id of the record this page belongs to.
    pub of: String,
    /// 1-based page index within the scanned container, matching `.pN` addressing.
    pub page: i64,
    /// Producing agent and version, e.g. `ABBYY FineReader 11.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// BCP-47 code of the text, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Pixel width of the page image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<i64>,
    /// Pixel height of the page image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<i64>,
    /// Resolution the page was scanned at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpi: Option<i64>,

    /// The page's text — the markdown body.
    ///
    /// **Not in the frontmatter.** `serde` skips it here so it cannot appear in both places
    /// and disagree with itself. It is now the only content this format carries, so unlike
    /// the previous version it is authoritative rather than derived, and there is nothing
    /// left for a consistency check to compare it against.
    #[serde(skip)]
    pub text: String,
}

/// The filename a page's OCR lives in, relative to the item's directory.
pub fn file_name(id: &str, page: i64) -> String {
    format!("{id}.p{page}{SUFFIX}")
}

// ---------------------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------------------

/// Render one page as markdown with YAML frontmatter.
///
/// `schema_rel` is the path from this file back to `schemas/ocr.json`, emitted as a
/// `yaml-language-server` directive — YAML's equivalent of the `#:schema` line the records
/// carry, and what gets the frontmatter validated and completed in an editor.
pub fn to_markdown(ocr: &Ocr, schema_rel: &str) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    let _ = writeln!(s, "# yaml-language-server: $schema={schema_rel}");
    let _ = writeln!(s, "of: {}", yaml_scalar(&ocr.of));
    let _ = writeln!(s, "page: {}", ocr.page);
    if let Some(engine) = &ocr.engine {
        let _ = writeln!(s, "engine: {}", yaml_scalar(engine));
    }
    if let Some(lang) = &ocr.lang {
        let _ = writeln!(s, "lang: {}", yaml_scalar(lang));
    }
    for (key, value) in [("w", ocr.w), ("h", ocr.h), ("dpi", ocr.dpi)] {
        if let Some(v) = value {
            let _ = writeln!(s, "{key}: {v}");
        }
    }
    s.push_str("---\n");

    // The body is the whole page and nothing else — no headings, no delimiters, nothing the
    // OCR could collide with.
    if !ocr.text.is_empty() {
        let _ = write!(s, "\n{}\n", ocr.text);
    }
    s
}

/// A YAML scalar: bare where that is unambiguous, quoted where it is not.
///
/// The bare form is the point — `lang: fr` reads as YAML rather than as an escaped blob. But
/// a bare scalar has to survive the round trip, and YAML will happily read `no` as `false`,
/// `1:30` as a sexagesimal integer, `~` as null, and a leading `%` as a directive. OCR of
/// worn 18th-century type produces exactly that debris, so the decision is made by asking
/// YAML itself whether the bare form parses back to the same string.
pub fn yaml_scalar(s: &str) -> String {
    if is_safe_bare(s) {
        return s.to_string();
    }
    yaml_quoted(s)
}

/// Words YAML 1.1 resolves to booleans or null, which YAML 1.2 leaves as strings.
///
/// `serde_norway` implements 1.2, so it reads `no` as the string it looks like. Most of the
/// world does not: PyYAML, libyaml, Ruby's Psych and Go's gopkg.in/yaml.v2 all implement 1.1,
/// where `no` is `false` — and `no` is also the ISO 639-1 code for Norwegian. This data is
/// published for other people's tools to read, so the bar is what *they* will do with it, not
/// what this crate's parser happens to accept.
const YAML11_RESERVED: &[&str] = &[
    "y", "Y", "yes", "Yes", "YES", "n", "N", "no", "No", "NO",
    "true", "True", "TRUE", "false", "False", "FALSE",
    "on", "On", "ON", "off", "Off", "OFF",
    "null", "Null", "NULL", "~",
];

/// Whether a value can be written bare.
///
/// Conservative on structure — anything with a leading or trailing space, a control
/// character, or a character YAML gives meaning to is quoted without further thought — then
/// checked against [`YAML11_RESERVED`], and finally round-tripped through the parser. The
/// round trip is what catches `1:30`, `0x10` and `2026-08-07` without this having to carry
/// its own copy of YAML's scalar-resolution rules, which would drift.
fn is_safe_bare(s: &str) -> bool {
    if s.is_empty() || s.trim() != s {
        return false;
    }
    if YAML11_RESERVED.contains(&s) {
        return false;
    }
    if s.chars().any(|c| {
        (c as u32) < 0x20
            || c as u32 == 0x7f
            || matches!(c, ':' | '#' | '"' | '\'' | '\\' | '{' | '}' | '[' | ']' | ',' | '&'
                | '*' | '!' | '|' | '>' | '%' | '@' | '`')
    }) {
        return false;
    }
    if s.starts_with('-') || s.starts_with('?') || s.starts_with('~') {
        return false;
    }
    // The authority on whether a bare scalar means itself is the YAML parser.
    matches!(
        serde_norway::from_str::<serde_norway::Value>(s),
        Ok(serde_norway::Value::String(round)) if round == s
    )
}

/// A YAML double-quoted string, escaped. The escapes are JSON's, which YAML accepts in full.
pub fn yaml_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------

/// Why an `.ocr.md` could not be read.
#[derive(Debug)]
pub enum ParseError {
    /// No `---` frontmatter block at the top of the file.
    NoFrontmatter,
    /// The frontmatter opened but never closed.
    UnterminatedFrontmatter,
    Yaml(serde_norway::Error),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::NoFrontmatter => {
                f.write_str("no YAML frontmatter: the file must begin with a line of ---")
            }
            ParseError::UnterminatedFrontmatter => {
                f.write_str("the YAML frontmatter opens but is never closed by a line of ---")
            }
            ParseError::Yaml(e) => write!(f, "frontmatter is not valid YAML: {e}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Split a markdown file into its frontmatter and its body.
///
/// Deliberately strict about the opening delimiter being the very first line. A file whose
/// frontmatter starts on line 2 is one someone has edited by hand and got slightly wrong, and
/// guessing at it would hide that.
pub fn split_frontmatter(text: &str) -> Result<(&str, &str), ParseError> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or(ParseError::NoFrontmatter)?;

    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Ok((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err(ParseError::UnterminatedFrontmatter)
}

/// Parse an `.ocr.md`.
pub fn from_markdown(text: &str) -> Result<Ocr, ParseError> {
    let (front, body) = split_frontmatter(text)?;
    let mut ocr: Ocr = serde_norway::from_str(front).map_err(ParseError::Yaml)?;
    ocr.text = body.trim_matches('\n').to_string();
    Ok(ocr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(text: &str) -> Ocr {
        Ocr {
            of: "abondance00unse".into(),
            page: 6,
            engine: Some("ABBYY FineReader 11.0".into()),
            lang: Some("fr".into()),
            w: Some(2597),
            h: Some(4418),
            dpi: Some(500),
            text: text.into(),
        }
    }

    #[test]
    fn the_filename_carries_the_page_number() {
        assert_eq!(file_name("abondance00unse", 3), "abondance00unse.p3.ocr.md");
    }

    /// The frontmatter should read as plain YAML, not as a wall of quoted blobs.
    #[test]
    fn ordinary_values_are_written_bare() {
        let text = to_markdown(&page("ABONDANCE"), "s.json");
        for line in [
            "of: abondance00unse",
            "page: 6",
            "engine: ABBYY FineReader 11.0",
            "lang: fr",
            "w: 2597",
            "h: 4418",
            "dpi: 500",
        ] {
            assert!(text.contains(line), "expected {line:?} in:\n{text}");
        }
    }

    #[test]
    fn a_page_round_trips() {
        let p = page("ABONDANCE NATIONALE,\nou, Découvertes");
        assert_eq!(from_markdown(&to_markdown(&p, "s.json")).unwrap(), p);
    }

    #[test]
    fn a_page_with_no_text_round_trips() {
        let mut p = page("");
        p.engine = None;
        p.lang = None;
        assert_eq!(from_markdown(&to_markdown(&p, "s.json")).unwrap(), p);
    }

    /// `no` is the ISO 639-1 code for Norwegian and, to every YAML 1.1 parser, the boolean
    /// false. `serde_norway` is 1.2 and would read it back correctly, but PyYAML would hand a
    /// Python user `False`, and this data is published for their tools rather than ours.
    #[test]
    fn a_language_code_that_yaml_reads_as_a_boolean_is_quoted() {
        let mut p = page("x");
        p.lang = Some("no".into());
        let text = to_markdown(&p, "s.json");
        assert!(text.contains(r#"lang: "no""#), "{text}");
        assert_eq!(from_markdown(&text).unwrap().lang.as_deref(), Some("no"));
    }

    /// Every value YAML would quietly reinterpret has to survive as the string it was.
    #[test]
    fn values_yaml_would_reinterpret_survive() {
        for hazard in [
            // YAML 1.1 booleans and nulls, in the casings real data uses.
            "no", "No", "NO", "yes", "Yes", "y", "n", "on", "off", "Off",
            "true", "True", "false", "False", "null", "Null", "~",
            // Resolved to non-strings by 1.1 and 1.2 alike.
            "1:30", "0x10", "0o17", ".inf", "nan", "2026-08-07", "007", "1_000",
            // Structurally unsafe.
            "", " leading", "trailing ", "a: b", "#comment", "%directive", "@reserved",
            "-dash", "*alias", "&anchor", "[list]", "{map}", "back\\slash", "quote\"d",
        ] {
            let mut p = page("x");
            p.engine = Some(hazard.to_string());
            let back = from_markdown(&to_markdown(&p, "s.json"))
                .unwrap_or_else(|e| panic!("{hazard:?} failed to parse: {e}"));
            assert_eq!(
                back.engine.as_deref(),
                Some(hazard),
                "{hazard:?} did not survive"
            );
        }
    }

    /// An identifier that is all digits would be read back as an integer.
    #[test]
    fn a_numeric_looking_identifier_is_quoted() {
        let mut p = page("x");
        p.of = "24516".into();
        let back = from_markdown(&to_markdown(&p, "s.json")).unwrap();
        assert_eq!(back.of, "24516");
    }

    /// Real identifiers in this corpus start with digits but are not numbers, and must not be
    /// quoted for no reason.
    #[test]
    fn an_identifier_starting_with_digits_stays_bare() {
        let mut p = page("x");
        p.of = "1789iemilseptcen00unse".into();
        let text = to_markdown(&p, "s.json");
        assert!(text.contains("of: 1789iemilseptcen00unse"), "{text}");
    }

    /// The body is the page and nothing else, so nothing in the OCR can be mistaken for a
    /// delimiter or for markup.
    #[test]
    fn a_body_that_looks_like_markup_is_still_just_text() {
        let p = page("## Page 3\n---\nof: someone-else");
        let back = from_markdown(&to_markdown(&p, "s.json")).unwrap();
        assert_eq!(back.text, p.text);
        assert_eq!(back.of, "abondance00unse");
    }

    /// A file someone has edited by hand and got slightly wrong must say so rather than be
    /// guessed at.
    #[test]
    fn malformed_frontmatter_is_an_error_not_a_guess() {
        assert!(matches!(
            from_markdown("of: x\npage: 1\n"),
            Err(ParseError::NoFrontmatter)
        ));
        assert!(matches!(
            from_markdown("---\nof: x\npage: 1\n"),
            Err(ParseError::UnterminatedFrontmatter)
        ));
        assert!(matches!(
            from_markdown("---\nof: [unclosed\n---\n"),
            Err(ParseError::Yaml(_))
        ));
    }
}
