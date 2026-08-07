---
license: odbl
language:
  - fr
  - la
  - en
tags:
  - french-revolution
  - ocr
  - primary-sources
  - digital-humanities
pretty_name: Scans — primary sources of the French Revolution
size_categories:
  - 10K<n<100K
---

# scans

Scans of primary sources from the French Revolution.

The product of this repository is **stable, citable addresses**. Structured facts and
translations live elsewhere and cite into this one. Everything here exists to make those
addresses resolvable, unambiguous, and correct.

## Addresses

```
turgot-1739                     a record
turgot-1739.p0                  its first page — the atlas key sheet
journal-de-paris-1789-01-03.p1  page 1 of that day's issue
```

Ids are flat and never contain `/`, so **directories are cosmetic**: folders can be
reorganised without breaking a single citation. Ask the tool what an address points at:

```sh
cd tools && cargo run -- resolve turgot-1739.p0
```

## The model

Four layers, named after their TEI equivalents. `layer` says which; `type` says the genre
within it (`newspaper`, `map`, `issue`, `supplement`, `engraving`, …).

| `layer`    | what it is                                        | example                      |
| ---------- | ------------------------------------------------- | ---------------------------- |
| `source`   | the publication or work, independent of any copy  | `journal-de-paris`           |
| `copy`     | one physical object that was digitised            | `journal-de-paris-1789-vol1` |
| `document` | the citable intellectual unit                     | `journal-de-paris-1789-01-03`|
| `page`     | one scanned side, carrying graphics               | `…-01-03.p1`                 |

Layers may be skipped: `of` points at any ancestor, so a one-off engraving is a single
`document` file and does not pay four files of ceremony. A page is never a file of its own —
it is declared inline as `[[page]]` in the record that owns it.

A file states only what is new. `[rights]`, `[[resp]]`, `language`, `scan.file` and the rest
are inherited down the `of` chain; `date`, `title`, `note`, `url` and `[[page]]` are not.

## Layout

```
sources/
  frc/                                      the Newberry French Revolution Collection
    ab/abondance00unse/
      abondance00unse.toml                  source, one per Internet Archive item
      abondance00unse.p1.ocr.md             OCR, one file per page
      abondance00unse.pdf                   the scan
    …38,377 items across ~700 two-character shards
  journal-de-paris/
    journal-de-paris.toml                   source
    1789/
      journal-de-paris-1789-vol1.pdf
      journal-de-paris-1789-vol1.toml       copy
      journal-de-paris-1789-vol2.pdf
      journal-de-paris-1789-vol2.toml       copy
  turgot/
    turgot-1739.toml                        source, 21 sheets inline as [[page]]
    turgot_00.jp2 … turgot_20.jp2
  verniquet/
    verniquet-1795.toml                     source, 73 sheets inline
    verniquet_00.jp2 … verniquet_72.jp2
schemas/source.json                         GENERATED from tools/src/model.rs
schemas/ocr.json                            GENERATED from tools/src/ocr.rs
tools/                                      the `scans` CLI
docs/                                       the design spec
```

Binary assets are in Git LFS.

## The tool

`tools/` is a Rust crate producing the `scans` binary. One definition in
`tools/src/model.rs` drives three things at once: Rust type checking, TOML deserialisation,
and — via `schemars` — `schemas/source.json`. The schema is never hand-written, or it would
drift from the validator and start lying.

```sh
cd tools
cargo run -- validate            # the ten semantic checks
cargo run -- validate --strict   # warnings fail the build too
cargo run -- resolve <address>   # what does this citation point at?
cargo run -- schema --check      # has the schema drifted from the types?
cargo test
```

`validate` never opens an image by default — the archive holds several gigabytes of JPEG
2000. The checks that read bytes (real image dimensions, real PDF page counts) are behind an
optional feature:

```sh
cargo run --features probe -- validate --probe
```

## Pixels

Two commands turn a citation into an image. Both take an address, so nothing ever names a
page number inside a PDF by hand.

```sh
cargo run --release --features render -- render journal-de-paris-1789-01-03
cargo run --release --features render -- render journal-de-paris-1789-01-03.p1 --grid 2x2
cargo run --release --features render -- extract journal-de-paris-1789-01-03.p1
```

`render` rasterises the page. `extract` writes out the bitmaps stored on it, untouched.
Output goes to `.render/` at the repository root, which is gitignored: these are derived
files, reproducible at any time, and at native resolution they outweigh the PDFs.

* **Resolution is native by default** — the scan's own, taken from its largest embedded
  image. Journal de Paris pages come out around 3500x4900. `--dpi` overrides it.
* **`--grid RxC` cuts the page into tiles** with `--overlap` percent of slack on the interior
  edges. A whole page shown to a reader that samples it down to fit is illegible type; for
  this paper's two-column setting, `--grid 2x2` puts roughly one half-column in each tile.
* Both are behind the `render` feature, which is what pulls in the rasteriser. Build
  `--release`: a debug build is about thirty times slower, and it shows over 888 pages.

`hayro` does the decoding rather than `lopdf`, because every page image in volume 1 is JBIG2
and volume 2 adds JPEG 2000 plates. It is pure Rust, so there is no Ghostscript, pdfium or C
toolchain to install.

Each record carries a `#:schema` directive on its first line, which is what gets it validated
in an editor. See `schemas/README.md` — the alternative, an editor schema association, was
tested and silently does nothing.

## OCR

Word-level OCR lives in a sidecar beside the record it belongs to, one markdown file per
scanned page, reached through the `[[text]]` that points at it.

```markdown
---
of: 1789iemilseptcen00unse
page: 6
engine: ABBYY FineReader 11.0
lang: fr
w: 2597
h: 4418
dpi: 500
---

<the page's text>
```

Frontmatter says which page of what, and how big the image is. The body is the page. That is
the whole format.

**Scalars are bare unless they would lie.** `lang: fr`, not `lang: "fr"` — the frontmatter is
meant to read as YAML. But `no` is the ISO 639-1 code for Norwegian and, to every YAML 1.1
parser, the boolean false; a Python user loading that with PyYAML gets `False`. So a value is
written bare when it round-trips as itself and quoted when it would not, which keeps the
common case clean and the pathological case correct.

**What is deliberately not here:** word boxes, per-word confidence, baselines, and DjVu's
nesting. An earlier version carried all of it and could write the source XML back out
unchanged. The boxes were seven eighths of the bytes — 8.3 GB against about 1.2 GB for the
text — and were not being used. Nothing is destroyed by that: the coordinates are still in
`XML_for_OCR/` in [frc-data](https://github.com/NewberryDIS/frc-data), one clone away, so a
later version that wants them can have them without asking archive.org for anything.

```sh
cargo run -- ocr import x_djvu.xml --out ./pages
cargo run -- ocr check sources/frc/ab/abondance00unse
```

## The French Revolution Collection

38,377 Internet Archive items from the Newberry Library, listed by
[NewberryDIS/frc-data](https://github.com/NewberryDIS/frc-data). Records and OCR come from a
local clone of that repository; only the PDFs are fetched.

```sh
git clone --filter=blob:none --no-checkout https://github.com/NewberryDIS/frc-data.git
cd frc-data && git sparse-checkout set --cone Metadata XML_for_OCR && git checkout master

cargo run --release -- frc ingest --frc-data ../frc-data      # records + OCR, ~3 minutes
cargo run --release --features fetch -- frc fetch             # the PDFs, ~21 hours
```

`frc fetch` makes one request at a time, a second apart, with a `User-Agent` naming this
repository, and verifies every download against the size and MD5 archive.org publishes for
it. It resumes from what is already on disk, so it can be killed at any moment.

## Licence

The **database** — the records, the OCR sidecars, the structure and the addresses — is under
the [Open Database License](https://opendatacommons.org/licenses/odbl/) (ODbL 1.0). Use it,
adapt it, build on it; if you publish a derived database, share it alike and keep the
attribution.

The **contents are a separate question, and mostly not ours to license.** Every work here is
an 18th-century imprint or a 19th-century study of one, long out of copyright everywhere, and
the digitisations are published as public domain by the institutions that made them. Each
record says so for itself:

```toml
[rights]
work        = "PD-old-100-expired"
scan        = "PD"
attribution = "Digitised by the Internet Archive, sponsored by The Newberry Library."
```

So: quoting a pamphlet, reprinting a page image, or lifting one item's OCR carries no
obligation from us — that material is public domain and stays public domain. ODbL bites on
the *collection*: the selection, the identifiers, the metadata mapping and the structured OCR
taken together. That distinction is the point of using a database licence rather than a
content one, and it is why the per-record `[rights]` fields are not redundant with this
section.

Scans and metadata originate with the [Newberry Library](https://www.newberry.org/) and the
[Internet Archive](https://archive.org/), and the item list comes from
[NewberryDIS/frc-data](https://github.com/NewberryDIS/frc-data).

## Design

`docs/superpowers/specs/2026-08-05-source-archive-schema-design.md` is the authority: the
four layers, the inheritance allowlist, EDTF dates, and the ten validation checks, with the
reasoning for each.
