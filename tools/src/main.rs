//! `scans` — the archive tool's command line.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use scans::load::{RefTarget, load_archive};
use scans::model;
use scans::{migrate, validate};

/// Path of the generated schema, relative to the repository root.
const SCHEMA_PATH: &str = "schemas/source.json";

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
