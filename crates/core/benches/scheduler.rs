//! Scheduler overhead (M0 task 5). The Interaction path's hand-off latency
//! eats into the 16 ms input budget, so it is measured here.

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use quire_core::scheduler::{JobKey, JobSpec, Priority, Scheduler, SchedulerConfig};

fn config() -> SchedulerConfig {
    SchedulerConfig {
        threads: 4,
        urgent_reserved: 1,
        history: 64,
        thread_name: "bench-worker",
    }
}

fn handoff(c: &mut Criterion) {
    let s = Scheduler::new(config());
    c.bench_function("submit_and_wait/interaction", |b| {
        b.iter(|| {
            s.submit(JobSpec::new("noop", Priority::Interaction), |_| Ok(()))
                .wait()
                .unwrap()
        });
    });
}

fn throughput(c: &mut Criterion) {
    let s = Scheduler::new(config());
    let mut group = c.benchmark_group("throughput");
    group.throughput(criterion::Throughput::Elements(10_000));
    group.bench_function("10k_noop_jobs", |b| {
        b.iter(|| {
            let handles: Vec<_> = (0..10_000u64)
                .map(|i| {
                    let p = Priority::ALL[(i % 5) as usize];
                    s.submit(JobSpec::new("noop", p), move |_| Ok(i))
                })
                .collect();
            for h in &handles {
                h.wait().unwrap();
            }
        });
    });
    group.finish();
}

fn coalesce_hit(c: &mut Criterion) {
    // One worker, kept busy, so the keyed job stays queued and every
    // submission below is a coalescing hit.
    let s = Scheduler::new(SchedulerConfig {
        threads: 1,
        urgent_reserved: 0,
        ..config()
    });
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let gate = s.submit(JobSpec::new("gate", Priority::Interaction), move |_| {
        let _ = wait.recv();
        Ok(())
    });
    std::thread::sleep(Duration::from_millis(10));
    let key = JobKey {
        kind: "tile",
        parts: [1, 2, 3, 4],
    };
    let _first = s.submit(JobSpec::new("tile", Priority::Prefetch).key(key), |_| {
        Ok(0u8)
    });
    c.bench_function("submit/coalesce_hit", |b| {
        b.iter(|| {
            s.submit(JobSpec::new("tile", Priority::Prefetch).key(key), |_| {
                Ok(0u8)
            })
        });
    });
    release.send(()).unwrap();
    gate.wait().unwrap();
}

criterion_group! {
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(3));
    targets = handoff, throughput, coalesce_hit
}
criterion_main!(benches);
