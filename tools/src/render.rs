//! Turning a page of a scanned container into pixels.
//!
//! Two commands live here, and they answer two different questions.
//!
//! * [`render`] — *what did this page look like?* The page is rasterised whole, through a PDF
//!   interpreter, so anything drawn on top of the scan (stamps, redactions, a library's
//!   inserted leaf) is in the result. This is the one to feed a reader, human or otherwise.
//! * [`extract`] — *what bitmap is actually stored in there?* The embedded image objects come
//!   out at their own resolution, with no resampling and no compositing. This is the one to
//!   use when the pixels themselves are the evidence.
//!
//! Both address pages the way everything else in this tool does: by citation. `scans render
//! journal-de-paris-1789-01-03` is the whole issue, `…-01-03.p1` is its first page. Nothing
//! here takes a page number *into the PDF*, because that number is the archive's business and
//! getting it right by hand is exactly what the `pages = { from, to }` machinery exists to
//! avoid.
//!
//! # Feature `render`
//!
//! Off by default. This module opens container bytes and pulls a rasteriser into the build;
//! the default `validate` path must do neither.
//!
//! # Why the output is not written into the archive
//!
//! Renders are derived data. Volume 1 alone is 888 pages, and at native resolution the PNGs
//! outweigh the PDF they came from. They go to `--out`, which defaults to a gitignored
//! `.render/` at the repository root, and the archive records the transcription rather than
//! the pixels it was read from.

// Without the `render` feature both entry points are stubs that refuse, so every private
// helper below has no caller. They are still compiled, and still unit-tested, by a default
// build — the tiling arithmetic is the fiddly part of this module and a mistake in it should
// not wait for someone to install a rasteriser to be found.
#![cfg_attr(not(feature = "render"), allow(dead_code))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::load::{Archive, RefTarget};

/// Where renders and extractions go when `--out` is not given, relative to the repo root.
pub const DEFAULT_OUT_DIR: &str = ".render";

/// Resolution used when a page has no embedded image to take a native resolution from.
///
/// Only reachable for a born-digital page — a scan always has an image to measure. 300 is the
/// low end of what a facsimile is normally digitised at, so a page that lands here is legible
/// rather than merely present.
pub const FALLBACK_DPI: f32 = 300.0;

/// PDF user space is 72 units to the inch, by definition. Everything that converts between a
/// dpi and a scale factor goes through this.
const POINTS_PER_INCH: f32 = 72.0;

/// A rasterised page is materialised as one RGBA buffer before it is cropped, and vello's
/// pixmap is addressed in `u16`. A page asking for more than this is a mistake — a `--dpi`
/// with an extra digit — and saying so beats an allocation failure.
const MAX_EDGE: u32 = u16::MAX as u32;

// ---------------------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------------------

/// How the page is divided up. `1x1` is the whole page.
///
/// Tiling exists because a reader's input resolution is finite. A Journal de Paris page is
/// two columns of eighteenth-century type at roughly 1800x2900; shown whole to something that
/// samples it down to fit 1568 pixels on its long edge, the type is four pixels tall and
/// simply cannot be read. Split into quarters, each quarter arrives near its native
/// resolution. The archive is not the right place to solve this — but the tool that produces
/// the pixels is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub rows: u32,
    pub cols: u32,
}

impl Grid {
    pub const WHOLE: Grid = Grid { rows: 1, cols: 1 };

    pub fn is_whole(self) -> bool {
        self.rows <= 1 && self.cols <= 1
    }
}

impl Default for Grid {
    fn default() -> Self {
        Grid::WHOLE
    }
}

impl std::fmt::Display for Grid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.rows, self.cols)
    }
}

impl std::str::FromStr for Grid {
    type Err = String;

    /// `RxC`, e.g. `2x2`. `x` and `*` are both accepted because the shell eats neither and
    /// people type both.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (r, c) = s
            .split_once(['x', 'X', '*'])
            .ok_or_else(|| format!("expected ROWSxCOLS, e.g. 2x2, got {s:?}"))?;
        let parse = |v: &str, what: &str| -> Result<u32, String> {
            let n: u32 = v
                .trim()
                .parse()
                .map_err(|_| format!("{what} in {s:?} is not a number"))?;
            if n == 0 {
                return Err(format!("{what} in {s:?} must be at least 1"));
            }
            Ok(n)
        };
        Ok(Grid {
            rows: parse(r, "rows")?,
            cols: parse(c, "columns")?,
        })
    }
}

/// Options for [`render`].
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Citations to render: `<id>` for every page of a record, `<id>.p<n>` for one page.
    pub addresses: Vec<String>,
    /// Directory to write into. Created if absent.
    pub out: PathBuf,
    /// Target resolution. `None` means the native resolution of the page's largest embedded
    /// image, which is the resolution the page was actually scanned at.
    pub dpi: Option<f32>,
    pub grid: Grid,
    /// Percentage of a tile's own size added to each interior edge, so a line of type cut by
    /// a tile boundary survives whole in the neighbouring tile. Ignored for a `1x1` grid.
    pub overlap: f32,
    /// Overwrite outputs that already exist. Without it they are left alone and counted.
    pub force: bool,
}

/// Options for [`extract`].
#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub addresses: Vec<String>,
    pub out: PathBuf,
    /// Write the stored bytes verbatim instead of decoding them: a JPEG comes out `.jpg`, a
    /// JPEG 2000 `.jp2`. Lossless and exact, but a JBIG2 stream out of a PDF is a fragment
    /// that no ordinary viewer opens — which is why it is not the default.
    pub raw: bool,
    pub force: bool,
}

/// What a run did.
#[derive(Debug, Default)]
pub struct Report {
    pub written: Vec<PathBuf>,
    /// Outputs that already existed and were left alone.
    pub skipped: Vec<PathBuf>,
    /// Anything the operator should read: a fallback taken, a page that could not be decoded.
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------------------
// Planning: citations to concrete (container, page index) pairs
// ---------------------------------------------------------------------------------------

/// One page to be worked on, already resolved to a container and a page index inside it.
#[derive(Debug, Clone)]
struct Target {
    /// `<id>.p<n>`, and the stem of every file written for it.
    address: String,
    /// Absolute path of the container.
    container: PathBuf,
    /// 1-based page index inside the container.
    index: i64,
}

/// Turn citations into targets, in the order given, without duplicates.
///
/// A citation naming a record expands to every page of it. A citation naming a page that has
/// no graphic, or whose graphic is a standalone image rather than a page of a container, is an
/// error naming the address — the alternative is a silent gap in a batch of several hundred.
fn plan(archive: &Archive, addresses: &[String]) -> Result<Vec<Target>> {
    let mut out: Vec<Target> = Vec::new();
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();

    for address in addresses {
        let pages = match archive.resolve_reference(address) {
            Err(e) => bail!("{address}: {e}"),
            Ok(RefTarget::Document(node)) => {
                if node.pages.is_empty() {
                    bail!(
                        "{address} resolves to {} ({}), which declares no pages",
                        node.id,
                        node.rel_path
                    );
                }
                node.pages.iter().collect::<Vec<_>>()
            }
            Ok(RefTarget::Page { page, .. }) => vec![page],
        };

        for page in pages {
            let address = page.address();
            let Some(graphic) = page.primary_graphic() else {
                bail!("{address} declares no graphic, so there is nothing to read");
            };
            let Some(index) = graphic.page else {
                bail!(
                    "{address} points at {}, a standalone image rather than a page inside a \
                     container; this command reads pages out of a PDF",
                    graphic.file_raw
                );
            };
            if index < 1 {
                bail!("{address} has graphic.page = {index}, which is not a page number");
            }
            if seen.insert(address.clone(), ()).is_none() {
                out.push(Target {
                    address,
                    container: graphic.file.clone(),
                    index,
                });
            }
        }
    }

    if out.is_empty() {
        bail!("no pages to work on");
    }
    Ok(out)
}

/// Group targets by container, preserving first-seen order within each group.
///
/// Volume 1 is an 87 MB PDF whose cross-reference table is parsed on open. Rendering 205
/// issues one process-call per issue would parse it 205 times; grouping parses it once.
fn by_container(targets: Vec<Target>) -> Vec<(PathBuf, Vec<Target>)> {
    let mut order: Vec<PathBuf> = Vec::new();
    let mut groups: BTreeMap<PathBuf, Vec<Target>> = BTreeMap::new();
    for t in targets {
        let key = t.container.clone();
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(t);
    }
    order
        .into_iter()
        .map(|k| {
            let v = groups.remove(&k).expect("key came from the map");
            (k, v)
        })
        .collect()
}

/// True when the destination should be left alone rather than written.
fn keep_existing(path: &Path, force: bool, report: &mut Report) -> bool {
    if !force && path.exists() {
        report.skipped.push(path.to_path_buf());
        return true;
    }
    false
}

fn write_file(path: &Path, bytes: &[u8], report: &mut Report) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    report.written.push(path.to_path_buf());
    Ok(())
}

// ---------------------------------------------------------------------------------------
// PNG encoding
// ---------------------------------------------------------------------------------------

/// A decoded raster, in whichever of the three shapes PNG can store directly.
#[cfg(feature = "render")]
pub(crate) enum Raster {
    /// One byte per pixel.
    Gray8 { w: u32, h: u32, data: Vec<u8> },
    /// Packed one bit per pixel, rows padded to a byte. 0 is black.
    Gray1 { w: u32, h: u32, data: Vec<u8> },
    Rgb8 { w: u32, h: u32, data: Vec<u8> },
    Rgba8 { w: u32, h: u32, data: Vec<u8> },
}

#[cfg(feature = "render")]
impl Raster {
    fn dimensions(&self) -> (u32, u32) {
        match self {
            Raster::Gray8 { w, h, .. }
            | Raster::Gray1 { w, h, .. }
            | Raster::Rgb8 { w, h, .. }
            | Raster::Rgba8 { w, h, .. } => (*w, *h),
        }
    }

    /// Encode as PNG.
    fn to_png(&self) -> Result<Vec<u8>> {
        use png::{BitDepth, ColorType};

        let (w, h) = self.dimensions();
        let (colour, depth, data) = match self {
            Raster::Gray8 { data, .. } => (ColorType::Grayscale, BitDepth::Eight, data),
            Raster::Gray1 { data, .. } => (ColorType::Grayscale, BitDepth::One, data),
            Raster::Rgb8 { data, .. } => (ColorType::Rgb, BitDepth::Eight, data),
            Raster::Rgba8 { data, .. } => (ColorType::Rgba, BitDepth::Eight, data),
        };

        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, w, h);
            encoder.set_color(colour);
            encoder.set_depth(depth);
            let mut writer = encoder.write_header().context("writing the PNG header")?;
            writer
                .write_image_data(data)
                .context("writing the PNG image data")?;
            writer.finish().context("finishing the PNG")?;
        }
        Ok(out)
    }
}

/// Choose the narrowest PNG shape that stores these pixels exactly.
///
/// A scan of a printed page is grey — usually bitonal — and storing it as RGBA quadruples the
/// file for pixels that are already identical across the three channels. The scan that decides
/// this is one pass over the buffer and is dwarfed by the rasterisation that produced it.
#[cfg(feature = "render")]
fn narrow_rgba(w: u32, h: u32, rgba: &[u8]) -> Raster {
    debug_assert_eq!(rgba.len(), (w as usize) * (h as usize) * 4);

    let mut opaque = true;
    let mut grey = true;
    for px in rgba.chunks_exact(4) {
        if px[3] != 0xFF {
            opaque = false;
            break;
        }
        if px[0] != px[1] || px[1] != px[2] {
            grey = false;
        }
    }

    if !opaque {
        return Raster::Rgba8 {
            w,
            h,
            data: rgba.to_vec(),
        };
    }
    if grey {
        return Raster::Gray8 {
            w,
            h,
            data: rgba.iter().step_by(4).copied().collect(),
        };
    }
    let mut data = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for px in rgba.chunks_exact(4) {
        data.extend_from_slice(&px[..3]);
    }
    Raster::Rgb8 { w, h, data }
}

// ---------------------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------------------

#[cfg(not(feature = "render"))]
pub fn render(_archive: &Archive, _opts: &RenderOptions) -> Result<Report> {
    bail!("this build cannot rasterise a PDF; rebuild with --features render")
}

/// Rasterise every addressed page, and write one PNG per tile.
#[cfg(feature = "render")]
pub fn render(archive: &Archive, opts: &RenderOptions) -> Result<Report> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{RenderCache, RenderSettings};

    if !(0.0..=100.0).contains(&opts.overlap) {
        bail!("--overlap must be a percentage between 0 and 100");
    }
    if let Some(dpi) = opts.dpi
        && dpi <= 0.0
    {
        bail!("--dpi must be greater than zero");
    }

    let mut report = Report::default();
    let settings = InterpreterSettings::default();

    for (container, targets) in by_container(plan(archive, &opts.addresses)?) {
        // Every output for this container may already exist, in which case the PDF is never
        // opened at all. That is the common case on a re-run of a large batch.
        let outputs: Vec<(Target, Vec<PathBuf>)> = targets
            .into_iter()
            .map(|t| {
                let paths = tile_paths(&opts.out, &t.address, opts.grid);
                (t, paths)
            })
            .collect();
        if !opts.force
            && outputs
                .iter()
                .all(|(_, paths)| paths.iter().all(|p| p.exists()))
        {
            for (_, paths) in &outputs {
                report.skipped.extend(paths.iter().cloned());
            }
            continue;
        }

        let bytes = std::fs::read(&container)
            .with_context(|| format!("reading {}", container.display()))?;
        let total = bytes.len();
        let pdf = Pdf::new(bytes).map_err(|e| {
            anyhow::anyhow!(
                "{} is not a PDF this tool can read ({e:?}); {} byte(s) were read — a Git LFS \
                 pointer instead of the file itself looks like this",
                container.display(),
                total
            )
        })?;
        let pages = pdf.pages();
        let cache = RenderCache::new();

        for (target, paths) in outputs {
            if !opts.force && paths.iter().all(|p| p.exists()) {
                report.skipped.extend(paths);
                continue;
            }

            let Some(page) = pages.get((target.index - 1) as usize) else {
                bail!(
                    "{} points at page {} of {}, which holds {} page(s)",
                    target.address,
                    target.index,
                    container.display(),
                    pages.len()
                );
            };

            let (base_w, base_h) = page.base_dimensions();
            let scale = match opts.dpi {
                Some(dpi) => dpi / POINTS_PER_INCH,
                None => native_scale(page, &mut report, &target.address),
            };
            let (w, h) = page.render_dimensions();
            let (px_w, px_h) = ((w * scale) as u32, (h * scale) as u32);
            if px_w == 0 || px_h == 0 {
                bail!(
                    "{} would rasterise to {px_w}x{px_h}; the page measures {base_w}x{base_h} \
                     points and the scale came out at {scale}",
                    target.address
                );
            }
            if px_w > MAX_EDGE || px_h > MAX_EDGE {
                bail!(
                    "{} would rasterise to {px_w}x{px_h}, over the {MAX_EDGE} pixel limit; \
                     lower --dpi",
                    target.address
                );
            }

            let pixmap = hayro::render(
                page,
                &cache,
                &settings,
                &RenderSettings {
                    x_scale: scale,
                    y_scale: scale,
                    width: Some(px_w as u16),
                    height: Some(px_h as u16),
                    // Opaque white, not transparent: this is paper.
                    bg_color: WHITE,
                },
            );
            let (pm_w, pm_h) = (pixmap.width() as u32, pixmap.height() as u32);
            let rgba: Vec<u8> = pixmap
                .take_unpremultiplied()
                .into_iter()
                .flat_map(|p| [p.r, p.g, p.b, p.a])
                .collect();

            for (rect, path) in tiles(pm_w, pm_h, opts.grid, opts.overlap)
                .into_iter()
                .zip(paths)
            {
                if keep_existing(&path, opts.force, &mut report) {
                    continue;
                }
                let cropped = crop(&rgba, pm_w, rect);
                let raster = narrow_rgba(rect.w, rect.h, &cropped);
                write_file(&path, &raster.to_png()?, &mut report)?;
            }
        }
    }

    Ok(report)
}

/// The scale at which the page's largest embedded image lands on the raster one pixel to one
/// pixel — that is, the resolution the page was scanned at.
///
/// Both axes are measured and the larger factor wins, so a page whose image is not quite the
/// shape of its media box is upsampled on one axis rather than losing detail on the other.
#[cfg(feature = "render")]
fn native_scale(
    page: &hayro::hayro_syntax::page::Page<'_>,
    report: &mut Report,
    address: &str,
) -> f32 {
    let (base_w, base_h) = page.base_dimensions();
    let mut best = 0.0_f32;
    for (_, stream) in page_images(page) {
        let (Some(w), Some(h)) = (
            stream.dict().get::<u32>(b"Width"),
            stream.dict().get::<u32>(b"Height"),
        ) else {
            continue;
        };
        if base_w > 0.0 {
            best = best.max(w as f32 / base_w);
        }
        if base_h > 0.0 {
            best = best.max(h as f32 / base_h);
        }
    }
    if best > 0.0 {
        return best;
    }
    report.notes.push(format!(
        "{address}: no embedded image to take a native resolution from; rendered at \
         {FALLBACK_DPI} dpi"
    ));
    FALLBACK_DPI / POINTS_PER_INCH
}

// ---------------------------------------------------------------------------------------
// Tiling
// ---------------------------------------------------------------------------------------

/// A crop rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Divide `w x h` into `grid` tiles, each grown by `overlap` percent of its own size on every
/// interior edge.
///
/// Tiles are produced row-major, which is reading order, and is the order [`tile_paths`]
/// names them in.
fn tiles(w: u32, h: u32, grid: Grid, overlap: f32) -> Vec<Rect> {
    if grid.is_whole() || w == 0 || h == 0 {
        return vec![Rect { x: 0, y: 0, w, h }];
    }

    let step_x = w as f64 / grid.cols as f64;
    let step_y = h as f64 / grid.rows as f64;
    let pad_x = step_x * (overlap as f64) / 100.0;
    let pad_y = step_y * (overlap as f64) / 100.0;

    let mut out = Vec::with_capacity((grid.rows * grid.cols) as usize);
    for r in 0..grid.rows {
        for c in 0..grid.cols {
            // The padded edges are clamped to the page, so the outer border of the grid is
            // never grown: an overlap is there to rescue a line cut in half, and there is
            // nothing beyond the paper to rescue it with.
            //
            // A grid finer than the page has pixels would otherwise produce tiles of zero
            // width, or an origin past the last pixel. Every named output must be a real
            // file, so the origin is held one pixel inside the page and the far edge one
            // pixel past the origin.
            let x0 = r_to_px(c as f64 * step_x - pad_x).min(w - 1);
            let x1 = r_to_px((c + 1) as f64 * step_x + pad_x).clamp(x0 + 1, w);
            let y0 = r_to_px(r as f64 * step_y - pad_y).min(h - 1);
            let y1 = r_to_px((r + 1) as f64 * step_y + pad_y).clamp(y0 + 1, h);
            out.push(Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            });
        }
    }
    out
}

/// Round a fractional pixel coordinate to a pixel, floored at zero.
fn r_to_px(v: f64) -> u32 {
    if v <= 0.0 { 0 } else { v.round() as u32 }
}

/// The output path of every tile of one page, in the order [`tiles`] produces them.
fn tile_paths(out: &Path, address: &str, grid: Grid) -> Vec<PathBuf> {
    if grid.is_whole() {
        return vec![out.join(format!("{address}.png"))];
    }
    let mut paths = Vec::with_capacity((grid.rows * grid.cols) as usize);
    for r in 1..=grid.rows {
        for c in 1..=grid.cols {
            paths.push(out.join(format!("{address}.r{r}c{c}.png")));
        }
    }
    paths
}

/// Copy a sub-rectangle out of an RGBA buffer.
fn crop(rgba: &[u8], stride_px: u32, rect: Rect) -> Vec<u8> {
    let mut out = Vec::with_capacity((rect.w as usize) * (rect.h as usize) * 4);
    for row in 0..rect.h {
        let start = (((rect.y + row) as usize) * (stride_px as usize) + rect.x as usize) * 4;
        let end = start + (rect.w as usize) * 4;
        out.extend_from_slice(&rgba[start..end]);
    }
    out
}

// ---------------------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------------------

#[cfg(not(feature = "render"))]
pub fn extract(_archive: &Archive, _opts: &ExtractOptions) -> Result<Report> {
    bail!("this build cannot read a PDF's images; rebuild with --features render")
}

/// Write out the image objects stored on every addressed page.
#[cfg(feature = "render")]
pub fn extract(archive: &Archive, opts: &ExtractOptions) -> Result<Report> {
    use hayro::hayro_syntax::Pdf;

    let mut report = Report::default();

    for (container, targets) in by_container(plan(archive, &opts.addresses)?) {
        let bytes = std::fs::read(&container)
            .with_context(|| format!("reading {}", container.display()))?;
        let total = bytes.len();
        let pdf = Pdf::new(bytes).map_err(|e| {
            anyhow::anyhow!(
                "{} is not a PDF this tool can read ({e:?}); {} byte(s) were read — a Git LFS \
                 pointer instead of the file itself looks like this",
                container.display(),
                total
            )
        })?;
        let pages = pdf.pages();

        for target in targets {
            let Some(page) = pages.get((target.index - 1) as usize) else {
                bail!(
                    "{} points at page {} of {}, which holds {} page(s)",
                    target.address,
                    target.index,
                    container.display(),
                    pages.len()
                );
            };

            let images = page_images(page);
            if images.is_empty() {
                report.notes.push(format!(
                    "{}: the page holds no image objects",
                    target.address
                ));
                continue;
            }

            for (name, stream) in images {
                let stem = format!("{}.{}", target.address, sanitise(&name));
                if opts.raw {
                    let (ext, note) = raw_extension(&stream);
                    if let Some(note) = note {
                        report.notes.push(format!("{stem}: {note}"));
                    }
                    let path = opts.out.join(format!("{stem}.{ext}"));
                    if keep_existing(&path, opts.force, &mut report) {
                        continue;
                    }
                    write_file(&path, &stream.raw_data(), &mut report)?;
                    continue;
                }

                let path = opts.out.join(format!("{stem}.png"));
                if keep_existing(&path, opts.force, &mut report) {
                    continue;
                }
                match decode_image(&stream) {
                    Ok(raster) => write_file(&path, &raster.to_png()?, &mut report)?,
                    Err(why) => {
                        // Falling back to the stored bytes rather than failing: an image this
                        // tool cannot turn into a PNG is still an image, and the operator is
                        // better served by having it than by having the run stop.
                        let (ext, _) = raw_extension(&stream);
                        let path = opts.out.join(format!("{stem}.{ext}"));
                        report
                            .notes
                            .push(format!("{stem}: {why}; wrote the stored bytes as {ext}"));
                        if keep_existing(&path, opts.force, &mut report) {
                            continue;
                        }
                        write_file(&path, &stream.raw_data(), &mut report)?;
                    }
                }
            }
        }
    }

    Ok(report)
}

/// Every image XObject reachable from a page's resources, nearest first, as
/// `(resource name, stream)`.
///
/// The resource chain is walked because a page may inherit its resources from the page tree,
/// and a name defined nearer the page shadows one further away — so an entry already seen is
/// not replaced by an outer one of the same name.
#[cfg(feature = "render")]
fn page_images<'a>(
    page: &hayro::hayro_syntax::page::Page<'a>,
) -> Vec<(String, hayro::hayro_syntax::object::Stream<'a>)> {
    use hayro::hayro_syntax::object::Stream;

    let mut out: Vec<(String, Stream<'a>)> = Vec::new();
    let mut seen: BTreeMap<Vec<u8>, ()> = BTreeMap::new();
    let mut resources = Some(page.resources());

    while let Some(r) = resources {
        // Fetched by key rather than read out of `entries()`, so an XObject stored as an
        // indirect reference — which is how every real PDF stores one — is followed.
        let keys: Vec<Vec<u8>> = r.x_objects.keys().map(|n| n.as_ref().to_vec()).collect();
        for key in keys {
            if seen.insert(key.clone(), ()).is_some() {
                continue;
            }
            let Some(stream) = r.x_objects.get::<Stream<'a>>(&key) else {
                continue;
            };
            if is_image(&stream) {
                out.push((String::from_utf8_lossy(&key).into_owned(), stream));
            }
        }
        resources = r.parent();
    }

    out
}

#[cfg(feature = "render")]
fn is_image(stream: &hayro::hayro_syntax::object::Stream<'_>) -> bool {
    stream
        .dict()
        .get::<hayro::hayro_syntax::object::Name<'_>>(b"Subtype")
        .is_some_and(|n| n.as_str() == "Image")
}

/// The extension the stored bytes should carry, and a caveat when they are not a file format
/// anything opens on its own.
#[cfg(feature = "render")]
fn raw_extension(
    stream: &hayro::hayro_syntax::object::Stream<'_>,
) -> (&'static str, Option<&'static str>) {
    use hayro::hayro_syntax::Filter;

    match stream.filters().last() {
        Some(Filter::DctDecode) => ("jpg", None),
        Some(Filter::JpxDecode) => ("jp2", None),
        // A PDF stores JBIG2 as an embedded stream: the file header and the page association
        // that a standalone .jb2 carries are stripped, and the symbol dictionary may live in
        // a separate globals stream that is not in these bytes at all.
        Some(Filter::Jbig2Decode) => (
            "jbig2",
            Some(
                "a JBIG2 stream embedded in a PDF is not a standalone file; without the \
                 globals stream most viewers will not open it",
            ),
        ),
        Some(Filter::CcittFaxDecode) => (
            "ccitt",
            Some("raw CCITT data carries no header; the parameters are in the PDF"),
        ),
        _ => ("bin", None),
    }
}

/// Decode one image stream into something PNG can hold.
#[cfg(feature = "render")]
fn decode_image(stream: &hayro::hayro_syntax::object::Stream<'_>) -> Result<Raster, String> {
    use hayro::hayro_syntax::object::stream::{ImageColorSpace, ImageDecodeParams};

    let dict = stream.dict();
    let width = dict.get::<u32>(b"Width").unwrap_or(0);
    let height = dict.get::<u32>(b"Height").unwrap_or(0);
    if width == 0 || height == 0 {
        return Err("the image dictionary states no usable /Width and /Height".into());
    }
    let dict_bpc = dict.get::<u8>(b"BitsPerComponent");
    let is_mask = dict.get::<bool>(b"ImageMask").unwrap_or(false);

    let params = ImageDecodeParams {
        is_indexed: false,
        bpc: dict_bpc.or(if is_mask { Some(1) } else { None }),
        num_components: None,
        target_dimension: None,
        width,
        height,
    };

    let result = stream
        .decoded_image(&params)
        .map_err(|e| format!("the image stream did not decode ({e:?})"))?;

    // A decoder that reports its own geometry is believed over the dictionary: JPEG 2000
    // carries the truth in its codestream, and the two have been seen to disagree.
    let (w, h, bpc, colour) = match &result.image_data {
        Some(d) => (
            d.width,
            d.height,
            d.bits_per_component,
            d.color_space.unwrap_or(ImageColorSpace::Gray),
        ),
        None => (
            width,
            height,
            dict_bpc.unwrap_or(if is_mask { 1 } else { 8 }),
            ImageColorSpace::Gray,
        ),
    };
    let data = result.data.into_owned();
    let px = (w as usize) * (h as usize);

    match (colour, bpc) {
        (ImageColorSpace::Gray, 8) if data.len() >= px => Ok(Raster::Gray8 {
            w,
            h,
            data: data[..px].to_vec(),
        }),
        (ImageColorSpace::Gray, 1) => {
            let need = (w as usize).div_ceil(8) * h as usize;
            if data.len() < need {
                return Err(format!(
                    "the decoder returned {} byte(s) for a {w}x{h} bitonal image needing {need}",
                    data.len()
                ));
            }
            Ok(Raster::Gray1 {
                w,
                h,
                data: data[..need].to_vec(),
            })
        }
        (ImageColorSpace::Rgb, 8) if data.len() >= px * 3 => Ok(Raster::Rgb8 {
            w,
            h,
            data: data[..px * 3].to_vec(),
        }),
        (ImageColorSpace::Cmyk, 8) if data.len() >= px * 4 => {
            // The naive conversion, and it is named as such in the note the caller prints.
            // Anything better needs the ICC profile, which is exactly what --raw is for.
            let mut rgb = Vec::with_capacity(px * 3);
            for p in data[..px * 4].chunks_exact(4) {
                let k = p[3] as u16;
                for c in &p[..3] {
                    rgb.push(((*c as u16 * k) / 255) as u8);
                }
            }
            Ok(Raster::Rgb8 { w, h, data: rgb })
        }
        (c, b) => Err(format!(
            "a {c:?} image at {b} bit(s) per component is not one this tool writes as PNG"
        )),
    }
}

/// Make a PDF resource name safe to put in a filename.
fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "image".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_grid_is_one_tile_covering_the_page() {
        let t = tiles(100, 200, Grid::WHOLE, 10.0);
        assert_eq!(
            t,
            vec![Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 200
            }]
        );
        assert_eq!(
            tile_paths(Path::new("out"), "x.p1", Grid::WHOLE),
            vec![PathBuf::from("out/x.p1.png")]
        );
    }

    #[test]
    fn tiles_cover_the_page_and_are_named_in_reading_order() {
        let grid = Grid { rows: 2, cols: 2 };
        let t = tiles(100, 200, grid, 0.0);
        assert_eq!(
            t,
            vec![
                Rect { x: 0, y: 0, w: 50, h: 100 },
                Rect { x: 50, y: 0, w: 50, h: 100 },
                Rect { x: 0, y: 100, w: 50, h: 100 },
                Rect { x: 50, y: 100, w: 50, h: 100 },
            ]
        );
        let names: Vec<String> = tile_paths(Path::new("out"), "x.p1", grid)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["x.p1.r1c1.png", "x.p1.r1c2.png", "x.p1.r2c1.png", "x.p1.r2c2.png"]
        );
    }

    #[test]
    fn overlap_grows_interior_edges_only() {
        let t = tiles(100, 100, Grid { rows: 2, cols: 1 }, 10.0);
        // Top tile keeps the top edge at 0 and reaches 5 px past the midpoint.
        assert_eq!(t[0], Rect { x: 0, y: 0, w: 100, h: 55 });
        // Bottom tile starts 5 px early and keeps the bottom edge at the page.
        assert_eq!(t[1], Rect { x: 0, y: 45, w: 100, h: 55 });
    }

    #[test]
    fn every_named_tile_is_a_real_rectangle_even_on_an_absurd_grid() {
        let grid = Grid { rows: 8, cols: 8 };
        let t = tiles(3, 3, grid, 0.0);
        assert_eq!(t.len(), tile_paths(Path::new("o"), "x.p1", grid).len());
        for r in t {
            assert!(r.w >= 1 && r.h >= 1, "{r:?}");
            assert!(r.x + r.w <= 3 && r.y + r.h <= 3, "{r:?}");
        }
    }

    #[test]
    fn crop_takes_the_rectangle_asked_for() {
        // 3x2 RGBA, each pixel's red channel is its index.
        let mut rgba = Vec::new();
        for i in 0..6u8 {
            rgba.extend_from_slice(&[i, 0, 0, 255]);
        }
        let out = crop(&rgba, 3, Rect { x: 1, y: 0, w: 2, h: 2 });
        let reds: Vec<u8> = out.iter().step_by(4).copied().collect();
        assert_eq!(reds, vec![1, 2, 4, 5]);
    }

    #[test]
    fn a_grid_is_parsed_from_either_separator_and_rejects_zero() {
        assert_eq!("2x3".parse::<Grid>().unwrap(), Grid { rows: 2, cols: 3 });
        assert_eq!("2*3".parse::<Grid>().unwrap(), Grid { rows: 2, cols: 3 });
        assert!("0x3".parse::<Grid>().is_err());
        assert!("2".parse::<Grid>().is_err());
        assert!("axb".parse::<Grid>().is_err());
    }

    #[test]
    fn a_resource_name_becomes_a_safe_filename() {
        assert_eq!(sanitise("Im0"), "Im0");
        assert_eq!(sanitise("a/b c"), "a_b_c");
        assert_eq!(sanitise(""), "image");
    }
}
