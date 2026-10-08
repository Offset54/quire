use super::*;
use proptest::prelude::*;
use std::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(10);

fn scheduler(threads: usize, urgent_reserved: usize) -> Scheduler {
    Scheduler::new(SchedulerConfig {
        threads,
        urgent_reserved,
        history: 16,
        thread_name: "test-worker",
    })
}

/// Occupies one worker until the returned sender is dropped or sent to.
fn block_worker(s: &Scheduler, priority: Priority) -> (mpsc::Sender<()>, JobHandle<()>) {
    let (release, wait) = mpsc::channel::<()>();
    let (started_tx, started_rx) = mpsc::channel();
    let handle = s.submit(JobSpec::new("gate", priority), move |_| {
        started_tx.send(()).unwrap();
        let _ = wait.recv();
        Ok(())
    });
    started_rx
        .recv_timeout(TIMEOUT)
        .expect("gate job never started");
    (release, handle)
}

/// Stats are updated just after a job's result is published, so wait for them.
fn wait_stats(s: &Scheduler, check: impl Fn(&SchedulerStats) -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !check(&s.stats()) {
        assert!(
            Instant::now() < deadline,
            "stats never matched: {:?}",
            s.stats()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn get<T: Send + Sync + 'static>(h: &JobHandle<T>) -> Result<Arc<T>, JobError> {
    h.wait_timeout(TIMEOUT).expect("job did not finish in time")
}

#[test]
fn runs_highest_priority_first_and_fifo_within_a_priority() {
    let s = scheduler(1, 0);
    let (release, gate) = block_worker(&s, Priority::Background);
    let order = Arc::new(Mutex::new(Vec::new()));
    let submit = |name: &'static str, p| {
        let order = Arc::clone(&order);
        s.submit(JobSpec::new(name, p), move |_| {
            order.lock().push(name);
            Ok(())
        })
    };
    let handles = [
        submit("bg", Priority::Background),
        submit("prefetch", Priority::Prefetch),
        submit("visible-1", Priority::Visible),
        submit("interaction", Priority::Interaction),
        submit("visible-2", Priority::Visible),
        submit("near", Priority::NearViewport),
    ];
    assert_eq!(s.stats().queued, [1, 2, 1, 1, 1]);
    release.send(()).unwrap();
    get(&gate).unwrap();
    for h in &handles {
        get(h).unwrap();
    }
    assert_eq!(
        *order.lock(),
        [
            "interaction",
            "visible-1",
            "visible-2",
            "near",
            "prefetch",
            "bg"
        ]
    );
}

#[test]
fn reserved_worker_runs_urgent_jobs_while_background_work_hogs_the_pool() {
    let s = scheduler(2, 1);
    let (release, gate) = block_worker(&s, Priority::Background);
    // The general worker is busy; another background job must wait...
    let bg = s.submit(JobSpec::new("bg", Priority::Background), |_| Ok(1));
    assert!(bg.wait_timeout(Duration::from_millis(50)).is_none());
    // ...but an interaction job runs at once on the reserved worker.
    let urgent = s.submit(JobSpec::new("key", Priority::Interaction), |_| Ok(2));
    assert_eq!(*get(&urgent).unwrap(), 2);
    release.send(()).unwrap();
    get(&gate).unwrap();
    assert_eq!(*get(&bg).unwrap(), 1);
}

#[test]
fn stale_jobs_are_dropped_without_running() {
    let s = scheduler(1, 0);
    let view = Generation::new();
    let (release, gate) = block_worker(&s, Priority::Visible);
    let ran = Arc::new(AtomicBool::new(false));
    let ran2 = Arc::clone(&ran);
    let old = s.submit(
        JobSpec::new("tile", Priority::Visible).generation(&view),
        move |_| {
            ran2.store(true, Ordering::SeqCst);
            Ok(())
        },
    );
    view.bump(); // the user scrolled
    let fresh = s.submit(
        JobSpec::new("tile", Priority::Visible).generation(&view),
        |_| Ok(()),
    );
    release.send(()).unwrap();
    get(&gate).unwrap();
    assert_eq!(get(&old).unwrap_err(), JobError::Stale);
    assert!(get(&fresh).is_ok());
    assert!(!ran.load(Ordering::SeqCst));
    wait_stats(&s, |st| st.stale_dropped == 1);
}

#[test]
fn running_job_sees_staleness_at_its_checkpoint() {
    let s = scheduler(1, 0);
    let view = Generation::new();
    let (started_tx, started_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let h = s.submit(
        JobSpec::new("long", Priority::Background).generation(&view),
        move |ctx| {
            started_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            ctx.checkpoint()?;
            Ok(())
        },
    );
    started_rx.recv_timeout(TIMEOUT).unwrap();
    view.bump();
    go_tx.send(()).unwrap();
    assert_eq!(get(&h).unwrap_err(), JobError::Stale);
}

#[test]
fn cancel_before_and_during_run() {
    let s = scheduler(1, 0);
    let (release, gate) = block_worker(&s, Priority::Background);
    let queued = s.submit(JobSpec::new("q", Priority::Visible), |_| Ok(()));
    queued.cancel();
    release.send(()).unwrap();
    get(&gate).unwrap();
    assert_eq!(get(&queued).unwrap_err(), JobError::Cancelled);

    let (started_tx, started_rx) = mpsc::channel();
    let running = s.submit(JobSpec::new("r", Priority::Visible), move |ctx| {
        started_tx.send(()).unwrap();
        while !ctx.is_cancelled() {
            std::thread::yield_now();
        }
        ctx.checkpoint()?;
        Ok(())
    });
    started_rx.recv_timeout(TIMEOUT).unwrap();
    running.cancel();
    assert_eq!(get(&running).unwrap_err(), JobError::Cancelled);
    wait_stats(&s, |st| st.cancelled == 2);
}

#[test]
fn duplicate_requests_coalesce_and_take_the_higher_priority() {
    let s = scheduler(1, 0);
    let (release, gate) = block_worker(&s, Priority::Background);
    let runs = Arc::new(AtomicUsize::new(0));
    let key = JobKey {
        kind: "tile",
        parts: [1, 2, 3, 4],
    };
    let order = Arc::new(Mutex::new(Vec::new()));
    let make = |label: &'static str| {
        let runs = Arc::clone(&runs);
        let order = Arc::clone(&order);
        move |_: &JobContext| {
            runs.fetch_add(1, Ordering::SeqCst);
            order.lock().push(label);
            Ok(label)
        }
    };
    let first = s.submit(
        JobSpec::new("t", Priority::Background).key(key),
        make("tile"),
    );
    let other = s.submit(JobSpec::new("o", Priority::Prefetch), make("other"));
    let second = s.submit(JobSpec::new("t", Priority::Visible).key(key), make("dup"));
    assert_eq!(s.stats().coalesced, 1);
    assert_eq!(s.stats().queued, [0, 1, 0, 1, 0]);

    release.send(()).unwrap();
    get(&gate).unwrap();
    let (a, b) = (get(&first).unwrap(), get(&second).unwrap());
    get(&other).unwrap();
    assert!(Arc::ptr_eq(&a, &b), "both requesters share one result");
    assert_eq!(
        *a, "tile",
        "the first closure runs; the duplicate is dropped"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(*order.lock(), ["tile", "other"], "upgraded to Visible");
}

#[test]
fn coalesced_job_runs_until_every_requester_cancels() {
    let s = scheduler(1, 0);
    let key = JobKey {
        kind: "k",
        parts: [0; 4],
    };

    let (release, gate) = block_worker(&s, Priority::Background);
    let a = s.submit(JobSpec::new("j", Priority::Visible).key(key), |_| Ok(1));
    let b = s.submit(JobSpec::new("j", Priority::Visible).key(key), |_| Ok(2));
    a.cancel();
    release.send(()).unwrap();
    get(&gate).unwrap();
    assert_eq!(*get(&b).unwrap(), 1, "b still wanted it");

    let (release, gate) = block_worker(&s, Priority::Background);
    let c = s.submit(JobSpec::new("j", Priority::Visible).key(key), |_| Ok(3));
    let d = s.submit(JobSpec::new("j", Priority::Visible).key(key), |_| Ok(4));
    c.cancel();
    d.cancel();
    release.send(()).unwrap();
    get(&gate).unwrap();
    assert_eq!(get(&c).unwrap_err(), JobError::Cancelled);
    assert_eq!(get(&d).unwrap_err(), JobError::Cancelled);
}

#[test]
fn coalescing_refreshes_the_generation() {
    let s = scheduler(1, 0);
    let view = Generation::new();
    let key = JobKey {
        kind: "tile",
        parts: [7, 0, 0, 0],
    };
    let (release, gate) = block_worker(&s, Priority::Background);
    let old = s.submit(
        JobSpec::new("t", Priority::Visible)
            .key(key)
            .generation(&view),
        |_| Ok(()),
    );
    view.bump();
    // The same tile is still wanted in the new viewport.
    let new = s.submit(
        JobSpec::new("t", Priority::Visible)
            .key(key)
            .generation(&view),
        |_| Ok(()),
    );
    release.send(()).unwrap();
    get(&gate).unwrap();
    assert!(get(&old).is_ok() && get(&new).is_ok());
    assert_eq!(s.stats().stale_dropped, 0);
}

#[test]
fn a_panicking_job_reports_and_the_worker_survives() {
    let s = scheduler(1, 0);
    let h = s.submit(
        JobSpec::new("boom", Priority::Visible),
        |_| -> Result<(), _> { panic!("corrupt record") },
    );
    assert_eq!(
        get(&h).unwrap_err(),
        JobError::Panicked("corrupt record".into())
    );
    let ok = s.submit(JobSpec::new("after", Priority::Visible), |_| Ok(5));
    assert_eq!(*get(&ok).unwrap(), 5);
    wait_stats(&s, |st| st.panicked == 1);
}

#[test]
fn shutdown_completes_queued_jobs() {
    let s = scheduler(1, 0);
    let (release, gate) = block_worker(&s, Priority::Background);
    let queued = s.submit(JobSpec::new("q", Priority::Visible), |_| Ok(()));
    // Release the gate shortly after drop starts, so drop can join it.
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        let _ = release.send(());
    });
    drop(s);
    releaser.join().unwrap();
    assert_eq!(get(&queued).unwrap_err(), JobError::Shutdown);
    assert!(get(&gate).is_ok());
}

#[test]
fn history_records_outcomes() {
    let s = scheduler(1, 0);
    get(&s.submit(JobSpec::new("a", Priority::Prefetch), |_| Ok(()))).unwrap();
    wait_stats(&s, |st| st.completed == 1);
    let records = s.recent_jobs();
    let last = records.last().unwrap();
    assert_eq!((last.name, last.priority), ("a", Priority::Prefetch));
    assert_eq!(last.outcome, Outcome::Completed);
}

fn priority_strategy() -> impl Strategy<Value = Priority> {
    prop::sample::select(Priority::ALL.to_vec())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// On one worker, jobs run in priority order, FIFO within a priority,
    /// and each distinct key runs exactly once at its best priority.
    #[test]
    fn execution_order_matches_a_model(
        jobs in prop::collection::vec((priority_strategy(), prop::option::of(0u64..6)), 1..40)
    ) {
        let s = scheduler(1, 0);
        let (release, gate) = block_worker(&s, Priority::Background);
        let order = Arc::new(Mutex::new(Vec::new()));

        // Model: one slot per key (first submission wins, best priority).
        let mut model: Vec<(Priority, usize)> = Vec::new();
        let mut slot_of_key: HashMap<u64, usize> = HashMap::new();
        let mut handles = Vec::new();
        for (i, (priority, key)) in jobs.iter().copied().enumerate() {
            let mut spec = JobSpec::new("p", priority);
            if let Some(k) = key {
                spec = spec.key(JobKey { kind: "p", parts: [k, 0, 0, 0] });
            }
            let order = Arc::clone(&order);
            handles.push(s.submit(spec, move |_| { order.lock().push(i); Ok(i) }));
            match key.and_then(|k| slot_of_key.get(&k).copied()) {
                Some(slot) => {
                    // Coalesced: an upgrade re-queues at the back of the new priority.
                    if priority < model[slot].0 {
                        let (_, first) = model.remove(slot);
                        model.push((priority, first));
                        slot_of_key.values_mut().for_each(|s| if *s > slot { *s -= 1 });
                        slot_of_key.insert(key.unwrap(), model.len() - 1);
                    }
                }
                None => {
                    model.push((priority, i));
                    if let Some(k) = key { slot_of_key.insert(k, model.len() - 1); }
                }
            }
        }
        release.send(()).unwrap();
        get(&gate).unwrap();
        for h in &handles { get(h).unwrap(); }

        // Stable sort by priority keeps queue (FIFO) order within a priority.
        model.sort_by_key(|(p, _)| *p);
        let expected: Vec<usize> = model.iter().map(|(_, i)| *i).collect();
        prop_assert_eq!(&*order.lock(), &expected);
    }
}
