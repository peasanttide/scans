//! The EDTF subset used by this archive.
//!
//! # Why this is hand-written
//!
//! The `edtf` crate (0.2.0, the only maintained general-purpose one) was evaluated against the
//! spec's accept/reject table before this module was written. It fails in **both** directions:
//!
//! * **Too permissive** — it accepts four constructs the spec rejects: seasons (`1789-21`),
//!   empty-side intervals (`1789/`, `/1789`), and negative years (`-0500`).
//! * **Too restrictive** — the spec's grammar is `year := d d d d` where each `d` is a digit
//!   *or* `X`, so `X` is legal in any position. The crate models unspecified digits as
//!   `Precision::Decade` / `Precision::Century` buckets, which only exist for *trailing* `X`.
//!   It rejects `1XXX`, `XXXX`, `1X89`, `17X9`, `X789`, `1789-X1`, `1789-1X`, `1789-01-X3`
//!   and `1789-01-1X` — nine of the eleven masked forms the grammar admits.
//! * **No bounds.** It exposes no earliest/latest interval, so [`Edtf::bounds`] — the whole
//!   reason the validator needs EDTF at all — would be hand-written regardless.
//!
//! Wrapping it would mean post-filtering four constructs, hand-parsing nine more it cannot
//! represent, and computing every bound myself: a second parser wearing the first as a hat.
//! That is exactly the "half-use" the brief forbids, so the crate is not a dependency.
//!
//! # What this module guarantees
//!
//! A narrow parser that errors loudly beats a permissive one that silently mis-reads. Anything
//! outside the grammar in [`parse`] is rejected with a message naming the construct.
//!
//! [`Edtf::bounds`] returns `(earliest, latest)` which is always a **superset** of the true
//! possible set, never narrower. That property is what makes the validator safe: it can fail
//! to catch an error, but it can never invent one.

use std::fmt;

// ---------------------------------------------------------------------------------------
// Civil dates
// ---------------------------------------------------------------------------------------

/// A proleptic Gregorian calendar date.
///
/// Field order is `(year, month, day)`, so the derived `Ord` is chronological order. That is
/// the only comparison this crate needs, which is why there is no `chrono` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl CivilDate {
    pub fn new(year: i32, month: u32, day: u32) -> Self {
        CivilDate { year, month, day }
    }
}

impl fmt::Display for CivilDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Proleptic Gregorian leap year.
pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Last day of the given month, proleptic Gregorian.
pub fn last_day(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    }
}

// ---------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------

/// Why a string is not valid EDTF under this subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdtfError {
    /// The offending input, verbatim.
    pub input: String,
    /// Human-readable reason, naming the construct where possible.
    pub reason: String,
}

impl fmt::Display for EdtfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for EdtfError {}

fn err(input: &str, reason: impl Into<String>) -> EdtfError {
    EdtfError {
        input: input.to_string(),
        reason: reason.into(),
    }
}

// ---------------------------------------------------------------------------------------
// The parsed forms
// ---------------------------------------------------------------------------------------

/// A single EDTF date, possibly with unspecified digits and a qualifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdtfDate {
    /// Four characters, digits and/or `X`, exactly as written.
    pub year: String,
    /// Two characters, or `None` at year precision.
    pub month: Option<String>,
    /// Two characters, or `None` above day precision.
    pub day: Option<String>,
    /// `?` or `%`.
    pub uncertain: bool,
    /// `~` or `%`.
    pub approximate: bool,
    pub raw: String,
}

/// An EDTF interval. At least one endpoint is a date; `None` means open (`..`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdtfInterval {
    pub start: Option<EdtfDate>,
    pub end: Option<EdtfDate>,
    pub raw: String,
}

/// Either form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edtf {
    Date(EdtfDate),
    Interval(EdtfInterval),
}

/// Precision of a date, for messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Year,
    Month,
    Day,
}

impl EdtfDate {
    pub fn precision(&self) -> Precision {
        match (&self.month, &self.day) {
            (None, _) => Precision::Year,
            (Some(_), None) => Precision::Month,
            (Some(_), Some(_)) => Precision::Day,
        }
    }

    /// `(earliest, latest)` — always a superset of the true possible set.
    ///
    /// Unspecified digits are **clamped**, not enumerated: `1789-01-X0` widens to days 1..=31
    /// rather than exactly `{10, 20, 30}`. Clamping is sound (it only ever widens) and avoids
    /// enumerating up to 10^4 candidates for `XXXX`.
    ///
    /// Qualifiers do **not** move bounds. `1795~` has exactly the bounds of `1795`. Any fuzz
    /// constant would be arbitrary, and would make the validator's verdicts depend on an
    /// invented number.
    pub fn bounds(&self) -> (CivilDate, CivilDate) {
        let year_min = mask_min(&self.year).max(1) as i32;
        let year_max = mask_max(&self.year) as i32;

        let Some(month) = &self.month else {
            return (
                CivilDate::new(year_min, 1, 1),
                CivilDate::new(year_max, 12, 31),
            );
        };

        let mon_min = mask_min(month).clamp(1, 12);
        let mon_max = mask_max(month).clamp(1, 12);

        let Some(day) = &self.day else {
            return (
                CivilDate::new(year_min, mon_min, 1),
                CivilDate::new(year_max, mon_max, last_day(year_max, mon_max)),
            );
        };

        // The earliest day must be a real day of the earliest month, and the latest a real day
        // of the latest month. Without both clamps a mask like `1789-02-3X` would produce
        // 1789-02-30, which is not a date and would compare wrongly.
        let day_min = mask_min(day).max(1).min(last_day(year_min, mon_min));
        let day_max = mask_max(day).clamp(1, last_day(year_max, mon_max));

        (
            CivilDate::new(year_min, mon_min, day_min),
            CivilDate::new(year_max, mon_max, day_max),
        )
    }
}

/// Smallest value a digit mask can take: every `X` becomes `0`.
fn mask_min(s: &str) -> u32 {
    s.replace('X', "0").parse().unwrap_or(0)
}

/// Largest value a digit mask can take: every `X` becomes `9`.
fn mask_max(s: &str) -> u32 {
    s.replace('X', "9").parse().unwrap_or(0)
}

impl EdtfInterval {
    /// `(earliest, latest)`, where `None` is unbounded in that direction.
    ///
    /// Note the asymmetry: the interval takes the **earliest** of its start and the
    /// **latest** of its end, so `178X/179X` is 1780-01-01 … 1799-12-31.
    pub fn bounds(&self) -> (Option<CivilDate>, Option<CivilDate>) {
        (
            self.start.as_ref().map(|d| d.bounds().0),
            self.end.as_ref().map(|d| d.bounds().1),
        )
    }
}

impl Edtf {
    /// Parse a string under this subset. See [`parse`].
    pub fn parse(input: &str) -> Result<Edtf, EdtfError> {
        parse(input)
    }

    /// `(earliest, latest)`, where `None` is unbounded in that direction.
    pub fn bounds(&self) -> (Option<CivilDate>, Option<CivilDate>) {
        match self {
            Edtf::Date(d) => {
                let (lo, hi) = d.bounds();
                (Some(lo), Some(hi))
            }
            Edtf::Interval(i) => i.bounds(),
        }
    }

    pub fn is_interval(&self) -> bool {
        matches!(self, Edtf::Interval(_))
    }

    pub fn raw(&self) -> &str {
        match self {
            Edtf::Date(d) => &d.raw,
            Edtf::Interval(i) => &i.raw,
        }
    }

    /// An interval with both endpoints present must not run backwards. Finding `E605`.
    pub fn start_after_end(&self) -> bool {
        match self {
            Edtf::Interval(i) => match (&i.start, &i.end) {
                (Some(s), Some(e)) => s.bounds().0 > e.bounds().1,
                _ => false,
            },
            Edtf::Date(_) => false,
        }
    }
}

impl fmt::Display for Edtf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.raw())
    }
}

// ---------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------

/// Parse a string under the archive's EDTF subset.
///
/// ```text
/// edtf     := interval | date
/// interval := endpoint "/" endpoint      (at least one endpoint must be a date)
/// endpoint := date | ".."
/// date     := body qualifier?
/// body     := year | year "-" month | year "-" month "-" day
/// year     := d d d d                    (d = digit or "X")
/// month    := d d
/// day      := d d
/// ```
pub fn parse(input: &str) -> Result<Edtf, EdtfError> {
    // Whitespace is a rejection, not something to trim: a date field that needs trimming is a
    // data-entry bug the archive should be told about.
    if input.chars().any(char::is_whitespace) {
        return Err(err(input, "whitespace is not permitted in a date"));
    }
    if input.is_empty() {
        return Err(err(input, "empty string is not a date"));
    }

    // Reject level-2 set and list syntax before anything else, so the message names it.
    if input.starts_with('[') || input.ends_with(']') {
        return Err(err(input, "sets ([1789,1791]) are not supported"));
    }
    if input.starts_with('{') || input.ends_with('}') {
        return Err(err(input, "lists ({1789..1791}) are not supported"));
    }
    if input.contains('T') || input.contains(':') {
        return Err(err(input, "times and timezones are not supported"));
    }
    if input.starts_with('Y') {
        return Err(err(input, "exponential years (Y17E2) are not supported"));
    }

    match input.split_once('/') {
        None => Ok(Edtf::Date(parse_date(input, input)?)),
        Some((lhs, rhs)) => {
            if rhs.contains('/') {
                return Err(err(input, "an interval has exactly one '/'"));
            }
            if lhs.is_empty() || rhs.is_empty() {
                return Err(err(
                    input,
                    "an open interval endpoint is written '..', not an empty string",
                ));
            }
            let start = parse_endpoint(input, lhs)?;
            let end = parse_endpoint(input, rhs)?;
            if start.is_none() && end.is_none() {
                return Err(err(input, "'../..' carries no information"));
            }
            let interval = EdtfInterval {
                start,
                end,
                raw: input.to_string(),
            };
            Ok(Edtf::Interval(interval))
        }
    }
}

fn parse_endpoint(whole: &str, part: &str) -> Result<Option<EdtfDate>, EdtfError> {
    if part == ".." {
        Ok(None)
    } else {
        Ok(Some(parse_date(whole, part)?))
    }
}

fn parse_date(whole: &str, part: &str) -> Result<EdtfDate, EdtfError> {
    let (body, uncertain, approximate) = match part.chars().last() {
        Some('?') => (&part[..part.len() - 1], true, false),
        Some('~') => (&part[..part.len() - 1], false, true),
        Some('%') => (&part[..part.len() - 1], true, true),
        _ => (part, false, false),
    };

    // A qualifier anywhere but the very end is EDTF level 2, which this subset rejects.
    if body.contains(['?', '~', '%']) {
        return Err(err(
            whole,
            format!(
                "{part:?}: per-component qualifiers are EDTF level 2 and are not supported; \
                 a qualifier may only follow the whole date"
            ),
        ));
    }
    if body.starts_with('-') {
        return Err(err(
            whole,
            format!("{part:?}: negative years are not supported"),
        ));
    }

    let fields: Vec<&str> = body.split('-').collect();
    if fields.len() > 3 {
        return Err(err(
            whole,
            format!("{part:?}: too many '-' separated fields"),
        ));
    }

    let year = fields[0];
    if year.len() != 4 || !is_mask(year) {
        return Err(err(
            whole,
            format!("{part:?}: year must be exactly 4 characters, each a digit or 'X'"),
        ));
    }

    let month = match fields.get(1) {
        None => None,
        Some(m) => {
            if m.len() != 2 || !is_mask(m) {
                return Err(err(
                    whole,
                    format!("{part:?}: month must be exactly 2 characters, each a digit or 'X'"),
                ));
            }
            // Digits that are 'X' are exempt; bounds handle those.
            if !m.contains('X') {
                let v: u32 = m.parse().expect("mask checked");
                if v == 0 {
                    return Err(err(whole, format!("{part:?}: month 00 is not a month")));
                }
                if (21..=24).contains(&v) {
                    return Err(err(
                        whole,
                        format!("{part:?}: seasons (21-24) are not supported"),
                    ));
                }
                if v > 12 {
                    return Err(err(
                        whole,
                        format!("{part:?}: month {v} is out of range 1-12"),
                    ));
                }
            }
            Some((*m).to_string())
        }
    };

    let day = match fields.get(2) {
        None => None,
        Some(d) => {
            if d.len() != 2 || !is_mask(d) {
                return Err(err(
                    whole,
                    format!("{part:?}: day must be exactly 2 characters, each a digit or 'X'"),
                ));
            }
            if !d.contains('X') {
                let v: u32 = d.parse().expect("mask checked");
                if v == 0 {
                    return Err(err(whole, format!("{part:?}: day 00 is not a day")));
                }
                if v > 31 {
                    return Err(err(
                        whole,
                        format!("{part:?}: day {v} is out of range 1-31"),
                    ));
                }
            }
            Some((*d).to_string())
        }
    };

    if day.is_some() && month.is_none() {
        return Err(err(whole, format!("{part:?}: a day requires a month")));
    }

    Ok(EdtfDate {
        year: year.to_string(),
        month,
        day,
        uncertain,
        approximate,
        raw: part.to_string(),
    })
}

/// Every character is an ASCII digit or a literal `X`.
fn is_mask(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == 'X')
}

// ---------------------------------------------------------------------------------------
// Comparison: "does this date fall inside this interval"
// ---------------------------------------------------------------------------------------

/// How a document's date sits against its copy's `covers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateRelation {
    /// Fully inside. Passes.
    Contained,
    /// Not fully inside, but not disjoint either. Finding `W603`.
    ///
    /// This is the imprecision case: a document dated `1789` inside a volume covering
    /// `1789-01-01/1789-06-30` is not *contained*, but it is obviously not misfiled.
    Overlaps,
    /// No possible common day. Finding `E602` — this is the misfiled-issue case, which is
    /// always disjoint.
    Disjoint,
}

/// Compare a date against a covering interval.
///
/// Both are compared as bound pairs, with `None` meaning unbounded, so an imprecise date is
/// handled without special-casing.
pub fn relate(date: &Edtf, covers: &Edtf) -> DateRelation {
    let (d0, d1) = date.bounds();
    let (c0, c1) = covers.bounds();

    let contained = c0.is_none_or(|c0| d0.is_some_and(|d0| d0 >= c0))
        && c1.is_none_or(|c1| d1.is_some_and(|d1| d1 <= c1));

    if contained {
        return DateRelation::Contained;
    }

    let overlaps =
        (c1.is_none() || d0.is_none() || d0 <= c1) && (c0.is_none() || d1.is_none() || d1 >= c0);

    if overlaps {
        DateRelation::Overlaps
    } else {
        DateRelation::Disjoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Edtf {
        parse(s).unwrap_or_else(|e| panic!("{s:?} should parse: {e}"))
    }

    // -- every accepted form in the spec's table -----------------------------------------

    #[test]
    fn accepts_day_precision() {
        let Edtf::Date(x) = d("1789-01-03") else {
            panic!("expected a date")
        };
        assert_eq!(x.year, "1789");
        assert_eq!(x.month.as_deref(), Some("01"));
        assert_eq!(x.day.as_deref(), Some("03"));
        assert_eq!(x.precision(), Precision::Day);
        assert_eq!(
            x.bounds(),
            (CivilDate::new(1789, 1, 3), CivilDate::new(1789, 1, 3))
        );
    }

    #[test]
    fn accepts_month_precision() {
        let (lo, hi) = d("1789-01").bounds();
        assert_eq!(lo, Some(CivilDate::new(1789, 1, 1)));
        assert_eq!(hi, Some(CivilDate::new(1789, 1, 31)));
    }

    #[test]
    fn accepts_year_precision() {
        let (lo, hi) = d("1739").bounds();
        assert_eq!(lo, Some(CivilDate::new(1739, 1, 1)));
        assert_eq!(hi, Some(CivilDate::new(1739, 12, 31)));
    }

    #[test]
    fn accepts_interval() {
        let x = d("1791/1799");
        assert!(x.is_interval());
        assert_eq!(
            x.bounds(),
            (
                Some(CivilDate::new(1791, 1, 1)),
                Some(CivilDate::new(1799, 12, 31))
            )
        );
    }

    #[test]
    fn accepts_qualifiers_without_moving_bounds() {
        for (s, unc, app) in [
            ("1795~", false, true),
            ("1789-01?", true, false),
            ("1789%", true, true),
        ] {
            let Edtf::Date(x) = d(s) else {
                panic!("expected a date")
            };
            assert_eq!(x.uncertain, unc, "{s}");
            assert_eq!(x.approximate, app, "{s}");
        }
        // A qualifier is inert for comparison purposes.
        assert_eq!(d("1795~").bounds(), d("1795").bounds());
        assert_eq!(d("1789%").bounds(), d("1789").bounds());
        assert_eq!(d("1789-01?").bounds(), d("1789-01").bounds());
    }

    #[test]
    fn accepts_unspecified_digits() {
        assert_eq!(
            d("178X").bounds(),
            (
                Some(CivilDate::new(1780, 1, 1)),
                Some(CivilDate::new(1789, 12, 31))
            )
        );
        assert_eq!(
            d("17XX").bounds(),
            (
                Some(CivilDate::new(1700, 1, 1)),
                Some(CivilDate::new(1799, 12, 31))
            )
        );
        assert_eq!(
            d("1789-XX").bounds(),
            (
                Some(CivilDate::new(1789, 1, 1)),
                Some(CivilDate::new(1789, 12, 31))
            )
        );
        assert_eq!(
            d("1789-01-XX").bounds(),
            (
                Some(CivilDate::new(1789, 1, 1)),
                Some(CivilDate::new(1789, 1, 31))
            )
        );
    }

    /// The grammar puts `X` in any position. This is precisely what the `edtf` crate cannot
    /// represent, and the reason this parser exists.
    #[test]
    fn accepts_x_in_any_position() {
        for s in [
            "1XXX",
            "XXXX",
            "1X89",
            "17X9",
            "X789",
            "1789-X1",
            "1789-1X",
            "1789-01-X3",
            "1789-01-1X",
        ] {
            assert!(parse(s).is_ok(), "{s:?} should parse");
        }
        // A leading X must not produce year 0, which is not a year.
        let (lo, hi) = d("XXXX").bounds();
        assert_eq!(lo, Some(CivilDate::new(1, 1, 1)));
        assert_eq!(hi, Some(CivilDate::new(9999, 12, 31)));
    }

    #[test]
    fn accepts_open_endpoints() {
        let x = d("../1789-06-30");
        assert_eq!(x.bounds(), (None, Some(CivilDate::new(1789, 6, 30))));
        let y = d("1789-01-01/..");
        assert_eq!(y.bounds(), (Some(CivilDate::new(1789, 1, 1)), None));
    }

    // -- everything the spec rejects ------------------------------------------------------

    #[test]
    fn rejects_the_spec_reject_list() {
        let cases = [
            ("1789-21", "season"),
            ("1789-22", "season"),
            ("1789-01-03T12:00", "time"),
            ("1789-01-03T12", "time"),
            ("?1789-?01-03", "level 2 qualifier"),
            ("1789-01~-03", "level 2 qualifier"),
            ("[1789,1791]", "set"),
            ("{1789..1791}", "list"),
            ("Y17E2", "exponential year"),
            ("1789/", "empty interval side"),
            ("/1789", "empty interval side"),
            ("../..", "both endpoints open"),
            ("-0500", "negative year"),
            ("178", "short year"),
            ("17890", "long year"),
            (" 1789", "leading whitespace"),
            ("1789 ", "trailing whitespace"),
            ("17 89", "inner whitespace"),
            ("1789-00", "month 00"),
            ("1789-13", "month 13"),
            ("1789-01-00", "day 00"),
            ("1789-01-32", "day 32"),
            ("1789-1", "one-digit month"),
            ("1789-01-3", "one-digit day"),
            ("1789-01-03-04", "too many fields"),
            ("1789/1790/1791", "two slashes"),
            ("", "empty"),
            ("abcd", "not digits"),
            ("1789-ab", "not digits"),
            ("1789x", "lowercase x is not a mask character"),
        ];
        for (input, what) in cases {
            assert!(
                parse(input).is_err(),
                "{input:?} ({what}) should be rejected"
            );
        }
    }

    #[test]
    fn rejection_messages_name_the_construct() {
        assert!(parse("1789-21").unwrap_err().reason.contains("season"));
        assert!(parse("Y17E2").unwrap_err().reason.contains("exponential"));
        assert!(parse("[1789,1791]").unwrap_err().reason.contains("sets"));
        assert!(parse("1789/").unwrap_err().reason.contains("'..'"));
        assert!(
            parse("?1789-?01-03")
                .unwrap_err()
                .reason
                .contains("level 2")
        );
    }

    #[test]
    fn significant_digits_rejected() {
        // `1950S2` is significant-digit notation; `S` is not a mask character.
        assert!(parse("1950S2").is_err());
    }

    // -- bounds edge cases ----------------------------------------------------------------

    #[test]
    fn leap_years_are_proleptic_gregorian() {
        assert!(!is_leap_year(1789));
        assert!(is_leap_year(1788));
        assert!(!is_leap_year(1700)); // divisible by 100, not by 400
        assert!(is_leap_year(1600));
        assert_eq!(d("1788-02").bounds().1, Some(CivilDate::new(1788, 2, 29)));
        assert_eq!(d("1789-02").bounds().1, Some(CivilDate::new(1789, 2, 28)));
    }

    /// A naive `day.replace("X","0")` would make this 1789-02-30, which is not a date.
    #[test]
    fn masked_day_is_clamped_into_a_real_month() {
        let (lo, hi) = d("1789-02-3X").bounds();
        assert_eq!(lo, Some(CivilDate::new(1789, 2, 28)));
        assert_eq!(hi, Some(CivilDate::new(1789, 2, 28)));
        assert!(lo <= hi, "bounds must not run backwards");
    }

    #[test]
    fn masked_month_clamps_to_twelve() {
        let (lo, hi) = d("1789-1X").bounds();
        assert_eq!(lo, Some(CivilDate::new(1789, 10, 1)));
        assert_eq!(hi, Some(CivilDate::new(1789, 12, 31)));
    }

    #[test]
    fn bounds_are_always_ordered() {
        for s in [
            "1789",
            "1789-01",
            "1789-01-03",
            "178X",
            "17XX",
            "XXXX",
            "1789-XX",
            "1789-X1",
            "1789-1X",
            "1789-02-3X",
            "1789-01-X0",
            "1791/1799",
            "178X/179X",
            "../1789",
            "1789/..",
        ] {
            let (lo, hi) = d(s).bounds();
            if let (Some(lo), Some(hi)) = (lo, hi) {
                assert!(lo <= hi, "{s}: {lo} > {hi}");
            }
        }
    }

    #[test]
    fn interval_takes_earliest_start_and_latest_end() {
        assert_eq!(
            d("178X/179X").bounds(),
            (
                Some(CivilDate::new(1780, 1, 1)),
                Some(CivilDate::new(1799, 12, 31))
            )
        );
    }

    #[test]
    fn backwards_interval_is_detected() {
        assert!(d("1799/1791").start_after_end());
        assert!(!d("1791/1799").start_after_end());
        assert!(!d("1789").start_after_end());
        assert!(!d("../1789").start_after_end());
        // Overlapping imprecise endpoints do not count as backwards.
        assert!(!d("178X/1785").start_after_end());
    }

    // -- the containment comparison -------------------------------------------------------

    #[test]
    fn date_inside_covers_is_contained() {
        let covers = d("1789-01-01/1789-06-30");
        assert_eq!(relate(&d("1789-01-03"), &covers), DateRelation::Contained);
        assert_eq!(relate(&d("1789-06-30"), &covers), DateRelation::Contained);
        assert_eq!(relate(&d("1789-01"), &covers), DateRelation::Contained);
    }

    /// The case that would fire a spurious error under strict containment: a year-precision
    /// date inside a half-year volume.
    #[test]
    fn imprecise_date_overlapping_covers_is_a_warning_not_an_error() {
        let covers = d("1789-01-01/1789-06-30");
        assert_eq!(relate(&d("1789"), &covers), DateRelation::Overlaps);
    }

    #[test]
    fn misfiled_date_is_disjoint() {
        let covers = d("1789-01-01/1789-06-30");
        assert_eq!(relate(&d("1789-07-01"), &covers), DateRelation::Disjoint);
        assert_eq!(relate(&d("1790"), &covers), DateRelation::Disjoint);
        assert_eq!(relate(&d("1788-12-31"), &covers), DateRelation::Disjoint);
    }

    #[test]
    fn open_covers_contains_everything_on_that_side() {
        assert_eq!(
            relate(&d("1500-01-01"), &d("../1789-06-30")),
            DateRelation::Contained
        );
        assert_eq!(
            relate(&d("1900-01-01"), &d("../1789-06-30")),
            DateRelation::Disjoint
        );
        assert_eq!(
            relate(&d("1900-01-01"), &d("1789-01-01/..")),
            DateRelation::Contained
        );
    }

    #[test]
    fn boundary_days_are_inclusive() {
        let covers = d("1789-07-01/1789-12-31");
        assert_eq!(relate(&d("1789-07-01"), &covers), DateRelation::Contained);
        assert_eq!(relate(&d("1789-12-31"), &covers), DateRelation::Contained);
        assert_eq!(relate(&d("1789-06-30"), &covers), DateRelation::Disjoint);
    }
}
