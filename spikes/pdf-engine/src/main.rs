//! M3 engine spike (docs/M3_PLAN.md): PDFium (bblanchon/pdfium-binaries,
//! chromium/7881, no V8/XFA) through a thin hand-written FFI, against hayro,
//! on the §10 PDF fixtures. Prints one JSON object per run.
//!
//! ```text
//! pdf-engine pdfium <pdfium.dll> <file.pdf> [--first-only]
//! pdf-engine text   <pdfium.dll> <file.pdf> <from> <to>   # extract text + boxes
//! pdf-engine spawn  <pdfium.dll> <file.pdf> <runs>        # worker process end to end
//! pdf-engine pool   <pdfium.dll> <file.pdf> <workers>     # full text pass across workers
//! pdf-engine walk   <pdfium.dll> <file.pdf>              # page-tree walk, then far jumps
//! pdf-engine save   <pdfium.dll> <file.pdf>              # incremental save of one note
//! pdf-engine append <scratch-file>                       # append to a mapped file
//! pdf-engine hayro  <file.pdf>
//! ```
//!
//! Get PDFium with `pwsh tools/pdfium/fetch-pdfium.ps1` (it prints the
//! library's path). `save` honours `SPIKE_NO_COMPARE`, `SPIKE_FAKE` (elide
//! the copy reads) and `SPIKE_TAIL=<path>` (write the update there).
//!
//! Scales follow the reference machine: 100 % zoom at 150 % display scaling
//! is 2.0 device px per PDF point (96 dpi × 1.5 / 72). The 800 % tile is a
//! 512 px square at the page's centre at 16 px per point.

use std::ffi::{c_int, c_ulong, c_void};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};

const SCALE_100: f32 = 2.0;
const SCALE_800: f32 = 16.0;
const TILE: i32 = 512;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = match args.get(1).map(String::as_str) {
        Some("pdfium") => pdfium(&args[2], &args[3], args.get(4).is_some()),
        Some("text") => text(
            &args[2],
            &args[3],
            args[4].parse().unwrap(),
            args[5].parse().unwrap(),
        ),
        Some("spawn") => spawn(&args[2], &args[3], args[4].parse().unwrap()),
        Some("pool") => pool(&args[2], &args[3], args[4].parse().unwrap()),
        Some("append") => append(&args[2]),
        Some("save") => save(&args[2], &args[3]),
        Some("walk") => walk(&args[2], &args[3]),
        Some("hayro") => hayro_run(&args[2]),
        _ => {
            eprintln!("usage: see the module docs");
            std::process::exit(2);
        }
    };
    let mut out = out;
    out["peak_private_mib"] = json!(peak_private_mib());
    println!("{out}");
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct MemCounters {
    cb: u32,
    page_faults: u32,
    peak_ws: usize,
    ws: usize,
    quota: [usize; 4],
    pagefile: usize,
    peak_pagefile: usize,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn K32GetProcessMemoryInfo(p: *mut c_void, c: *mut MemCounters, cb: u32) -> i32;
}

/// Peak private (pagefile-backed) bytes: what a worker costs beyond the
/// mapped file, which shows up in the working set but is shared and evictable.
#[cfg(windows)]
fn peak_private_mib() -> f64 {
    let mut c = MemCounters {
        cb: size_of::<MemCounters>() as u32,
        ..Default::default()
    };
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    c.peak_pagefile as f64 / 1_048_576.0
}

#[cfg(not(windows))]
fn peak_private_mib() -> f64 {
    0.0
}

#[repr(C)]
struct FileAccess {
    len: c_ulong,
    get_block: unsafe extern "C" fn(*mut c_void, c_ulong, *mut u8, c_ulong) -> c_int,
    param: *mut c_void,
}

#[repr(C)]
struct Matrix {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

#[repr(C)]
struct RectF {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

type H = *mut c_void;

/// The handful of PDFium entry points the spike needs.
struct Api {
    _lib: libloading::Library,
    init: unsafe extern "C" fn(),
    load_custom: unsafe extern "C" fn(*mut FileAccess, *const u8) -> H,
    page_count: unsafe extern "C" fn(H) -> c_int,
    load_page: unsafe extern "C" fn(H, c_int) -> H,
    close_page: unsafe extern "C" fn(H),
    page_w: unsafe extern "C" fn(H) -> f32,
    page_h: unsafe extern "C" fn(H) -> f32,
    bmp_create: unsafe extern "C" fn(c_int, c_int, c_int) -> H,
    bmp_fill: unsafe extern "C" fn(H, c_int, c_int, c_int, c_int, u32) -> c_int,
    bmp_buf: unsafe extern "C" fn(H) -> *mut u8,
    bmp_stride: unsafe extern "C" fn(H) -> c_int,
    bmp_destroy: unsafe extern "C" fn(H),
    render: unsafe extern "C" fn(H, H, *const Matrix, *const RectF, c_int),
    text_load: unsafe extern "C" fn(H) -> H,
    text_close: unsafe extern "C" fn(H),
    text_count: unsafe extern "C" fn(H) -> c_int,
    text_get: unsafe extern "C" fn(H, c_int, c_int, *mut u16) -> c_int,
    text_box: unsafe extern "C" fn(H, c_int, *mut f64, *mut f64, *mut f64, *mut f64) -> c_int,
    size_by_index: unsafe extern "C" fn(H, c_int, *mut [f32; 2]) -> c_int,
    annot_create: unsafe extern "C" fn(H, c_int) -> H,
    annot_rect: unsafe extern "C" fn(H, *const RectF) -> c_int,
    annot_close: unsafe extern "C" fn(H),
    save_copy: unsafe extern "C" fn(H, *mut FileWrite, u32) -> c_int,
}

#[repr(C)]
struct FileWrite {
    version: c_int,
    write_block: unsafe extern "C" fn(*mut FileWrite, *const u8, c_ulong) -> c_int,
    // Ours, after PDFium's two fields.
    total: u64,
    skip: u64,
    tail: Vec<u8>,
    prefix_ok: bool,
    original: *const memmap2::Mmap,
}

/// Keeps only what PDFium writes past the original's length (the update
/// itself) and checks that the prefix it rewrites equals the original.
unsafe extern "C" fn write_block(this: *mut FileWrite, data: *const u8, size: c_ulong) -> c_int {
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let w = unsafe { &mut *this };
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let bytes = unsafe { std::slice::from_raw_parts(data, size as usize) };
    let start = w.total;
    w.total += bytes.len() as u64;
    if start >= w.skip {
        w.tail.extend_from_slice(bytes);
    } else {
        // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
        let orig = unsafe { &*w.original };
        let n = ((w.skip - start) as usize).min(bytes.len());
        if std::env::var_os("SPIKE_NO_COMPARE").is_none()
            && orig[start as usize..start as usize + n] != bytes[..n]
        {
            w.prefix_ok = false;
        }
        w.tail.extend_from_slice(&bytes[n..]);
    }
    1
}

impl Api {
    fn load(path: &str) -> Api {
        // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
        unsafe {
            let lib = libloading::Library::new(path).unwrap();
            macro_rules! f {
                ($name:literal) => {
                    *lib.get($name).unwrap()
                };
            }
            let api = Api {
                init: f!(b"FPDF_InitLibrary"),
                load_custom: f!(b"FPDF_LoadCustomDocument"),
                page_count: f!(b"FPDF_GetPageCount"),
                load_page: f!(b"FPDF_LoadPage"),
                close_page: f!(b"FPDF_ClosePage"),
                page_w: f!(b"FPDF_GetPageWidthF"),
                page_h: f!(b"FPDF_GetPageHeightF"),
                bmp_create: f!(b"FPDFBitmap_Create"),
                bmp_fill: f!(b"FPDFBitmap_FillRect"),
                bmp_buf: f!(b"FPDFBitmap_GetBuffer"),
                bmp_stride: f!(b"FPDFBitmap_GetStride"),
                bmp_destroy: f!(b"FPDFBitmap_Destroy"),
                render: f!(b"FPDF_RenderPageBitmapWithMatrix"),
                text_load: f!(b"FPDFText_LoadPage"),
                text_close: f!(b"FPDFText_ClosePage"),
                text_count: f!(b"FPDFText_CountChars"),
                text_get: f!(b"FPDFText_GetText"),
                text_box: f!(b"FPDFText_GetCharBox"),
                size_by_index: f!(b"FPDF_GetPageSizeByIndexF"),
                annot_create: f!(b"FPDFPage_CreateAnnot"),
                annot_rect: f!(b"FPDFAnnot_SetRect"),
                annot_close: f!(b"FPDFPage_CloseAnnot"),
                save_copy: f!(b"FPDF_SaveAsCopy"),
                _lib: lib,
            };
            (api.init)();
            api
        }
    }
}

/// Reads blocks from the memory-mapped file (production code would use the
/// guarded copies of ADR-0007).
unsafe extern "C" fn get_block(
    param: *mut c_void,
    pos: c_ulong,
    buf: *mut u8,
    size: c_ulong,
) -> c_int {
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let map = unsafe { &*(param as *const memmap2::Mmap) };
    let (pos, size) = (pos as usize, size as usize);
    if pos + size > map.len() {
        return 0;
    }
    if SAVING.load(Relaxed) {
        // Reads made while saving: sequential from the expected offset are
        // the copy of the original (zero-filled when SPIKE_FAKE is set),
        // anything else is counted and served for real.
        let next = SAVE_NEXT.load(Relaxed);
        if pos == next {
            SAVE_NEXT.store(pos + size, Relaxed);
            SAVE_SEQ.fetch_add(1, Relaxed);
            if std::env::var_os("SPIKE_FAKE").is_some() {
                // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
                unsafe { std::ptr::write_bytes(buf, 0, size) };
                return 1;
            }
        } else {
            SAVE_OTHER.fetch_add(1, Relaxed);
        }
    }
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    unsafe { std::ptr::copy_nonoverlapping(map.as_ptr().add(pos), buf, size) };
    1
}

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
static SAVING: AtomicBool = AtomicBool::new(false);
static SAVE_NEXT: AtomicUsize = AtomicUsize::new(0);
static SAVE_SEQ: AtomicUsize = AtomicUsize::new(0);
static SAVE_OTHER: AtomicUsize = AtomicUsize::new(0);

struct Doc {
    _map: Box<memmap2::Mmap>,
    _access: Box<FileAccess>,
    handle: H,
}

fn open(api: &Api, path: &str) -> Doc {
    let file = File::open(path).unwrap();
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let map = Box::new(unsafe { memmap2::Mmap::map(&file).unwrap() });
    let mut access = Box::new(FileAccess {
        len: c_ulong::try_from(map.len())
            .expect("PDFium's file length is a 32-bit unsigned long on Windows"),
        get_block,
        param: &*map as *const memmap2::Mmap as *mut c_void,
    });
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let handle = unsafe { (api.load_custom)(&mut *access, std::ptr::null()) };
    assert!(!handle.is_null(), "FPDF_LoadCustomDocument failed");
    Doc {
        _map: map,
        _access: access,
        handle,
    }
}

/// Renders page `index` at `scale`: the whole page, or a 512 px tile at its
/// centre. Returns the time and a checksum of the pixels.
fn render(api: &Api, doc: &Doc, index: c_int, scale: f32, full: bool) -> (f64, u64, (i32, i32)) {
    let t = Instant::now();
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    unsafe {
        let page = (api.load_page)(doc.handle, index);
        let (pw, ph) = ((api.page_w)(page) * scale, (api.page_h)(page) * scale);
        let (w, h, ox, oy) = if full {
            (pw.ceil() as i32, ph.ceil() as i32, 0.0, 0.0)
        } else {
            (TILE, TILE, pw / 2.0, ph / 2.0)
        };
        let bmp = (api.bmp_create)(w, h, 0);
        (api.bmp_fill)(bmp, 0, 0, w, h, 0xFFFF_FFFF);
        let m = Matrix {
            a: scale,
            b: 0.0,
            c: 0.0,
            d: scale,
            e: -ox,
            f: -oy,
        };
        let clip = RectF {
            left: 0.0,
            top: 0.0,
            right: w as f32,
            bottom: h as f32,
        };
        // FPDF_ANNOT | FPDF_REVERSE_BYTE_ORDER (RGBA order, as the tiles use).
        (api.render)(bmp, page, &m, &clip, 0x01 | 0x10);
        let len = ((api.bmp_stride)(bmp) * h) as usize;
        let px = std::slice::from_raw_parts((api.bmp_buf)(bmp), len);
        let sum = px.iter().step_by(97).map(|&x| u64::from(x)).sum();
        (api.bmp_destroy)(bmp);
        (api.close_page)(page);
        (ms(t), sum, (w, h))
    }
}

fn pdfium(dll: &str, path: &str, first_only: bool) -> Value {
    let t = Instant::now();
    let api = Api::load(dll);
    let bind_ms = ms(t);
    let t = Instant::now();
    let doc = open(&api, path);
    let open_ms = ms(t);
    let t = Instant::now();
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let count = unsafe { (api.page_count)(doc.handle) };
    let count_ms = ms(t);

    let (first_ms, first_sum, first_px) = render(&api, &doc, 0, SCALE_100, true);
    if first_only {
        return json!({ "bind_ms": bind_ms, "open_ms": open_ms, "count_ms": count_ms, "first_ms": first_ms, "sum": first_sum });
    }
    let pick = |xs: &[c_int]| -> Vec<c_int> { xs.iter().copied().filter(|&i| i < count).collect() };
    let typical: Vec<f64> = pick(&[1, 1000, 2500, 3999, 4998, 1, 1000])
        .into_iter()
        .map(|i| render(&api, &doc, i, SCALE_100, true).0)
        .collect();
    let tiles: Vec<f64> = pick(&[0, 1000, 3999, 0])
        .into_iter()
        .map(|i| render(&api, &doc, i, SCALE_800, false).0)
        .collect();
    json!({
        "engine": "pdfium-7881", "file": path, "pages": count,
        "bind_ms": bind_ms, "open_ms": open_ms, "count_ms": count_ms,
        "first_page_ms": first_ms, "first_page_px": first_px, "first_sum": first_sum,
        "typical_page_ms": typical, "tile_800_ms": tiles,
    })
}

/// Can a file we hold mapped (as the source of the document) be appended
/// to in place, flushed, and read back through a fresh mapping?
fn append(path: &str) -> Value {
    use std::io::Write;
    std::fs::write(path, vec![b'a'; 1 << 20]).unwrap();
    let file = File::open(path).unwrap();
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let map = unsafe { memmap2::Mmap::map(&file).unwrap() };
    let t = Instant::now();
    let r = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut f| {
            f.write_all(&vec![b'b'; 700_000])?;
            f.sync_all()
        });
    let append_ms = ms(t);
    let len = std::fs::metadata(path).unwrap().len();
    let shrink = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_len(1 << 20));
    let old_map_ok = map[..16] == [b'a'; 16];
    json!({ "append": format!("{r:?}"), "append_ms": append_ms, "len_after": len,
            "old_map_still_reads": old_map_ok, "truncate_while_mapped": format!("{shrink:?}") })
}

/// Adds a note annotation to page 2,500 and saves incrementally: how long
/// PDFium takes to stream original + update, and how big the update is.
fn save(dll: &str, path: &str) -> Value {
    let api = Api::load(dll);
    let doc = open(&api, path);
    let t = Instant::now();
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    unsafe {
        let page = (api.load_page)(doc.handle, 2499);
        let a = (api.annot_create)(page, 1); // FPDF_ANNOT_TEXT
        (api.annot_rect)(
            a,
            &RectF {
                left: 100.0,
                top: 700.0,
                right: 120.0,
                bottom: 680.0,
            },
        );
        (api.annot_close)(a);
        (api.close_page)(page);
    }
    let edit_ms = ms(t);
    let mut w = FileWrite {
        version: 1,
        write_block,
        total: 0,
        skip: doc._map.len() as u64,
        tail: Vec::new(),
        prefix_ok: true,
        original: &*doc._map,
    };
    let t = Instant::now();
    SAVING.store(true, Relaxed);
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let ok = unsafe { (api.save_copy)(doc.handle, &mut w, 1) }; // FPDF_INCREMENTAL
    SAVING.store(false, Relaxed);
    let save_ms = ms(t);
    if let Some(out) = std::env::var_os("SPIKE_TAIL") {
        std::fs::write(out, &w.tail).unwrap();
    }
    let tail_hash = w.tail.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    json!({ "file": path, "ok": ok, "edit_ms": edit_ms, "save_ms": save_ms, "written": w.total,
            "update_bytes": w.tail.len(), "prefix_identical": w.prefix_ok,
            "tail_fnv": format!("{tail_hash:016x}"), "seq_reads": SAVE_SEQ.load(Relaxed), "other_reads": SAVE_OTHER.load(Relaxed) })
}

/// Opens, walks the page tree (every page's size, as an index would), then
/// jumps to far pages: is the cold cost of a jump the page-tree walk?
fn walk(dll: &str, path: &str) -> Value {
    let api = Api::load(dll);
    let t = Instant::now();
    let doc = open(&api, path);
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let count = unsafe { (api.page_count)(doc.handle) };
    let open_ms = ms(t);
    let t = Instant::now();
    let mut area = 0f64;
    for i in 0..count {
        let mut wh = [0f32; 2];
        // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
        unsafe { (api.size_by_index)(doc.handle, i, &mut wh) };
        area += f64::from(wh[0] * wh[1]);
    }
    let walk_ms = ms(t);
    let jumps: Vec<f64> = [1000, 2500, 3999, 4998, 2]
        .into_iter()
        .filter(|&i| i < count)
        .map(|i| render(&api, &doc, i, SCALE_100, true).0)
        .collect();
    json!({ "file": path, "open_ms": open_ms, "walk_ms": walk_ms, "area": area, "jumps_after_walk_ms": jumps })
}

/// Extracts the text and every character box of pages `[from, to)`, as the
/// text index would, and counts pages whose `token-NNNNN` is found.
fn text(dll: &str, path: &str, from: c_int, to: c_int) -> Value {
    let t0 = Instant::now();
    let api = Api::load(dll);
    let doc = open(&api, path);
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let to = to.min(unsafe { (api.page_count)(doc.handle) });
    let t = Instant::now();
    let (mut chars, mut hits, mut x) = (0usize, 0usize, 0f64);
    for i in from..to {
        // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
        unsafe {
            let page = (api.load_page)(doc.handle, i);
            let tp = (api.text_load)(page);
            let n = (api.text_count)(tp);
            let mut buf = vec![0u16; n as usize + 1];
            (api.text_get)(tp, 0, n, buf.as_mut_ptr());
            let s = String::from_utf16_lossy(&buf[..n as usize]);
            if s.contains(&format!("token-{:05}", i + 1)) {
                hits += 1;
            }
            for c in 0..n {
                let (mut l, mut r, mut b, mut tt) = (0.0, 0.0, 0.0, 0.0);
                (api.text_box)(tp, c, &mut l, &mut r, &mut b, &mut tt);
                x += l;
            }
            chars += n as usize;
            (api.text_close)(tp);
            (api.close_page)(page);
        }
    }
    let pages = (to - from).max(0);
    json!({ "from": from, "to": to, "ms": ms(t), "total_ms": ms(t0),
            "ms_per_page": ms(t) / f64::from(pages.max(1)), "chars": chars, "token_hits": hits, "x": x })
}

fn spawn(dll: &str, path: &str, runs: usize) -> Value {
    let exe = std::env::current_exe().unwrap();
    let mut totals = Vec::new();
    let mut inner = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        let mut child = Command::new(&exe)
            .args(["pdfium", dll, path, "--first-only"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        totals.push(ms(t));
        child.wait().unwrap();
        inner.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    json!({ "mode": "spawn", "file": path, "end_to_end_ms": totals, "inner": inner })
}

/// A full text pass split across `workers` processes in contiguous ranges.
fn pool(dll: &str, path: &str, workers: c_int) -> Value {
    let exe = std::env::current_exe().unwrap();
    let count = {
        let api = Api::load(dll);
        let doc = open(&api, path);
        // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
        unsafe { (api.page_count)(doc.handle) }
    };
    let per = (count + workers - 1) / workers;
    let t = Instant::now();
    let children: Vec<_> = (0..workers)
        .map(|w| {
            Command::new(&exe)
                .args([
                    "text",
                    dll,
                    path,
                    &(w * per).to_string(),
                    &((w + 1) * per).to_string(),
                ])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let results: Vec<Value> = children
        .into_iter()
        .map(|c| serde_json::from_slice(&c.wait_with_output().unwrap().stdout).unwrap())
        .collect();
    json!({ "mode": "pool", "workers": workers, "pages": count, "wall_ms": ms(t), "workers_out": results })
}

fn hayro_run(path: &str) -> Value {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings};

    let file = File::open(path).unwrap();
    // SAFETY: spike FFI. Handles and pointers come from PDFium or the OS for this call, and the callbacks get what PDFium's FPDF_FILEACCESS/FPDF_FILEWRITE contract promises.
    let map = unsafe { memmap2::Mmap::map(&file).unwrap() };
    let t = Instant::now();
    let pdf = Pdf::new(Arc::new(map)).unwrap();
    let new_ms = ms(t);
    let t = Instant::now();
    let pages = pdf.pages();
    let count = pages.len();
    let pages_ms = ms(t);
    let open_ms = new_ms + pages_ms;

    let cache = RenderCache::new();
    let interp = InterpreterSettings::default();
    let render = |i: usize, scale: f32| {
        let t = Instant::now();
        let pix = hayro::render(
            &pages[i],
            &cache,
            &interp,
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            },
        );
        let sum: u64 = pix
            .data_as_u8_slice()
            .iter()
            .step_by(97)
            .map(|&x| u64::from(x))
            .sum();
        (ms(t), sum, (pix.width(), pix.height()))
    };
    let (first_ms, first_sum, first_px) = render(0, SCALE_100);
    let typical: Vec<f64> = [1usize, 1000, 2500, 3999, 4998, 1, 1000]
        .iter()
        .filter(|&&i| i < count)
        .map(|&i| render(i, SCALE_100).0)
        .collect();
    json!({
        "engine": "hayro-0.8.0", "file": path, "pages": count, "open_ms": open_ms,
        "pdf_new_ms": new_ms, "pages_ms": pages_ms,
        "first_page_ms": first_ms, "first_page_px": first_px, "first_sum": first_sum,
        "typical_page_ms": typical,
        "note": "no clip-to-tile entry point in the simple API; text extraction, forms, annotations and saving are not in hayro"
    })
}
