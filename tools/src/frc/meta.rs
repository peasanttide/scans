//! Reading Internet Archive's `_meta.xml`.
//!
//! The file is flat: a `<metadata>` element containing one level of children, any of which
//! may repeat. `<creator>` appears once per author, `<subject>` once per heading, and
//! `<description>` once per note the cataloguer wrote. So the natural shape is a multimap
//! from element name to the values in document order, which is what [`Meta`] is.
//!
//! Nothing here interprets the values; that is [`super::record`]'s job. Keeping the two apart
//! means the mapping can be tested against a hand-written [`Meta`] without a file, and the
//! parser can be tested against awkward XML without caring what the fields mean.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use quick_xml::Reader;
use quick_xml::events::Event;

/// The fields of one `_meta.xml`, in document order within each name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    fields: BTreeMap<String, Vec<String>>,
}

impl Meta {
    /// The first value of a field, trimmed, or `None` if it is absent or blank.
    ///
    /// Blank is treated as absent throughout: IA records contain a fair number of empty
    /// elements, and an empty `title` is not a title.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.all(name).first().copied()
    }

    /// The first value of whichever of `names` is present, in the order given.
    ///
    /// Used where IA has more than one spelling for the same fact — including outright typos
    /// that are now load-bearing, such as `dfate` for `date`.
    pub fn first_of(&self, names: &[&str]) -> Option<&str> {
        names.iter().find_map(|n| self.get(n))
    }

    /// Every non-blank value of a field, trimmed, in document order.
    pub fn all(&self, name: &str) -> Vec<&str> {
        self.fields
            .get(name)
            .map(|vs| {
                vs.iter()
                    .map(|v| v.trim())
                    .filter(|v| !v.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every value of a field, de-duplicated, keeping first-seen order.
    ///
    /// IA records routinely repeat a creator verbatim, or repeat it once with a trailing full
    /// stop and once without. Exact duplicates are dropped here; near-duplicates are a
    /// judgement the mapping makes.
    pub fn unique(&self, name: &str) -> Vec<&str> {
        let mut seen = Vec::new();
        for v in self.all(name) {
            if !seen.contains(&v) {
                seen.push(v);
            }
        }
        seen
    }

    /// Whether a field is present at all.
    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// The names of every field present, for reporting what a mapping ignored.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.fields.keys().map(String::as_str)
    }

    /// Build one directly. For tests and for callers that already have the values.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (k, v) in pairs {
            fields.entry(k.into()).or_default().push(v.into());
        }
        Meta { fields }
    }
}

/// Parse a `_meta.xml`.
pub fn parse(xml: &str) -> Result<Meta> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // The element currently open, and the text collected inside it. Only one level below
    // `<metadata>` carries text, so a single slot is enough — but the text arrives in several
    // events when it contains an entity, so it must accumulate rather than assign.
    let mut open: Option<(String, String)> = None;

    loop {
        match reader
            .read_event()
            .with_context(|| format!("reading _meta.xml at byte {}", reader.buffer_position()))?
        {
            Event::Eof => break,

            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if name != "metadata" {
                    open = Some((name, String::new()));
                }
            }

            Event::Text(t) => {
                if let Some((_, buf)) = open.as_mut() {
                    buf.push_str(&t.decode().context("undecodable metadata text")?);
                }
            }

            // `&amp;` arrives as its own event, and these titles are full of ampersands —
            // "munitions, grains & farines". Dropping this arm deletes them all.
            Event::GeneralRef(r) => {
                if let Some((_, buf)) = open.as_mut() {
                    let text = match r.resolve_char_ref().ok().flatten() {
                        Some(c) => c.to_string(),
                        None => match String::from_utf8_lossy(r.as_ref()).as_ref() {
                            "amp" => "&".into(),
                            "lt" => "<".into(),
                            "gt" => ">".into(),
                            "quot" => "\"".into(),
                            "apos" => "'".into(),
                            other => format!("&{other};"),
                        },
                    };
                    buf.push_str(&text);
                }
            }

            Event::End(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if let Some((open_name, text)) = open.take()
                    && open_name == name
                {
                    fields.entry(open_name).or_default().push(text);
                }
            }

            _ => {}
        }
    }

    Ok(Meta { fields })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <language>fre</language>
  <title>Abondance nationale, ou, Découvertes d'artillerie, munitions, grains &amp; farines, &amp;c.</title>
  <creator>Tallien, Jean-Lambert, 1767-2399</creator>
  <creator>Vidaillet</creator>
  <creator>Cailleau, André-Charles, 1731-1798, printer</creator>
  <subject>Providence and government of God</subject>
  <identifier>abondance00unse</identifier>
  <imagecount>10</imagecount>
  <empty></empty>
  <blank>   </blank>
</metadata>
"#;

    #[test]
    fn repeated_elements_keep_document_order() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(
            m.all("creator"),
            [
                "Tallien, Jean-Lambert, 1767-2399",
                "Vidaillet",
                "Cailleau, André-Charles, 1731-1798, printer"
            ]
        );
    }

    /// These titles are full of ampersands, and an entity arrives as its own event.
    #[test]
    fn entities_survive() {
        let m = parse(SAMPLE).unwrap();
        assert!(
            m.get("title").unwrap().contains("grains & farines, &c."),
            "{:?}",
            m.get("title")
        );
    }

    /// IA records carry a good number of empty elements. An empty title is not a title.
    #[test]
    fn blank_values_read_as_absent() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.get("empty"), None);
        assert_eq!(m.get("blank"), None);
        assert!(!m.has("blank"));
        assert_eq!(m.get("missing"), None);
    }

    #[test]
    fn values_are_trimmed() {
        let m = parse("<metadata><date>  1789  </date></metadata>").unwrap();
        assert_eq!(m.get("date"), Some("1789"));
    }

    /// `dfate` is a real typo in real IA records, and it is the only date some of them have.
    #[test]
    fn first_of_falls_through_to_the_alternative_spelling() {
        let m = Meta::from_pairs([("dfate", "1789")]);
        assert_eq!(m.first_of(&["date", "dfate"]), Some("1789"));

        let both = Meta::from_pairs([("date", "1790"), ("dfate", "1789")]);
        assert_eq!(both.first_of(&["date", "dfate"]), Some("1790"));
    }

    #[test]
    fn unique_drops_exact_repeats_and_keeps_order() {
        let m = Meta::from_pairs([("creator", "Lambert"), ("creator", "Lambert"), ("creator", "Barère")]);
        assert_eq!(m.unique("creator"), ["Lambert", "Barère"]);
    }

    #[test]
    fn a_document_with_no_fields_is_not_an_error() {
        let m = parse("<metadata></metadata>").unwrap();
        assert_eq!(m.names().count(), 0);
    }
}
