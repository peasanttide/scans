//! The one-shot migration from the legacy layout to the schema in
//! `docs/superpowers/specs/2026-08-05-source-archive-schema-design.md`.
//!
//! # The property that matters: losslessness is audited, not asserted
//!
//! Every key in every legacy file is placed in exactly one of three buckets, and the dry run
//! prints which bucket each one landed in, per file and again as a summary over distinct keys:
//!
//! | bucket | meaning |
//! |---|---|
//! | **carried** | the value survives, at the named target field |
//! | **dropped — derivable** | the value is recomputable from data that does survive |
//! | **dropped — formulaic** | the value is a template over data that does survive |
//!
//! A key with no rule is [`M001`](Codes), an **error**, never a silent drop. That is the whole
//! design: the migration would rather refuse than quietly lose a hand-researched fact.
//!
//! Derivability is **verified, not claimed**. `sheets = 21` is only dropped once 21 sheet files
//! have actually been folded in; a sheet's `licence` is only dropped once it has been compared
//! byte-for-byte with the value it would inherit. When a check fails the value is carried
//! instead, or — where the target schema has no slot for it at all — the migration errors.
//!
//! Emission is verified too. [`emit`] renders with `toml_edit` and then re-parses the result as
//! a [`Record`], erroring unless it round-trips equal to the record that was built. A field the
//! emitter forgot is therefore a migration failure rather than a data loss.
//!
//! # Two facts about the repository that contradict the spec
//!
//! * **The 516 Journal de Paris issue files do not exist.** `journal-de-paris/1789/` holds two
//!   PDFs and two copy TOMLs; below it are twelve *empty* month directories. Migration steps 6
//!   and 7 (`source = "…pdf"` → `scan.file`, `pages = "13-16"` → `{from, to}`) therefore have
//!   zero real inputs. They are implemented and unit-tested against synthetic fixtures, and on
//!   this repository they touch nothing. The real work is the Journal de Paris source and its
//!   two copies, the Turgot fold (22 files → 1) and the Verniquet fold (74 → 1).
//! * **There is an empty `sources/` directory at the repo root; the spec says `source/`.** The
//!   destination root is [`SOURCE_ROOT`] and nothing else. The stray `sources/` holds no TOML,
//!   so the migration never looks at it.
//!
//! # Safety
//!
//! * Dry run is the default. [`plan`] never touches the working tree.
//! * [`apply`] refuses to run if the plan carries any error diagnostic.
//! * Moves are `git mv`, so history follows the file. Untracked files fall back to a rename.
//! * **Nothing is deleted until the migration is verified.** Deletions run last, and only after
//!   [`load_archive`] has re-read the rewritten tree and reported no errors.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

use crate::load::{Diagnostic, SOURCE_ROOT, load_archive};
use crate::model::{
    CopyRecord, Document, Graphic, Holding, Identifier, Link, Page, PageRange, Record, Resp,
    Rights, Roles, Scan, Source, Text,
};

/// Finding codes this module owns. Documented as a unit so they can be grepped.
///
/// | code | severity | meaning |
/// |---|---|---|
/// | `M001` | error | legacy key with no migration rule |
/// | `M002` | error | file cannot be classified into a layer |
/// | `M003` | error | `author` string with no `[[resp]]` rule |
/// | `M004` | error | malformed legacy value |
/// | `M005` | error | value differs from its ancestor's and the target schema has no slot |
/// | `M006` | error | `supplement_to` does not resolve |
/// | `M007` | error | no image file found beside a sheet record |
/// | `M008` | warning | a number survives only as prose in `scan.note` |
/// | `M009` | warning | a value was synthesised rather than migrated; review it |
/// | `M010` | error | a declared count disagrees with what was folded in |
/// | `M011` | warning | a comment was carried into `note` |
/// | `M012` | error | emitted TOML did not round-trip back to the record it came from |
/// | `M013` | error | two records would migrate to the same id |
#[derive(Debug)]
pub struct Codes;

// ---------------------------------------------------------------------------------------
// Public interface — kept exactly as the scaffolding agent defined it
// ---------------------------------------------------------------------------------------

/// How to run the migration.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Describe what would happen without touching the working tree.
    pub dry_run: bool,
}

/// One filesystem change the migration intends to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Move a file, preserving history. Run as `git mv`.
    Move { from: PathBuf, to: PathBuf },
    /// Write a file, creating or replacing it.
    Write { path: PathBuf, contents: String },
    /// Remove a file that has been folded into another. Only ever run after verification.
    Delete { path: PathBuf },
}

/// What the migration intends to do, before it does any of it.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub actions: Vec<Action>,
    /// Anything that needs a human eye — a value with no slot in the target schema, an
    /// `author` string needing judgement, and so on.
    pub diagnostics: Vec<Diagnostic>,
    /// The complete audit trail: what became of every legacy key, and every field the
    /// migration invented. This is the record that makes losslessness checkable.
    pub ledger: Ledger,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// True when some finding would stop [`apply`].
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}

/// Work out what the migration would do. Never touches the working tree.
///
/// The field-level ledger is printed here rather than by the caller because `main.rs` belongs
/// to another agent and prints only `actions` and `diagnostics`. Tests read [`Plan::ledger`]
/// directly and never go near stdout.
pub fn plan(root: &Path, options: &Options) -> Result<Plan> {
    let plan = build_plan(root)?;
    if options.dry_run && !plan.ledger.is_empty() {
        print!("{}", plan.ledger);
    }
    Ok(plan)
}

/// Carry out a plan.
///
/// Order is moves, then writes, then **verification**, then deletions. Nothing is removed
/// until [`load_archive`] has re-read the rewritten tree and found no errors, so a migration
/// that goes wrong leaves every original file where it was.
pub fn apply(root: &Path, plan: &Plan, options: &Options) -> Result<usize> {
    if options.dry_run {
        bail!("apply called with dry_run set; this is a bug in the caller");
    }
    if plan.has_errors() {
        let first = plan
            .diagnostics
            .iter()
            .find(|d| d.is_error())
            .expect("has_errors");
        bail!(
            "refusing to apply: the plan has {} error(s), the first being\n  {first}",
            plan.diagnostics.iter().filter(|d| d.is_error()).count()
        );
    }

    let mut applied = 0usize;

    for action in &plan.actions {
        if let Action::Move { from, to } = action {
            move_file(root, from, to)?;
            applied += 1;
        }
    }
    for action in &plan.actions {
        if let Action::Write { path, contents } = action {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(path, contents)
                .with_context(|| format!("writing {}", path.display()))?;
            applied += 1;
        }
    }

    let deletions = plan
        .actions
        .iter()
        .filter(|a| matches!(a, Action::Delete { .. }))
        .count();
    if deletions > 0 {
        verify_before_deleting(root)?;
        for action in &plan.actions {
            if let Action::Delete { path } = action {
                delete_file(root, path)?;
                applied += 1;
            }
        }
    }

    prune_emptied_directories(root, plan);

    Ok(applied)
}

/// Remove the legacy directories the migration emptied.
///
/// Git does not track directories, so a moved-out folder leaves no trace in `git status` and
/// survives the migration in the working tree. That matters here: an empty
/// `journal-de-paris/1789/01/` sitting beside `source/journal-de-paris/1789/01/` is precisely
/// where the next hand-written issue file gets put by mistake, and the loader would never see
/// it. Only genuinely empty directories are removed, deepest first, and only ones the
/// migration itself emptied — `remove_dir` refuses a non-empty directory, so a stray file
/// anywhere in the tree keeps it and everything above it.
fn prune_emptied_directories(root: &Path, plan: &Plan) {
    let mut candidates: BTreeSet<PathBuf> = BTreeSet::new();
    for action in &plan.actions {
        let from = match action {
            Action::Move { from, .. } | Action::Delete { path: from } => from,
            Action::Write { .. } => continue,
        };
        let mut dir = from.parent();
        while let Some(d) = dir {
            if d == root {
                break;
            }
            candidates.insert(d.to_path_buf());
            dir = d.parent();
        }
    }
    // Deepest first, so a parent becomes empty before it is tried.
    for dir in candidates.iter().rev() {
        let _ = std::fs::remove_dir(dir);
    }
}

/// Re-read the rewritten tree. Any error means the migration is wrong, and the originals stay.
fn verify_before_deleting(root: &Path) -> Result<()> {
    let archive = load_archive(root).context("re-reading the migrated archive for verification")?;
    let errors: Vec<String> = archive
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(std::string::ToString::to_string)
        .collect();
    if !errors.is_empty() {
        bail!(
            "the migrated archive does not load cleanly, so nothing has been deleted; the \
             original files are untouched:\n  {}",
            errors.join("\n  ")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// The ledger
// ---------------------------------------------------------------------------------------

/// What became of one legacy key. Exactly three outcomes exist; a fourth is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// The value survives, at `to`.
    Carried { to: String },
    /// The value is recomputable from data that does survive, by `how`.
    DroppedDerivable { how: String },
    /// The value is a template over data that does survive, described by `how`.
    DroppedFormulaic { how: String },
}

impl Disposition {
    pub fn label(&self) -> &'static str {
        match self {
            Disposition::Carried { .. } => "carried",
            Disposition::DroppedDerivable { .. } => "dropped/derivable",
            Disposition::DroppedFormulaic { .. } => "dropped/formulaic",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Disposition::Carried { to } => to,
            Disposition::DroppedDerivable { how } | Disposition::DroppedFormulaic { how } => how,
        }
    }
}

/// One legacy key, and what the migration did with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFate {
    /// Repo-relative path of the legacy file.
    pub file: String,
    pub key: String,
    /// The legacy value, abbreviated for display.
    pub value: String,
    pub disposition: Disposition,
}

/// A field the migration invented rather than migrated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Synthesised {
    /// Repo-relative path of the file it was written into.
    pub file: String,
    pub field: String,
    pub value: String,
    pub why: String,
}

/// The complete audit trail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    pub keys: Vec<KeyFate>,
    pub added: Vec<Synthesised>,
}

impl Ledger {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.added.is_empty()
    }

    fn carried(&mut self, file: &str, key: &str, value: String, to: impl Into<String>) {
        self.keys.push(KeyFate {
            file: file.to_string(),
            key: key.to_string(),
            value,
            disposition: Disposition::Carried { to: to.into() },
        });
    }

    fn derivable(&mut self, file: &str, key: &str, value: String, how: impl Into<String>) {
        self.keys.push(KeyFate {
            file: file.to_string(),
            key: key.to_string(),
            value,
            disposition: Disposition::DroppedDerivable { how: how.into() },
        });
    }

    fn formulaic(&mut self, file: &str, key: &str, value: String, how: impl Into<String>) {
        self.keys.push(KeyFate {
            file: file.to_string(),
            key: key.to_string(),
            value,
            disposition: Disposition::DroppedFormulaic { how: how.into() },
        });
    }

    fn added(&mut self, file: &str, field: &str, value: String, why: impl Into<String>) {
        self.added.push(Synthesised {
            file: file.to_string(),
            field: field.to_string(),
            value,
            why: why.into(),
        });
    }

    /// The disposition of `key` in `file`, if the migration recorded one. Test helper.
    pub fn fate(&self, file: &str, key: &str) -> Option<&Disposition> {
        self.keys
            .iter()
            .find(|f| f.file == file && f.key == key)
            .map(|f| &f.disposition)
    }

    /// Every distinct legacy key, with the distinct dispositions it received and how many
    /// files each applied to. This is the summary the dry run ends with, and it is the thing
    /// to read when checking that no key was quietly lost.
    pub fn summary(&self) -> Vec<(String, String, String, usize)> {
        let mut files: BTreeMap<(String, String, String), BTreeSet<&str>> = BTreeMap::new();
        for fate in &self.keys {
            files
                .entry((
                    fate.key.clone(),
                    fate.disposition.label().to_string(),
                    fate.disposition.detail().to_string(),
                ))
                .or_default()
                .insert(fate.file.as_str());
        }
        files
            .into_iter()
            .map(|((k, label, detail), seen)| (k, label, detail, seen.len()))
            .collect()
    }
}

/// Widest a column is allowed to get before its content is elided. Without a cap, one long
/// comment or one long URL pushes every other row off the right of the terminal.
const KEY_COLUMN: usize = 26;
const VALUE_COLUMN: usize = 34;

/// Pad to `width`, eliding with `…` when too long. Counts characters, not bytes: the archive
/// is full of accented French.
fn column(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        return format!("{text}{}", " ".repeat(width - len));
    }
    let head: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{head}…")
}

impl std::fmt::Display for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", "=".repeat(78))?;
        writeln!(
            f,
            "field-level diff — every legacy key is carried, dropped as derivable, or"
        )?;
        writeln!(f, "dropped as formulaic. A key with no rule is an error, never a drop.")?;
        writeln!(f, "{}", "=".repeat(78))?;

        let mut files: Vec<&str> = self
            .keys
            .iter()
            .map(|k| k.file.as_str())
            .chain(self.added.iter().map(|a| a.file.as_str()))
            .collect();
        files.sort_unstable();
        files.dedup();

        for file in files {
            writeln!(f, "\n{file}")?;
            for fate in self.keys.iter().filter(|k| k.file == file) {
                writeln!(
                    f,
                    "  {}  {}  {}  {}",
                    column(&fate.key, KEY_COLUMN),
                    column(&fate.value, VALUE_COLUMN),
                    column(fate.disposition.label(), 17),
                    fate.disposition.detail()
                )?;
            }
            for add in self.added.iter().filter(|a| a.file == file) {
                writeln!(
                    f,
                    "  + {}  {}  {}  {}",
                    column(&add.field, KEY_COLUMN.saturating_sub(2)),
                    column(&add.value, VALUE_COLUMN),
                    column("synthesised", 17),
                    add.why
                )?;
            }
        }

        writeln!(f, "\n{}", "=".repeat(78))?;
        writeln!(f, "every distinct legacy key, and what became of it")?;
        writeln!(f, "{}", "=".repeat(78))?;
        for (key, label, detail, n) in self.summary() {
            writeln!(
                f,
                "  {}  {}  {:>4} file(s)  {detail}",
                column(&key, KEY_COLUMN),
                column(&label, 17),
                n
            )?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Reading the legacy files
// ---------------------------------------------------------------------------------------

/// Directory names the migration never descends into.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "docs",
    "target",
    "tools",
    "schemas",
    "node_modules",
    SOURCE_ROOT,
];

/// TOML files that are tool configuration rather than archive content.
const CONFIG_FILES: &[&str] = &["Cargo.toml", "Cargo.lock", "rustfmt.toml", "taplo.toml"];

/// Extensions the migration will accept as a sheet's image, most-likely first.
const IMAGE_EXTENSIONS: &[&str] = &["jp2", "jpg", "jpeg", "png", "tif", "tiff", "webp"];

/// One legacy TOML file, parsed with `toml_edit` so comments survive the read.
struct Legacy {
    path: PathBuf,
    /// Repo-relative, forward slashes.
    rel: String,
    /// Filename stem, which is the id unless the file declares one.
    stem: String,
    doc: DocumentMut,
}

impl Legacy {
    fn read(root: &Path, path: PathBuf) -> Result<Self> {
        let rel = rel_display(root, &path);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let doc: DocumentMut = text
            .parse()
            .with_context(|| format!("{rel} is not valid TOML"))?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Legacy {
            path,
            rel,
            stem,
            doc,
        })
    }

    /// The id this record will have after migration.
    fn id(&self) -> String {
        self.str("id").unwrap_or_else(|| self.stem.clone())
    }

    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    fn has(&self, key: &str) -> bool {
        self.doc.get(key).is_some()
    }

    fn str(&self, key: &str) -> Option<String> {
        self.doc.get(key)?.as_str().map(str::to_string)
    }

    fn int(&self, key: &str) -> Option<i64> {
        self.doc.get(key)?.as_integer()
    }

    /// Top-level keys in declaration order.
    fn keys(&self) -> Vec<String> {
        self.doc.iter().map(|(k, _)| k.to_string()).collect()
    }

    /// A legacy value rendered for the ledger, abbreviated.
    fn show(&self, key: &str) -> String {
        match self.doc.get(key) {
            None => "<absent>".to_string(),
            Some(item) => abbreviate(item.to_string().trim()),
        }
    }

    /// A scalar as a string whatever its TOML type, so a legacy `date = 1789` (an integer)
    /// or a bare `1789-01-03` (a TOML local date) still migrates to a quoted EDTF string.
    fn scalar_as_string(&self, key: &str) -> Option<String> {
        let item = self.doc.get(key)?;
        if let Some(s) = item.as_str() {
            return Some(s.to_string());
        }
        if let Some(i) = item.as_integer() {
            return Some(i.to_string());
        }
        if let Some(d) = item.as_datetime() {
            return Some(d.to_string());
        }
        None
    }

    /// Comments attached to each top-level key, in declaration order.
    ///
    /// `toml_edit` keeps a comment in the decor prefix of whatever follows it, so this also
    /// tells us which key a comment introduces — which is what decides whether the comment is
    /// already said by the prose the migration generates from that key.
    fn comments(&self) -> Vec<(String, Vec<String>)> {
        let mut out = Vec::new();
        for (name, item) in self.doc.iter() {
            let prefix = match item {
                Item::ArrayOfTables(aot) => aot
                    .iter()
                    .next()
                    .and_then(|t| t.decor().prefix())
                    .and_then(|r| r.as_str())
                    .map(str::to_string),
                Item::Table(t) => t
                    .decor()
                    .prefix()
                    .and_then(|r| r.as_str())
                    .map(str::to_string),
                _ => self
                    .doc
                    .key(name)
                    .and_then(|k| k.leaf_decor().prefix())
                    .and_then(|r| r.as_str())
                    .map(str::to_string),
            };
            let lines = comment_lines(prefix.as_deref().unwrap_or(""));
            if !lines.is_empty() {
                out.push((name.to_string(), lines));
            }
        }
        out
    }
}

/// Pull the comment text out of a decor prefix.
fn comment_lines(prefix: &str) -> Vec<String> {
    prefix
        .lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix('#'))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn abbreviate(s: &str) -> String {
    let flat = s.replace(['\n', '\r'], " ");
    if flat.chars().count() <= 46 {
        return flat;
    }
    let head: String = flat.chars().take(45).collect();
    format!("{head}…")
}

fn rel_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------------------

/// Which layer a legacy file becomes. `Sheet` is not a layer: it is a file that folds into
/// its parent's `[[page]]` array and then ceases to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Source,
    Copy,
    Document,
    Sheet,
}

/// `kind` values that name a genre of publication — the `source` layer.
const SOURCE_KINDS: &[&str] = &[
    "newspaper",
    "map",
    "diary",
    "periodical",
    "journal",
    "book",
    "atlas",
];
/// `kind` values that name a physical object — the `copy` layer.
const COPY_KINDS: &[&str] = &["volume", "roll", "folder", "box", "album"];
/// `kind` values that name an intellectual unit — the `document` layer.
const DOCUMENT_KINDS: &[&str] = &[
    "issue",
    "supplement",
    "sheet",
    "letter",
    "play",
    "engraving",
    "insert",
    "report",
    "pamphlet",
];

fn classify(legacy: &Legacy) -> std::result::Result<Shape, String> {
    match legacy.str("kind") {
        Some(kind) => {
            if SOURCE_KINDS.contains(&kind.as_str()) {
                Ok(Shape::Source)
            } else if COPY_KINDS.contains(&kind.as_str()) {
                Ok(Shape::Copy)
            } else if DOCUMENT_KINDS.contains(&kind.as_str()) {
                Ok(Shape::Document)
            } else {
                Err(format!(
                    "kind = {kind:?} is not in any of the three vocabularies the migration \
                     knows; add it to SOURCE_KINDS, COPY_KINDS or DOCUMENT_KINDS in migrate.rs"
                ))
            }
        }
        None if legacy.has("sheet") && legacy.has("of") => Ok(Shape::Sheet),
        None => Err(
            "no 'kind', and no 'sheet' + 'of' either, so the layer cannot be determined"
                .to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------------------
// `author` -> [[resp]]
//
// A lookup table rather than a parser. The spec calls this "the one step needing judgment,
// and there are only a handful of distinct strings" — so an unrecognised string is an error
// asking a human for a rule, never a guess. A fuzzy splitter would mis-read "P.T. Bartholome
// and A.J. Mathieu" sooner or later and nothing would notice.
// ---------------------------------------------------------------------------------------

/// `(author string, [(name, roles)])`.
type RespRule = (&'static str, &'static [(&'static str, &'static [&'static str])]);

const AUTHOR_RULES: &[RespRule] = &[
    (
        "Louis Bretez (survey and drawing); Claude Lucas (engraving); Aubin (lettering)",
        &[
            ("Louis Bretez", &["surveyor", "draughtsman"]),
            ("Claude Lucas", &["engraver"]),
            ("Aubin", &["lettering"]),
        ],
    ),
    (
        "Edme Verniquet; engraved by P.T. Bartholome and A.J. Mathieu",
        &[
            // No role phrase qualifies the leading name, so the role is the one the legacy
            // field itself asserts: `author`. Inventing "surveyor" would be a historical
            // claim the string does not make.
            ("Edme Verniquet", &["author"]),
            ("P.T. Bartholome", &["engraver"]),
            ("A.J. Mathieu", &["engraver"]),
        ],
    ),
];

fn author_to_resp(author: &str) -> Option<Vec<Resp>> {
    let rule = AUTHOR_RULES.iter().find(|(s, _)| *s == author)?;
    Some(
        rule.1
            .iter()
            .map(|(name, roles)| Resp {
                name: (*name).to_string(),
                role: Some(match roles.len() {
                    1 => Roles::One(roles[0].to_string()),
                    _ => Roles::Many(roles.iter().map(|r| (*r).to_string()).collect()),
                }),
                note: None,
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------
// Small conversions
// ---------------------------------------------------------------------------------------

/// `"13-16"` -> `{ from = 13, to = 16 }`. Deliberately strict: the only two range strings in
/// the archive are exactly `N-M`, and a parser that also accepted `13-16,19` would be
/// guessing about a form nobody has written yet.
fn parse_page_range(raw: &str) -> std::result::Result<PageRange, String> {
    let (from, to) = raw
        .split_once('-')
        .ok_or_else(|| format!("{raw:?} is not a page range of the form N-M"))?;
    let parse = |s: &str, which: &str| -> std::result::Result<i64, String> {
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!(
                "{raw:?} is not a page range of the form N-M: the {which} bound is {s:?}"
            ));
        }
        s.parse::<i64>()
            .map_err(|_| format!("{raw:?}: the {which} bound is out of range"))
    };
    let from = parse(from, "lower")?;
    let to = parse(to, "upper")?;
    if to < from {
        return Err(format!("{raw:?}: the upper bound is below the lower bound"));
    }
    Ok(PageRange { from, to })
}

/// French month names as they appear in the volume titles, plus the accentless spellings the
/// archive also uses.
const FRENCH_MONTHS: &[(&str, u32)] = &[
    ("janvier", 1),
    ("fevrier", 2),
    ("février", 2),
    ("mars", 3),
    ("avril", 4),
    ("mai", 5),
    ("juin", 6),
    ("juillet", 7),
    ("aout", 8),
    ("août", 8),
    ("septembre", 9),
    ("octobre", 10),
    ("novembre", 11),
    ("decembre", 12),
    ("décembre", 12),
];

/// `("… volume 1 (janvier–juin)", 1789)` -> `"1789-01-01/1789-06-30"`.
///
/// Returns `None` unless the title ends in a parenthesised span of two month names, so a title
/// the migration does not fully understand simply yields no `covers` rather than a wrong one.
fn covers_from_title(title: &str, year: i32) -> Option<String> {
    let inner = title.trim_end().strip_suffix(')')?.rsplit_once('(')?.1;
    let month = |name: &str| -> Option<u32> {
        let name = name.trim().to_lowercase();
        FRENCH_MONTHS
            .iter()
            .find(|(m, _)| *m == name)
            .map(|(_, n)| *n)
    };
    // The archive writes an en dash; accept a hyphen too.
    let (first, last) = inner
        .split_once('–')
        .or_else(|| inner.split_once('-'))
        .or_else(|| inner.split_once('—'))?;
    let (from, to) = (month(first)?, month(last)?);
    if to < from {
        return None;
    }
    Some(format!(
        "{year:04}-{from:02}-01/{year:04}-{to:02}-{:02}",
        crate::edtf::last_day(year, to)
    ))
}

/// `"Digitised by Google Books."` -> `"Google Books"`.
///
/// Refuses anything starting `the `, because "the David Rumsey Map Collection" is a holding
/// repository rather than a digitising agent and would read badly as `scan.by`.
fn scan_by_from_attribution(attribution: &str) -> Option<String> {
    let agent = attribution
        .trim()
        .strip_prefix("Digitised by ")
        .or_else(|| attribution.trim().strip_prefix("Digitized by "))?
        .trim_end_matches('.')
        .trim();
    if agent.is_empty() || agent.starts_with("the ") {
        return None;
    }
    Some(agent.to_string())
}

// ---------------------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------------------

/// Everything the builder needs to see at once.
struct Builder {
    root: PathBuf,
    /// Every legacy record, in path order.
    files: Vec<Legacy>,
    shapes: Vec<Shape>,
    /// Legacy id -> index into `files`.
    by_id: BTreeMap<String, usize>,
    /// Parent index -> its sheet children, sorted by sheet number.
    sheets: BTreeMap<usize, Vec<usize>>,
    ledger: Ledger,
    diagnostics: Vec<Diagnostic>,
    actions: Vec<Action>,
}

/// Build the plan. Pure with respect to the working tree: it reads, and nothing else.
pub fn build_plan(root: &Path) -> Result<Plan> {
    let root = std::fs::canonicalize(root)
        .with_context(|| format!("migration root {} does not exist", root.display()))?;

    let mut builder = Builder {
        root: root.clone(),
        files: Vec::new(),
        shapes: Vec::new(),
        by_id: BTreeMap::new(),
        sheets: BTreeMap::new(),
        ledger: Ledger::default(),
        diagnostics: Vec::new(),
        actions: Vec::new(),
    };

    builder.discover()?;
    if builder.files.is_empty() {
        return Ok(Plan::default());
    }
    builder.index();
    builder.build_records();
    builder.move_assets()?;

    let mut diagnostics = builder.diagnostics;
    crate::load::sort_diagnostics(&mut diagnostics);

    Ok(Plan {
        actions: builder.actions,
        diagnostics,
        ledger: builder.ledger,
    })
}

impl Builder {
    // -- discovery ----------------------------------------------------------------------

    fn discover(&mut self) -> Result<()> {
        for path in walk_toml(&self.root)? {
            let legacy = Legacy::read(&self.root, path)?;
            match classify(&legacy) {
                Ok(shape) => {
                    self.files.push(legacy);
                    self.shapes.push(shape);
                }
                Err(why) => {
                    self.diagnostics
                        .push(Diagnostic::error(legacy.rel.clone(), "M002", why));
                    self.files.push(legacy);
                    // Classified as a document so the loop below has something to hold; the
                    // error above already stops the plan being applied.
                    self.shapes.push(Shape::Document);
                }
            }
        }
        Ok(())
    }

    fn index(&mut self) {
        for (i, legacy) in self.files.iter().enumerate() {
            let id = legacy.id();
            match self.by_id.get(&id) {
                None => {
                    self.by_id.insert(id, i);
                }
                Some(&first) => self.diagnostics.push(
                    Diagnostic::error(
                        legacy.rel.clone(),
                        "M013",
                        format!("two records would migrate to the id {id:?}"),
                    )
                    .also(self.files[first].rel.clone()),
                ),
            }
        }

        // Attach each sheet to its parent, ordered by sheet number so the folded `[[page]]`
        // array comes out in `n` order regardless of filename.
        for (i, legacy) in self.files.iter().enumerate() {
            if self.shapes[i] != Shape::Sheet {
                continue;
            }
            let Some(of) = legacy.str("of") else { continue };
            let Some(&parent) = self.by_id.get(&of) else {
                self.diagnostics.push(Diagnostic::error(
                    legacy.rel.clone(),
                    "M006",
                    format!("of = {of:?} does not name any record in the archive"),
                ));
                continue;
            };
            self.sheets.entry(parent).or_default().push(i);
        }
        for children in self.sheets.values_mut() {
            children.sort_by_key(|i| {
                (
                    self.files[*i].int("sheet").unwrap_or(i64::MAX),
                    self.files[*i].stem.clone(),
                )
            });
        }
    }

    /// The value `key` would resolve to on this record's nearest ancestor that declares it.
    fn inherited(&self, index: usize, key: &str) -> Option<String> {
        let mut cursor = self.files[index].str("of")?;
        let mut seen = BTreeSet::new();
        while seen.insert(cursor.clone()) {
            let &parent = self.by_id.get(&cursor)?;
            if let Some(value) = self.files[parent].str(key) {
                return Some(value);
            }
            cursor = self.files[parent].str("of")?;
        }
        None
    }

    fn err(&mut self, index: usize, code: &'static str, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::error(self.files[index].rel.clone(), code, message));
    }

    fn warn(&mut self, index: usize, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::warning(
            self.files[index].rel.clone(),
            code,
            message,
        ));
    }

    // -- record construction ----------------------------------------------------------

    fn build_records(&mut self) {
        for index in 0..self.files.len() {
            match self.shapes[index] {
                // Sheets have no file of their own after the migration; they are folded in
                // by whichever record owns them.
                Shape::Sheet => self.plan_sheet_deletion(index),
                Shape::Source => self.build_source(index),
                Shape::Copy => self.build_copy(index),
                Shape::Document => self.build_document(index),
            }
        }
    }

    fn plan_sheet_deletion(&mut self, index: usize) {
        self.actions.push(Action::Delete {
            path: self.files[index].path.clone(),
        });
    }

    /// Destination of a legacy path: the same relative location, under `source/`.
    fn target(&self, path: &Path) -> PathBuf {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        self.root.join(SOURCE_ROOT).join(rel)
    }

    /// Queue the move-then-rewrite pair for a record that keeps its own file.
    fn emit_record(&mut self, index: usize, record: &Record) {
        let from = self.files[index].path.clone();
        let to = self.target(&from);
        let directive = schema_directive(&self.root, &to);
        match emit(record, &directive) {
            Ok(contents) => {
                self.actions.push(Action::Move {
                    from,
                    to: to.clone(),
                });
                self.actions.push(Action::Write { path: to, contents });
            }
            Err(e) => self.err(index, "M012", format!("{e:#}")),
        }
    }

    // -- shared key handling ------------------------------------------------------------

    /// Handle the keys every layer treats identically, and report any key with no rule.
    ///
    /// `handled` lists the keys the caller has already dealt with. Everything else must be
    /// matched here or it is `M001`.
    #[allow(clippy::too_many_lines)]
    fn common_keys(&mut self, index: usize, handled: &mut BTreeSet<String>, out: &mut Common) {
        let rel = self.files[index].rel.clone();

        for key in self.files[index].keys() {
            if handled.contains(&key) {
                continue;
            }
            let shown = self.files[index].show(&key);
            match key.as_str() {
                // `id` is restated explicitly in the target, so it is carried, not dropped.
                "id" => {
                    self.ledger.carried(&rel, &key, shown, "id");
                }
                "of" => {
                    out.of = self.files[index].str("of");
                    self.ledger.carried(&rel, &key, shown, "of");
                }
                "title" => {
                    out.title = self.files[index].str("title");
                    self.ledger.carried(&rel, &key, shown, "title");
                }
                "short_title" => {
                    out.short_title = self.files[index].str("short_title");
                    self.ledger.carried(&rel, &key, shown, "short_title");
                }
                "language" | "place" | "country" | "frequency" => {
                    let v = self.files[index].str(&key);
                    match key.as_str() {
                        "language" => out.language = v,
                        "place" => out.place = v,
                        "country" => out.country = v,
                        _ => out.frequency = v,
                    }
                    self.ledger.carried(&rel, &key, shown, key.clone());
                }
                "note" => {
                    out.note = self.files[index].str("note");
                    self.ledger.carried(&rel, &key, shown, "note");
                }
                "url" => {
                    out.url = self.files[index].str("url");
                    self.ledger.carried(&rel, &key, shown, "url");
                }
                "index" => {
                    if let Some(url) = self.files[index].str("index") {
                        out.links.push(Link {
                            rel: "index".to_string(),
                            url,
                            title: None,
                            note: None,
                        });
                    }
                    self.ledger
                        .carried(&rel, &key, shown, "[[link]] rel = \"index\"");
                }
                "date" | "founded" => {
                    let raw = self.files[index].scalar_as_string(&key);
                    match raw {
                        None => self.err(
                            index,
                            "M004",
                            format!("{key} is not a scalar the migration can render as EDTF"),
                        ),
                        Some(raw) => {
                            if let Err(e) = crate::edtf::parse(&raw) {
                                self.err(
                                    index,
                                    "M004",
                                    format!("{key} = {raw:?} is not valid EDTF: {e}"),
                                );
                            }
                            let quoted = self.files[index].doc.get(&key).is_some_and(|i| {
                                i.as_str().is_some()
                            });
                            let to = if quoted {
                                format!("{key} (already a quoted EDTF string)")
                            } else {
                                format!("{key} (quoted as an EDTF string)")
                            };
                            if key == "date" {
                                out.date = Some(raw);
                            } else {
                                out.founded = Some(raw);
                            }
                            self.ledger.carried(&rel, &key, shown, to);
                        }
                    }
                }
                "held" | "covers" => {
                    let Some(raw) = self.files[index].str(&key) else {
                        self.err(index, "M004", format!("{key} must be a quoted string"));
                        continue;
                    };
                    // `covers` is compared as an interval by check 6, so a bare year is
                    // widened to the interval it already means.
                    let (covers, how) = match (key.as_str(), raw.parse::<i32>()) {
                        ("held", Ok(year)) => (
                            format!("{year:04}-01-01/{year:04}-12-31"),
                            "covers (the year widened to the interval it already denotes)",
                        ),
                        _ => (raw, "covers"),
                    };
                    if let Err(e) = crate::edtf::parse(&covers) {
                        self.err(
                            index,
                            "M004",
                            format!("{key} becomes covers = {covers:?}, which is not valid EDTF: {e}"),
                        );
                    }
                    out.covers = Some(covers);
                    self.ledger.carried(&rel, &key, shown, how);
                }
                "licence" | "license" => {
                    let value = self.files[index].str(&key);
                    match (value, self.inherited(index, "licence")) {
                        (Some(v), Some(parent)) if v == parent => self.ledger.derivable(
                            &rel,
                            &key,
                            shown,
                            "identical to the ancestor's, so it is inherited as rights.work",
                        ),
                        (Some(v), _) => {
                            out.rights.work = Some(v);
                            self.ledger.carried(&rel, &key, shown, "rights.work");
                        }
                        (None, _) => {
                            self.err(index, "M004", format!("{key} must be a quoted string"));
                        }
                    }
                }
                "attribution" => {
                    let value = self.files[index].str(&key);
                    match (value, self.inherited(index, "attribution")) {
                        (Some(v), Some(parent)) if v == parent => self.ledger.derivable(
                            &rel,
                            &key,
                            shown,
                            "identical to the ancestor's, so it is inherited as \
                             rights.attribution",
                        ),
                        (Some(v), _) => {
                            out.rights.attribution = Some(v);
                            self.ledger.carried(&rel, &key, shown, "rights.attribution");
                        }
                        (None, _) => {
                            self.err(index, "M004", "attribution must be a quoted string");
                        }
                    }
                }
                "author" => {
                    let Some(author) = self.files[index].str("author") else {
                        self.err(index, "M004", "author must be a quoted string");
                        continue;
                    };
                    match author_to_resp(&author) {
                        Some(resp) => {
                            let n = resp.len();
                            out.resp = Some(resp);
                            self.ledger.carried(
                                &rel,
                                &key,
                                shown,
                                format!("[[resp]] ({n} record(s))"),
                            );
                        }
                        None => self.err(
                            index,
                            "M003",
                            format!(
                                "author = {author:?} has no [[resp]] rule; add one to \
                                 AUTHOR_RULES in migrate.rs rather than letting the migration \
                                 guess how to split it"
                            ),
                        ),
                    }
                }
                "holding" => match self.files[index].str("holding") {
                    Some(v) => {
                        out.holding.repository = Some(v);
                        self.ledger.carried(&rel, &key, shown, "holding.repository");
                    }
                    None => self.err(index, "M004", "holding must be a quoted string"),
                },
                "shelfmark" => {
                    // Always a string in the target: leading zeros and letters occur, and an
                    // integer shelfmark would lose them on the next round-trip.
                    match self.files[index].scalar_as_string("shelfmark") {
                        Some(v) => {
                            if self.files[index].str("shelfmark").is_none() {
                                self.warn(
                                    index,
                                    "M009",
                                    format!(
                                        "shelfmark was an integer and becomes the string \
                                         {v:?}; check it has no leading zeros"
                                    ),
                                );
                            }
                            out.holding.shelfmark = Some(v);
                            self.ledger.carried(&rel, &key, shown, "holding.shelfmark");
                        }
                        None => self.err(index, "M004", "shelfmark must be a scalar"),
                    }
                }
                "google_books_id" => match self.files[index].str("google_books_id") {
                    Some(v) => {
                        out.identifier.insert("google_books".to_string(), v);
                        self.ledger
                            .carried(&rel, &key, shown, "identifier.google_books");
                    }
                    None => self.err(index, "M004", "google_books_id must be a quoted string"),
                },
                other => self.err(
                    index,
                    "M001",
                    format!(
                        "no migration rule for the key {other:?}; add one to migrate.rs — a \
                         key is never dropped silently"
                    ),
                ),
            }
            handled.insert(key);
        }

        self.carry_comments(index, out);
    }

    /// A comment introducing a key whose value the migration turns into prose is already said
    /// by that prose. Every other comment is real information with nowhere else to go, so it
    /// joins `note`.
    fn carry_comments(&mut self, index: usize, out: &mut Common) {
        let rel = self.files[index].rel.clone();
        let mut carried: Vec<String> = Vec::new();
        for (key, lines) in self.files[index].comments() {
            let text = lines.join(" ");
            if out.prose_keys.contains(&key) {
                self.ledger.derivable(
                    &rel,
                    "# comment",
                    abbreviate(&text),
                    format!("already said by the scan.note prose generated from {key:?}"),
                );
                continue;
            }
            self.ledger
                .carried(&rel, "# comment", abbreviate(&text), "note");
            carried.push(text);
        }
        if carried.is_empty() {
            return;
        }
        let joined = carried.join(" ");
        self.warn(
            index,
            "M011",
            format!("comment carried into 'note': {}", abbreviate(&joined)),
        );
        out.note = Some(match out.note.take() {
            Some(existing) => format!("{} {joined}", existing.trim_end()),
            None => joined,
        });
    }

    // -- source ---------------------------------------------------------------------------

    fn build_source(&mut self, index: usize) {
        let rel = self.files[index].rel.clone();
        let id = self.files[index].id();
        let mut handled: BTreeSet<String> = BTreeSet::new();
        let mut common = Common::default();

        let r#type = self.files[index].str("kind");
        handled.insert("kind".to_string());
        self.ledger.carried(
            &rel,
            "kind",
            self.files[index].show("kind"),
            "layer = \"source\" + type",
        );

        // Sheets fold into this source's `[[page]]` array.
        let sheet_indices = self.sheets.get(&index).cloned().unwrap_or_default();
        let pages = self.fold_sheets(index, &sheet_indices);

        // `sheets = 21` is only derivable once 21 sheets have actually been folded in.
        if self.files[index].has("sheets") {
            handled.insert("sheets".to_string());
            let declared = self.files[index].int("sheets");
            let shown = self.files[index].show("sheets");
            match declared {
                Some(n) if n == pages.len() as i64 => self.ledger.derivable(
                    &rel,
                    "sheets",
                    shown,
                    format!("equals the {n} [[page]] entries folded in"),
                ),
                Some(n) => self.err(
                    index,
                    "M010",
                    format!(
                        "sheets = {n} but {} sheet file(s) were folded in; the count is not \
                         derivable, so nothing has been dropped",
                        pages.len()
                    ),
                ),
                None => self.err(index, "M004", "sheets must be an integer"),
            }
        }

        self.common_keys(index, &mut handled, &mut common);

        let record = Record::Source(Source {
            id: Some(id.clone()),
            of: common.of,
            r#type,
            title: common.title.unwrap_or_default(),
            short_title: common.short_title,
            language: common.language,
            place: common.place,
            country: common.country,
            founded: common.founded,
            frequency: common.frequency,
            date: common.date,
            covers: common.covers,
            note: common.note,
            url: common.url,
            pages: None,
            rights: common.rights.into_option(),
            holding: common.holding.into_option(),
            identifier: (!common.identifier.is_empty()).then_some(common.identifier),
            scan: None,
            resp: common.resp,
            link: common.links,
            page: pages,
            text: Vec::new(),
        });

        self.note_explicit_id(index, &id);
        self.check_title(index, &record);
        self.emit_record(index, &record);
    }

    // -- copy ------------------------------------------------------------------------------

    #[allow(clippy::too_many_lines)]
    fn build_copy(&mut self, index: usize) {
        let rel = self.files[index].rel.clone();
        let id = self.files[index].id();
        let mut handled: BTreeSet<String> = BTreeSet::new();
        let mut common = Common::default();
        let mut scan = Scan::default();
        let mut note_parts: Vec<String> = Vec::new();

        let r#type = self.files[index].str("kind");
        handled.insert("kind".to_string());
        self.ledger.carried(
            &rel,
            "kind",
            self.files[index].show("kind"),
            "layer = \"copy\" + type",
        );

        // The container and its page count.
        if self.files[index].has("file") {
            handled.insert("file".to_string());
            match self.files[index].str("file") {
                Some(f) => {
                    self.ledger
                        .carried(&rel, "file", self.files[index].show("file"), "scan.file");
                    scan.file = Some(f);
                }
                None => self.err(index, "M004", "file must be a quoted string"),
            }
        }
        if self.files[index].has("pages") {
            handled.insert("pages".to_string());
            let shown = self.files[index].show("pages");
            match self.files[index].int("pages") {
                Some(n) => {
                    // A copy's `pages` is a count, not a range. The document layer's `pages`
                    // is a range. Same key, two meanings, two conversions.
                    self.ledger.carried(&rel, "pages", shown, "scan.count");
                    scan.count = Some(n);
                }
                None => self.err(
                    index,
                    "M004",
                    "pages on a copy must be an integer page count",
                ),
            }
        }

        // Facts the target schema has no numeric slot for. They were measured by hand and
        // cannot be recomputed, so they survive as prose and each one is flagged.
        for key in ["google_frontmatter_removed", "repeat_scanned_pages", "repeat_scan_groups"] {
            if !self.files[index].has(key) {
                continue;
            }
            handled.insert(key.to_string());
            let shown = self.files[index].show(key);
            let Some(n) = self.files[index].int(key) else {
                self.err(index, "M004", format!("{key} must be an integer"));
                continue;
            };
            common.prose_keys.insert(key.to_string());
            self.ledger.carried(&rel, key, shown, "scan.note (as prose)");
            self.warn(
                index,
                "M008",
                format!(
                    "{key} = {n} has no numeric slot in the target schema and survives only as \
                     prose in scan.note; it cannot be recomputed"
                ),
            );
            note_parts.push(match key {
                "google_frontmatter_removed" if n == 1 => "The Google-generated front-matter \
                     leaf was removed from the PDF; graphic page indices are 1-based into the \
                     trimmed file."
                    .to_string(),
                "google_frontmatter_removed" => format!(
                    "{n} Google-generated front-matter leaves were removed from the PDF; \
                     graphic page indices are 1-based into the trimmed file."
                ),
                "repeat_scanned_pages" => format!(
                    "{n} pages are repeat scans of a leaf photographed more than once, \
                     measured by text comparison across the volume."
                ),
                _ => format!("Those repeat scans fall into {n} groups."),
            });
        }

        // `year` and `volume` restate what `date` and the id already say.
        if self.files[index].has("year") {
            handled.insert("year".to_string());
            let shown = self.files[index].show("year");
            match (
                self.files[index].int("year"),
                self.files[index].scalar_as_string("date"),
            ) {
                (Some(y), Some(d)) if d == y.to_string() => {
                    self.ledger
                        .derivable(&rel, "year", shown, format!("equals date = {d:?}"));
                }
                (Some(y), _) => self.err(
                    index,
                    "M005",
                    format!(
                        "year = {y} does not equal date, and the target schema has no separate \
                         'year' field; reconcile them by hand"
                    ),
                ),
                (None, _) => self.err(index, "M004", "year must be an integer"),
            }
        }
        if self.files[index].has("volume") {
            handled.insert("volume".to_string());
            let shown = self.files[index].show("volume");
            match self.files[index].int("volume") {
                Some(v) if id.ends_with(&format!("-vol{v}")) => self.ledger.derivable(
                    &rel,
                    "volume",
                    shown,
                    format!("equals the '-vol{v}' suffix of the id {id:?}"),
                ),
                Some(v) => self.err(
                    index,
                    "M005",
                    format!(
                        "volume = {v} is not recoverable from the id {id:?}, and the target \
                         schema has no 'volume' field; rename the file or record it in 'note'"
                    ),
                ),
                None => self.err(index, "M004", "volume must be an integer"),
            }
        }

        // Documents bound into the volume become documents of their own.
        if self.files[index].has("insert") {
            handled.insert("insert".to_string());
            let n = self.build_inserts(index, &id);
            self.ledger.carried(
                &rel,
                "insert",
                self.files[index].show("insert"),
                format!("{n} separate layer = \"document\" file(s), type = \"insert\""),
            );
        }

        self.common_keys(index, &mut handled, &mut common);

        // Now that `attribution` has been read, the digitising agent can be named.
        if scan.file.is_some()
            && let Some(by) = common
                .rights
                .attribution
                .as_deref()
                .and_then(scan_by_from_attribution)
        {
            self.ledger
                .added(&rel, "scan.by", format!("{by:?}"), "read out of the attribution");
            scan.by = Some(by);
        }
        if !note_parts.is_empty() {
            scan.note = Some(note_parts.join(" "));
        }

        // `covers` is what check 6 files a document against, and no copy declares one. It is
        // synthesised from the month span the title already states, and skipped when the
        // title does not state one.
        if common.covers.is_none() {
            let year = common.date.as_deref().and_then(|d| d.parse::<i32>().ok());
            if let (Some(title), Some(year)) = (common.title.as_deref(), year)
                && let Some(covers) = covers_from_title(title, year)
            {
                self.ledger.added(
                    &rel,
                    "covers",
                    format!("{covers:?}"),
                    "the month span in the title, over the year in 'date'",
                );
                self.warn(
                    index,
                    "M009",
                    format!(
                        "covers = {covers:?} was synthesised from the title's month span; \
                         check it against the volume"
                    ),
                );
                common.covers = Some(covers);
            }
        }

        let record = Record::Copy(CopyRecord {
            id: Some(id.clone()),
            of: common.of,
            r#type,
            title: common.title.unwrap_or_default(),
            short_title: common.short_title,
            language: common.language,
            place: common.place,
            country: common.country,
            covers: common.covers,
            date: common.date,
            note: common.note,
            url: common.url,
            scan: scan_into_option(scan),
            holding: common.holding.into_option(),
            identifier: (!common.identifier.is_empty()).then_some(common.identifier),
            rights: common.rights.into_option(),
            resp: common.resp,
            link: common.links,
            page: Vec::new(),
            text: Vec::new(),
        });

        self.note_explicit_id(index, &id);
        self.check_title(index, &record);
        self.emit_record(index, &record);
    }

    /// `[[insert]]` entries become documents in their own right, so check 4 sees their page
    /// ranges as siblings of the issues that surround them.
    fn build_inserts(&mut self, index: usize, copy_id: &str) -> usize {
        let rel = self.files[index].rel.clone();
        let dir = self.files[index].dir().to_path_buf();
        let Some(entries) = self.files[index]
            .doc
            .get("insert")
            .and_then(Item::as_array_of_tables)
        else {
            self.err(index, "M004", "insert must be an array of tables");
            return 0;
        };

        // Collected first so the borrow of `self.files` ends before anything is recorded.
        struct RawInsert {
            title: Option<String>,
            pages: Option<String>,
            pagination: Option<String>,
            unknown: Vec<String>,
        }
        let raw: Vec<RawInsert> = entries
            .iter()
            .map(|t| RawInsert {
                title: t.get("title").and_then(Item::as_str).map(str::to_string),
                pages: t.get("pages").and_then(Item::as_str).map(str::to_string),
                pagination: t
                    .get("pagination")
                    .and_then(Item::as_str)
                    .map(str::to_string),
                unknown: t
                    .iter()
                    .map(|(k, _)| k.to_string())
                    .filter(|k| !matches!(k.as_str(), "title" | "pages" | "pagination"))
                    .collect(),
            })
            .collect();

        let count = raw.len();
        for (i, entry) in raw.into_iter().enumerate() {
            let n = i + 1;
            for key in &entry.unknown {
                self.err(
                    index,
                    "M001",
                    format!("no migration rule for the key {key:?} inside [[insert]]#{i}"),
                );
            }
            let Some(range_raw) = entry.pages else {
                self.err(index, "M004", format!("[[insert]]#{i} has no 'pages'"));
                continue;
            };
            let pages = match parse_page_range(&range_raw) {
                Ok(p) => p,
                Err(e) => {
                    self.err(index, "M004", format!("[[insert]]#{i}: {e}"));
                    continue;
                }
            };
            self.ledger.carried(
                &rel,
                "insert.pages",
                abbreviate(&format!("{range_raw:?}")),
                format!("pages = {{ from = {}, to = {} }}", pages.from, pages.to),
            );
            if let Some(title) = &entry.title {
                self.ledger.carried(
                    &rel,
                    "insert.title",
                    abbreviate(&format!("{title:?}")),
                    "title",
                );
            }
            // `pagination = "roman v-xxxvj"` describes the printed pagination of the insert as
            // a whole. It does not decompose onto individual pages — the label run is shorter
            // than the page run — so `page.label` cannot hold it and `note` is its only
            // honest home.
            let note = entry.pagination.as_ref().map(|p| {
                self.ledger.carried(
                    &rel,
                    "insert.pagination",
                    abbreviate(&format!("{p:?}")),
                    "note (the run does not decompose onto page.label)",
                );
                format!("Printed pagination: {p}.")
            });

            let id = format!("{copy_id}-insert-{n}");
            let record = Record::Document(Document {
                id: Some(id.clone()),
                of: Some(copy_id.to_string()),
                r#type: Some("insert".to_string()),
                title: entry.title,
                pages: Some(pages),
                note,
                ..Document::default()
            });
            let path = dir.join(format!("{id}.toml"));
            let target = self.target(&path);
            let target_rel = rel_display(&self.root, &target);
            self.ledger.added(
                &target_rel,
                "id",
                format!("{id:?}"),
                format!("[[insert]]#{i} of {copy_id} promoted to a document of its own"),
            );
            let directive = schema_directive(&self.root, &target);
            match emit(&record, &directive) {
                Ok(contents) => self.actions.push(Action::Write {
                    path: target,
                    contents,
                }),
                Err(e) => self.err(index, "M012", format!("{e:#}")),
            }
        }
        count
    }

    // -- document ---------------------------------------------------------------------------

    fn build_document(&mut self, index: usize) {
        let rel = self.files[index].rel.clone();
        let id = self.files[index].id();
        let mut handled: BTreeSet<String> = BTreeSet::new();
        let mut common = Common::default();
        let mut scan = Scan::default();

        let r#type = self.files[index].str("kind");
        handled.insert("kind".to_string());
        if self.files[index].has("kind") {
            self.ledger.carried(
                &rel,
                "kind",
                self.files[index].show("kind"),
                "layer = \"document\" + type",
            );
        }

        let mut no = None;
        if self.files[index].has("no") {
            handled.insert("no".to_string());
            match self.files[index].int("no") {
                Some(n) => {
                    no = Some(n);
                    self.ledger
                        .carried(&rel, "no", self.files[index].show("no"), "no");
                }
                None => self.err(index, "M004", "no must be an integer"),
            }
        }

        // Step 7: `pages = "13-16"` -> `pages = { from = 13, to = 16 }`.
        let mut pages = None;
        if self.files[index].has("pages") {
            handled.insert("pages".to_string());
            let shown = self.files[index].show("pages");
            match self.files[index].str("pages") {
                Some(raw) => match parse_page_range(&raw) {
                    Ok(range) => {
                        self.ledger.carried(
                            &rel,
                            "pages",
                            shown,
                            format!("pages = {{ from = {}, to = {} }}", range.from, range.to),
                        );
                        pages = Some(range);
                    }
                    Err(e) => self.err(index, "M004", e),
                },
                None => self.err(
                    index,
                    "M004",
                    "pages on a document must be a quoted range of the form \"N-M\"",
                ),
            }
        }

        // Step 6: a document's `source = "…vol1.pdf"` is the copy's `scan.file`, inherited.
        if self.files[index].has("source") {
            handled.insert("source".to_string());
            let shown = self.files[index].show("source");
            let declared = self.files[index].str("source");
            let from_copy = self.inherited(index, "file");
            match (declared, from_copy) {
                (Some(v), Some(parent)) if v == parent => self.ledger.derivable(
                    &rel,
                    "source",
                    shown,
                    "identical to the copy's container, so it is inherited as scan.file",
                ),
                (Some(v), _) => {
                    self.ledger.carried(&rel, "source", shown, "scan.file");
                    self.warn(
                        index,
                        "M009",
                        format!(
                            "source = {v:?} differs from the container the ancestors declare, \
                             so it is kept as this document's own scan.file"
                        ),
                    );
                    scan.file = Some(v);
                }
                (None, _) => self.err(index, "M004", "source must be a quoted string"),
            }
        }

        // Supplements point at an id rather than a bare integer.
        let mut supplement_to = None;
        if self.files[index].has("supplement_to") {
            handled.insert("supplement_to".to_string());
            let shown = self.files[index].show("supplement_to");
            if let Some(target) = self.files[index].str("supplement_to") {
                supplement_to = Some(target);
                self.ledger
                    .carried(&rel, "supplement_to", shown, "supplement_to");
            } else if let Some(number) = self.files[index].int("supplement_to") {
                match self.sibling_with_no(index, number) {
                    Some(sibling) => {
                        self.ledger.carried(
                            &rel,
                            "supplement_to",
                            shown,
                            format!("supplement_to = {sibling:?} (the id of no. {number})"),
                        );
                        supplement_to = Some(sibling);
                    }
                    None => self.err(
                        index,
                        "M006",
                        format!(
                            "supplement_to = {number} names no sibling document with that \
                             'no'; supplements must point at an id, and there is nothing to \
                             point at"
                        ),
                    ),
                }
            } else {
                self.err(index, "M004", "supplement_to must be an integer or a string");
            }
        }

        self.common_keys(index, &mut handled, &mut common);

        let record = Record::Document(Document {
            id: Some(id.clone()),
            of: common.of,
            r#type,
            title: common.title,
            short_title: common.short_title,
            no,
            date: common.date,
            covers: common.covers,
            supplement_to,
            pages,
            note: common.note,
            url: common.url,
            language: common.language,
            place: common.place,
            country: common.country,
            rights: common.rights.into_option(),
            holding: common.holding.into_option(),
            identifier: (!common.identifier.is_empty()).then_some(common.identifier),
            scan: scan_into_option(scan),
            resp: common.resp,
            link: common.links,
            page: Vec::new(),
            text: Vec::new(),
        });

        self.note_explicit_id(index, &id);
        self.emit_record(index, &record);
    }

    /// The id of the document under the same parent whose legacy `no` is `number`.
    fn sibling_with_no(&self, index: usize, number: i64) -> Option<String> {
        let of = self.files[index].str("of")?;
        self.files
            .iter()
            .enumerate()
            .find(|(i, f)| {
                *i != index
                    && self.shapes[*i] == Shape::Document
                    && f.str("of").as_deref() == Some(of.as_str())
                    && f.int("no") == Some(number)
            })
            .map(|(_, f)| f.id())
    }

    // -- sheets -> [[page]] -----------------------------------------------------------------

    /// Step 8. Each sheet contributes `n`, its page title, and one `[[page.graphic]]`.
    #[allow(clippy::too_many_lines)]
    fn fold_sheets(&mut self, parent: usize, sheets: &[usize]) -> Vec<Page> {
        let parent_rel = self.files[parent].rel.clone();
        let parent_id = self.files[parent].id();
        let parent_title = self.files[parent].str("title").unwrap_or_default();

        let mut pages = Vec::with_capacity(sheets.len());
        for &index in sheets {
            let rel = self.files[index].rel.clone();
            let mut page = Page::default();
            let mut graphic = Graphic::default();
            let mut handled: BTreeSet<String> = BTreeSet::new();

            // `of` is what pointed the sheet at this file in the first place.
            handled.insert("of".to_string());
            self.ledger.derivable(
                &rel,
                "of",
                self.files[index].show("of"),
                format!("the fold target: this page now lives inside {parent_rel}"),
            );

            handled.insert("sheet".to_string());
            let Some(n) = self.files[index].int("sheet") else {
                self.err(index, "M004", "sheet must be an integer");
                continue;
            };
            page.n = Some(n);
            self.ledger
                .carried(&rel, "sheet", self.files[index].show("sheet"), "[[page]].n");

            // The title is `<source title>, sheet NN`, optionally with a parenthetical that
            // is the only part carrying information.
            handled.insert("title".to_string());
            let shown = self.files[index].show("title");
            let title = self.files[index].str("title").unwrap_or_default();
            let formula = format!("{parent_title}, sheet {n:02}");
            match title.strip_prefix(&formula) {
                Some("") => self.ledger.formulaic(
                    &rel,
                    "title",
                    shown,
                    "\"<source title>, sheet NN\", reproduced from title and [[page]].n",
                ),
                Some(rest) if rest.starts_with(" (") && rest.ends_with(')') => {
                    let inner = &rest[2..rest.len() - 1];
                    page.title = Some(inner.to_string());
                    self.ledger.carried(
                        &rel,
                        "title",
                        shown,
                        format!(
                            "[[page]].title = {inner:?} (the \"<source title>, sheet NN\" \
                             prefix is formulaic)"
                        ),
                    );
                }
                _ => {
                    page.title = Some(title.clone());
                    self.ledger.carried(
                        &rel,
                        "title",
                        shown,
                        "[[page]].title, in full — it does not match the sheet-title formula",
                    );
                }
            }

            for (key, slot) in [("width", 0u8), ("height", 1u8)] {
                if !self.files[index].has(key) {
                    continue;
                }
                handled.insert(key.to_string());
                match self.files[index].int(key) {
                    Some(v) => {
                        if slot == 0 {
                            graphic.width = Some(v);
                        } else {
                            graphic.height = Some(v);
                        }
                        self.ledger.carried(
                            &rel,
                            key,
                            self.files[index].show(key),
                            format!("[[page.graphic]].{key}"),
                        );
                    }
                    None => self.err(index, "M004", format!("{key} must be an integer")),
                }
            }

            if self.files[index].has("url") {
                handled.insert("url".to_string());
                page.url = self.files[index].str("url");
                self.ledger.carried(
                    &rel,
                    "url",
                    self.files[index].show("url"),
                    "[[page]].url — each sheet has its own landing page and url never inherits",
                );
            }
            if self.files[index].has("fetch") {
                handled.insert("fetch".to_string());
                graphic.url = self.files[index].str("fetch");
                self.ledger.carried(
                    &rel,
                    "fetch",
                    self.files[index].show("fetch"),
                    "[[page.graphic]].url",
                );
            }

            // A page carries no author, date, licence or attribution of its own, so these can
            // only be dropped if they are exactly what the source already says. If one
            // differs there is nowhere to put it, and that is an error rather than a loss.
            for key in ["author", "date", "licence", "license", "attribution"] {
                if !self.files[index].has(key) {
                    continue;
                }
                handled.insert(key.to_string());
                let shown = self.files[index].show(key);
                let mine = self.files[index].scalar_as_string(key);
                let theirs = self.files[parent].scalar_as_string(key);
                match (mine, theirs) {
                    (Some(a), Some(b)) if a == b => self.ledger.derivable(
                        &rel,
                        key,
                        shown,
                        format!(
                            "identical to {key} on {parent_id}, and a [[page]] has no {key} of \
                             its own"
                        ),
                    ),
                    (Some(a), b) => self.err(
                        index,
                        "M005",
                        format!(
                            "{key} = {a:?} differs from the source's ({b:?}), and a [[page]] \
                             has no slot for it; reconcile them by hand"
                        ),
                    ),
                    (None, _) => self.err(index, "M004", format!("{key} must be a scalar")),
                }
            }

            // The image is never named by the sheet file, so it is found on disk beside it.
            match self.find_image(index) {
                Some(name) => {
                    self.ledger.added(
                        &rel,
                        "[[page.graphic]].file",
                        format!("{name:?}"),
                        format!("the image sitting beside this record, folded into {parent_rel}"),
                    );
                    graphic.file = Some(name);
                }
                None => self.err(
                    index,
                    "M007",
                    format!(
                        "no image file found beside this sheet (looked for {} named {}.*); a \
                         [[page.graphic]] must name its file",
                        IMAGE_EXTENSIONS.join("/"),
                        self.files[index].stem
                    ),
                ),
            }

            for key in self.files[index].keys() {
                if handled.contains(&key) {
                    continue;
                }
                self.err(
                    index,
                    "M001",
                    format!(
                        "no migration rule for the key {key:?} on a sheet; add one to \
                         fold_sheets in migrate.rs — a key is never dropped silently"
                    ),
                );
            }

            page.graphic = vec![graphic];
            pages.push(page);
        }
        pages
    }

    /// The image beside a sheet record: same stem, a known image extension.
    fn find_image(&self, index: usize) -> Option<String> {
        let legacy = &self.files[index];
        for ext in IMAGE_EXTENSIONS {
            let candidate = legacy.dir().join(format!("{}.{ext}", legacy.stem));
            if candidate.is_file() {
                return Some(format!("{}.{ext}", legacy.stem));
            }
        }
        None
    }

    // -- assets and bookkeeping ----------------------------------------------------------

    fn note_explicit_id(&mut self, index: usize, id: &str) {
        if self.files[index].has("id") {
            return;
        }
        let target = self.target(&self.files[index].path.clone());
        let rel = rel_display(&self.root, &target);
        self.ledger.added(
            &rel,
            "id",
            format!("{id:?}"),
            "the filename stem, now stated explicitly as the spec's examples do",
        );
    }

    /// A source and a copy must have a title; the model would otherwise emit an empty one.
    fn check_title(&mut self, index: usize, record: &Record) {
        if record.title().is_some_and(|t| !t.is_empty()) {
            return;
        }
        self.err(
            index,
            "M004",
            format!("a {} must have a non-empty title", record.layer()),
        );
    }

    /// Every non-record file under a migrated directory moves with it: the PDFs and the 94
    /// `.jp2` sheets. They are not renamed — they are in LFS and their names carry no meaning.
    fn move_assets(&mut self) -> Result<()> {
        let mut records: BTreeSet<PathBuf> = BTreeSet::new();
        let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
        for legacy in &self.files {
            records.insert(legacy.path.clone());
            // The top-level directory the record lives under is what gets migrated wholesale.
            if let Ok(rel) = legacy.path.strip_prefix(&self.root)
                && let Some(first) = rel.components().next()
            {
                dirs.insert(self.root.join(first.as_os_str()));
            }
        }

        let mut assets = Vec::new();
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&dir).sort_by_file_name() {
                let entry = entry.with_context(|| format!("walking {}", dir.display()))?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.into_path();
                if records.contains(&path) {
                    continue;
                }
                assets.push(path);
            }
        }
        assets.sort();
        for path in assets {
            let to = self.target(&path);
            self.actions.push(Action::Move { from: path, to });
        }
        Ok(())
    }
}

/// Fields every layer shares, collected before the record is assembled.
#[derive(Debug, Default)]
struct Common {
    of: Option<String>,
    title: Option<String>,
    short_title: Option<String>,
    language: Option<String>,
    place: Option<String>,
    country: Option<String>,
    frequency: Option<String>,
    founded: Option<String>,
    date: Option<String>,
    covers: Option<String>,
    note: Option<String>,
    url: Option<String>,
    rights: Rights,
    holding: Holding,
    identifier: Identifier,
    resp: Option<Vec<Resp>>,
    links: Vec<Link>,
    /// Keys whose value the migration turns into prose, so a comment introducing one of them
    /// is already said and need not be carried again.
    prose_keys: BTreeSet<String>,
}

trait IntoOption: Sized {
    fn is_blank(&self) -> bool;
    fn into_option(self) -> Option<Self> {
        if self.is_blank() { None } else { Some(self) }
    }
}

impl IntoOption for Rights {
    fn is_blank(&self) -> bool {
        *self == Rights::default()
    }
}

impl IntoOption for Holding {
    fn is_blank(&self) -> bool {
        *self == Holding::default()
    }
}

fn scan_into_option(scan: Scan) -> Option<Scan> {
    (scan != Scan::default()).then_some(scan)
}

/// Recursively find the legacy `*.toml` records under `root`.
fn walk_toml(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 || !e.file_type().is_dir() {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref())
        });
    for entry in walker {
        let entry = entry.with_context(|| format!("walking {}", root.display()))?;
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "toml") {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name.starts_with('.') || CONFIG_FILES.contains(&name.as_ref()) {
            continue;
        }
        out.push(entry.into_path());
    }
    out.sort();
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------------------

/// The `#:schema` line a record carries as its first line, pointing at
/// `schemas/source.json` relative to the record's own location.
///
/// This is the only portable way to get a record validated in an editor. As
/// `schemas/README.md` records, `evenBetterToml.schema.associations` cannot express a
/// repo-relative schema path — a relative value resolves against an internal `root:///`
/// base, `${workspaceFolder}` is not expanded, and a bare Windows path parses as a URL with
/// the scheme `d` — and in every one of those cases the association matches the document and
/// then reports nothing, which looks exactly like a clean file. So the directive is written
/// into the record itself, with the `../` depth computed from where the record lands.
fn schema_directive(root: &Path, target: &Path) -> String {
    let depth = target
        .strip_prefix(root)
        .unwrap_or(target)
        .components()
        .count()
        .saturating_sub(1);
    let up = "../".repeat(depth);
    format!("#:schema {up}schemas/source.json\n")
}

/// Render a record as TOML formatted for a human, and prove nothing was lost doing it.
///
/// The emitter writes fields in a fixed order and aligns the `=` within each block, which
/// `toml_edit`'s serialiser will not do. The price of hand-writing it is that a field the
/// emitter forgets would vanish silently — so the result is parsed straight back into a
/// [`Record`] and compared. A mismatch is an error, not a lost value.
///
/// `directive` is prepended before that check, not after, so the round trip covers the exact
/// bytes that reach disk rather than a prefix of them.
pub fn emit(record: &Record, directive: &str) -> Result<String> {
    let text = format!("{directive}{}", render(record));
    let round_trip: Record = toml::from_str(&text).map_err(|e| {
        anyhow!("the emitted TOML does not parse back as a record: {e}\n--- emitted ---\n{text}")
    })?;
    if &round_trip != record {
        bail!(
            "the emitted TOML does not round-trip: a field was dropped or changed by the \
             emitter.\n--- emitted ---\n{text}"
        );
    }
    Ok(text)
}

fn render(record: &Record) -> String {
    let mut doc = DocumentMut::new();
    let (layer, scalars, tables) = describe(record);

    // Scalars first, in a fixed order, `=` aligned.
    doc.insert("id", none_if_empty(scalars.id.clone()));
    doc["layer"] = value(layer);
    for (key, item) in scalars.rest() {
        doc.insert(key, item);
    }
    doc.retain(|_, item| !item.is_none());
    align(doc.as_table_mut());

    for (key, item) in tables {
        let mut item = item;
        space_before(&mut item);
        doc.insert(key, item);
    }

    doc.to_string()
}

/// A blank line before a block, and none between the entries within it — `[[resp]]` and
/// `[[link]]` read as a list, not as five separate sections. `[[page]]` overrides this in
/// [`page_item`], because pages do want air between them.
fn space_before(item: &mut Item) {
    match item {
        Item::Table(t) => t.decor_mut().set_prefix("\n"),
        Item::ArrayOfTables(aot) => {
            for (i, t) in aot.iter_mut().enumerate() {
                if i == 0 {
                    t.decor_mut().set_prefix("\n");
                } else if t.decor().prefix().is_none() {
                    t.decor_mut().set_prefix("");
                }
            }
        }
        _ => {}
    }
}

/// Align the `=` of every scalar entry in a table. Sub-tables are skipped: they have their own
/// block and their own alignment.
fn align(table: &mut Table) {
    let width = table
        .iter()
        .filter(|(_, item)| item.is_value())
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0);
    for (mut key, item) in table.iter_mut() {
        if !item.is_value() {
            continue;
        }
        let pad = width.saturating_sub(key.get().chars().count()) + 1;
        key.leaf_decor_mut().set_suffix(" ".repeat(pad));
    }
}

fn none_if_empty(v: Option<String>) -> Item {
    match v {
        Some(s) => value(s),
        None => Item::None,
    }
}

fn opt_str(v: &Option<String>) -> Item {
    none_if_empty(v.clone())
}

fn opt_int(v: Option<i64>) -> Item {
    match v {
        Some(n) => value(n),
        None => Item::None,
    }
}

/// The scalar block of a record, in emission order.
struct Scalars {
    id: Option<String>,
    entries: Vec<(&'static str, Item)>,
}

impl Scalars {
    fn rest(&self) -> Vec<(&'static str, Item)> {
        self.entries.clone()
    }
}

/// Split a record into `(layer, scalars, tables)` in emission order.
fn describe(record: &Record) -> (&'static str, Scalars, Vec<(&'static str, Item)>) {
    match record {
        Record::Source(r) => (
            "source",
            Scalars {
                id: r.id.clone(),
                entries: vec![
                    ("of", opt_str(&r.of)),
                    ("type", opt_str(&r.r#type)),
                    ("title", value(r.title.clone())),
                    ("short_title", opt_str(&r.short_title)),
                    ("language", opt_str(&r.language)),
                    ("place", opt_str(&r.place)),
                    ("country", opt_str(&r.country)),
                    ("founded", opt_str(&r.founded)),
                    ("frequency", opt_str(&r.frequency)),
                    ("date", opt_str(&r.date)),
                    ("covers", opt_str(&r.covers)),
                    ("pages", page_range_item(r.pages)),
                    ("note", opt_str(&r.note)),
                    ("url", opt_str(&r.url)),
                ],
            },
            common_tables(
                r.resp.as_deref(),
                r.scan.as_ref(),
                r.holding.as_ref(),
                r.identifier.as_ref(),
                r.rights.as_ref(),
                &r.link,
                &r.text,
                &r.page,
            ),
        ),
        Record::Copy(r) => (
            "copy",
            Scalars {
                id: r.id.clone(),
                entries: vec![
                    ("of", opt_str(&r.of)),
                    ("type", opt_str(&r.r#type)),
                    ("title", value(r.title.clone())),
                    ("short_title", opt_str(&r.short_title)),
                    ("language", opt_str(&r.language)),
                    ("place", opt_str(&r.place)),
                    ("country", opt_str(&r.country)),
                    ("date", opt_str(&r.date)),
                    ("covers", opt_str(&r.covers)),
                    ("note", opt_str(&r.note)),
                    ("url", opt_str(&r.url)),
                ],
            },
            common_tables(
                r.resp.as_deref(),
                r.scan.as_ref(),
                r.holding.as_ref(),
                r.identifier.as_ref(),
                r.rights.as_ref(),
                &r.link,
                &r.text,
                &r.page,
            ),
        ),
        Record::Document(r) => (
            "document",
            Scalars {
                id: r.id.clone(),
                entries: vec![
                    ("of", opt_str(&r.of)),
                    ("type", opt_str(&r.r#type)),
                    ("title", opt_str(&r.title)),
                    ("short_title", opt_str(&r.short_title)),
                    ("no", opt_int(r.no)),
                    ("date", opt_str(&r.date)),
                    ("covers", opt_str(&r.covers)),
                    ("supplement_to", opt_str(&r.supplement_to)),
                    ("pages", page_range_item(r.pages)),
                    ("language", opt_str(&r.language)),
                    ("place", opt_str(&r.place)),
                    ("country", opt_str(&r.country)),
                    ("note", opt_str(&r.note)),
                    ("url", opt_str(&r.url)),
                ],
            },
            common_tables(
                r.resp.as_deref(),
                r.scan.as_ref(),
                r.holding.as_ref(),
                r.identifier.as_ref(),
                r.rights.as_ref(),
                &r.link,
                &r.text,
                &r.page,
            ),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn common_tables(
    resp: Option<&[Resp]>,
    scan: Option<&Scan>,
    holding: Option<&Holding>,
    identifier: Option<&Identifier>,
    rights: Option<&Rights>,
    link: &[Link],
    text: &[Text],
    page: &[Page],
) -> Vec<(&'static str, Item)> {
    let mut out: Vec<(&'static str, Item)> = Vec::new();
    if let Some(resp) = resp {
        out.push(("resp", resp_item(resp)));
    }
    if let Some(scan) = scan {
        out.push(("scan", scan_item(scan)));
    }
    if let Some(holding) = holding {
        out.push(("holding", holding_item(holding)));
    }
    if let Some(identifier) = identifier {
        out.push(("identifier", identifier_item(identifier)));
    }
    if let Some(rights) = rights {
        out.push(("rights", rights_item(rights)));
    }
    if !link.is_empty() {
        out.push(("link", link_item(link)));
    }
    if !text.is_empty() {
        out.push(("text", text_item(text)));
    }
    if !page.is_empty() {
        out.push(("page", page_item(page)));
    }
    out
}

/// A table built from `(key, Item)` pairs, dropping absent ones and aligning what remains.
fn table_of(entries: Vec<(&str, Item)>) -> Table {
    let mut table = Table::new();
    for (key, item) in entries {
        if item.is_none() {
            continue;
        }
        table.insert(key, item);
    }
    align(&mut table);
    table
}

fn page_range_item(range: Option<PageRange>) -> Item {
    match range {
        None => Item::None,
        Some(r) => {
            let mut inline = toml_edit::InlineTable::new();
            inline.insert("from", r.from.into());
            inline.insert("to", r.to.into());
            value(inline)
        }
    }
}

fn resp_item(resp: &[Resp]) -> Item {
    // `resp = []` is the documented way to clear an inherited value, so an empty array must
    // survive as an empty array rather than disappearing.
    if resp.is_empty() {
        return value(Array::new());
    }
    let mut aot = ArrayOfTables::new();
    for entry in resp {
        let role = match &entry.role {
            None => Item::None,
            Some(Roles::One(r)) => value(r.clone()),
            Some(Roles::Many(rs)) => {
                let mut array = Array::new();
                for r in rs {
                    array.push(r.as_str());
                }
                value(array)
            }
        };
        aot.push(table_of(vec![
            ("name", value(entry.name.clone())),
            ("role", role),
            ("note", opt_str(&entry.note)),
        ]));
    }
    Item::ArrayOfTables(aot)
}

fn scan_item(scan: &Scan) -> Item {
    Item::Table(table_of(vec![
        ("file", opt_str(&scan.file)),
        ("count", opt_int(scan.count)),
        ("by", opt_str(&scan.by)),
        ("url", opt_str(&scan.url)),
        ("note", opt_str(&scan.note)),
    ]))
}

fn holding_item(holding: &Holding) -> Item {
    Item::Table(table_of(vec![
        ("repository", opt_str(&holding.repository)),
        ("shelfmark", opt_str(&holding.shelfmark)),
        ("collection", opt_str(&holding.collection)),
        ("note", opt_str(&holding.note)),
    ]))
}

fn identifier_item(identifier: &Identifier) -> Item {
    let entries: Vec<(&str, Item)> = identifier
        .iter()
        .map(|(k, v)| (k.as_str(), value(v.clone())))
        .collect();
    Item::Table(table_of(entries))
}

fn rights_item(rights: &Rights) -> Item {
    Item::Table(table_of(vec![
        ("work", opt_str(&rights.work)),
        ("scan", opt_str(&rights.scan)),
        ("attribution", opt_str(&rights.attribution)),
        ("note", opt_str(&rights.note)),
    ]))
}

fn link_item(links: &[Link]) -> Item {
    let mut aot = ArrayOfTables::new();
    for link in links {
        aot.push(table_of(vec![
            ("rel", value(link.rel.clone())),
            ("url", value(link.url.clone())),
            ("title", opt_str(&link.title)),
            ("note", opt_str(&link.note)),
        ]));
    }
    Item::ArrayOfTables(aot)
}

fn text_item(texts: &[Text]) -> Item {
    let mut aot = ArrayOfTables::new();
    for text in texts {
        aot.push(table_of(vec![
            ("file", value(text.file.clone())),
            ("kind", opt_str(&text.kind)),
            ("by", opt_str(&text.by)),
            ("lang", opt_str(&text.lang)),
            ("note", opt_str(&text.note)),
        ]));
    }
    Item::ArrayOfTables(aot)
}

/// Pages in declared order, each followed immediately by its graphics — which is how the spec
/// prints them, and how they diff as a table.
fn page_item(pages: &[Page]) -> Item {
    let mut aot = ArrayOfTables::new();
    for page in pages {
        let mut table = table_of(vec![
            ("n", opt_int(page.n)),
            ("title", opt_str(&page.title)),
            ("label", opt_str(&page.label)),
            ("url", opt_str(&page.url)),
            ("note", opt_str(&page.note)),
        ]);
        for (key, item) in [
            ("graphic", graphic_item(&page.graphic)),
            ("text", text_item(&page.text)),
            ("zone", zone_item(&page.zone)),
            ("link", link_item(&page.link)),
        ] {
            let Item::ArrayOfTables(mut nested) = item else {
                continue;
            };
            if nested.is_empty() {
                continue;
            }
            // A page's graphics belong to the page above them, so they get no blank line.
            // `toml_edit` would otherwise put one there.
            for t in nested.iter_mut() {
                t.decor_mut().set_prefix("");
            }
            table.insert(key, Item::ArrayOfTables(nested));
        }
        aot.push(table);
    }
    // A blank line between pages; none within one.
    for table in aot.iter_mut() {
        table.decor_mut().set_prefix("\n");
    }
    Item::ArrayOfTables(aot)
}

fn graphic_item(graphics: &[Graphic]) -> Item {
    let mut aot = ArrayOfTables::new();
    for graphic in graphics {
        aot.push(table_of(vec![
            ("file", opt_str(&graphic.file)),
            ("page", opt_int(graphic.page)),
            ("width", opt_int(graphic.width)),
            ("height", opt_int(graphic.height)),
            ("url", opt_str(&graphic.url)),
            ("mimetype", opt_str(&graphic.mimetype)),
            ("note", opt_str(&graphic.note)),
        ]));
    }
    Item::ArrayOfTables(aot)
}

fn zone_item(zones: &[crate::model::Zone]) -> Item {
    let mut aot = ArrayOfTables::new();
    for zone in zones {
        aot.push(table_of(vec![
            ("id", opt_str(&zone.id)),
            ("ulx", value(zone.ulx)),
            ("uly", value(zone.uly)),
            ("lrx", value(zone.lrx)),
            ("lry", value(zone.lry)),
            ("label", opt_str(&zone.label)),
            ("note", opt_str(&zone.note)),
        ]));
    }
    Item::ArrayOfTables(aot)
}

// ---------------------------------------------------------------------------------------
// Filesystem effects
// ---------------------------------------------------------------------------------------

/// `git mv`, falling back to a plain rename for a file git does not track.
fn move_file(root: &Path, from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let output = std::process::Command::new("git")
        .current_dir(root)
        .arg("mv")
        .arg(from)
        .arg(to)
        .output();
    match output {
        Ok(o) if o.status.success() => return Ok(()),
        Ok(_) | Err(_) => {}
    }
    std::fs::rename(from, to).with_context(|| {
        format!(
            "moving {} to {} (git mv declined it, so it is probably untracked)",
            from.display(),
            to.display()
        )
    })
}

/// `git rm`, falling back to a plain removal for a file git does not track.
fn delete_file(root: &Path, path: &Path) -> Result<()> {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .arg("rm")
        .arg("--quiet")
        .arg(path)
        .output();
    match output {
        Ok(o) if o.status.success() => return Ok(()),
        Ok(_) | Err(_) => {}
    }
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
}

// ---------------------------------------------------------------------------------------
// Unit tests for the pure parts. The end-to-end fixtures live in tests/migrate.rs.
// ---------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ranges_parse_only_in_the_form_n_dash_m() {
        assert_eq!(
            parse_page_range("13-16"),
            Ok(PageRange { from: 13, to: 16 })
        );
        assert_eq!(
            parse_page_range("1103-1150"),
            Ok(PageRange {
                from: 1103,
                to: 1150
            })
        );
        for bad in ["13", "13-16,19", "13 - 16", "13–16", "16-13", "-16", "a-b"] {
            assert!(parse_page_range(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn covers_is_read_out_of_the_month_span_in_the_title() {
        assert_eq!(
            covers_from_title("Journal de Paris, annee 1789, volume 1 (janvier–juin)", 1789)
                .as_deref(),
            Some("1789-01-01/1789-06-30")
        );
        assert_eq!(
            covers_from_title(
                "Journal de Paris, annee 1789, volume 2 (juillet–décembre)",
                1789
            )
            .as_deref(),
            Some("1789-07-01/1789-12-31")
        );
        // February in a leap year, to prove the last day is computed rather than guessed.
        assert_eq!(
            covers_from_title("Something (janvier–fevrier)", 1788).as_deref(),
            Some("1788-01-01/1788-02-29")
        );
        // A title the migration does not fully understand yields nothing, never a guess.
        assert_eq!(covers_from_title("Plan de Turgot", 1739), None);
        assert_eq!(covers_from_title("Volume 1 (part one)", 1789), None);
        assert_eq!(covers_from_title("Volume 1 (juin–janvier)", 1789), None);
    }

    #[test]
    fn the_digitising_agent_is_read_out_of_the_attribution() {
        assert_eq!(
            scan_by_from_attribution("Digitised by Google Books.").as_deref(),
            Some("Google Books")
        );
        // A holding repository is not a digitising agent.
        assert_eq!(
            scan_by_from_attribution("Digitised by the David Rumsey Map Collection."),
            None
        );
        assert_eq!(scan_by_from_attribution("Photographed by hand."), None);
    }

    #[test]
    fn author_strings_are_a_lookup_and_never_a_guess() {
        let resp = author_to_resp(
            "Louis Bretez (survey and drawing); Claude Lucas (engraving); Aubin (lettering)",
        )
        .expect("a known author string");
        assert_eq!(resp.len(), 3);
        assert_eq!(resp[0].name, "Louis Bretez");
        assert_eq!(resp[0].roles(), vec!["surveyor", "draughtsman"]);
        assert_eq!(resp[1].roles(), vec!["engraver"]);
        assert_eq!(author_to_resp("Someone Unknown"), None);
    }

    #[test]
    fn comments_are_lifted_out_of_the_decor() {
        assert_eq!(
            comment_lines("\n# one\n#  two \n"),
            vec!["one".to_string(), "two".to_string()]
        );
        assert_eq!(comment_lines("\n\n"), Vec::<String>::new());
    }
}
