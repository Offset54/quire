//! PDF engine host (MASTER_PROMPT §5.2, §6.6): PDFium runs only in worker
//! processes (ADR-0035, ADR-0036). This crate holds both sides of their
//! protocol: the host's pool and the worker's request loop.

pub mod cpu;
mod ffi;
pub mod pool;
pub mod protocol;
pub mod tiles;
pub mod worker;

pub use pool::{DocInfo, PdfError, Pool, PoolConfig, PoolStats, WorkerCommand};
