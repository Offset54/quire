//! Logging and trace recording for the desktop host.
//!
//! - `RUST_LOG` filters log output (default: `info`).
//! - `QUIRE_TRACE=<path>` records a Chrome trace and writes it on exit.
//! - `QUIRE_TRACE_FILTER` filters what the trace records (default: `info,quire=trace`).

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use quire_core::trace::TraceRecorder;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Enough for several minutes of dense tracing; older events are dropped.
const TRACE_CAPACITY: usize = 1_000_000;

pub struct Tracing {
    pub recorder: Option<TraceRecorder>,
    path: Option<PathBuf>,
}

pub fn init() -> Tracing {
    let path = std::env::var_os("QUIRE_TRACE").map(PathBuf::from);
    let recorder = path.as_ref().map(|_| TraceRecorder::new(TRACE_CAPACITY));

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter);
    tracing_subscriber::registry()
        .with(fmt)
        .with(recorder.as_ref().map(|r| {
            let filter =
                std::env::var("QUIRE_TRACE_FILTER").unwrap_or_else(|_| "info,quire=trace".into());
            r.layer().with_filter(EnvFilter::new(filter))
        }))
        .init();

    Tracing { recorder, path }
}

impl Tracing {
    pub fn write_trace(&self) {
        let (Some(recorder), Some(path)) = (&self.recorder, &self.path) else {
            return;
        };
        let result = File::create(path).and_then(|f| recorder.write_json(BufWriter::new(f)));
        match result {
            Ok(()) => tracing::info!(
                path = %path.display(),
                events = recorder.len(),
                dropped = recorder.dropped(),
                "trace written"
            ),
            Err(err) => tracing::error!(path = %path.display(), %err, "failed to write trace"),
        }
    }
}
