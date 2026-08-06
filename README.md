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
source/
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

Each record carries a `#:schema` directive on its first line, which is what gets it validated
in an editor. See `schemas/README.md` — the alternative, an editor schema association, was
tested and silently does nothing.

## Design

`docs/superpowers/specs/2026-08-05-source-archive-schema-design.md` is the authority: the
four layers, the inheritance allowlist, EDTF dates, and the ten validation checks, with the
reasoning for each.
