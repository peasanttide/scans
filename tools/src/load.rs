//! Discovery, parsing, the id index, `of` resolution, inheritance and page expansion.
//!
//! [`load_archive`] does all of it in one pass and hands back an [`Archive`] in which every
//! node already carries its resolved inherited values ([`Node::resolved`]) and its expanded
//! page list ([`Node::pages`]). Nothing is computed lazily, so the result is deterministic and
//! shareable by `&`.
//!
//! # Which findings this module owns
//!
//! Loading emits the structural findings — the ones that must be settled before any other
//! check can run. `validate.rs` must **not** re-report these, or every one will appear twice:
//!
//! | code | meaning |
//! |---|---|
//! | `E010` | not valid TOML |
//! | `E011` | missing required field |
//! | `E012` | field has the wrong type |
//! | `E013` | unrecognised field (`deny_unknown_fields`) |
//! | `E014` | `layer` is not one of source/copy/document |
//! | `E015` | `layer = "page"` on a file |
//! | `E016` | path is absolute or escapes the repository root |
//! | `E101` | duplicate id |
//! | `E102` | `of` does not resolve |
//! | `E103` | cycle in the `of` chain |
//! | `E104` | id contains a reserved character |
//! | `E108` | `of` chain deeper than 32 |
//! | `E301` | duplicate resolved page `n` within one owner |
//! | `E901`, `E902`, `E903`, `E904`, `E905`, `E906` | page expansion |
//! | `W903`, `W904` | page expansion warnings |
//!
//! Everything else belongs to `validate.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::Context;

use crate::model::{Graphic, Layer, Page, Record, Resp, Text, Zone};

/// The directory beneath the repository root that holds all sources.
///
/// The spec's layout section and migration step 1 both say `source/` (singular). Everything
/// that needs to name the destination root reads it from here, so flipping it to `sources/`
/// is a one-line change rather than a rewrite of every path in the repo.
pub const SOURCE_ROOT: &str = "source";

/// Directory names never descended into during discovery.
const SKIP_DIRS: &[&str] = &[".git", "docs", "target", "node_modules", "tools", "schemas"];

/// TOML files that are tool configuration rather than archive content. Any file whose name
/// begins with `.` is skipped as well.
const CONFIG_FILES: &[&str] = &["Cargo.toml", "Cargo.lock", "rustfmt.toml", "taplo.toml"];

/// Guard against a runaway `of` chain.
pub const MAX_CHAIN_DEPTH: usize = 32;

// ---------------------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// One finding, from loading or from validation.
///
/// Rendered as `<repo-relative-path>: <CODE>: <message>`, with the locator appended to the
/// path and a second location appended as ` (also <path>)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Repo-relative path, forward slashes.
    pub path: String,
    /// TOML-ish locator within the file, e.g. `[[page]]#3`.
    pub locator: Option<String>,
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    /// A second location, for findings that name two files.
    pub also: Option<String>,
}

impl Diagnostic {
    pub fn error(path: impl Into<String>, code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            path: path.into(),
            locator: None,
            code,
            severity: Severity::Error,
            message: message.into(),
            also: None,
        }
    }

    pub fn warning(
        path: impl Into<String>,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            ..Diagnostic::error(path, code, message)
        }
    }

    pub fn at(mut self, locator: impl Into<String>) -> Self {
        self.locator = Some(locator.into());
        self
    }

    pub fn also(mut self, other: impl Into<String>) -> Self {
        self.also = Some(other.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The sort key that fixes output order: path, then code, then locator.
    pub fn sort_key(&self) -> (&str, &str, &str) {
        (
            &self.path,
            self.code,
            self.locator.as_deref().unwrap_or_default(),
        )
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)?;
        if let Some(loc) = &self.locator {
            write!(f, " {loc}")?;
        }
        write!(f, ": {}: {}", self.code, self.message)?;
        if let Some(also) = &self.also {
            write!(f, " (also {also})")?;
        }
        Ok(())
    }
}

/// Sort findings into the documented deterministic order.
pub fn sort_diagnostics(diags: &mut [Diagnostic]) {
    diags.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
}

// ---------------------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------------------

/// A resolved value together with the node and file that actually declared it.
///
/// Provenance is not decoration. `scan.file` is resolved against the directory of the file
/// that *declared* it, not the file that inherited it — an issue three directories below its
/// copy must still find the copy's PDF. It also lets an error message name the file a bad
/// inherited value came from rather than the file that merely suffered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prov<T> {
    pub value: T,
    /// Id of the node that declared it.
    pub node: String,
    /// Absolute path of the file that declared it.
    pub file: PathBuf,
}

impl<T> Prov<T> {
    /// Directory the declaring file sits in — the base for relative path resolution.
    pub fn dir(&self) -> &Path {
        self.file.parent().unwrap_or(Path::new("."))
    }
}

impl<T> std::ops::Deref for Prov<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

fn prov<T>(node: &Node, value: T) -> Prov<T> {
    Prov {
        value,
        node: node.id.clone(),
        file: node.path.clone(),
    }
}

// ---------------------------------------------------------------------------------------
// Resolved inheritance
// ---------------------------------------------------------------------------------------

/// `[rights]` after key-by-key merge up the chain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedRights {
    pub work: Option<Prov<String>>,
    pub scan: Option<Prov<String>>,
    pub attribution: Option<Prov<String>>,
    pub note: Option<Prov<String>>,
}

/// `[holding]` after key-by-key merge up the chain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedHolding {
    pub repository: Option<Prov<String>>,
    pub shelfmark: Option<Prov<String>>,
    pub collection: Option<Prov<String>>,
    pub note: Option<Prov<String>>,
}

/// Everything a node inherits, already merged.
///
/// The allowlist is exactly these entries and nothing else. Unknown and extension fields
/// never inherit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    pub language: Option<Prov<String>>,
    pub place: Option<Prov<String>>,
    pub rights: ResolvedRights,
    pub holding: ResolvedHolding,
    /// Open identifier table, merged key by key.
    pub identifier: BTreeMap<String, Prov<String>>,
    /// Wholesale from the first node in the chain that declares `resp` at all.
    /// `Some(prov)` whose value is empty means a node wrote `resp = []` to clear.
    pub resp: Option<Prov<Vec<Resp>>>,

    // `scan` is NOT merged as a unit: `file`, `by` and `url` inherit; `count` and `note` are
    // taken from self only. A document that inherits `scan.file` from its copy must not also
    // inherit `count`, because the count describes the container as the copy knows it.
    pub scan_file: Option<Prov<String>>,
    pub scan_by: Option<Prov<String>>,
    pub scan_url: Option<Prov<String>>,
    pub scan_ppi: Option<Prov<i64>>,
    /// Self only. Never inherited.
    pub scan_count: Option<i64>,
    /// Self only. Never inherited.
    pub scan_note: Option<String>,
}

impl Resolved {
    /// The resolved `scan.file`, made absolute against the directory of the file that
    /// declared it.
    pub fn scan_file_path(&self) -> Option<PathBuf> {
        let p = self.scan_file.as_ref()?;
        Some(normalise(&p.dir().join(&p.value)))
    }

    pub fn resp(&self) -> &[Resp] {
        self.resp
            .as_ref()
            .map(|p| p.value.as_slice())
            .unwrap_or(&[])
    }
}

// ---------------------------------------------------------------------------------------
// Expanded pages
// ---------------------------------------------------------------------------------------

/// One image, with its path already resolved to an absolute location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGraphic {
    /// Absolute path, resolved against the declaring file's directory.
    pub file: PathBuf,
    /// The path exactly as written, for messages.
    pub file_raw: String,
    /// 1-based page index inside `file`. `None` for a standalone image.
    pub page: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub url: Option<String>,
    pub mimetype: Option<String>,
    /// True when this graphic was generated from a `pages` range rather than written out as
    /// a `[[page.graphic]]`.
    pub synthesised: bool,
}

/// One page after expansion. `n` is always concrete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPage {
    pub n: i64,
    pub owner: NodeId,
    pub owner_id: String,
    pub title: Option<String>,
    pub label: Option<String>,
    pub url: Option<String>,
    pub note: Option<String>,
    pub graphics: Vec<ResolvedGraphic>,
    pub texts: Vec<Text>,
    pub zones: Vec<Zone>,
    /// 0-based position in the declared `[[page]]` array, for messages. For a page generated
    /// from a `pages` range this is its offset within the range.
    pub source_index: usize,
}

impl ResolvedPage {
    /// The graphic a `.pN` reference denotes, and whose pixel space zones are in: the first
    /// in declaration order.
    pub fn primary_graphic(&self) -> Option<&ResolvedGraphic> {
        self.graphics.first()
    }

    /// The citable address of this page.
    pub fn address(&self) -> String {
        format!("{}.p{}", self.owner_id, self.n)
    }
}

// ---------------------------------------------------------------------------------------
// Nodes and the archive
// ---------------------------------------------------------------------------------------

/// Index into [`Archive::nodes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

/// One loaded file.
#[derive(Debug, Clone)]
pub struct Node {
    pub index: NodeId,
    /// Effective id: the declared `id`, or the filename stem.
    pub id: String,
    /// True when `id` was written in the file rather than derived from the filename.
    pub id_declared: bool,
    /// Absolute path to the TOML file.
    pub path: PathBuf,
    /// Repo-relative path with forward slashes. This is what findings print.
    pub rel_path: String,
    pub record: Record,
    /// `[self, parent, grandparent, …]`, cycle-truncated and depth-capped.
    pub chain: Vec<NodeId>,
    pub resolved: Resolved,
    pub pages: Vec<ResolvedPage>,
}

impl Node {
    pub fn layer(&self) -> Layer {
        self.record.layer()
    }

    /// Directory the file sits in.
    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }
}

/// Every file in the archive, loaded and resolved.
#[derive(Debug, Clone)]
pub struct Archive {
    /// Repository root the archive was loaded from.
    pub root: PathBuf,
    pub nodes: Vec<Node>,
    by_id: BTreeMap<String, NodeId>,
    /// Findings produced while loading. See the module docs for which codes these are.
    pub diagnostics: Vec<Diagnostic>,
}

impl Archive {
    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }

    pub fn by_id(&self, id: &str) -> Option<&Node> {
        self.by_id.get(id).map(|n| self.get(*n))
    }

    pub fn id_index(&self) -> &BTreeMap<String, NodeId> {
        &self.by_id
    }

    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter()
    }

    /// The first node in `id`'s chain whose layer is `copy`, if any.
    ///
    /// Checks 4, 5 and 6 consult this. When it is `None` those checks are skipped
    /// **silently** — Turgot and Verniquet have no copy layer by design, and warning on them
    /// would train people to ignore the validator.
    pub fn nearest_copy(&self, id: NodeId) -> Option<&Node> {
        self.get(id)
            .chain
            .iter()
            .map(|n| self.get(*n))
            .find(|n| n.layer() == Layer::Copy)
    }

    /// The first node in `id`'s chain whose layer is `source`, if any.
    pub fn nearest_source(&self, id: NodeId) -> Option<&Node> {
        self.get(id)
            .chain
            .iter()
            .map(|n| self.get(*n))
            .find(|n| n.layer() == Layer::Source)
    }

    /// The node named by `of`, if it resolves.
    pub fn parent(&self, id: NodeId) -> Option<&Node> {
        let of = self.get(id).record.of()?;
        self.by_id(of)
    }

    /// Resolve a reference, `<id>` or `<id>.p<n>`.
    pub fn resolve_reference(&self, reference: &str) -> Result<RefTarget<'_>, RefError> {
        match parse_reference(reference)? {
            Reference::Document(id) => {
                let node = self.by_id(&id).ok_or_else(|| RefError {
                    code: "E111",
                    message: format!("unknown id {id:?}"),
                })?;
                Ok(RefTarget::Document(node))
            }
            Reference::Page(id, n) => {
                let node = self.by_id(&id).ok_or_else(|| RefError {
                    code: "E111",
                    message: format!("unknown id {id:?}"),
                })?;
                // Resolution never walks the `of` chain looking for a page: pages are never
                // inherited, so the owner's own expansion is the whole search space.
                let page = node
                    .pages
                    .iter()
                    .find(|p| p.n == n)
                    .ok_or_else(|| RefError {
                        code: "E112",
                        message: format!("{id:?} has no page n = {n}"),
                    })?;
                Ok(RefTarget::Page {
                    node,
                    page,
                    graphic: page.primary_graphic(),
                })
            }
        }
    }
}

/// What a reference points at.
#[derive(Debug, Clone, Copy)]
pub enum RefTarget<'a> {
    /// A bare id. A valid address that resolves to the record itself.
    Document(&'a Node),
    Page {
        node: &'a Node,
        page: &'a ResolvedPage,
        /// The page's primary graphic, if it has one.
        graphic: Option<&'a ResolvedGraphic>,
    },
}

// ---------------------------------------------------------------------------------------
// Id grammar
// ---------------------------------------------------------------------------------------

/// The spec's hard rule: no `/`, no `:`, no `.`, no whitespace, no control characters, and
/// no leading or trailing hyphen. Violating this is `E104`.
pub fn id_is_valid(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id
            .chars()
            .any(|c| matches!(c, '/' | ':' | '.') || c.is_whitespace() || c.is_control())
}

/// House style: lowercase alphanumerics separated by single hyphens. Failing this while
/// satisfying [`id_is_valid`] is `W105`, a warning — underscores survive on `.jp2` filenames
/// and someone will eventually paste one in.
pub fn id_is_preferred(id: &str) -> bool {
    // Underscore is a group separator as well as hyphen, because an Internet Archive
    // identifier uses it: `procesverbal00_1_0` and `case_oversize_frc_27598` are the handles
    // archive.org publishes and everyone else cites. Ruling them out of house style would
    // mean 1,263 warnings that say nothing a reader can act on, which is how a warning
    // channel stops being read.
    !id.is_empty()
        && id.split(['-', '_']).all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

// ---------------------------------------------------------------------------------------
// Reference grammar
// ---------------------------------------------------------------------------------------

/// A parsed reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    Document(String),
    Page(String, i64),
}

/// Why a reference did not parse or did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for RefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RefError {}

fn ref_err(message: impl Into<String>) -> RefError {
    RefError {
        code: "E110",
        message: message.into(),
    }
}

/// Parse `<id>` or `<id>.p<n>`.
///
/// Because ids may not contain `.`, a reference containing a `.` must be a page reference;
/// the split is on the last `.` so that malformed input like `x.p1.g0` has fixed behaviour
/// (rejected, rather than silently read as something else). `<id>.p<n>.g<k>` is reserved for
/// explicit graphic selection and is deliberately not implemented.
pub fn parse_reference(reference: &str) -> Result<Reference, RefError> {
    if reference.is_empty() {
        return Err(ref_err("empty reference"));
    }
    if reference.chars().any(char::is_whitespace) {
        return Err(ref_err(format!(
            "{reference:?}: whitespace is not permitted in a reference"
        )));
    }

    let Some((id, suffix)) = reference.rsplit_once('.') else {
        return Ok(Reference::Document(reference.to_string()));
    };

    if id.is_empty() {
        return Err(ref_err(format!("{reference:?}: empty id")));
    }
    let Some(num) = suffix.strip_prefix('p') else {
        return Err(ref_err(format!(
            "{reference:?}: expected '.p<n>' after the id (note: '.P' is not '.p', and \
             '.p<n>.g<k>' graphic selection is not implemented)"
        )));
    };

    let (negative, digits) = match num.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, num),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(ref_err(format!(
            "{reference:?}: page number must be an integer"
        )));
    }
    // Reject `p01` so `x.p01` and `x.p1` can never be two spellings of one address, and `p-0`
    // so zero has exactly one spelling.
    if digits.len() > 1 && digits.starts_with('0') {
        return Err(ref_err(format!(
            "{reference:?}: page number must not have leading zeros"
        )));
    }
    if negative && digits == "0" {
        return Err(ref_err(format!("{reference:?}: write page zero as 'p0'")));
    }

    let value: i64 = digits
        .parse::<i64>()
        .map_err(|_| ref_err(format!("{reference:?}: page number out of range")))?;

    Ok(Reference::Page(
        id.to_string(),
        if negative { -value } else { value },
    ))
}

// ---------------------------------------------------------------------------------------
// Path handling
// ---------------------------------------------------------------------------------------

/// Lexically normalise a path: resolve `.` and `..` without touching the filesystem.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Repo-relative path with forward slashes, for findings.
fn rel_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Resolve a `file` value against the directory of the file that declared it.
///
/// Returns the finding code and message on rejection.
fn resolve_file_path(
    root: &Path,
    declaring_dir: &Path,
    raw: &str,
) -> Result<PathBuf, (&'static str, String)> {
    if raw.contains('\\') {
        return Err((
            "E012",
            format!("path {raw:?} must use forward slashes on all platforms"),
        ));
    }
    if raw.is_empty() {
        return Err(("E012", "path must not be empty".to_string()));
    }
    if Path::new(raw).is_absolute() || raw.starts_with('/') {
        return Err((
            "E016",
            format!("path {raw:?} must be relative and must not escape the repository root"),
        ));
    }
    let joined = normalise(&declaring_dir.join(raw));
    if !joined.starts_with(root) {
        return Err((
            "E016",
            format!("path {raw:?} must be relative and must not escape the repository root"),
        ));
    }
    Ok(joined)
}

// ---------------------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------------------

/// Load every record under `root` and resolve it.
///
/// Discovery prefers `<root>/source/` when it exists, and otherwise walks `<root>` itself
/// skipping `.git`, `docs`, `target`, `tools`, `schemas` and `node_modules`. The fallback is
/// what makes the tool usable before the migration has moved anything.
///
/// Per-file problems are [`Diagnostic`]s on the returned [`Archive`], not `Err`. `Err` is
/// reserved for a root that cannot be walked at all.
pub fn load_archive(root: impl AsRef<Path>) -> anyhow::Result<Archive> {
    let root = root.as_ref();
    let root = std::fs::canonicalize(root)
        .with_context(|| format!("archive root {} does not exist", root.display()))?;

    let mut diagnostics = Vec::new();
    let files = discover(&root)?;

    // -- pass 1: parse ------------------------------------------------------------------
    let mut parsed: Vec<(PathBuf, String, Record)> = Vec::new();
    for path in files {
        let rel = rel_display(&root, &path);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                diagnostics.push(Diagnostic::error(
                    rel,
                    "E010",
                    format!("cannot read file: {e}"),
                ));
                continue;
            }
        };
        match toml::from_str::<Record>(&text) {
            Ok(record) => parsed.push((path, rel, record)),
            Err(e) => {
                let (code, message) = classify_toml_error(&e);
                diagnostics.push(Diagnostic::error(rel, code, message));
            }
        }
    }

    // -- pass 2: ids and the index --------------------------------------------------------
    let mut nodes: Vec<Node> = Vec::with_capacity(parsed.len());
    let mut by_id: BTreeMap<String, NodeId> = BTreeMap::new();

    for (index, (path, rel_path, record)) in parsed.into_iter().enumerate() {
        let declared = record.declared_id().map(str::to_string);
        let id_declared = declared.is_some();
        let id = declared.unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });

        if !id_is_valid(&id) {
            diagnostics.push(Diagnostic::error(
                rel_path.clone(),
                "E104",
                format!(
                    "id {id:?} contains a reserved character (one of / : .), whitespace, or a \
                     leading/trailing hyphen"
                ),
            ));
        }

        nodes.push(Node {
            index: NodeId(index),
            id,
            id_declared,
            path,
            rel_path,
            record,
            chain: Vec::new(),
            resolved: Resolved::default(),
            pages: Vec::new(),
        });
    }

    for node in &nodes {
        match by_id.get(&node.id) {
            None => {
                by_id.insert(node.id.clone(), node.index);
            }
            Some(first) => {
                let other = nodes[first.0].rel_path.clone();
                diagnostics.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E101",
                        format!("duplicate id {:?}", node.id),
                    )
                    .also(other),
                );
            }
        }
    }

    // -- pass 3: chains -------------------------------------------------------------------
    let mut chains: Vec<Vec<NodeId>> = Vec::with_capacity(nodes.len());
    for node in &nodes {
        chains.push(build_chain(&nodes, &by_id, node.index, &mut diagnostics));
    }
    for (node, chain) in nodes.iter_mut().zip(chains) {
        node.chain = chain;
    }

    // -- pass 4: inheritance ---------------------------------------------------------------
    let resolved: Vec<Resolved> = (0..nodes.len())
        .map(|i| resolve_inheritance(&nodes, NodeId(i)))
        .collect();
    for (node, r) in nodes.iter_mut().zip(resolved) {
        node.resolved = r;
    }

    // -- pass 5: page expansion -------------------------------------------------------------
    let expanded: Vec<Vec<ResolvedPage>> = (0..nodes.len())
        .map(|i| expand_pages(&root, &nodes[i], &mut diagnostics))
        .collect();
    for (node, pages) in nodes.iter_mut().zip(expanded) {
        node.pages = pages;
    }

    sort_diagnostics(&mut diagnostics);

    Ok(Archive {
        root,
        nodes,
        by_id,
        diagnostics,
    })
}

/// Recursively find `*.toml`, preferring `<root>/source/`.
fn discover(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let base = {
        let candidate = root.join(SOURCE_ROOT);
        if candidate.is_dir() {
            candidate
        } else {
            root.to_path_buf()
        }
    };

    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(&base)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            if !e.file_type().is_dir() {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            !SKIP_DIRS.contains(&name.as_ref())
        });

    for entry in walker {
        let entry = entry.with_context(|| format!("walking {}", base.display()))?;
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "toml") {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        // Tool configuration is not archive content. This matters mainly before the
        // migration, when discovery falls back to walking the repository root and would
        // otherwise try to read `.taplo.toml` and `Cargo.toml` as records.
        if name.starts_with('.') || CONFIG_FILES.contains(&name.as_ref()) {
            continue;
        }
        // Note there is no guard here against the OCR sidecars: they are `.ocr.md`, and this
        // walk only considers `.toml`, so they are never candidates for being read as
        // records. They are reached through the `[[text]]` that points at them.
        out.push(entry.into_path());
    }
    out.sort();
    Ok(out)
}

/// Map a serde/TOML failure onto the finding code that describes it.
///
/// `deny_unknown_fields` gives typo detection for free, and the internally tagged enum turns
/// `layer = "page"` into "unknown variant `page`" — which is exactly finding `E015`.
fn classify_toml_error(e: &toml::de::Error) -> (&'static str, String) {
    let message = tidy_toml_error(&e.to_string());

    if message.contains("unknown variant") {
        if message.contains("`page`") {
            return (
                "E015",
                "layer = \"page\" is not allowed on a file; pages are declared inline as \
                 [[page]]"
                    .to_string(),
            );
        }
        return (
            "E014",
            format!("layer must be one of source, copy, document; {message}"),
        );
    }
    if message.contains("missing field") {
        return ("E011", message);
    }
    if message.contains("unknown field") {
        return ("E013", message);
    }
    if message.contains("invalid type") || message.contains("out of range") {
        return ("E012", message);
    }
    ("E010", format!("not valid TOML: {message}"))
}

/// Reduce a multi-line TOML error to one useful line.
///
/// The crate renders errors as a location line, a source snippet with carets, and then the
/// reason. Flattening the whole thing produces `... | 1 | kind = "volume" | ^ missing field`,
/// which buries the reason. Keep the reason and the location, drop the snippet.
fn tidy_toml_error(raw: &str) -> String {
    let lines: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let location = lines
        .first()
        .filter(|l| l.starts_with("TOML parse error at "))
        .map(|l| l.trim_start_matches("TOML parse error at ").to_string());
    let reason = lines
        .iter()
        .rev()
        .find(|l| {
            !l.starts_with("TOML parse error at ") && !l.starts_with('|') && !l.contains(" | ")
        })
        .copied()
        .unwrap_or(raw);

    match location {
        Some(where_) => format!("{reason} (at {where_})"),
        None => reason.to_string(),
    }
}

/// Build `[self, parent, …]`, stopping at a root, a broken pointer, a cycle, or the depth cap.
fn build_chain(
    nodes: &[Node],
    by_id: &BTreeMap<String, NodeId>,
    start: NodeId,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<NodeId> {
    let mut chain = vec![start];
    let mut seen: BTreeSet<NodeId> = BTreeSet::from([start]);
    let mut cursor = start;

    loop {
        let node = &nodes[cursor.0];
        let Some(of) = node.record.of() else { break };

        let Some(&next) = by_id.get(of) else {
            // Reported only on the file that declares the bad pointer, so one mistake does
            // not produce a finding on every descendant.
            if cursor == start {
                diagnostics.push(Diagnostic::error(
                    node.rel_path.clone(),
                    "E102",
                    format!("of = {of:?} does not resolve to any known id"),
                ));
            }
            break;
        };

        if seen.contains(&next) {
            if cursor == start || next == start {
                let mut names: Vec<&str> = chain.iter().map(|n| nodes[n.0].id.as_str()).collect();
                names.push(nodes[next.0].id.as_str());
                diagnostics.push(Diagnostic::error(
                    nodes[start.0].rel_path.clone(),
                    "E103",
                    format!("cycle in 'of' chain: {}", names.join(" -> ")),
                ));
            }
            // Truncate at the repeat; inheritance proceeds over the acyclic prefix.
            break;
        }

        if chain.len() >= MAX_CHAIN_DEPTH {
            diagnostics.push(Diagnostic::error(
                nodes[start.0].rel_path.clone(),
                "E108",
                format!(
                    "'of' chain deeper than {MAX_CHAIN_DEPTH} from {:?}; refusing to resolve",
                    nodes[start.0].id
                ),
            ));
            break;
        }

        chain.push(next);
        seen.insert(next);
        cursor = next;
    }

    chain
}

/// First-found scanning from self toward the root.
///
/// Declaring a key with any value — including an empty string — stops the search. Absence in
/// a node means "not declared here", never "cleared".
fn resolve_inheritance(nodes: &[Node], id: NodeId) -> Resolved {
    let mut out = Resolved::default();
    let chain = &nodes[id.0].chain;

    // `scan.count` and `scan.note` are self-only.
    if let Some(scan) = nodes[id.0].record.scan() {
        out.scan_count = scan.count;
        out.scan_note = scan.note.clone();
    }

    for step in chain {
        let node = &nodes[step.0];
        let rec = &node.record;

        take(&mut out.language, node, rec.language());
        take(&mut out.place, node, rec.place());

        if let Some(rights) = rec.rights() {
            take(&mut out.rights.work, node, rights.work.as_deref());
            take(&mut out.rights.scan, node, rights.scan.as_deref());
            take(
                &mut out.rights.attribution,
                node,
                rights.attribution.as_deref(),
            );
            take(&mut out.rights.note, node, rights.note.as_deref());
        }

        if let Some(holding) = rec.holding() {
            take(
                &mut out.holding.repository,
                node,
                holding.repository.as_deref(),
            );
            take(
                &mut out.holding.shelfmark,
                node,
                holding.shelfmark.as_deref(),
            );
            take(
                &mut out.holding.collection,
                node,
                holding.collection.as_deref(),
            );
            take(&mut out.holding.note, node, holding.note.as_deref());
        }

        if let Some(identifier) = rec.identifier() {
            for (key, value) in identifier {
                out.identifier
                    .entry(key.clone())
                    .or_insert_with(|| prov(node, value.clone()));
            }
        }

        // Arrays replace wholesale. A node that declares `resp = []` declares it, so the
        // search stops there and the resolved value is empty — that is how you clear.
        if let (None, Some(resp)) = (&out.resp, rec.resp()) {
            out.resp = Some(prov(node, resp.to_vec()));
        }

        if let Some(scan) = rec.scan() {
            take(&mut out.scan_file, node, scan.file.as_deref());
            take(&mut out.scan_by, node, scan.by.as_deref());
            take(&mut out.scan_url, node, scan.url.as_deref());
            if out.scan_ppi.is_none()
                && let Some(ppi) = scan.ppi
            {
                out.scan_ppi = Some(prov(node, ppi));
            }
        }
    }

    out
}

/// Fill a slot only if it is still empty — this is what makes the scan first-found.
fn take(slot: &mut Option<Prov<String>>, node: &Node, value: Option<&str>) {
    if let (None, Some(v)) = (&slot, value) {
        *slot = Some(prov(node, v.to_string()));
    }
}

// ---------------------------------------------------------------------------------------
// Page expansion
// ---------------------------------------------------------------------------------------

/// Turn `pages` and/or `[[page]]` into an ordered list of concrete pages.
fn expand_pages(root: &Path, node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Vec<ResolvedPage> {
    let range = node.record.pages();
    let entries = node.record.page();

    let mut pages = match (range, entries.is_empty()) {
        // Shape A: neither. Sources that only hold metadata are normal.
        (None, true) => Vec::new(),
        // Shape B: range only.
        (Some(range), true) => expand_range_only(root, node, range, diagnostics),
        // Shape C: explicit pages only.
        (None, false) => expand_explicit(root, node, None, diagnostics),
        // Shape D: both.
        (Some(range), false) => {
            let expected = range.len();
            if !range_is_sane(node, range, diagnostics) {
                Vec::new()
            } else if expected != entries.len() as i64 {
                diagnostics.push(Diagnostic::error(
                    node.rel_path.clone(),
                    "E903",
                    format!(
                        "pages range covers {expected} page(s) but {} [[page]] entries are \
                         declared",
                        entries.len()
                    ),
                ));
                Vec::new()
            } else {
                expand_explicit(root, node, Some(range), diagnostics)
            }
        }
    };

    check_unique_n(node, &mut pages, diagnostics);
    pages
}

/// Shared preconditions on a `pages` table.
fn range_is_sane(
    node: &Node,
    range: crate::model::PageRange,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let mut ok = true;
    if range.from < 1 {
        diagnostics.push(Diagnostic::error(
            node.rel_path.clone(),
            "E902",
            format!("pages.from = {} must be at least 1", range.from),
        ));
        ok = false;
    }
    if range.to < range.from {
        diagnostics.push(Diagnostic::error(
            node.rel_path.clone(),
            "E901",
            format!(
                "pages.to = {} is less than pages.from = {}",
                range.to, range.from
            ),
        ));
        ok = false;
    }
    // Expansion materialises one ResolvedPage per page in the range, so an unbounded
    // range is an out-of-memory crash rather than a diagnostic. A typo of a few extra
    // digits is enough to reach it. The cap is far above any physical object - the
    // largest volume in the archive is 1346 pages - and far below anything that
    // threatens the allocator.
    if ok && range.len() > MAX_RANGE_LEN {
        diagnostics.push(Diagnostic::error(
            node.rel_path.clone(),
            "E905",
            format!(
                "pages = {{ from = {}, to = {} }} spans {} pages, over the {MAX_RANGE_LEN} \
                 maximum; this is a typo, not a book",
                range.from,
                range.to,
                range.len()
            ),
        ));
        ok = false;
    }
    ok
}

/// Upper bound on a `pages = { from, to }` span. See [`range_is_sane`].
const MAX_RANGE_LEN: i64 = 100_000;

/// Shape B. `n` starts at 1, unconditionally.
fn expand_range_only(
    root: &Path,
    node: &Node,
    range: crate::model::PageRange,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ResolvedPage> {
    if !range_is_sane(node, range, diagnostics) {
        return Vec::new();
    }

    let Some(scan) = scan_graphic_base(root, node, diagnostics) else {
        diagnostics.push(Diagnostic::error(
            node.rel_path.clone(),
            "E904",
            "pages = { from, to } needs a scan.file to index into, and none is declared or \
             inherited",
        ));
        return Vec::new();
    };

    (0..range.len())
        .map(|i| {
            let graphic = ResolvedGraphic {
                file: scan.0.clone(),
                file_raw: scan.1.clone(),
                page: Some(range.from + i),
                width: None,
                height: None,
                url: None,
                mimetype: None,
                synthesised: true,
            };
            ResolvedPage {
                n: 1 + i,
                owner: node.index,
                owner_id: node.id.clone(),
                title: None,
                label: None,
                url: None,
                note: None,
                graphics: vec![graphic],
                texts: Vec::new(),
                zones: Vec::new(),
                source_index: i as usize,
            }
        })
        .collect()
}

/// The parts of an expansion that do not change from page to page.
struct Expansion<'a> {
    root: &'a Path,
    node: &'a Node,
    /// The resolved `scan.file` as `(absolute, as-written)`, if there is one.
    scan: Option<(PathBuf, String)>,
}

/// Shapes C and D. `range` is `Some` only in shape D, where it supplies graphic page numbers.
fn expand_explicit(
    root: &Path,
    node: &Node,
    range: Option<crate::model::PageRange>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ResolvedPage> {
    let ctx = Expansion {
        root,
        node,
        scan: scan_graphic_base(root, node, diagnostics),
    };
    let mut out = Vec::new();
    let mut previous: Option<i64> = None;

    for (index, entry) in node.record.page().iter().enumerate() {
        // An explicit `n` re-bases the count for everything after it.
        let n = match entry.n {
            Some(n) => n,
            None => previous.map_or(1, |p| p + 1),
        };
        previous = Some(n);

        let expected_page = range.map(|r| r.from + index as i64);
        let graphics = resolve_graphics(&ctx, entry, index, n, expected_page, diagnostics);

        if graphics.is_empty() {
            diagnostics.push(
                Diagnostic::warning(
                    node.rel_path.clone(),
                    "W904",
                    format!("page n = {n} declares no graphic"),
                )
                .at(format!("[[page]]#{index}")),
            );
        }

        out.push(ResolvedPage {
            n,
            owner: node.index,
            owner_id: node.id.clone(),
            title: entry.title.clone(),
            label: entry.label.clone(),
            url: entry.url.clone(),
            note: entry.note.clone(),
            graphics,
            texts: entry.text.clone(),
            zones: entry.zone.clone(),
            source_index: index,
        });
    }

    out
}

/// Build the graphics for one explicit page entry.
fn resolve_graphics(
    ctx: &Expansion<'_>,
    entry: &Page,
    index: usize,
    n: i64,
    expected_page: Option<i64>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ResolvedGraphic> {
    let node = ctx.node;
    let locator = format!("[[page]]#{index}");

    // A page that declares no graphics in shape D gets one synthesised from the range.
    if entry.graphic.is_empty() {
        let Some(expected) = expected_page else {
            return Vec::new();
        };
        let Some((file, file_raw)) = ctx.scan.clone() else {
            diagnostics.push(
                Diagnostic::error(
                    node.rel_path.clone(),
                    "E904",
                    format!(
                        "page n = {n} declares a graphic with no 'file' and no scan.file is \
                         inherited"
                    ),
                )
                .at(locator),
            );
            return Vec::new();
        };
        return vec![ResolvedGraphic {
            file,
            file_raw,
            page: Some(expected),
            width: None,
            height: None,
            url: None,
            mimetype: None,
            synthesised: true,
        }];
    }

    let mut out = Vec::new();
    for (gi, graphic) in entry.graphic.iter().enumerate() {
        let Some(resolved) = resolve_one_graphic(ctx, graphic, n, &locator, gi, diagnostics) else {
            continue;
        };
        out.push(resolved);
    }

    // Only the primary graphic is tied to the range; alternates are never touched by it.
    if let (Some(expected), Some(primary)) = (expected_page, out.first_mut()) {
        match primary.page {
            None => primary.page = Some(expected),
            Some(declared) if declared != expected => {
                let range = node.record.pages().expect("shape D has a range");
                diagnostics.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E906",
                        format!(
                            "page n = {n} declares graphic.page = {declared} but its position \
                             in pages = {{ from = {}, to = {} }} implies {expected}",
                            range.from, range.to
                        ),
                    )
                    .at(locator),
                );
            }
            Some(_) => {}
        }
    }

    out
}

fn resolve_one_graphic(
    ctx: &Expansion<'_>,
    graphic: &Graphic,
    n: i64,
    locator: &str,
    graphic_index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ResolvedGraphic> {
    let node = ctx.node;
    let (file, file_raw, synthesised) = match &graphic.file {
        Some(raw) => match resolve_file_path(ctx.root, node.dir(), raw) {
            Ok(path) => (path, raw.clone(), false),
            Err((code, message)) => {
                diagnostics.push(
                    Diagnostic::error(node.rel_path.clone(), code, message)
                        .at(format!("{locator} graphic#{graphic_index}")),
                );
                return None;
            }
        },
        // A graphic with no `file` falls back to the resolved scan.file.
        None => match ctx.scan.clone() {
            Some((path, raw)) => (path, raw, true),
            None => {
                diagnostics.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E904",
                        format!(
                            "page n = {n} declares a graphic with no 'file' and no scan.file \
                             is inherited"
                        ),
                    )
                    .at(format!("{locator} graphic#{graphic_index}")),
                );
                return None;
            }
        },
    };

    for (label, value) in [
        ("graphic.page", graphic.page),
        ("graphic.width", graphic.width),
        ("graphic.height", graphic.height),
    ] {
        if value.is_some_and(|v| v < 1) {
            diagnostics.push(
                Diagnostic::error(
                    node.rel_path.clone(),
                    "E012",
                    format!(
                        "field '{label}' must be at least 1, got {}",
                        value.expect("checked")
                    ),
                )
                .at(format!("{locator} graphic#{graphic_index}")),
            );
        }
    }

    Some(ResolvedGraphic {
        file,
        file_raw,
        page: graphic.page,
        width: graphic.width,
        height: graphic.height,
        url: graphic.url.clone(),
        mimetype: graphic.mimetype.clone(),
        synthesised,
    })
}

/// The resolved `scan.file` as `(absolute, as-written)`, if there is one.
fn scan_graphic_base(
    root: &Path,
    node: &Node,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(PathBuf, String)> {
    let declared = node.resolved.scan_file.as_ref()?;
    match resolve_file_path(root, declared.dir(), &declared.value) {
        Ok(path) => Some((path, declared.value.clone())),
        Err((code, message)) => {
            diagnostics.push(Diagnostic::error(
                node.rel_path.clone(),
                code,
                format!("scan.file (declared in {:?}): {message}", declared.node),
            ));
            None
        }
    }
}

/// Compares resolved `n`, so a collision between an explicit and a counted value is caught.
fn check_unique_n(node: &Node, pages: &mut [ResolvedPage], diagnostics: &mut Vec<Diagnostic>) {
    let mut first_seen: BTreeMap<i64, usize> = BTreeMap::new();
    for page in pages.iter() {
        match first_seen.get(&page.n) {
            None => {
                first_seen.insert(page.n, page.source_index);
            }
            Some(first) => {
                diagnostics.push(
                    Diagnostic::error(
                        node.rel_path.clone(),
                        "E301",
                        format!(
                            "duplicate page n = {} within {:?}; declared at [[page]]#{first} \
                             and [[page]]#{}",
                            page.n, node.id, page.source_index
                        ),
                    )
                    .at(format!("[[page]]#{}", page.source_index)),
                );
            }
        }
    }
}
