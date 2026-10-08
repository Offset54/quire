//! Priority job scheduler (MASTER_PROMPT §5.2; M0 task 5).
//!
//! - Five priorities, highest first: [`Priority::Interaction`] →
//!   [`Priority::Visible`] → [`Priority::NearViewport`] → [`Priority::Prefetch`]
//!   → [`Priority::Background`]. FIFO within a priority.
//! - Every job has a [`CancelToken`]. A job may also be tied to a
//!   [`Generation`] (for example, one per view): bumping it on scroll or zoom
//!   makes older jobs stale, and stale jobs are dropped before they run.
//! - Jobs with the same [`JobKey`] that are still queued are coalesced: the
//!   pending job takes the highest requested priority and the newest
//!   generation, and every requester gets the same result.
//! - Some workers are reserved for `Interaction` and `Visible` work, so a
//!   long background job can never delay a keystroke or a visible tile.
//! - A panicking job completes with [`JobError::Panicked`]; the worker
//!   survives (§8).

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde::Serialize;

/// Job priority, highest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Priority {
    /// Edit relayout, hit-testing: work the user is waiting on right now.
    Interaction,
    /// Content in the viewport.
    Visible,
    /// Content just outside the viewport.
    NearViewport,
    /// Content likely to be needed soon.
    Prefetch,
    /// Indexing, pagination, thumbnails.
    Background,
}

impl Priority {
    pub const ALL: [Priority; 5] = [
        Priority::Interaction,
        Priority::Visible,
        Priority::NearViewport,
        Priority::Prefetch,
        Priority::Background,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// Whether reserved workers may run jobs at this priority.
    pub fn is_urgent(self) -> bool {
        self <= Priority::Visible
    }
}

/// Shared cancellation flag.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// A generation counter, typically one per view. Jobs submitted under an older
/// generation are stale once it's bumped.
#[derive(Clone, Debug, Default)]
pub struct Generation(Arc<AtomicU64>);

impl Generation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    /// Advances the generation and returns the new value.
    pub fn bump(&self) -> u64 {
        self.0.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn same_counter(&self, other: &Generation) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Identity of a job's output, used to coalesce duplicate requests.
/// `parts` holds exact identifiers (document, unit, revision, zoom bucket,
/// tile...), never a hash, so different work can't collide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobKey {
    pub kind: &'static str,
    pub parts: [u64; 4],
}

/// Why a job produced no value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobError {
    /// Cancelled through its token, before or while running.
    Cancelled,
    /// Its generation moved on before it ran.
    Stale,
    /// The job panicked; the message is kept for diagnostics.
    Panicked(String),
    /// The scheduler shut down before the job ran.
    Shutdown,
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobError::Cancelled => f.write_str("job cancelled"),
            JobError::Stale => f.write_str("job stale"),
            JobError::Panicked(msg) => write!(f, "job panicked: {msg}"),
            JobError::Shutdown => f.write_str("scheduler shut down"),
        }
    }
}

impl std::error::Error for JobError {}

/// Returned by [`JobContext::checkpoint`] when the job should stop early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interrupted;

/// Passed to a running job so it can stop cooperatively.
#[derive(Debug)]
pub struct JobContext {
    token: CancelToken,
    generation: Option<(Generation, u64)>,
}

impl JobContext {
    fn is_stale(&self) -> bool {
        self.generation
            .as_ref()
            .is_some_and(|(g, at)| g.current() != *at)
    }

    /// True if the job was cancelled or its generation moved on.
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled() || self.is_stale()
    }

    /// Call between chunks of long work: `ctx.checkpoint()?;`
    pub fn checkpoint(&self) -> Result<(), Interrupted> {
        if self.is_cancelled() {
            Err(Interrupted)
        } else {
            Ok(())
        }
    }
}

/// What to run and how to schedule it.
#[derive(Debug, Clone)]
pub struct JobSpec {
    name: &'static str,
    priority: Priority,
    key: Option<JobKey>,
    generation: Option<Generation>,
}

impl JobSpec {
    pub fn new(name: &'static str, priority: Priority) -> Self {
        Self {
            name,
            priority,
            key: None,
            generation: None,
        }
    }

    /// Coalesce with queued jobs that have the same key.
    pub fn key(mut self, key: JobKey) -> Self {
        self.key = Some(key);
        self
    }

    /// Tie the job to the generation's current value.
    pub fn generation(mut self, generation: &Generation) -> Self {
        self.generation = Some(generation.clone());
        self
    }
}

struct JobState<T> {
    result: Mutex<Option<Result<Arc<T>, JobError>>>,
    done: Condvar,
    token: CancelToken,
    /// Handles that still want the result; the job is cancelled at zero.
    interest: AtomicUsize,
}

impl<T> JobState<T> {
    fn complete(&self, result: Result<Arc<T>, JobError>) {
        let mut slot = self.result.lock();
        if slot.is_none() {
            *slot = Some(result);
            self.done.notify_all();
        }
    }
}

/// A requester's view of a job. Several handles can share one job when
/// requests are coalesced.
pub struct JobHandle<T> {
    state: Arc<JobState<T>>,
    withdrawn: AtomicBool,
}

impl<T> fmt::Debug for JobHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobHandle")
            .field("done", &self.state.result.lock().is_some())
            .finish()
    }
}

impl<T> JobHandle<T> {
    fn new(state: Arc<JobState<T>>) -> Self {
        Self {
            state,
            withdrawn: AtomicBool::new(false),
        }
    }

    /// Withdraws this handle's interest. The job is cancelled once every
    /// handle sharing it has cancelled. Dropping a handle does not cancel.
    pub fn cancel(&self) {
        if !self.withdrawn.swap(true, Ordering::AcqRel)
            && self.state.interest.fetch_sub(1, Ordering::AcqRel) == 1
        {
            self.state.token.cancel();
        }
    }

    /// The result if the job has finished.
    pub fn try_get(&self) -> Option<Result<Arc<T>, JobError>> {
        self.state.result.lock().clone()
    }

    /// Blocks until the job finishes.
    pub fn wait(&self) -> Result<Arc<T>, JobError> {
        let mut slot = self.state.result.lock();
        loop {
            if let Some(result) = slot.as_ref() {
                return result.clone();
            }
            self.state.done.wait(&mut slot);
        }
    }

    /// Blocks until the job finishes or the timeout passes.
    pub fn wait_timeout(&self, timeout: Duration) -> Option<Result<Arc<T>, JobError>> {
        let deadline = Instant::now() + timeout;
        let mut slot = self.state.result.lock();
        while slot.is_none() {
            if self.state.done.wait_until(&mut slot, deadline).timed_out() {
                break;
            }
        }
        slot.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Completed,
    Cancelled,
    Stale,
    Panicked,
}

/// One finished job, for the performance HUD.
#[derive(Clone, Debug, Serialize)]
pub struct JobRecord {
    pub name: &'static str,
    pub priority: Priority,
    pub queued_us: u64,
    pub run_us: u64,
    pub outcome: Outcome,
}

/// Counters for the performance HUD.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SchedulerStats {
    /// Live queued jobs per priority, in [`Priority::ALL`] order.
    pub queued: [usize; 5],
    pub running: usize,
    pub completed: u64,
    pub cancelled: u64,
    pub stale_dropped: u64,
    pub coalesced: u64,
    pub panicked: u64,
}

#[derive(Clone, Debug)]
pub struct SchedulerConfig {
    /// Worker threads in total.
    pub threads: usize,
    /// Workers that only run `Interaction` and `Visible` jobs.
    pub urgent_reserved: usize,
    /// How many finished jobs [`Scheduler::recent_jobs`] keeps.
    pub history: usize,
    pub thread_name: &'static str,
}

impl Default for SchedulerConfig {
    /// One worker per logical CPU minus one (the UI and IPC threads need a
    /// core). `QUIRE_CPU_LOGICAL` overrides the CPU count, so benchmarks can
    /// size the pool for an emulated machine (ADR-0021).
    fn default() -> Self {
        let cores = std::env::var("QUIRE_CPU_LOGICAL")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| n.get()));
        Self {
            threads: cores.saturating_sub(1).max(2),
            urgent_reserved: 1,
            history: 64,
            thread_name: "quire-worker",
        }
    }
}

type RunFn = Box<dyn FnOnce(&JobContext) -> Outcome + Send>;
type AbortFn = Box<dyn FnOnce(JobError) + Send>;

struct Entry {
    name: &'static str,
    priority: Priority,
    key: Option<JobKey>,
    generation: Option<(Generation, u64)>,
    token: CancelToken,
    run: RunFn,
    abort: AbortFn,
    /// The typed `Arc<JobState<T>>`, for coalescing.
    state: Arc<dyn Any + Send + Sync>,
    enqueued_at: Instant,
}

#[derive(Default)]
struct State {
    next_id: u64,
    /// Queued job ids per priority. Ids can be stale (job upgraded or
    /// removed); `entries` is authoritative.
    queues: [VecDeque<u64>; 5],
    entries: HashMap<u64, Entry>,
    by_key: HashMap<JobKey, u64>,
    stats: SchedulerStats,
    history: VecDeque<JobRecord>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    work: Condvar,
    history_len: usize,
}

/// Runs jobs on a pool of worker threads. Dropping it stops the workers;
/// jobs still queued complete with [`JobError::Shutdown`].
pub struct Scheduler {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scheduler")
            .field("workers", &self.workers.len())
            .finish()
    }
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        let threads = config.threads.max(1);
        // At least one worker must accept every priority.
        let urgent = config.urgent_reserved.min(threads - 1);
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            work: Condvar::new(),
            history_len: config.history,
        });
        let workers = (0..threads)
            .map(|i| {
                let shared = Arc::clone(&shared);
                let urgent_only = i < urgent;
                let suffix = if urgent_only { "-urgent" } else { "" };
                std::thread::Builder::new()
                    .name(format!("{}-{i}{suffix}", config.thread_name))
                    .spawn(move || worker_loop(&shared, urgent_only))
                    .expect("failed to spawn scheduler worker")
            })
            .collect();
        Self { shared, workers }
    }

    /// Queues a job. If a job with the same key is still queued, the request
    /// is coalesced into it and `f` is dropped unused.
    pub fn submit<T, F>(&self, spec: JobSpec, f: F) -> JobHandle<T>
    where
        T: Send + Sync + 'static,
        F: FnOnce(&JobContext) -> Result<T, Interrupted> + Send + 'static,
    {
        let generation = spec.generation.as_ref().map(|g| (g.clone(), g.current()));
        let mut st = self.shared.state.lock();

        if let Some(handle) = coalesce::<T>(&mut st, &spec, generation.as_ref()) {
            return handle;
        }

        let token = CancelToken::new();
        let job: Arc<JobState<T>> = Arc::new(JobState {
            result: Mutex::new(None),
            done: Condvar::new(),
            token: token.clone(),
            interest: AtomicUsize::new(1),
        });
        if st.shutdown {
            job.complete(Err(JobError::Shutdown));
            return JobHandle::new(job);
        }

        let run_state = Arc::clone(&job);
        let run: RunFn = Box::new(move |ctx| {
            match panic::catch_unwind(AssertUnwindSafe(|| f(ctx))) {
                Ok(Ok(value)) => {
                    run_state.complete(Ok(Arc::new(value)));
                    Outcome::Completed
                }
                Ok(Err(Interrupted)) => {
                    // Stopped at a checkpoint: report why.
                    let (err, outcome) = if ctx.token.is_cancelled() {
                        (JobError::Cancelled, Outcome::Cancelled)
                    } else {
                        (JobError::Stale, Outcome::Stale)
                    };
                    run_state.complete(Err(err));
                    outcome
                }
                Err(payload) => {
                    run_state.complete(Err(JobError::Panicked(panic_message(&*payload))));
                    Outcome::Panicked
                }
            }
        });
        let abort_state = Arc::clone(&job);
        let abort: AbortFn = Box::new(move |err| abort_state.complete(Err(err)));

        let id = st.next_id;
        st.next_id += 1;
        if let Some(key) = spec.key {
            st.by_key.insert(key, id);
        }
        st.queues[spec.priority.index()].push_back(id);
        st.stats.queued[spec.priority.index()] += 1;
        let erased: Arc<dyn Any + Send + Sync> = Arc::clone(&job) as _;
        st.entries.insert(
            id,
            Entry {
                name: spec.name,
                priority: spec.priority,
                key: spec.key,
                generation,
                token,
                run,
                abort,
                state: erased,
                enqueued_at: Instant::now(),
            },
        );
        drop(st);
        // Wake everyone: a reserved worker can't take a background job, so
        // waking only one could leave the job waiting.
        self.shared.work.notify_all();
        JobHandle::new(job)
    }

    pub fn stats(&self) -> SchedulerStats {
        self.shared.state.lock().stats.clone()
    }

    /// The most recently finished jobs, oldest first.
    pub fn recent_jobs(&self) -> Vec<JobRecord> {
        self.shared.state.lock().history.iter().cloned().collect()
    }
}

/// Merges a request into a queued job with the same key, if there is one.
fn coalesce<T: Send + Sync + 'static>(
    st: &mut State,
    spec: &JobSpec,
    generation: Option<&(Generation, u64)>,
) -> Option<JobHandle<T>> {
    let key = spec.key?;
    let id = *st.by_key.get(&key)?;
    let entry = st.entries.get_mut(&id)?;
    let job = Arc::clone(&entry.state).downcast::<JobState<T>>().ok()?;
    if entry.token.is_cancelled() {
        return None;
    }

    if spec.priority < entry.priority {
        st.stats.queued[entry.priority.index()] -= 1;
        st.stats.queued[spec.priority.index()] += 1;
        entry.priority = spec.priority;
        st.queues[spec.priority.index()].push_back(id);
    }
    match (generation, &mut entry.generation) {
        // The newest requester's view decides whether the job is stale.
        (Some(new), Some(old)) if new.0.same_counter(&old.0) => old.1 = old.1.max(new.1),
        (Some(new), slot) => *slot = Some(new.clone()),
        // A requester without a generation needs the result regardless.
        (None, slot) => *slot = None,
    }
    job.interest.fetch_add(1, Ordering::AcqRel);
    st.stats.coalesced += 1;
    Some(JobHandle::new(job))
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

/// Takes the highest-priority live job this worker may run.
fn pop(st: &mut State, urgent_only: bool) -> Option<Entry> {
    for priority in Priority::ALL {
        if urgent_only && !priority.is_urgent() {
            break;
        }
        while let Some(id) = st.queues[priority.index()].pop_front() {
            match st.entries.get(&id) {
                Some(e) if e.priority == priority => {}
                // Upgraded to another queue, or already gone.
                _ => continue,
            }
            let entry = st.entries.remove(&id)?;
            if let Some(key) = entry.key
                && st.by_key.get(&key) == Some(&id)
            {
                st.by_key.remove(&key);
            }
            st.stats.queued[priority.index()] -= 1;
            return Some(entry);
        }
    }
    None
}

fn worker_loop(shared: &Shared, urgent_only: bool) {
    loop {
        let entry = {
            let mut st = shared.state.lock();
            loop {
                if st.shutdown {
                    return;
                }
                if let Some(entry) = pop(&mut st, urgent_only) {
                    st.stats.running += 1;
                    break entry;
                }
                shared.work.wait(&mut st);
            }
        };

        let started = Instant::now();
        let queued_us = (started - entry.enqueued_at).as_micros() as u64;
        let ctx = JobContext {
            token: entry.token,
            generation: entry.generation,
        };
        let outcome = if ctx.token.is_cancelled() {
            (entry.abort)(JobError::Cancelled);
            Outcome::Cancelled
        } else if ctx.is_stale() {
            (entry.abort)(JobError::Stale);
            Outcome::Stale
        } else {
            let _span = tracing::trace_span!(
                "job",
                name = entry.name,
                priority = ?entry.priority,
                queued_us
            )
            .entered();
            (entry.run)(&ctx)
        };
        let run_us = started.elapsed().as_micros() as u64;

        let mut st = shared.state.lock();
        st.stats.running -= 1;
        match outcome {
            Outcome::Completed => st.stats.completed += 1,
            Outcome::Cancelled => st.stats.cancelled += 1,
            Outcome::Stale => st.stats.stale_dropped += 1,
            Outcome::Panicked => st.stats.panicked += 1,
        }
        if shared.history_len > 0 {
            if st.history.len() == shared.history_len {
                st.history.pop_front();
            }
            st.history.push_back(JobRecord {
                name: entry.name,
                priority: entry.priority,
                queued_us,
                run_us,
                outcome,
            });
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        let pending: Vec<Entry> = {
            let mut st = self.shared.state.lock();
            st.shutdown = true;
            st.queues.iter_mut().for_each(VecDeque::clear);
            st.by_key.clear();
            st.entries.drain().map(|(_, e)| e).collect()
        };
        self.shared.work.notify_all();
        for entry in pending {
            (entry.abort)(JobError::Shutdown);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests;
