//! The text engine (MASTER_PROMPT §6.1): byte sources, encodings, the line
//! index, the piece tree, editing, search, saving and the recovery journal.

pub mod buffer;
pub mod chars;
pub mod document;
pub mod encoding;
pub mod file;
pub mod history;
pub mod index;
pub mod journal;
pub mod layout;
pub mod replace;
pub mod search;
pub mod source;
pub mod transcode;
pub mod tree;
pub mod watch;
