//! Integration tests for `scans render` and `scans extract`.
//!
//! The fixture PDF is built byte by byte here rather than checked in. It is 500-odd bytes and
//! its every field is visible in [`one_page_pdf`], which matters because these tests assert
//! things about resolution: a checked-in binary would leave "why is the render 4 pixels wide?"
//! answerable only by opening it in something else.
//!
//! The real archive is deliberately not used — same reason as the other integration tests:
//! a test that needs 3.5 GB of LFS content is not a test.

use std::path::{Path, PathBuf};

use scans::load::load_archive;
use scans::render::{DEFAULT_OUT_DIR, ExtractOptions, Grid, RenderOptions};

// ---------------------------------------------------------------------------------------
// A minimal PDF
// ---------------------------------------------------------------------------------------

/// The greyscale samples of the fixture's image: 4 wide, 2 high, one byte each.
const IMAGE: [u8; 8] = [0x00, 0x40, 0x80, 0xFF, 0xFF, 0x80, 0x40, 0x00];
const IMAGE_W: u32 = 4;
const IMAGE_H: u32 = 2;

/// The page measures 100x50 points and the image is drawn over the whole of it. The image is
/// therefore 25 times coarser than a point, and a *native* render must come out at exactly
/// `IMAGE_W x IMAGE_H` — which is what makes the native-resolution assertion meaningful.
const PAGE_W: u32 = 100;
const PAGE_H: u32 = 50;

/// One page, one image, no compression, with a correct cross-reference table.
fn one_page_pdf() -> Vec<u8> {
    let content = b"q 100 0 0 50 0 0 cm /Im0 Do Q\n";

    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] \
             /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>"
        )
        .into_bytes(),
        {
            let mut o = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            o.extend_from_slice(content);
            o.extend_from_slice(b"endstream");
            o
        },
        {
            let mut o = format!(
                "<< /Type /XObject /Subtype /Image /Width {IMAGE_W} /Height {IMAGE_H} \
                 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} >>\nstream\n",
                IMAGE.len()
            )
            .into_bytes();
            o.extend_from_slice(&IMAGE);
            o.extend_from_slice(b"\nendstream");
            o
        },
    ];

    let mut pdf: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let xref_at = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

// ---------------------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------------------

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    /// A copy holding the fixture PDF, and one document citing its only page.
    fn new() -> Self {
        let f = Fixture {
            dir: tempfile::tempdir().expect("tempdir"),
        };
        let dir = f.dir.path().join("source/test");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("vol.pdf"), one_page_pdf()).expect("write pdf");
        std::fs::write(
            dir.join("vol.toml"),
            "id = \"vol\"\n\
             layer = \"copy\"\n\
             title = \"A volume\"\n\
             [scan]\n\
             file = \"vol.pdf\"\n\
             count = 1\n",
        )
        .expect("write copy");
        std::fs::write(
            dir.join("doc.toml"),
            "id = \"doc\"\n\
             layer = \"document\"\n\
             of = \"vol\"\n\
             pages = { from = 1, to = 1 }\n",
        )
        .expect("write document");
        f
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn out(&self) -> PathBuf {
        self.root().join("out")
    }
}

/// `(width, height, bit depth, colour type)` read out of a PNG's IHDR.
///
/// Read by hand rather than through a decoder: the header is at a fixed offset and this keeps
/// the test's dependencies to the ones the crate already has.
#[cfg(feature = "render")]
fn png_header(path: &Path) -> (u32, u32, u8, u8) {
    let bytes = std::fs::read(path).expect("read png");
    assert!(
        bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        "{} is not a PNG",
        path.display()
    );
    assert_eq!(&bytes[12..16], b"IHDR", "{} has no IHDR", path.display());
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
    (word(16), word(20), bytes[24], bytes[25])
}

/// PNG colour type 0 is greyscale.
#[cfg(feature = "render")]
const PNG_GREY: u8 = 0;

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

#[test]
fn the_default_output_directory_is_gitignored() {
    // The gitignore entry and the constant are two spellings of one decision, and nothing
    // else would notice them drifting apart until several gigabytes of PNG were staged.
    let ignore = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .join(".gitignore"),
    )
    .expect("read .gitignore");
    assert!(
        ignore.lines().any(|l| l.trim() == format!("/{DEFAULT_OUT_DIR}/")),
        "'/{DEFAULT_OUT_DIR}/' is not in .gitignore"
    );
}

#[cfg(not(feature = "render"))]
#[test]
fn without_the_feature_both_commands_refuse_rather_than_doing_nothing() {
    let f = Fixture::new();
    let archive = load_archive(f.root()).expect("loads");
    let err = scans::render::render(
        &archive,
        &RenderOptions {
            addresses: vec!["doc".into()],
            out: f.out(),
            dpi: None,
            grid: Grid::WHOLE,
            overlap: 4.0,
            force: false,
        },
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("--features render"), "{err}");

    let err = scans::render::extract(
        &archive,
        &ExtractOptions {
            addresses: vec!["doc".into()],
            out: f.out(),
            raw: false,
            force: false,
        },
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("--features render"), "{err}");
}

#[cfg(feature = "render")]
mod with_render {
    use super::*;

    fn render_opts(f: &Fixture, grid: Grid) -> RenderOptions {
        RenderOptions {
            addresses: vec!["doc".into()],
            out: f.out(),
            dpi: None,
            grid,
            overlap: 0.0,
            force: false,
        }
    }

    #[test]
    fn a_native_render_comes_out_at_the_scan_s_own_resolution() {
        let f = Fixture::new();
        let archive = load_archive(f.root()).expect("loads");
        let report =
            scans::render::render(&archive, &render_opts(&f, Grid::WHOLE)).expect("renders");

        assert_eq!(report.written.len(), 1, "{:?}", report.written);
        let path = f.out().join("doc.p1.png");
        assert_eq!(report.written[0], path);
        // The page is 25 points per image pixel; native means the image's own size, not the
        // page's. A default-dpi render would have produced 100x50 and lost nothing but would
        // have invented 12 times the pixels.
        assert_eq!(png_header(&path), (IMAGE_W, IMAGE_H, 8, PNG_GREY));
    }

    #[test]
    fn dpi_overrides_the_native_resolution() {
        let f = Fixture::new();
        let archive = load_archive(f.root()).expect("loads");
        let opts = RenderOptions {
            // 144 dpi is two device pixels to the point.
            dpi: Some(144.0),
            ..render_opts(&f, Grid::WHOLE)
        };
        scans::render::render(&archive, &opts).expect("renders");
        let (w, h, _, _) = png_header(&f.out().join("doc.p1.png"));
        assert_eq!((w, h), (PAGE_W * 2, PAGE_H * 2));
    }

    #[test]
    fn a_grid_writes_one_named_tile_per_cell() {
        let f = Fixture::new();
        let archive = load_archive(f.root()).expect("loads");
        let opts = RenderOptions {
            dpi: Some(720.0),
            ..render_opts(&f, Grid { rows: 2, cols: 2 })
        };
        let report = scans::render::render(&archive, &opts).expect("renders");

        let names: Vec<String> = report
            .written
            .iter()
            .map(|p| p.file_name().expect("named").to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                "doc.p1.r1c1.png",
                "doc.p1.r1c2.png",
                "doc.p1.r2c1.png",
                "doc.p1.r2c2.png"
            ]
        );
        // 720 dpi is ten device pixels to the point: a 1000x500 page in four 500x250 tiles,
        // with --overlap 0 so the arithmetic is exact.
        for name in &names {
            let (w, h, _, _) = png_header(&f.out().join(name));
            assert_eq!((w, h), (PAGE_W * 10 / 2, PAGE_H * 10 / 2), "{name}");
        }
    }

    #[test]
    fn an_existing_output_is_left_alone_unless_force_is_given() {
        let f = Fixture::new();
        let archive = load_archive(f.root()).expect("loads");
        scans::render::render(&archive, &render_opts(&f, Grid::WHOLE)).expect("first run");

        let path = f.out().join("doc.p1.png");
        let marker = b"not a png at all".to_vec();
        std::fs::write(&path, &marker).expect("clobber");

        let again = scans::render::render(&archive, &render_opts(&f, Grid::WHOLE)).expect("rerun");
        assert!(again.written.is_empty());
        assert_eq!(again.skipped, vec![path.clone()]);
        assert_eq!(std::fs::read(&path).expect("read"), marker, "it was rewritten");

        let forced = scans::render::render(
            &archive,
            &RenderOptions {
                force: true,
                ..render_opts(&f, Grid::WHOLE)
            },
        )
        .expect("forced");
        assert_eq!(forced.written, vec![path.clone()]);
        assert_eq!(png_header(&path), (IMAGE_W, IMAGE_H, 8, PNG_GREY));
    }

    #[test]
    fn extract_writes_the_stored_image_unresampled_and_names_it_after_the_resource() {
        let f = Fixture::new();
        let archive = load_archive(f.root()).expect("loads");
        let report = scans::render::extract(
            &archive,
            &ExtractOptions {
                addresses: vec!["doc.p1".into()],
                out: f.out(),
                raw: false,
                force: false,
            },
        )
        .expect("extracts");

        let path = f.out().join("doc.p1.Im0.png");
        assert_eq!(report.written, vec![path.clone()]);
        assert_eq!(png_header(&path), (IMAGE_W, IMAGE_H, 8, PNG_GREY));
    }

    #[test]
    fn raw_extraction_writes_the_bytes_that_are_in_the_file() {
        let f = Fixture::new();
        let archive = load_archive(f.root()).expect("loads");
        let report = scans::render::extract(
            &archive,
            &ExtractOptions {
                addresses: vec!["doc.p1".into()],
                out: f.out(),
                raw: true,
                force: false,
            },
        )
        .expect("extracts");

        // No filter on the fixture's image, so `.bin` and the samples verbatim.
        let path = f.out().join("doc.p1.Im0.bin");
        assert_eq!(report.written, vec![path.clone()]);
        assert_eq!(std::fs::read(&path).expect("read"), IMAGE);
    }

    #[test]
    fn a_page_outside_a_container_is_named_in_the_error() {
        let f = Fixture::new();
        std::fs::write(
            f.root().join("source/test/plate.toml"),
            "id = \"plate\"\n\
             layer = \"document\"\n\
             [[page]]\n\
             n = 1\n\
             [[page.graphic]]\n\
             file = \"vol.pdf\"\n",
        )
        .expect("write");
        let archive = load_archive(f.root()).expect("loads");
        let err = scans::render::render(
            &archive,
            &RenderOptions {
                addresses: vec!["plate".into()],
                ..render_opts(&f, Grid::WHOLE)
            },
        )
        .expect_err("must refuse");
        assert!(err.to_string().contains("plate.p1"), "{err}");
        assert!(err.to_string().contains("standalone image"), "{err}");
    }

    #[test]
    fn an_unknown_address_is_refused_before_anything_is_written() {
        let f = Fixture::new();
        let archive = load_archive(f.root()).expect("loads");
        let err = scans::render::render(
            &archive,
            &RenderOptions {
                addresses: vec!["doc".into(), "nope".into()],
                ..render_opts(&f, Grid::WHOLE)
            },
        )
        .expect_err("must refuse");
        assert!(err.to_string().contains("nope"), "{err}");
        assert!(!f.out().exists(), "the run wrote something before failing");
    }
}
