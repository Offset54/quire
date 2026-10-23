//! The subset of PDFium's C API the worker uses (ADR-0035), loaded at run
//! time from the pinned library. Declarations follow the headers of
//! `chromium/7881` (`fpdfview.h`, `fpdf_text.h`); only the worker process
//! calls them, from one thread.

use std::ffi::{c_char, c_int, c_ulong, c_void};
use std::path::Path;

pub type Handle = *mut c_void;

/// `FPDF_FILEACCESS`. `len` is an `unsigned long`: 32 bits on Windows, so
/// files of 4 GiB or more can't be opened this way there (ADR-0037).
#[repr(C)]
pub struct FileAccess {
    pub len: c_ulong,
    pub get_block: unsafe extern "C" fn(*mut c_void, c_ulong, *mut u8, c_ulong) -> c_int,
    pub param: *mut c_void,
}

/// `FS_MATRIX`.
#[repr(C)]
pub struct FsMatrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

/// `FS_RECTF`.
#[repr(C)]
pub struct FsRectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// `FPDFBitmap_BGRA`.
pub const BITMAP_BGRA: c_int = 4;
/// `FPDF_REVERSE_BYTE_ORDER`: the bitmap is filled as RGBA, not BGRA.
pub const REVERSE_BYTE_ORDER: c_int = 0x10;

pub struct Api {
    _lib: libloading::Library,
    pub init: unsafe extern "C" fn(),
    pub get_last_error: unsafe extern "C" fn() -> c_ulong,
    pub load_custom: unsafe extern "C" fn(*mut FileAccess, *const c_char) -> Handle,
    pub load_document: unsafe extern "C" fn(*const c_char, *const c_char) -> Handle,
    pub close_document: unsafe extern "C" fn(Handle),
    pub page_count: unsafe extern "C" fn(Handle) -> c_int,
    pub file_version: unsafe extern "C" fn(Handle, *mut c_int) -> c_int,
    pub valid_xref: unsafe extern "C" fn(Handle) -> c_int,
    pub size_by_index: unsafe extern "C" fn(Handle, c_int, *mut FsSizeF) -> c_int,
    pub load_page: unsafe extern "C" fn(Handle, c_int) -> Handle,
    pub close_page: unsafe extern "C" fn(Handle),
    pub bitmap_create_ex: unsafe extern "C" fn(c_int, c_int, c_int, *mut c_void, c_int) -> Handle,
    pub bitmap_fill: unsafe extern "C" fn(Handle, c_int, c_int, c_int, c_int, u32) -> c_int,
    pub bitmap_destroy: unsafe extern "C" fn(Handle),
    pub render_matrix: unsafe extern "C" fn(Handle, Handle, *const FsMatrix, *const FsRectF, c_int),
}

/// `FS_SIZEF`.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct FsSizeF {
    pub width: f32,
    pub height: f32,
}

impl Api {
    /// Loads the library and initializes PDFium.
    pub fn load(path: &Path) -> Result<Api, String> {
        // SAFETY: loading PDFium runs its initializers, which have no
        // preconditions; the symbols below are looked up by their C names
        // and given the signatures of the 7881 headers (ADR-0035 pins it).
        unsafe {
            let lib = libloading::Library::new(path)
                .map_err(|e| format!("loading {}: {e}", path.display()))?;
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get($name).map_err(|e| {
                        format!("{}: {e}", String::from_utf8_lossy(&$name[..$name.len()]))
                    })?
                };
            }
            let api = Api {
                init: sym!(b"FPDF_InitLibrary"),
                get_last_error: sym!(b"FPDF_GetLastError"),
                load_custom: sym!(b"FPDF_LoadCustomDocument"),
                load_document: sym!(b"FPDF_LoadDocument"),
                close_document: sym!(b"FPDF_CloseDocument"),
                page_count: sym!(b"FPDF_GetPageCount"),
                file_version: sym!(b"FPDF_GetFileVersion"),
                valid_xref: sym!(b"FPDF_DocumentHasValidCrossReferenceTable"),
                size_by_index: sym!(b"FPDF_GetPageSizeByIndexF"),
                load_page: sym!(b"FPDF_LoadPage"),
                close_page: sym!(b"FPDF_ClosePage"),
                bitmap_create_ex: sym!(b"FPDFBitmap_CreateEx"),
                bitmap_fill: sym!(b"FPDFBitmap_FillRect"),
                bitmap_destroy: sym!(b"FPDFBitmap_Destroy"),
                render_matrix: sym!(b"FPDF_RenderPageBitmapWithMatrix"),
                _lib: lib,
            };
            (api.init)();
            Ok(api)
        }
    }
}
