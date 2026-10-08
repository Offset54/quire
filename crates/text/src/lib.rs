//! The text engine (MASTER_PROMPT §6.1): byte sources, encodings, the line
//! index, the piece tree, editing, search, saving and the recovery journal.

pub mod buffer;
pub mod encoding;
pub mod index;
pub mod source;
pub mod transcode;
pub mod tree;
