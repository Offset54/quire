//! The text engine (MASTER_PROMPT §6.1): byte sources, encodings, the line
//! index, the piece tree, editing, search, saving and the recovery journal.

pub mod buffer;
pub mod chars;
pub mod document;
pub mod encoding;
pub mod history;
pub mod index;
pub mod layout;
pub mod source;
pub mod transcode;
pub mod tree;
