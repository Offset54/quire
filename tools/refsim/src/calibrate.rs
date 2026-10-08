//! Fixed CPU kernels that mirror our hot paths (ADR-0021). The scores are
//! recorded with every ref-sim run; if the suite is ever run on a real
//! reference-class laptop, the per-kernel ratio replaces the
//! Geekbench-derived `D_cpu`.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use quire_raster::{Rgb, TileCanvas};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
}

/// Median of `runs` timings of `f`, in milliseconds.
fn time(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let s = Instant::now();
            f();
            s.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    t.sort_by(f64::total_cmp);
    t[t.len() / 2]
}

fn text_buffer(len: usize) -> Vec<u8> {
    let mut r = Lcg(1);
    (0..len)
        .map(|_| match r.next() % 64 {
            0 => b'\n',
            n => b'a' + (n % 26) as u8,
        })
        .collect()
}

pub fn run() -> serde_json::Value {
    let text = text_buffer(256 << 20);
    let mut out = serde_json::Map::new();

    // Newline counting over 256 MiB (line index, §6.1).
    let ms = time(5, || {
        black_box(text.iter().filter(|&&b| b == b'\n').count());
    });
    out.insert(
        "newlineScan256MiB".into(),
        serde_json::json!({ "ms": ms, "gbPerSecond": 0.268 / (ms / 1e3) }),
    );

    // Literal search for a 12-byte needle (search, §6.1).
    let needle = b"zzqzzqzzqzzq";
    let ms = time(5, || {
        black_box(text.windows(needle.len()).filter(|w| *w == needle).count());
    });
    out.insert(
        "literalSearch256MiB".into(),
        serde_json::json!({ "ms": ms }),
    );

    // Tile rasterization: 512 px tiles with rectangles and polygons (§5.4).
    let ms = time(5, || {
        for t in 0..64u32 {
            let mut c = TileCanvas::new(512, 512, (0.0, f64::from(t) * 512.0), Rgb(255, 255, 255))
                .expect("tile");
            for i in 0..200 {
                let y = f64::from(t) * 512.0 + f64::from(i % 17) * 30.0;
                c.fill_rect(f64::from(i * 7 % 400), y, 60.0, 9.0, Rgb(150, 156, 180));
            }
            for i in 0..40 {
                let x = f64::from(i * 11 % 450);
                let y = f64::from(t) * 512.0 + f64::from(i * 13 % 450);
                c.fill_polygon(
                    &[(x, y), (x + 40.0, y + 5.0), (x + 20.0, y + 45.0)],
                    Rgb(40, 44, 60),
                );
            }
            black_box(c.into_rgba());
        }
    });
    out.insert(
        "rasterize64Tiles512".into(),
        serde_json::json!({ "ms": ms, "msPerTile": ms / 64.0 }),
    );

    // Balanced-tree churn (piece tree / caches).
    let ms = time(3, || {
        let mut r = Lcg(7);
        let mut map = BTreeMap::new();
        for i in 0..1_000_000u64 {
            map.insert(r.next(), i);
            if i % 3 == 0 {
                let k = r.next();
                map.remove(&k);
            }
        }
        black_box(map.len());
    });
    out.insert("btreeChurn1M".into(), serde_json::json!({ "ms": ms }));

    // Sorting 10M integers (search result ordering, indexing).
    let ms = time(3, || {
        let mut r = Lcg(9);
        let mut v: Vec<u64> = (0..10_000_000).map(|_| r.next()).collect();
        v.sort_unstable();
        black_box(v.len());
    });
    out.insert("sort10M".into(), serde_json::json!({ "ms": ms }));

    // Multi-threaded newline scan across all available threads.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let ms = time(5, || {
        let chunk = text.len().div_ceil(threads);
        std::thread::scope(|s| {
            for part in text.chunks(chunk) {
                s.spawn(move || black_box(part.iter().filter(|&&b| b == b'\n').count()));
            }
        });
    });
    out.insert(
        "newlineScan256MiBParallel".into(),
        serde_json::json!({ "ms": ms, "threads": threads }),
    );
    serde_json::Value::Object(out)
}
