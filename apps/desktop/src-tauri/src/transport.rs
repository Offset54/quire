//! Tile delivery from the core to the webview (ADR-0004).
//!
//! - Windows: WebView2 SharedBuffer with raw RGBA, posted on the UI thread.
//! - Elsewhere, when forced with `QUIRE_TRANSPORT=channel`, or after a
//!   SharedBuffer failure: a per-view Channel carrying a 48-byte header and a
//!   QOI or raw payload (QOI only when it saves at least 20 %).

use quire_core::tile::RenderedTile;
use tauri::ipc::{Channel, InvokeResponseBody};

/// Receives finished tiles for one view.
pub trait TileSink: Send + Sync {
    fn deliver(&self, tile: RenderedTile);
    /// Transport name, reported to the frontend and the HUD.
    fn name(&self) -> &'static str;
}

const MAGIC: u32 = u32::from_le_bytes(*b"QTL1");
pub const HEADER_LEN: usize = 48;
const ENCODING_RAW: u32 = 0;
const ENCODING_QOI: u32 = 1;

/// Little-endian header; must match `apps/desktop/src/surface/protocol.ts`.
fn header(tile: &RenderedTile, encoding: u32) -> [u8; HEADER_LEN] {
    let k = &tile.key;
    let mut h = [0u8; HEADER_LEN];
    let fields: [(usize, &[u8]); 9] = [
        (0, &MAGIC.to_le_bytes()),
        (4, &encoding.to_le_bytes()),
        (8, &k.view.to_le_bytes()),
        (16, &k.layout.to_le_bytes()),
        (24, &k.row.to_le_bytes()),
        (32, &k.col.to_le_bytes()),
        (36, &k.size.to_le_bytes()),
        (40, &tile.width.to_le_bytes()),
        (44, &tile.height.to_le_bytes()),
    ];
    for (at, bytes) in fields {
        h[at..at + bytes.len()].copy_from_slice(bytes);
    }
    h
}

/// Encodes a tile message: QOI if it saves at least 20 %, raw otherwise.
pub fn encode_message(tile: &RenderedTile) -> Vec<u8> {
    let qoi = qoi::encode_to_vec(&tile.rgba, tile.width, tile.height).ok();
    let (encoding, payload): (u32, &[u8]) = match &qoi {
        Some(q) if q.len() * 5 <= tile.rgba.len() * 4 => (ENCODING_QOI, q),
        _ => (ENCODING_RAW, &tile.rgba),
    };
    let mut msg = Vec::with_capacity(HEADER_LEN + payload.len());
    msg.extend_from_slice(&header(tile, encoding));
    msg.extend_from_slice(payload);
    msg
}

/// One Channel per view, created by the command that opened the view, so
/// message ordering holds (ADR-0004).
pub struct ChannelSink {
    channel: Channel<InvokeResponseBody>,
}

impl ChannelSink {
    pub fn new(channel: Channel<InvokeResponseBody>) -> Self {
        Self { channel }
    }
}

impl TileSink for ChannelSink {
    fn deliver(&self, tile: RenderedTile) {
        if let Err(err) = self
            .channel
            .send(InvokeResponseBody::Raw(encode_message(&tile)))
        {
            tracing::warn!(%err, "tile channel send failed");
        }
    }

    fn name(&self) -> &'static str {
        "channel"
    }
}

#[cfg(windows)]
pub use shared::SharedBufferSink;

#[cfg(windows)]
mod shared {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use quire_core::tile::RenderedTile;
    use tauri::webview::PlatformWebview;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY, ICoreWebView2_17, ICoreWebView2Environment12,
    };
    use windows_core::{HSTRING, Interface};

    use super::{ChannelSink, TileSink};

    /// Posts raw tiles as WebView2 shared buffers. Falls back to the channel
    /// for good after the first failure, so a broken path degrades instead of
    /// leaving the view blank.
    pub struct SharedBufferSink {
        window: tauri::WebviewWindow,
        fallback: Arc<ChannelSink>,
        failed: Arc<AtomicBool>,
    }

    impl SharedBufferSink {
        pub fn new(window: tauri::WebviewWindow, fallback: ChannelSink) -> Self {
            Self {
                window,
                fallback: Arc::new(fallback),
                failed: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl TileSink for SharedBufferSink {
        fn deliver(&self, tile: RenderedTile) {
            if self.failed.load(Ordering::Acquire) {
                return self.fallback.deliver(tile);
            }
            let failed = Arc::clone(&self.failed);
            let fallback = Arc::clone(&self.fallback);
            let posted = self.window.with_webview(move |webview| {
                // SAFETY: with_webview runs this on the UI thread that owns
                // the WebView2 objects.
                if let Err(err) = unsafe { post(&webview, &tile) } {
                    tracing::error!(%err, "SharedBuffer post failed; using the channel from now on");
                    failed.store(true, Ordering::Release);
                    fallback.deliver(tile);
                }
            });
            if let Err(err) = posted {
                tracing::warn!(%err, "with_webview failed");
            }
        }

        fn name(&self) -> &'static str {
            if self.failed.load(Ordering::Acquire) {
                "channel (SharedBuffer failed)"
            } else {
                "shared-buffer"
            }
        }
    }

    /// # Safety
    /// Must run on the webview's UI thread.
    unsafe fn post(webview: &PlatformWebview, tile: &RenderedTile) -> windows_core::Result<()> {
        let k = &tile.key;
        let json = HSTRING::from(format!(
            r#"{{"quireTile":1,"view":{},"layout":{},"row":{},"col":{},"size":{},"width":{},"height":{}}}"#,
            k.view, k.layout, k.row, k.col, k.size, tile.width, tile.height
        ));
        // SAFETY: COM calls on live interfaces owned by this webview, on its
        // UI thread; the copy stays within the buffer's requested size.
        unsafe {
            let env: ICoreWebView2Environment12 = webview.environment().cast()?;
            let buffer = env.CreateSharedBuffer(tile.rgba.len() as u64)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Buffer(&mut ptr)?;
            std::ptr::copy_nonoverlapping(tile.rgba.as_ptr(), ptr, tile.rgba.len());
            let core: ICoreWebView2_17 = webview.controller().CoreWebView2()?.cast()?;
            core.PostSharedBufferToScript(
                &buffer,
                COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
                &json,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quire_core::tile::TileKey;

    fn tile(fill: impl FnMut(usize) -> u8) -> RenderedTile {
        let rgba = (0..64 * 64 * 4).map(fill).collect();
        RenderedTile {
            key: TileKey {
                view: 7,
                layout: 9,
                col: 2,
                row: 123_456_789_012,
                size: 64,
            },
            width: 64,
            height: 64,
            rgba,
        }
    }

    fn u32_at(m: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(m[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn header_fields_round_trip() {
        let m = encode_message(&tile(|_| 255));
        assert_eq!(&m[0..4], b"QTL1");
        assert_eq!(u64::from_le_bytes(m[8..16].try_into().unwrap()), 7);
        assert_eq!(u64::from_le_bytes(m[16..24].try_into().unwrap()), 9);
        assert_eq!(
            u64::from_le_bytes(m[24..32].try_into().unwrap()),
            123_456_789_012
        );
        assert_eq!((u32_at(&m, 32), u32_at(&m, 36)), (2, 64));
        assert_eq!((u32_at(&m, 40), u32_at(&m, 44)), (64, 64));
    }

    #[test]
    fn compressible_tiles_use_qoi_and_noise_stays_raw() {
        let flat = encode_message(&tile(|_| 255));
        assert_eq!(u32_at(&flat, 4), ENCODING_QOI);
        assert!(flat.len() < HEADER_LEN + 64 * 64);

        let mut x: u32 = 1;
        let noisy = encode_message(&tile(|i| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if i % 4 == 3 { 255 } else { (x >> 16) as u8 }
        }));
        assert_eq!(u32_at(&noisy, 4), ENCODING_RAW);
        assert_eq!(noisy.len(), HEADER_LEN + 64 * 64 * 4);
    }
}
