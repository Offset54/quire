//! The Markdown engine (MASTER_PROMPT §6.2).
//!
//! The text buffer (`quire-text`) is the source of truth. This crate keeps a
//! top-level block index over it, built viewport-first and completed in the
//! background, and re-parses only the blocks an edit touches, widened until
//! block boundaries line up again (ADR-0027). Each block is parsed and
//! rendered on its own with comrak (ADR-0026); reference definitions and
//! footnotes, which can be defined anywhere, live in document-wide tables.

pub mod analysis;
pub mod document;
pub mod images;
pub mod index;
pub mod options;
pub mod render;
pub mod sanitize;
pub mod scan;
pub mod tables;
pub mod text;
