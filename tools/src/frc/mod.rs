//! Ingesting the Newberry Library's French Revolution Collection.
//!
//! 38,377 Internet Archive items listed by [NewberryDIS/frc-data]. See
//! `docs/superpowers/specs/2026-08-07-frc-corpus-ingest-and-ocr-format-design.md`.
//!
//! [NewberryDIS/frc-data]: https://github.com/NewberryDIS/frc-data

pub mod fetch;
pub mod ingest;
pub mod limiter;
pub mod meta;
pub mod record;
