//! `scans` — the primary source and scan archive tool.
//!
//! The archive is a tree of hand-maintained TOML files describing sources, the physical
//! copies of them that were digitised, and the citable documents inside those copies. Its
//! product is stable, citable addresses of the form `<id>` and `<id>.p<n>`.
//!
//! # Layout
//!
//! | module | role |
//! |---|---|
//! | [`model`] | serde + schemars types. The single definition the schema is generated from. |
//! | [`edtf`] | the archive's EDTF subset, and the date comparison the validator needs. |
//! | [`load`] | discovery, id index, `of` chains, inheritance, page expansion, addressing. |
//! | [`validate`] | the ten checks. |
//! | [`migrate`] | the one-shot migration from the legacy layout. |
//! | [`ingest`] | recovering issue documents from a bound volume's PDF text layer. |
//! | [`render`] | rasterising a cited page, and extracting the images stored on it. |
//!
//! # Feature `probe`
//!
//! Off by default. Only `--probe` checks may open an image, a PDF, or a socket; the repo
//! holds several gigabytes of `.jp2` in Git LFS and the default `validate` path must never
//! touch them.

/// Path of the generated record schema, relative to the repository root.
pub const SCHEMA_PATH: &str = "schemas/source.json";

pub mod djvu;
pub mod edtf;
pub mod frc;
pub mod ingest;
pub mod load;
pub mod migrate;
pub mod model;
pub mod ocr;
pub mod render;
pub mod validate;

pub use load::{
    Archive, Diagnostic, Node, NodeId, RefTarget, Reference, Resolved, ResolvedGraphic,
    ResolvedPage, Severity, load_archive,
};
pub use model::{Layer, Record};
