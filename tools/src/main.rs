//! `scans` — the archive tool's command line.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use scans::load::{RefTarget, load_archive};
use scans::model;
use scans::render::Grid;
use scans::{ingest, migrate, render, validate};

use scans::SCHEMA_PATH;

#[derive(Debug, Parser)]
#[command(
    name = "scans",
    about = "Validate, migrate and describe the primary source and scan archive.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the checks over the archive.
    Validate(ValidateArgs),
    /// Convert the legacy layout to the current schema.
    Migrate(MigrateArgs),
    /// Say what an address points at.
    Resolve(ResolveArgs),
    /// Generate schemas/source.json from the Rust types.
    Schema(SchemaArgs),
    /// Recover the issue documents inside a bound volume from its PDF text layer.
    Ingest(IngestArgs),
    /// Rasterise cited pages to PNG.
    Render(RenderArgs),
    /// Write out the images stored on cited pages, without rasterising the page.
    Extract(ExtractArgs),
    /// Convert, check and export the word-level OCR sidecars.
    Ocr(OcrArgs),
    /// Ingest the Newberry French Revolution Collection.
    Frc(FrcArgs),
}

#[derive(Debug, Args)]
struct FrcArgs {
    #[command(subcommand)]
    command: FrcCommand,
}

#[derive(Debug, Subcommand)]
enum FrcCommand {
    /// Write records and OCR sidecars from a local frc-data checkout.
    ///
    /// Touches no network: the item list, the catalogue metadata and the word-level OCR are
    /// all in frc-data. Only the PDFs have to be fetched, which is a separate step.
    Ingest {
        /// Path to a frc-data checkout — the directory holding `Metadata/` and `XML_for_OCR/`.
        #[arg(long)]
        frc_data: PathBuf,
        /// Repository root. Defaults to the nearest ancestor containing .git.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Stop after this many items. For trying the run out before committing to 38,377.
        #[arg(long)]
        limit: Option<usize>,
        /// Only these identifiers.
        #[arg(long = "id")]
        ids: Vec<String>,
        /// Rewrite items that are already done.
        #[arg(long)]
        force: bool,
        /// Report what would be written without writing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Download the corpus PDFs from archive.org.
    ///
    /// One request at a time, paced, with an honest User-Agent. Every download is verified
    /// against the size and MD5 archive.org publishes for it. Resumable: an item with its PDF
    /// already on disk is skipped, so an interrupted run costs nothing to restart.
    Fetch {
        /// Repository root. Defaults to the nearest ancestor containing .git.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Minimum seconds between requests. Lower this only if you have reason to think
        /// archive.org wants you to.
        #[arg(long, default_value_t = 1.0)]
        interval: f64,
        /// Stop after this many items.
        #[arg(long)]
        limit: Option<usize>,
        /// Only these identifiers.
        #[arg(long = "id")]
        ids: Vec<String>,
        /// Re-fetch items that already have a PDF.
        #[arg(long)]
        force: bool,
        /// Ask archive.org what it would send, without downloading it.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
struct OcrArgs {
    #[command(subcommand)]
    command: OcrCommand,
}

#[derive(Debug, Subcommand)]
enum OcrCommand {
    /// Convert a DjVu XML file into an `.ocr.md`.
    Import {
        /// The `_djvu.xml` to read.
        xml: PathBuf,
        /// Id of the record this is the OCR of. Defaults to the filename with `_djvu.xml`
        /// trimmed off, which is how both derivations name their files.
        #[arg(long)]
        of: Option<String>,
        /// Directory to write the per-page files into. Defaults to the current directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Report the internal inconsistencies of one or more `.ocr.md` files.
    Check {
        /// Files to check, or directories of them.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

#[derive(Debug, Args)]
struct RenderArgs {
    /// Addresses to render: a record id such as `journal-de-paris-1789-01-03`, which renders
    /// every page of it, or a single page such as `journal-de-paris-1789-01-03.p1`.
    #[arg(required = true)]
    addresses: Vec<String>,
    /// Repository root. Defaults to the nearest ancestor containing .git.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Directory to write into. Defaults to <root>/.render, which is gitignored — these are
    /// derived files and do not belong in the archive.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Resolution. Omit for the native resolution of the page's own scan, which is the
    /// default because resampling a facsimile loses the only thing it has.
    #[arg(long)]
    dpi: Option<f32>,
    /// Split each page into a ROWSxCOLS grid of PNGs, e.g. `2x2`. A whole page shown to a
    /// reader that samples it down is unreadable type; a quarter of one is not.
    #[arg(long, default_value_t = Grid::WHOLE)]
    grid: Grid,
    /// Percentage added to each interior tile edge, so a line cut by a boundary survives
    /// whole in its neighbour. Ignored for a 1x1 grid.
    #[arg(long, default_value_t = 4.0)]
    overlap: f32,
    /// Overwrite outputs that already exist.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct ExtractArgs {
    /// Addresses whose stored images to write out.
    #[arg(required = true)]
    addresses: Vec<String>,
    /// Repository root. Defaults to the nearest ancestor containing .git.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Directory to write into. Defaults to <root>/.render.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Write the stored bytes verbatim instead of decoding them to PNG.
    #[arg(long)]
    raw: bool,
    /// Overwrite outputs that already exist.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct IngestArgs {
    /// Id of the copy to read, e.g. `journal-de-paris-1789-vol1`.
    copy_id: String,
    /// Repository root. Defaults to the nearest ancestor containing .git.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Write the records. Without this nothing is written, only reported.
    #[arg(long)]
    apply: bool,
    /// The regular span of one issue, used to recover headers the OCR could not read.
    #[arg(long, default_value_t = 4)]
    pages_per_issue: i64,
}

#[derive(Debug, Args)]
struct ResolveArgs {
    /// Addresses to resolve: a record id such as `turgot-1739`, or a page such as
    /// `turgot-1739.p0`.
    #[arg(required = true)]
    addresses: Vec<String>,
    /// Repository root. Defaults to the nearest ancestor containing .git.
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ValidateArgs {
    /// Restrict reporting to these paths. Loading is always whole-archive, because the id
    /// index and the sibling checks need every file.
    paths: Vec<PathBuf>,
    /// Repository root. Defaults to the nearest ancestor containing .git.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Enable the checks that must read image and container bytes.
    #[arg(long)]
    probe: bool,
    /// Treat warnings as errors for the purposes of the exit code.
    #[arg(long)]
    strict: bool,
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
}

#[derive(Debug, Args)]
struct MigrateArgs {
    /// Repository root. Defaults to the nearest ancestor containing .git.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Describe what would happen without touching the working tree. This is the default;
    /// pass --apply to actually make the changes.
    #[arg(long)]
    dry_run: bool,
    /// Carry the migration out.
    #[arg(long, conflicts_with = "dry_run")]
    apply: bool,
}

#[derive(Debug, Args)]
struct SchemaArgs {
    /// Repository root. Defaults to the nearest ancestor containing .git.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Write the schema here instead of <root>/schemas/source.json.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Do not write. Exit non-zero if the file on disk differs from what the types generate.
    /// This is what stops the schema drifting away from the validator.
    #[arg(long)]
    check: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Text,
    Json,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("scans: {e:#}");
            // 2 is reserved for usage and internal failure.
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Command::Validate(args) => cmd_validate(args),
        Command::Migrate(args) => cmd_migrate(args),
        Command::Resolve(args) => cmd_resolve(args),
        Command::Schema(args) => cmd_schema(args),
        Command::Ingest(args) => cmd_ingest(args),
        Command::Render(args) => cmd_render(args),
        Command::Extract(args) => cmd_extract(args),
        Command::Ocr(args) => cmd_ocr(args),
        Command::Frc(args) => cmd_frc(args),
    }
}

// ---------------------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------------------

fn cmd_validate(args: ValidateArgs) -> Result<ExitCode> {
    let root = resolve_root(args.root.as_deref())?;
    let archive = load_archive(&root)?;

    if args.probe && !cfg!(feature = "probe") {
        eprintln!(
            "scans: --probe was given but this binary was built without the 'probe' feature; \
             byte-reading checks are skipped. Rebuild with --features probe."
        );
    }

    let options = validate::Options {
        probe: args.probe,
        strict: args.strict,
        select: args.paths.clone(),
    };
    let report = validate::validate(&archive, &options);

    match args.format {
        Format::Text => {
            for finding in &report.findings {
                println!("{finding}");
            }
            eprintln!(
                "{} record(s) loaded, {} error(s), {} warning(s)",
                archive.nodes.len(),
                report.errors(),
                report.warnings()
            );
        }
        Format::Json => {
            let findings: Vec<_> = report
                .findings
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "path": f.path,
                        "locator": f.locator,
                        "code": f.code,
                        "severity": f.severity.as_str(),
                        "message": f.message,
                        "also": f.also,
                    })
                })
                .collect();
            let out = serde_json::json!({
                "files": archive.nodes.len(),
                "errors": report.errors(),
                "warnings": report.warnings(),
                "findings": findings,
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
    }

    Ok(ExitCode::from(report.exit_code(args.strict) as u8))
}

// ---------------------------------------------------------------------------------------
// migrate
// ---------------------------------------------------------------------------------------

fn cmd_migrate(args: MigrateArgs) -> Result<ExitCode> {
    let root = resolve_root(args.root.as_deref())?;
    // Dry run is the default: a migration that rewrites the archive on a bare `scans migrate`
    // is not a migration anyone should have to be brave about.
    let dry_run = !args.apply;
    let options = migrate::Options { dry_run };

    let plan = migrate::plan(&root, &options)?;

    for diagnostic in &plan.diagnostics {
        println!("{diagnostic}");
    }
    for action in &plan.actions {
        match action {
            migrate::Action::Move { from, to } => {
                println!("move   {} -> {}", show(&root, from), show(&root, to))
            }
            migrate::Action::Write { path, .. } => println!("write  {}", show(&root, path)),
            migrate::Action::Delete { path } => println!("delete {}", show(&root, path)),
        }
    }

    if dry_run {
        eprintln!(
            "{} action(s) planned; nothing written. Pass --apply to carry them out.",
            plan.actions.len()
        );
        return Ok(ExitCode::SUCCESS);
    }

    let applied = migrate::apply(&root, &plan, &options)?;
    eprintln!("{applied} action(s) applied.");
    Ok(ExitCode::SUCCESS)
}

fn show(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------------------
// resolve
// ---------------------------------------------------------------------------------------

/// Answer the one question the archive exists to answer: given a citation, which file, and
/// which page of it?
///
/// `.pN` addressing was chosen knowing it gives up citations you can check by eye. This is
/// how you check one instead — by hand, without reading the loader's source.
fn cmd_resolve(args: ResolveArgs) -> Result<ExitCode> {
    let root = resolve_root(args.root.as_deref())?;
    let archive = load_archive(&root)?;

    let mut failed = false;
    for address in &args.addresses {
        match archive.resolve_reference(address) {
            Err(e) => {
                failed = true;
                println!("{address}\n  unresolved  {e}");
            }
            Ok(RefTarget::Document(node)) => {
                println!("{address}");
                println!(
                    "  record   {}  (layer = {}, id = {})",
                    show(&root, &node.path),
                    node.record.layer().as_str(),
                    node.id
                );
            }
            Ok(RefTarget::Page {
                node,
                page,
                graphic,
            }) => {
                println!("{address}");
                println!(
                    "  record   {}  (layer = {}, id = {})",
                    show(&root, &node.path),
                    node.record.layer().as_str(),
                    node.id
                );
                let title = page
                    .title
                    .as_deref()
                    .map_or(String::new(), |t| format!(", title = {t:?}"));
                println!("  page     n = {}{title}", page.n);
                match graphic {
                    None => println!("  graphic  none declared"),
                    Some(g) => {
                        // `page` is the index inside a multi-page container; a standalone
                        // image has none, and saying "page 1" of a JP2 would be a fiction.
                        let at = g
                            .page
                            .map_or_else(|| " (standalone image)".to_string(), |p| format!(" page {p}"));
                        let size = match (g.width, g.height) {
                            (Some(w), Some(h)) => format!("  {w}x{h}"),
                            _ => String::new(),
                        };
                        println!("  graphic  {}{at}{size}", show(&root, &g.file));
                        if !g.file.is_file() {
                            failed = true;
                            println!("  MISSING  that file is not on disk");
                        }
                    }
                }
            }
        }
    }

    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

// ---------------------------------------------------------------------------------------
// schema
// ---------------------------------------------------------------------------------------

fn cmd_schema(args: SchemaArgs) -> Result<ExitCode> {
    // Both schemas are generated from the same Rust types by the same code path, so neither
    // can drift into being hand-maintained. `--out` writes only the record schema, because it
    // names a single file.
    if args.out.is_none() {
        let root = resolve_root(args.root.as_deref())?;
        let ocr_out = root.join(scans::ocr::SCHEMA_PATH);
        let ocr_generated = model::ocr_json_schema_text();
        if args.check {
            let on_disk = std::fs::read_to_string(&ocr_out).unwrap_or_default();
            if on_disk.replace("\r\n", "\n") != ocr_generated.replace("\r\n", "\n") {
                eprintln!(
                    "scans: {} is out of date with the Rust types in tools/src/ocr.rs.",
                    show_cwd(&ocr_out)
                );
                return Ok(ExitCode::FAILURE);
            }
        } else {
            if let Some(parent) = ocr_out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&ocr_out, &ocr_generated)
                .with_context(|| format!("writing {}", ocr_out.display()))?;
            eprintln!("wrote {}", show_cwd(&ocr_out));
        }
    }

    let generated = model::json_schema_text();

    let out = match args.out {
        Some(p) => p,
        None => resolve_root(args.root.as_deref())?.join(SCHEMA_PATH),
    };

    if args.check {
        let on_disk = match std::fs::read_to_string(&out) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("scans: cannot read {}: {e}", out.display());
                eprintln!("scans: run `scans schema` to generate it.");
                return Ok(ExitCode::FAILURE);
            }
        };
        // Compare ignoring line-ending differences, so a checkout with CRLF does not read as
        // drift.
        if on_disk.replace("\r\n", "\n") == generated.replace("\r\n", "\n") {
            eprintln!("{} is up to date.", show_cwd(&out));
            return Ok(ExitCode::SUCCESS);
        }
        eprintln!(
            "scans: {} is out of date with the Rust types in tools/src/model.rs.",
            show_cwd(&out)
        );
        eprintln!("scans: run `scans schema` to regenerate it.");
        return Ok(ExitCode::FAILURE);
    }

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&out, &generated).with_context(|| format!("writing {}", out.display()))?;
    eprintln!("wrote {}", show_cwd(&out));
    Ok(ExitCode::SUCCESS)
}

/// A path for reading, relative to the working directory where that is possible.
///
/// The working directory is canonicalised before comparing, because the paths handed here are
/// canonical: on Windows an uncanonical cwd fails to strip and the whole `\\?\D:\...` spelling
/// is printed instead.
fn show_cwd(path: &Path) -> String {
    std::env::current_dir()
        .and_then(std::fs::canonicalize)
        .ok()
        .and_then(|cwd| path.strip_prefix(&cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------------------
// Root discovery
// ---------------------------------------------------------------------------------------

/// The repository root: the explicit `--root`, else the nearest ancestor of the working
/// directory containing `.git`, else the working directory.
fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return canonical(p);
    }
    let cwd = std::env::current_dir().context("cannot determine the working directory")?;
    let mut cursor = cwd.as_path();
    loop {
        if cursor.join(".git").exists() {
            return canonical(cursor);
        }
        match cursor.parent() {
            Some(parent) => cursor = parent,
            None => return canonical(&cwd),
        }
    }
}

/// The root has to be canonicalised even when it was discovered rather than given, because
/// `load_archive` canonicalises the paths it reports. A root in any other spelling fails to
/// strip off them, and every path the tool prints comes out as `\\?\D:\...` instead of the
/// repo-relative path the reader wants.
fn canonical(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).with_context(|| format!("root {} does not exist", path.display()))
}

// ---------------------------------------------------------------------------------------
// ingest
// ---------------------------------------------------------------------------------------

fn cmd_ingest(args: IngestArgs) -> Result<ExitCode> {
    let root = resolve_root(args.root.as_deref())?;
    let archive = load_archive(&root)?;

    if !cfg!(feature = "ingest") {
        eprintln!(
            "scans: this binary was built without the 'ingest' feature, so it cannot read a              PDF. Rebuild with --features ingest."
        );
        return Ok(ExitCode::from(2));
    }

    let options = ingest::Options {
        copy_id: args.copy_id.clone(),
        // Reporting is the default. Writing several hundred records is not something to do
        // by accident.
        apply: args.apply,
        pages_per_issue: args.pages_per_issue,
    };
    let report = ingest::ingest(&root, &archive, &options)?;

    for note in &report.notes {
        eprintln!("scans: {note}");
    }

    let certain = report.issues.len() - report.uncertain.len();
    let n_issues = report
        .issues
        .iter()
        .filter(|i| i.kind == ingest::Kind::Issue)
        .count();
    let n_suppl = report.issues.len() - n_issues;
    println!(
        "{} document(s): {n_issues} issue(s), {n_suppl} supplement(s) — \
         {certain} certain, {} needing a look",
        report.issues.len(),
        report.uncertain.len()
    );
    // The copy states how many it should hold. Saying so here turns a silent shortfall into
    // a number the operator has to look at before writing anything.
    // Name the gaps. A count alone ("2 unaccounted for") cannot be acted on; the numbers
    // point straight at the pages to look at.
    let have: std::collections::BTreeSet<i64> = report
        .issues
        .iter()
        .filter(|i| i.kind == ingest::Kind::Issue)
        .map(|i| i.no)
        .collect();
    if let (Some(lo), Some(hi)) = (have.iter().next(), have.iter().next_back()) {
        let missing: Vec<String> = (*lo..=*hi)
            .filter(|n| !have.contains(n))
            .map(|n| n.to_string())
            .collect();
        if !missing.is_empty() {
            println!("  missing issue number(s): {}", missing.join(", "));
        }
    }
    if let Some(want) = report.expected_issues
        && want != n_issues as i64
    {
        println!(
            "  NOTE: covers implies {want} issues, but {n_issues} were recovered —              {} header(s) unaccounted for",
            (want - n_issues as i64).abs()
        );
    }

    if !report.uncertain.is_empty() {
        println!("
worklist — check these against the scan:");
        for iss in &report.uncertain {
            println!(
                "  {} pdf {}-{}: {}",
                iss.date.edtf(),
                iss.from,
                iss.to,
                iss.inferred.as_deref().unwrap_or("signals disagreed")
            );
        }
    }

    if !args.apply {
        eprintln!(
            "
{} record(s) planned; nothing written. Pass --apply to write them.",
            report.written.len()
        );
    } else {
        eprintln!("
{} record(s) written.", report.written.len());
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------------------
// render and extract
// ---------------------------------------------------------------------------------------

fn cmd_render(args: RenderArgs) -> Result<ExitCode> {
    let root = resolve_root(args.root.as_deref())?;
    let archive = load_archive(&root)?;
    let options = render::RenderOptions {
        addresses: args.addresses,
        out: out_dir(&root, args.out),
        dpi: args.dpi,
        grid: args.grid,
        overlap: args.overlap,
        force: args.force,
    };
    let report = render::render(&archive, &options)?;
    report_pixels(&root, &report);
    Ok(ExitCode::SUCCESS)
}

fn cmd_extract(args: ExtractArgs) -> Result<ExitCode> {
    let root = resolve_root(args.root.as_deref())?;
    let archive = load_archive(&root)?;
    let options = render::ExtractOptions {
        addresses: args.addresses,
        out: out_dir(&root, args.out),
        raw: args.raw,
        force: args.force,
    };
    let report = render::extract(&archive, &options)?;
    report_pixels(&root, &report);
    Ok(ExitCode::SUCCESS)
}

fn out_dir(root: &Path, explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| root.join(render::DEFAULT_OUT_DIR))
}

/// Print what was written, on stdout, one path per line.
///
/// One path per line and nothing else on stdout, so the caller can pipe it. Everything that is
/// commentary — the notes, the tally — goes to stderr, where it does not corrupt the list.
fn report_pixels(root: &Path, report: &render::Report) {
    for path in &report.written {
        println!("{}", show(root, path));
    }
    for note in &report.notes {
        eprintln!("scans: {note}");
    }
    if report.skipped.is_empty() {
        eprintln!("{} file(s) written.", report.written.len());
    } else {
        eprintln!(
            "{} file(s) written, {} left alone because they already existed (--force to \
             overwrite).",
            report.written.len(),
            report.skipped.len()
        );
    }
}

// ---------------------------------------------------------------------------------------
// ocr
// ---------------------------------------------------------------------------------------

fn cmd_ocr(args: OcrArgs) -> Result<ExitCode> {
    match args.command {
        OcrCommand::Import { xml, of, out } => {
            let of = match of {
                Some(of) => of,
                None => default_id_for(&xml)?,
            };
            let text = std::fs::read_to_string(&xml)
                .with_context(|| format!("reading {}", xml.display()))?;
            let pages = scans::djvu::parse(&text, &of)?;

            // One file per page, so `--out` names a directory rather than a file.
            let dir = out.unwrap_or_else(|| PathBuf::from("."));
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating {}", dir.display()))?;
            let schema_rel = schema_rel_for(&dir.join("x"));

            for page in &pages {
                let name = scans::ocr::file_name(&of, page.page);
                std::fs::write(dir.join(&name), scans::ocr::to_markdown(page, &schema_rel))
                    .with_context(|| format!("writing {name}"))?;
            }
            eprintln!("wrote {} page(s) into {}", pages.len(), show_cwd(&dir));
            Ok(ExitCode::SUCCESS)
        }

        OcrCommand::Check { files } => {
            let mut bad = 0usize;
            let mut checked = 0usize;
            for arg in &files {
                for path in expand_ocr_paths(arg)? {
                    checked += 1;
                    let ocr = match read_ocr(&path) {
                        Ok(ocr) => ocr,
                        Err(e) => {
                            println!("{}: {e:#}", path.display());
                            bad += 1;
                            continue;
                        }
                    };
                    // The page number is stated twice — in the filename and in the
                    // frontmatter — and a mismatch means one of them is lying about which
                    // page this is.
                    let expected = scans::ocr::file_name(&ocr.of, ocr.page);
                    if !path.to_string_lossy().ends_with(&expected) {
                        println!(
                            "{}: of/page say this should be named {expected}",
                            path.display()
                        );
                        bad += 1;
                    }
                }
            }
            eprintln!("{checked} file(s) checked, {bad} problem(s)");
            Ok(if bad == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}

/// One `.ocr.md`, or every one in a directory.
///
/// Accepting a directory is what makes this usable: an item's OCR is one file per page, and
/// naming 3,378 of them on a command line is not a thing anyone should have to do.
fn expand_ocr_paths(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(path).with_context(|| format!("reading {}", path.display()))?
    {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.ends_with(scans::ocr::SUFFIX))
        {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

fn read_ocr(path: &Path) -> Result<scans::ocr::Ocr> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    scans::ocr::from_markdown(&text).with_context(|| format!("parsing {}", path.display()))
}

/// The id implied by a DjVu XML filename: both derivations name theirs `<id>_djvu.xml`.
fn default_id_for(xml: &Path) -> Result<String> {
    let name = xml
        .file_name()
        .and_then(|n| n.to_str())
        .context("the XML path has no filename")?;
    Ok(name
        .strip_suffix("_djvu.xml")
        .unwrap_or_else(|| name.trim_end_matches(".xml"))
        .to_string())
}

/// A `#:schema` path from the sidecar's own directory back up to the repository root.
///
/// Counted from the path rather than assumed, so a sidecar written somewhere other than the
/// standard two-deep shard still points at a schema that exists.
fn schema_rel_for(out: &Path) -> String {
    let depth = out
        .parent()
        .map(|p| p.components().count())
        .unwrap_or_default();
    let up = "../".repeat(depth.min(8));
    format!("{up}{}", scans::ocr::SCHEMA_PATH)
}

// ---------------------------------------------------------------------------------------
// frc
// ---------------------------------------------------------------------------------------

fn cmd_frc(args: FrcArgs) -> Result<ExitCode> {
    match args.command {
        FrcCommand::Ingest {
            frc_data,
            root,
            limit,
            ids,
            force,
            dry_run,
        } => {
            let root = resolve_root(root.as_deref())?;

            let mut ids = if ids.is_empty() {
                scans::frc::ingest::identifiers(&frc_data)?
            } else {
                ids
            };
            if let Some(limit) = limit {
                ids.truncate(limit);
            }

            if dry_run {
                eprintln!(
                    "{} item(s) would be ingested into {}",
                    ids.len(),
                    show_cwd(&root.join(scans::frc::ingest::DIR))
                );
                for id in ids.iter().take(10) {
                    println!("{}", scans::frc::ingest::item_dir(id).display());
                }
                if ids.len() > 10 {
                    println!("… and {} more", ids.len() - 10);
                }
                return Ok(ExitCode::SUCCESS);
            }

            let total = ids.len();
            let started = std::time::Instant::now();
            let summary = scans::frc::ingest::ingest(&root, &frc_data, &ids, force, |i, id, outcome| {
                if let scans::frc::ingest::Outcome::Failed(why) = outcome {
                    eprintln!("scans: {id}: {why}");
                }
                // Progress on one rewritten line: 38,377 items of scrollback is not progress.
                if i % 200 == 0 || i + 1 == total {
                    let done = i + 1;
                    let rate = done as f64 / started.elapsed().as_secs_f64().max(0.001);
                    eprint!("\r  {done}/{total} ({rate:.0}/s)          ");
                }
            });
            eprintln!();

            eprintln!(
                "{} written ({} without OCR), {} already done, {} failed",
                summary.written,
                summary.without_ocr,
                summary.skipped,
                summary.failed.len()
            );
            for (id, why) in summary.failed.iter().take(20) {
                eprintln!("  {id}: {why}");
            }

            Ok(if summary.failed.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }

        FrcCommand::Fetch {
            root,
            interval,
            limit,
            ids,
            force,
            dry_run,
        } => cmd_frc_fetch(root, interval, limit, ids, force, dry_run),
    }
}

#[cfg(not(feature = "fetch"))]
fn cmd_frc_fetch(
    _root: Option<PathBuf>,
    _interval: f64,
    _limit: Option<usize>,
    _ids: Vec<String>,
    _force: bool,
    _dry_run: bool,
) -> Result<ExitCode> {
    anyhow::bail!(
        "this binary was built without the 'fetch' feature, which is what opens a socket.\n\
         Rebuild with: cargo build --release --features fetch"
    )
}

#[cfg(feature = "fetch")]
fn cmd_frc_fetch(
    root: Option<PathBuf>,
    interval: f64,
    limit: Option<usize>,
    ids: Vec<String>,
    force: bool,
    dry_run: bool,
) -> Result<ExitCode> {
    use scans::frc::fetch;

    let root = resolve_root(root.as_deref())?;

    // The item list is the archive itself: everything `frc ingest` has already written a
    // record for. That keeps the two halves in step without a shared manifest.
    let ids = if ids.is_empty() {
        let dir = root.join(scans::frc::ingest::DIR);
        let mut found = Vec::new();
        for shard in std::fs::read_dir(&dir)
            .with_context(|| format!("reading {} — run `scans frc ingest` first", dir.display()))?
        {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for item in std::fs::read_dir(shard.path())? {
                let item = item?;
                if item.file_type()?.is_dir()
                    && let Some(name) = item.file_name().to_str()
                {
                    found.push(name.to_string());
                }
            }
        }
        found.sort();
        found
    } else {
        ids
    };

    let options = fetch::Options {
        interval: std::time::Duration::from_secs_f64(interval.max(0.0)),
        limit,
        force,
        dry_run,
    };

    let mut ids = ids;
    if let Some(limit) = options.limit {
        ids.truncate(limit);
    }

    let total = ids.len();
    eprintln!(
        "{total} item(s), one request at a time, {interval}s apart, as {}",
        fetch::USER_AGENT
    );

    let started = std::time::Instant::now();
    let summary = fetch::fetch(&root, &ids, &options, |i, id, outcome| {
        match outcome {
            fetch::Outcome::Failed(why) => eprintln!("\rscans: {id}: {why}"),
            fetch::Outcome::NoPdf => eprintln!("\rscans: {id}: no PDF derivative"),
            _ => {}
        }
        if i % 10 == 0 || i + 1 == total {
            let done = i + 1;
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            let left = (total - done) as f64 * (elapsed / done as f64);
            eprint!("\r  {done}/{total}  {:.0}h left      ", left / 3600.0);
        }
    });
    eprintln!();

    eprintln!(
        "{} fetched ({:.1} GB), {} already had one, {} have no PDF, {} failed",
        summary.fetched,
        summary.bytes as f64 / 1e9,
        summary.skipped,
        summary.no_pdf,
        summary.failed.len()
    );
    for (id, why) in summary.failed.iter().take(20) {
        eprintln!("  {id}: {why}");
    }

    Ok(if summary.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
