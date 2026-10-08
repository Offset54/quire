use super::*;
use proptest::prelude::*;

fn governor(per_document: u64, global: u64) -> MemoryGovernor {
    MemoryGovernor::new(GovernorConfig {
        per_document,
        global,
        low_watermark: 0.5,
        background_retain: 0.25,
    })
}

type Cache = Arc<LruCache<u32, ()>>;

fn cache(g: &MemoryGovernor, doc: Option<u64>, class: CacheClass) -> Cache {
    LruCache::new(g, "test", doc.map(DocId), class)
}

#[test]
fn charges_roll_up_to_document_and_global_totals() {
    let g = governor(1000, 10_000);
    let a = cache(&g, Some(1), CacheClass::Raster);
    let b = cache(&g, Some(2), CacheClass::Raster);
    a.insert(1, (), 100);
    a.insert(2, (), 50);
    b.insert(1, (), 30);
    a.insert(1, (), 60); // replacement charges the difference
    assert_eq!(a.bytes(), 110);
    assert_eq!(g.document_bytes(DocId(1)), 110);
    assert_eq!(g.document_bytes(DocId(2)), 30);
    assert_eq!(g.total_bytes(), 140);
    a.remove(&2);
    assert_eq!(g.total_bytes(), 90);
    drop(b);
    assert_eq!((g.total_bytes(), g.document_bytes(DocId(2))), (60, 0));
    assert_eq!(g.stats().caches.len(), 1, "dropped caches unregister");
}

#[test]
fn going_over_a_document_budget_evicts_lru_entries_down_to_the_watermark() {
    let g = governor(1000, 1_000_000);
    let tiles = cache(&g, Some(1), CacheClass::Raster);
    for i in 0..10 {
        tiles.insert(i, (), 100);
    }
    assert_eq!(tiles.bytes(), 1000, "at the budget, nothing evicted yet");
    tiles.get(&0); // keep 0 hot
    tiles.insert(10, (), 100);
    assert_eq!(tiles.bytes(), 500, "evicted to 50 % of 1000");
    assert!(tiles.contains(&0) && tiles.contains(&10));
    assert!(!tiles.contains(&1), "least recently used went first");
}

#[test]
fn cheaper_classes_are_evicted_first() {
    let g = governor(1000, 1_000_000);
    let layout = cache(&g, Some(1), CacheClass::Layout);
    let speculative = cache(&g, Some(1), CacheClass::Speculative);
    let raster = cache(&g, Some(1), CacheClass::Raster);
    layout.insert(0, (), 400);
    raster.insert(0, (), 300);
    speculative.insert(0, (), 300);
    speculative.insert(1, (), 100); // 1100 > 1000: evict to 500
    assert_eq!(speculative.bytes(), 0);
    assert_eq!(raster.bytes(), 0);
    assert_eq!(layout.bytes(), 400, "layout survives");
}

#[test]
fn global_pressure_sheds_background_documents_first() {
    let g = governor(10_000, 1000);
    let fg = cache(&g, Some(1), CacheClass::Speculative);
    let bg = cache(&g, Some(2), CacheClass::Layout);
    bg.insert(0, (), 400);
    bg.insert(1, (), 100);
    fg.insert(0, (), 400);
    g.set_document_state(DocId(2), DocState::Background);
    // Background shedding keeps 25 % of the per-document budget: 2500 > 500.
    assert_eq!(bg.bytes(), 500);
    fg.insert(1, (), 200); // 1100 > 1000: evict to 500
    assert_eq!(
        bg.bytes(),
        0,
        "background layout goes before foreground speculation"
    );
    // Still 100 over the 500 target, so the foreground's LRU entry (400) goes.
    assert_eq!(fg.bytes(), 200, "then the foreground pays the rest");
}

#[test]
fn background_transition_sheds_to_the_retain_fraction() {
    let g = governor(1000, 1_000_000);
    let tiles = cache(&g, Some(1), CacheClass::Raster);
    for i in 0..9 {
        tiles.insert(i, (), 100);
    }
    g.set_document_state(DocId(1), DocState::Background);
    assert_eq!(tiles.bytes(), 200, "25 % of 1000, rounded to whole entries");
    let evicted: u64 = g.stats().caches.iter().map(|c| c.evicted_bytes).sum();
    assert_eq!(evicted, 700);
}

#[test]
fn fixed_memory_counts_but_is_never_evicted() {
    let g = governor(1000, 1_000_000);
    let model = g.register_fixed("model", Some(DocId(1)));
    model.charge(800);
    let tiles = cache(&g, Some(1), CacheClass::Raster);
    tiles.insert(0, (), 300);
    assert_eq!(model.bytes(), 800);
    assert_eq!(tiles.bytes(), 0, "only the evictable cache can give way");
}

#[test]
fn lowering_the_budget_enforces_it_immediately() {
    let g = governor(10_000, 10_000);
    let tiles = cache(&g, None, CacheClass::Raster);
    for i in 0..10 {
        tiles.insert(i, (), 100);
    }
    g.set_config(GovernorConfig {
        global: 600,
        ..g.config()
    });
    assert_eq!(tiles.bytes(), 300);
}

#[test]
fn concurrent_use_keeps_accounting_exact() {
    let g = governor(50_000, 80_000);
    let caches: Vec<Cache> = (0..4)
        .map(|d| cache(&g, Some(d), CacheClass::Raster))
        .collect();
    std::thread::scope(|s| {
        for (t, c) in caches.iter().enumerate() {
            s.spawn(move || {
                for i in 0..5_000u32 {
                    let key = i % 700;
                    match i % 3 {
                        0 | 1 => c.insert(key, (), u64::from(key % 97 + t as u32)),
                        _ => {
                            c.remove(&key);
                        }
                    }
                }
            });
        }
    });
    g.rebalance();
    let sum: u64 = caches.iter().map(|c| c.bytes()).sum();
    assert_eq!(g.total_bytes(), sum, "no drift between caches and totals");
    assert!(g.total_bytes() <= 80_000);
}

proptest! {
    /// With only evictable caches, a single-threaded caller never sees a
    /// document above its budget after an insert.
    #[test]
    fn documents_stay_within_budget(
        ops in prop::collection::vec((0u64..3, 0u32..50, 1u64..300, 0usize..3), 1..300)
    ) {
        let g = governor(1000, 2000);
        let classes = [CacheClass::Speculative, CacheClass::Raster, CacheClass::Layout];
        let caches: Vec<Vec<Cache>> = (0..3)
            .map(|d| classes.iter().map(|c| cache(&g, Some(d), *c)).collect())
            .collect();
        for (doc, key, size, class) in ops {
            caches[doc as usize][class].insert(key, (), size);
            for d in 0..3 {
                prop_assert!(g.document_bytes(DocId(d)) <= 1000);
            }
            prop_assert!(g.total_bytes() <= 2000);
            let sum: u64 = caches.iter().flatten().map(|c| c.bytes()).sum();
            prop_assert_eq!(g.total_bytes(), sum);
        }
    }
}
