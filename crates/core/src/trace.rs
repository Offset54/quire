//! Chrome-trace recording (MASTER_PROMPT §5.1, §10; M0 task 8).
//!
//! [`ChromeTraceLayer`] turns `tracing` spans into complete (`"X"`) events and
//! `tracing` events into instant (`"i"`) events. The frontend's own timings
//! (performance measures, long tasks) are merged into the same timeline with
//! [`TraceRecorder::add_external`], which converts wall-clock timestamps to the
//! trace's time base. The result opens in `chrome://tracing` or Perfetto.
//!
//! The buffer is bounded: once it holds `capacity` events, the oldest are
//! dropped and counted, so tracing can stay on for long sessions.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

/// Thread id used for events that come from the frontend (webview) process.
pub const FRONTEND_TID: u64 = 0;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TraceEvent {
    pub name: Cow<'static, str>,
    pub cat: Cow<'static, str>,
    /// Chrome trace phase: `X` complete, `i` instant, `M` metadata.
    pub ph: char,
    /// Microseconds since the recorder started.
    pub ts: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
    pub pid: u32,
    pub tid: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
}

/// A timing recorded outside this process, in wall-clock milliseconds
/// (for the webview: `performance.timeOrigin + entry.startTime`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEvent {
    pub name: String,
    pub category: String,
    pub start_epoch_ms: f64,
    /// `None` records an instant event.
    pub duration_ms: Option<f64>,
}

#[derive(Debug)]
struct Buffer {
    events: VecDeque<TraceEvent>,
    dropped: u64,
}

/// Shared, bounded store of trace events. Cheap to clone.
#[derive(Debug, Clone)]
pub struct TraceRecorder {
    inner: Arc<RecorderInner>,
}

#[derive(Debug)]
struct RecorderInner {
    start: Instant,
    start_epoch_us: f64,
    capacity: usize,
    buffer: Mutex<Buffer>,
    thread_names: Mutex<Vec<(u64, String)>>,
}

impl TraceRecorder {
    pub fn new(capacity: usize) -> Self {
        let start_epoch_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1e6);
        Self {
            inner: Arc::new(RecorderInner {
                start: Instant::now(),
                start_epoch_us,
                capacity: capacity.max(1),
                buffer: Mutex::new(Buffer {
                    events: VecDeque::new(),
                    dropped: 0,
                }),
                thread_names: Mutex::new(vec![(FRONTEND_TID, "webview".to_owned())]),
            }),
        }
    }

    /// The `tracing` layer that feeds this recorder.
    pub fn layer(&self) -> ChromeTraceLayer {
        ChromeTraceLayer {
            recorder: self.clone(),
        }
    }

    fn now_us(&self) -> f64 {
        self.inner.start.elapsed().as_secs_f64() * 1e6
    }

    fn push(&self, event: TraceEvent) {
        let mut buf = self.inner.buffer.lock();
        if buf.events.len() == self.inner.capacity {
            buf.events.pop_front();
            buf.dropped += 1;
        }
        buf.events.push_back(event);
    }

    /// Merges timings from another process into this trace.
    pub fn add_external(&self, events: &[ExternalEvent]) {
        for e in events {
            let ts = e.start_epoch_ms * 1e3 - self.inner.start_epoch_us;
            self.push(TraceEvent {
                name: Cow::Owned(e.name.clone()),
                cat: Cow::Owned(e.category.clone()),
                ph: if e.duration_ms.is_some() { 'X' } else { 'i' },
                ts,
                dur: e.duration_ms.map(|d| d * 1e3),
                pid: std::process::id(),
                tid: FRONTEND_TID,
                args: None,
            });
        }
    }

    pub fn len(&self) -> usize {
        self.inner.buffer.lock().events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Events dropped because the buffer was full.
    pub fn dropped(&self) -> u64 {
        self.inner.buffer.lock().dropped
    }

    pub fn snapshot(&self) -> Vec<TraceEvent> {
        self.inner.buffer.lock().events.iter().cloned().collect()
    }

    /// Writes the trace in Chrome's JSON object format.
    pub fn write_json<W: io::Write>(&self, mut out: W) -> io::Result<()> {
        let pid = std::process::id();
        let mut events: Vec<TraceEvent> = self
            .inner
            .thread_names
            .lock()
            .iter()
            .map(|(tid, name)| TraceEvent {
                name: Cow::Borrowed("thread_name"),
                cat: Cow::Borrowed("__metadata"),
                ph: 'M',
                ts: 0.0,
                dur: None,
                pid,
                tid: *tid,
                args: Some(serde_json::json!({ "name": name })),
            })
            .collect();
        let dropped = {
            let buf = self.inner.buffer.lock();
            events.extend(buf.events.iter().cloned());
            buf.dropped
        };
        let doc = serde_json::json!({
            "traceEvents": events,
            "displayTimeUnit": "ms",
            "otherData": { "droppedEvents": dropped },
        });
        serde_json::to_writer(&mut out, &doc)?;
        out.flush()
    }

    fn current_tid(&self) -> u64 {
        static NEXT_TID: AtomicU64 = AtomicU64::new(FRONTEND_TID + 1);
        thread_local! {
            static TID: u64 = NEXT_TID.fetch_add(1, Ordering::Relaxed);
        }
        thread_local! {
            // Recorders this thread has already registered its name with.
            static REGISTERED: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        let tid = TID.with(|t| *t);
        let key = Arc::as_ptr(&self.inner) as usize;
        let first_time = REGISTERED.with(|r| {
            let mut r = r.borrow_mut();
            let new = !r.contains(&key);
            if new {
                r.push(key);
            }
            new
        });
        if first_time {
            let thread = std::thread::current();
            let name = thread
                .name()
                .map_or_else(|| format!("thread {tid}"), str::to_owned);
            self.inner.thread_names.lock().push((tid, name));
        }
        tid
    }
}

/// Span start data, stored in the span's extensions until it closes.
struct SpanTiming {
    start_us: f64,
    tid: u64,
    args: Option<serde_json::Value>,
}

/// `tracing` layer that records into a [`TraceRecorder`].
#[derive(Debug, Clone)]
pub struct ChromeTraceLayer {
    recorder: TraceRecorder,
}

impl<S> Layer<S> for ChromeTraceLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut fields = FieldCollector::default();
        attrs.record(&mut fields);
        span.extensions_mut().insert(SpanTiming {
            start_us: self.recorder.now_us(),
            tid: self.recorder.current_tid(),
            args: fields.into_args(),
        });
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else { return };
        let Some(timing) = span.extensions_mut().remove::<SpanTiming>() else {
            return;
        };
        let meta = span.metadata();
        let end = self.recorder.now_us();
        self.recorder.push(TraceEvent {
            name: Cow::Borrowed(meta.name()),
            cat: Cow::Borrowed(meta.target()),
            ph: 'X',
            ts: timing.start_us,
            dur: Some(end - timing.start_us),
            pid: std::process::id(),
            tid: timing.tid,
            args: timing.args,
        });
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = FieldCollector::default();
        event.record(&mut fields);
        let meta = event.metadata();
        let name = fields
            .message
            .take()
            .map_or(Cow::Borrowed(meta.name()), Cow::Owned);
        self.recorder.push(TraceEvent {
            name,
            cat: Cow::Borrowed(meta.target()),
            ph: 'i',
            ts: self.recorder.now_us(),
            dur: None,
            pid: std::process::id(),
            tid: self.recorder.current_tid(),
            args: fields.into_args(),
        });
    }
}

#[derive(Default)]
struct FieldCollector {
    message: Option<String>,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl FieldCollector {
    fn into_args(self) -> Option<serde_json::Value> {
        (!self.fields.is_empty()).then_some(serde_json::Value::Object(self.fields))
    }
}

impl Visit for FieldCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let mut s = String::new();
        let _ = write!(s, "{value:?}");
        if field.name() == "message" {
            self.message = Some(s);
        } else {
            self.fields.insert(field.name().to_owned(), s.into());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_owned());
        } else {
            self.fields.insert(field.name().to_owned(), value.into());
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.insert(field.name().to_owned(), value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.insert(field.name().to_owned(), value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields.insert(field.name().to_owned(), value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields.insert(field.name().to_owned(), value.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    fn with_recorder(capacity: usize, f: impl FnOnce()) -> TraceRecorder {
        let recorder = TraceRecorder::new(capacity);
        let subscriber = tracing_subscriber::registry().with(recorder.layer());
        tracing::subscriber::with_default(subscriber, f);
        recorder
    }

    #[test]
    fn nested_spans_become_complete_events_with_containment() {
        let rec = with_recorder(100, || {
            let outer = tracing::info_span!("outer", unit = 7u64);
            let _o = outer.enter();
            {
                let inner = tracing::info_span!("inner");
                let _i = inner.enter();
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        let events = rec.snapshot();
        let inner = events.iter().find(|e| e.name == "inner").unwrap();
        let outer = events.iter().find(|e| e.name == "outer").unwrap();
        assert_eq!((inner.ph, outer.ph), ('X', 'X'));
        assert!(inner.dur.unwrap() >= 2000.0);
        assert!(outer.ts <= inner.ts);
        assert!(outer.ts + outer.dur.unwrap() >= inner.ts + inner.dur.unwrap());
        assert_eq!(outer.args, Some(serde_json::json!({ "unit": 7 })));
    }

    #[test]
    fn events_are_instants_named_by_message() {
        let rec = with_recorder(100, || tracing::warn!(ms = 61u64, "long task"));
        let e = &rec.snapshot()[0];
        assert_eq!((e.ph, e.name.as_ref()), ('i', "long task"));
        assert_eq!(e.args, Some(serde_json::json!({ "ms": 61 })));
    }

    #[test]
    fn buffer_is_bounded_and_counts_drops() {
        let rec = with_recorder(3, || {
            for i in 0..5u64 {
                tracing::info!(i, "tick");
            }
        });
        assert_eq!(rec.len(), 3);
        assert_eq!(rec.dropped(), 2);
        let first = rec.snapshot()[0].args.clone().unwrap();
        assert_eq!(first["i"], 2, "oldest events are dropped first");
    }

    #[test]
    fn external_events_are_rebased_to_trace_time() {
        let rec = TraceRecorder::new(10);
        let start_ms = rec.inner.start_epoch_us / 1e3;
        rec.add_external(&[
            ExternalEvent {
                name: "frame".into(),
                category: "frontend".into(),
                start_epoch_ms: start_ms + 5.0,
                duration_ms: Some(1.5),
            },
            ExternalEvent {
                name: "longtask".into(),
                category: "frontend".into(),
                start_epoch_ms: start_ms + 9.0,
                duration_ms: None,
            },
        ]);
        let events = rec.snapshot();
        assert!((events[0].ts - 5000.0).abs() < 1e-3);
        assert_eq!(events[0].dur, Some(1500.0));
        assert_eq!((events[1].ph, events[1].tid), ('i', FRONTEND_TID));
    }

    #[test]
    fn json_output_is_a_valid_chrome_trace() {
        let rec = with_recorder(10, || {
            let _s = tracing::info_span!("work").entered();
        });
        let mut out = Vec::new();
        rec.write_json(&mut out).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let events = v["traceEvents"].as_array().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e["ph"] == "M" && e["args"]["name"] == "webview")
        );
        assert!(events.iter().any(|e| e["ph"] == "X" && e["name"] == "work"));
        assert_eq!(v["otherData"]["droppedEvents"], 0);
    }
}
