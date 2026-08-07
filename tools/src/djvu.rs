//! Reading archive.org's `_djvu.xml` into page text.
//!
//! This is the only place that knows what DjVu XML looks like. [`crate::ocr`] knows the
//! markdown form; the two meet here.
//!
//! ## The source format
//!
//! ```xml
//! <OBJECT type="image/x.djvu" usemap="abondance00unse_0001.djvu" width="2544" height="4426">
//!  <PARAM name="DPI" value="500"/>
//!  <HIDDENTEXT>
//!   <PAGECOLUMN><REGION><PARAGRAPH><LINE>
//!     <WORD coords="278,1378,1726,1250" x-confidence="71">ABONDANC</WORD>
//! ```
//!
//! One `OBJECT` per page, and one element per word, nested
//! `PAGECOLUMN > REGION > PARAGRAPH > LINE > WORD`.
//!
//! ## What is taken, and what is not
//!
//! Taken: the page's dimensions, its resolution, and its words. **Not taken:** the word
//! boxes, the per-word confidence, and the baselines. An earlier version of this carried all
//! of it and could write the XML back out unchanged; the boxes were not being used and were
//! seven eighths of the bytes, so they are gone at the repository owner's direction. The
//! coordinates still exist in frc-data, one clone away, if a later version wants them.
//!
//! The nesting is still read, but only to lay the text out: a line break ends a line, and
//! anything above it leaves a blank line. That is the whole of what it is used for now.
//!
//! ## Why a real parser
//!
//! The rest of this crate hand-rolls its parsing. This does not, because 38,377 files of
//! machine-generated XML from a decade of changing derive pipelines is exactly the input that
//! finds the gap between "the format I read about" and "the bytes that arrived". `quick-xml`
//! is pure Rust with no C toolchain, which is the constraint that actually matters here.

use anyhow::{Context, Result};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::ocr::Ocr;

/// Where a word sits in the nesting, as counters within its page.
///
/// Compared between neighbouring words to work out what whitespace separates them; the
/// absolute values mean nothing on their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Nest {
    column: u32,
    region: u32,
    paragraph: u32,
    line: u32,
}

impl Nest {
    /// The whitespace between this word and the next.
    ///
    /// A change of line is a newline; a change of anything above it is a blank line, because
    /// the distinction between a paragraph, a region and a column is not one a reader of the
    /// text can act on.
    fn separator_to(self, next: Nest) -> &'static str {
        if (self.column, self.region, self.paragraph) != (next.column, next.region, next.paragraph)
        {
            "\n\n"
        } else if self.line != next.line {
            "\n"
        } else {
            " "
        }
    }
}

/// Parse a `_djvu.xml` into one [`Ocr`] per page.
///
/// `of` is the record id the pages belong to; the XML does not reliably state it.
pub fn parse(xml: &str, of: &str) -> Result<Vec<Ocr>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut pages: Vec<Ocr> = Vec::new();
    let mut nest = Nest::default();

    // The page under construction, and the words collected for it so far.
    let mut cur: Option<Ocr> = None;
    let mut words: Vec<(String, Nest)> = Vec::new();
    // Set while inside a <WORD>: its text arrives as separate events, and in more than one
    // piece when it contains an entity reference.
    let mut in_word = false;

    loop {
        let event = reader.read_event().with_context(|| {
            format!("reading {of}_djvu.xml at byte {}", reader.buffer_position())
        })?;

        // `<WORD .../>` is self-closing and has no text; `<WORD ...>x</WORD>` does.
        let opens = matches!(event, Event::Start(_));

        match event {
            Event::Eof => break,

            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"OBJECT" => {
                    flush(&mut pages, &mut cur, &mut words);
                    cur = Some(Ocr {
                        of: of.to_string(),
                        page: pages.len() as i64 + 1,
                        w: attr_i64(&e, b"width")?,
                        h: attr_i64(&e, b"height")?,
                        ..Ocr::default()
                    });
                    nest = Nest::default();
                }
                b"PARAM" => {
                    // <PARAM name="DPI" value="500"/>
                    if attr(&e, b"name")?.as_deref() == Some("DPI")
                        && let Some(page) = cur.as_mut()
                        && let Some(v) = attr(&e, b"value")?
                    {
                        page.dpi = v.trim().parse::<i64>().ok();
                    }
                }
                b"PAGECOLUMN" => nest.column += 1,
                b"REGION" => nest.region += 1,
                b"PARAGRAPH" => nest.paragraph += 1,
                b"LINE" => nest.line += 1,
                b"WORD" => {
                    words.push((String::new(), nest));
                    in_word = opens;
                }
                _ => {}
            },

            Event::End(e) if e.local_name().as_ref() == b"WORD" => in_word = false,

            Event::Text(t) if in_word => {
                let s = t
                    .decode()
                    .with_context(|| format!("{of}: undecodable word text"))?;
                if let Some((word, _)) = words.last_mut() {
                    word.push_str(&s);
                }
            }

            // `&amp;` and friends arrive as their own event rather than inside the text, so a
            // word containing one is split across three events. Dropping this arm silently
            // deletes every ampersand in the corpus.
            Event::GeneralRef(r) if in_word => {
                let text = match r.resolve_char_ref().ok().flatten() {
                    Some(c) => c.to_string(),
                    None => match String::from_utf8_lossy(r.as_ref()).as_ref() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        // DjVu XML declares no entities of its own, so anything else is
                        // damage. Keep it as written rather than dropping the characters.
                        other => format!("&{other};"),
                    },
                };
                if let Some((word, _)) = words.last_mut() {
                    word.push_str(&text);
                }
            }

            _ => {}
        }
    }

    flush(&mut pages, &mut cur, &mut words);
    Ok(pages)
}

/// Close off the page under construction, laying its words out as text, and push it.
fn flush(pages: &mut Vec<Ocr>, cur: &mut Option<Ocr>, words: &mut Vec<(String, Nest)>) {
    let Some(mut page) = cur.take() else {
        // A prologue before the first `<OBJECT>`. Whatever was collected belongs to no page.
        words.clear();
        return;
    };

    let mut text = String::new();
    for (i, (word, nest)) in words.iter().enumerate() {
        text.push_str(word);
        if let Some((_, next)) = words.get(i + 1) {
            text.push_str(nest.separator_to(*next));
        }
    }
    // ABBYY puts a trailing space inside the element often enough that the joins double up.
    page.text = text.trim_end().to_string();

    words.clear();
    pages.push(page);
}

fn attr(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Result<Option<String>> {
    for a in e.attributes().with_checks(false) {
        let a = a.context("unreadable attribute")?;
        if a.key.as_ref() == name {
            return Ok(Some(
                a.unescape_value().context("undecodable attribute")?.into_owned(),
            ));
        }
    }
    Ok(None)
}

fn attr_i64(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Result<Option<i64>> {
    Ok(attr(e, name)?.and_then(|v| v.trim().parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<DjVuXML>
 <BODY>
  <OBJECT type="image/x.djvu" usemap="x_0001.djvu" width="2544" height="4426">
   <PARAM name="DPI" value="500"/>
   <HIDDENTEXT>
    <PAGECOLUMN><REGION><PARAGRAPH>
       <LINE>
        <WORD coords="278,1378,1726,1250" x-confidence="71">ABONDANC</WORD>
        <WORD coords="279,2039,1128,1947" x-confidence="57">DECOUVERTES </WORD>
       </LINE>
       <LINE>
        <WORD coords="483,1647,1721,1527" x-confidence="0">NATIONALE,</WORD>
       </LINE>
      </PARAGRAPH>
      <PARAGRAPH><LINE>
        <WORD coords="1017,1823,1073,1763" x-confidence="3">O&amp;U</WORD>
      </LINE></PARAGRAPH>
     </REGION></PAGECOLUMN>
   </HIDDENTEXT>
  </OBJECT>
 </BODY>
</DjVuXML>
"#;

    #[test]
    fn a_page_carries_its_dimensions_and_resolution() {
        let pages = parse(SAMPLE, "x").unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].of, "x");
        assert_eq!(pages[0].page, 1);
        assert_eq!(pages[0].w, Some(2544));
        assert_eq!(pages[0].h, Some(4426));
        assert_eq!(pages[0].dpi, Some(500));
    }

    /// The nesting is read only to lay the text out — a line ends a line, anything above it
    /// leaves a blank line.
    #[test]
    fn the_nesting_becomes_line_and_paragraph_breaks() {
        let pages = parse(SAMPLE, "x").unwrap();
        assert_eq!(
            pages[0].text,
            "ABONDANC DECOUVERTES \nNATIONALE,\n\nO&U"
        );
    }

    /// These titles are full of ampersands, and an entity arrives as its own event.
    #[test]
    fn entities_are_decoded_once() {
        let pages = parse(SAMPLE, "x").unwrap();
        assert!(pages[0].text.contains("O&U"), "{}", pages[0].text);
    }

    /// The five-coordinate derivation frc-data carries. Coordinates are ignored now, so it
    /// must simply not trip over them.
    #[test]
    fn the_five_coordinate_derivation_reads_the_same() {
        let xml = r#"<DjVuXML><BODY><OBJECT width="10" height="20"><HIDDENTEXT>
          <PAGECOLUMN><REGION backgroundColor="14866620"><PARAGRAPH><LINE>
            <WORD coords="1562,366,1570,348,381">mot</WORD>
            <WORD coords="1596,396,1614,336,396">suivant</WORD>
          </LINE></PARAGRAPH></REGION></PAGECOLUMN>
        </HIDDENTEXT></OBJECT></BODY></DjVuXML>"#;
        let pages = parse(xml, "x").unwrap();
        assert_eq!(pages[0].text, "mot suivant");
    }

    #[test]
    fn a_page_with_no_hidden_text_is_legal() {
        let xml = r#"<DjVuXML><BODY>
          <OBJECT width="10" height="20"><PARAM name="DPI" value="300"/></OBJECT>
        </BODY></DjVuXML>"#;
        let pages = parse(xml, "x").unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].text, "");
        assert_eq!(pages[0].w, Some(10));
    }

    #[test]
    fn pages_are_numbered_from_one() {
        let xml = r#"<DjVuXML><BODY>
          <OBJECT width="1" height="1"></OBJECT>
          <OBJECT width="2" height="2"></OBJECT>
          <OBJECT width="3" height="3"></OBJECT>
        </BODY></DjVuXML>"#;
        let pages = parse(xml, "x").unwrap();
        assert_eq!(pages.iter().map(|p| p.page).collect::<Vec<_>>(), [1, 2, 3]);
    }

    /// The text goes straight into a markdown body, so it must survive that trip intact.
    #[test]
    fn parsed_text_round_trips_through_the_markdown() {
        let pages = parse(SAMPLE, "x").unwrap();
        let md = crate::ocr::to_markdown(&pages[0], "s.json");
        assert_eq!(crate::ocr::from_markdown(&md).unwrap(), pages[0]);
    }
}
