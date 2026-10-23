//! The host ↔ worker protocol (ADR-0036): length-prefixed binary frames over
//! the worker's stdin and stdout, little-endian, no serialization library.
//!
//! A frame is `len: u32` (bytes that follow), `id: u64`, `tag: u8`, then the
//! payload. Responses carry the id of the request they answer and, before
//! their payload, the worker's private bytes (`mem: u64`) so the host can
//! charge them to the memory governor without asking.
//!
//! The host decodes what a worker sends, and a worker that has parsed a
//! hostile file may be compromised, so decoding never panics, never trusts
//! a length beyond the frame, and caps allocations (`MAX_FRAME`).

use std::io::{self, Read, Write};

/// Bumped on any change to a message's layout.
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest frame either side accepts: a 4096 × 4096 RGBA tile plus slack.
pub const MAX_FRAME: usize = 4096 * 4096 * 4 + 4096;

/// A 2D affine transform from page space (points, origin at the page's top
/// left, y down, after the page's own `/Rotate`) to bitmap pixels:
/// `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Matrix {
    /// Scale by `scale` px per point, then move `(x, y)` (bitmap pixels at
    /// that scale) to the bitmap's origin.
    pub fn scale_translate(scale: f32, x: f32, y: f32) -> Self {
        Self {
            a: scale,
            b: 0.0,
            c: 0.0,
            d: scale,
            e: -x,
            f: -y,
        }
    }
}

/// Render flags, as PDFium defines them (`fpdfview.h`).
pub mod flags {
    /// Draw annotations that have an appearance (not widgets or popups).
    pub const ANNOT: u32 = 0x01;
    /// LCD-optimized text: not used (tiles are scaled and composited).
    pub const LCD_TEXT: u32 = 0x02;
    pub const GRAYSCALE: u32 = 0x08;
    pub const PRINTING: u32 = 0x800;
}

/// PDFium's `FPDF_ERR_*` codes, plus our own above 100.
pub mod errors {
    pub const UNKNOWN: u32 = 1;
    pub const FILE: u32 = 2;
    pub const FORMAT: u32 = 3;
    pub const PASSWORD: u32 = 4;
    pub const SECURITY: u32 = 5;
    pub const PAGE: u32 = 6;
    /// The worker has no open document with that id.
    pub const NO_DOCUMENT: u32 = 101;
    /// A request the worker can't decode or doesn't know.
    pub const BAD_REQUEST: u32 = 102;
    /// A bitmap that would exceed `MAX_FRAME`.
    pub const TOO_LARGE: u32 = 103;
}

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Hello {
        protocol: u32,
    },
    /// Opens `path` as document `doc`. `overlay` is appended to the file's
    /// bytes as the worker sees them: unsaved incremental updates and view
    /// state (ADR-0037).
    Open {
        doc: u64,
        path: String,
        password: Option<String>,
        overlay: Vec<u8>,
    },
    Close {
        doc: u64,
    },
    /// Sizes of pages `[from, to)`, in points, after `/Rotate`.
    PageSizes {
        doc: u64,
        from: u32,
        to: u32,
    },
    /// Renders into an opaque `width × height` RGBA bitmap filled with white.
    Render {
        doc: u64,
        page: u32,
        matrix: Matrix,
        width: u32,
        height: u32,
        flags: u32,
    },
    /// Test hook: the worker aborts. Honoured only when the worker was
    /// started with `QUIRE_PDF_FAULTS=1`.
    Crash,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Hello {
        protocol: u32,
        pdfium: String,
    },
    Opened {
        pages: u32,
        /// False when PDFium had to rebuild the cross-reference table.
        valid_xref: bool,
        /// PDF version × 10 (14 = PDF 1.4), 0 if unknown.
        version: u32,
    },
    Closed,
    PageSizes {
        sizes: Vec<(f32, f32)>,
    },
    Pixels {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Error {
        code: u32,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    /// The worker's private bytes when it sent this.
    pub mem: u64,
    pub body: Body,
}

// --- Encoding -------------------------------------------------------------

struct Enc(Vec<u8>);

impl Enc {
    fn new(id: u64, tag: u8) -> Self {
        let mut v = Vec::with_capacity(64);
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(&id.to_le_bytes());
        v.push(tag);
        Enc(v)
    }
    fn u8(&mut self, x: u8) {
        self.0.push(x);
    }
    fn u32(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn u64(&mut self, x: u64) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn f32(&mut self, x: f32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn finish(mut self) -> Vec<u8> {
        let len = (self.0.len() - 4) as u32;
        self.0[..4].copy_from_slice(&len.to_le_bytes());
        self.0
    }
}

pub fn encode_request(id: u64, req: &Request) -> Vec<u8> {
    let mut e;
    match req {
        Request::Hello { protocol } => {
            e = Enc::new(id, 1);
            e.u32(*protocol);
        }
        Request::Open {
            doc,
            path,
            password,
            overlay,
        } => {
            e = Enc::new(id, 2);
            e.u64(*doc);
            e.str(path);
            match password {
                Some(p) => {
                    e.u8(1);
                    e.str(p);
                }
                None => e.u8(0),
            }
            e.bytes(overlay);
        }
        Request::Close { doc } => {
            e = Enc::new(id, 3);
            e.u64(*doc);
        }
        Request::PageSizes { doc, from, to } => {
            e = Enc::new(id, 4);
            e.u64(*doc);
            e.u32(*from);
            e.u32(*to);
        }
        Request::Render {
            doc,
            page,
            matrix,
            width,
            height,
            flags,
        } => {
            e = Enc::new(id, 5);
            e.u64(*doc);
            e.u32(*page);
            for v in [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f] {
                e.f32(v);
            }
            e.u32(*width);
            e.u32(*height);
            e.u32(*flags);
        }
        Request::Crash => e = Enc::new(id, 99),
    }
    e.finish()
}

pub fn encode_response(id: u64, resp: &Response) -> Vec<u8> {
    let tag = match &resp.body {
        Body::Hello { .. } => 1,
        Body::Opened { .. } => 2,
        Body::Closed => 3,
        Body::PageSizes { .. } => 4,
        Body::Pixels { .. } => 5,
        Body::Error { .. } => 6,
    };
    let mut e = Enc::new(id, tag);
    e.u64(resp.mem);
    match &resp.body {
        Body::Hello { protocol, pdfium } => {
            e.u32(*protocol);
            e.str(pdfium);
        }
        Body::Opened {
            pages,
            valid_xref,
            version,
        } => {
            e.u32(*pages);
            e.u8(u8::from(*valid_xref));
            e.u32(*version);
        }
        Body::Closed => {}
        Body::PageSizes { sizes } => {
            e.u32(sizes.len() as u32);
            for &(w, h) in sizes {
                e.f32(w);
                e.f32(h);
            }
        }
        Body::Pixels {
            width,
            height,
            rgba,
        } => {
            e.u32(*width);
            e.u32(*height);
            e.bytes(rgba);
        }
        Body::Error { code, message } => {
            e.u32(*code);
            e.str(message);
        }
    }
    e.finish()
}

// --- Decoding -------------------------------------------------------------

/// A frame that doesn't decode. The host treats the worker as broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub &'static str);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bad frame: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

struct Dec<'a>(&'a [u8]);

impl<'a> Dec<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.0.len() < n {
            return Err(DecodeError("truncated"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().unwrap_or([0; 4]),
        ))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().unwrap_or([0; 8]),
        ))
    }
    fn f32(&mut self) -> Result<f32, DecodeError> {
        let v = f32::from_le_bytes(self.take(4)?.try_into().unwrap_or([0; 4]));
        if v.is_finite() {
            Ok(v)
        } else {
            Err(DecodeError("non-finite number"))
        }
    }
    fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn string(&mut self) -> Result<String, DecodeError> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| DecodeError("invalid UTF-8"))
    }
    fn end(&self) -> Result<(), DecodeError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(DecodeError("trailing bytes"))
        }
    }
}

/// Splits a frame body (everything after the length) into id, tag, payload.
fn header(frame: &[u8]) -> Result<(u64, u8, Dec<'_>), DecodeError> {
    let mut d = Dec(frame);
    let id = d.u64()?;
    let tag = d.u8()?;
    Ok((id, tag, d))
}

pub fn decode_request(frame: &[u8]) -> Result<(u64, Request), DecodeError> {
    let (id, tag, mut d) = header(frame)?;
    let req = match tag {
        1 => Request::Hello { protocol: d.u32()? },
        2 => {
            let doc = d.u64()?;
            let path = d.string()?;
            let password = match d.u8()? {
                0 => None,
                1 => Some(d.string()?),
                _ => return Err(DecodeError("bad option")),
            };
            let overlay = d.bytes()?.to_vec();
            Request::Open {
                doc,
                path,
                password,
                overlay,
            }
        }
        3 => Request::Close { doc: d.u64()? },
        4 => Request::PageSizes {
            doc: d.u64()?,
            from: d.u32()?,
            to: d.u32()?,
        },
        5 => {
            let doc = d.u64()?;
            let page = d.u32()?;
            let matrix = Matrix {
                a: d.f32()?,
                b: d.f32()?,
                c: d.f32()?,
                d: d.f32()?,
                e: d.f32()?,
                f: d.f32()?,
            };
            Request::Render {
                doc,
                page,
                matrix,
                width: d.u32()?,
                height: d.u32()?,
                flags: d.u32()?,
            }
        }
        99 => Request::Crash,
        _ => return Err(DecodeError("unknown request")),
    };
    d.end()?;
    Ok((id, req))
}

pub fn decode_response(frame: &[u8]) -> Result<(u64, Response), DecodeError> {
    let (id, tag, mut d) = header(frame)?;
    let mem = d.u64()?;
    let body = match tag {
        1 => Body::Hello {
            protocol: d.u32()?,
            pdfium: d.string()?,
        },
        2 => Body::Opened {
            pages: d.u32()?,
            valid_xref: d.u8()? != 0,
            version: d.u32()?,
        },
        3 => Body::Closed,
        4 => {
            let n = d.u32()? as usize;
            if n > d.0.len() / 8 {
                return Err(DecodeError("too many sizes"));
            }
            let mut sizes = Vec::with_capacity(n);
            for _ in 0..n {
                sizes.push((d.f32()?, d.f32()?));
            }
            Body::PageSizes { sizes }
        }
        5 => {
            let width = d.u32()?;
            let height = d.u32()?;
            let rgba = d.bytes()?;
            if u64::from(width) * u64::from(height) * 4 != rgba.len() as u64 {
                return Err(DecodeError("pixel count"));
            }
            Body::Pixels {
                width,
                height,
                rgba: rgba.to_vec(),
            }
        }
        6 => Body::Error {
            code: d.u32()?,
            message: d.string()?,
        },
        _ => return Err(DecodeError("unknown response")),
    };
    d.end()?;
    Ok((id, Response { mem, body }))
}

/// Reads one frame body. `Ok(None)` at a clean end of stream.
pub fn read_frame(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if !(9..=MAX_FRAME).contains(&len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len}"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

pub fn write_frame(w: &mut impl Write, frame: &[u8]) -> io::Result<()> {
    w.write_all(frame)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_req(req: Request) {
        let f = encode_request(7, &req);
        let body = read_frame(&mut &f[..]).unwrap().unwrap();
        assert_eq!(decode_request(&body).unwrap(), (7, req));
    }

    fn roundtrip_resp(body: Body) {
        let resp = Response { mem: 123, body };
        let f = encode_response(9, &resp);
        let b = read_frame(&mut &f[..]).unwrap().unwrap();
        assert_eq!(decode_response(&b).unwrap(), (9, resp));
    }

    #[test]
    fn requests_round_trip() {
        roundtrip_req(Request::Hello { protocol: 1 });
        roundtrip_req(Request::Open {
            doc: 3,
            path: "C:\\문서\\a.pdf".into(),
            password: Some("pw".into()),
            overlay: vec![1, 2, 3],
        });
        roundtrip_req(Request::Open {
            doc: 3,
            path: "/x.pdf".into(),
            password: None,
            overlay: vec![],
        });
        roundtrip_req(Request::Close { doc: 3 });
        roundtrip_req(Request::PageSizes {
            doc: 3,
            from: 0,
            to: 10,
        });
        roundtrip_req(Request::Render {
            doc: 3,
            page: 4999,
            matrix: Matrix::scale_translate(16.0, 4000.5, 6000.0),
            width: 512,
            height: 512,
            flags: flags::ANNOT,
        });
        roundtrip_req(Request::Crash);
    }

    #[test]
    fn responses_round_trip() {
        roundtrip_resp(Body::Hello {
            protocol: 1,
            pdfium: "151.0.7881.0".into(),
        });
        roundtrip_resp(Body::Opened {
            pages: 5000,
            valid_xref: true,
            version: 17,
        });
        roundtrip_resp(Body::Closed);
        roundtrip_resp(Body::PageSizes {
            sizes: vec![(595.0, 842.0), (842.0, 595.0)],
        });
        roundtrip_resp(Body::Pixels {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
        });
        roundtrip_resp(Body::Error {
            code: errors::PASSWORD,
            message: "password".into(),
        });
    }

    #[test]
    fn hostile_frames_are_errors_not_panics() {
        // Pixel count that doesn't match the size.
        let mut e = Enc::new(1, 5);
        e.u64(0);
        e.u32(1000);
        e.u32(1000);
        e.bytes(&[0; 16]);
        let f = e.finish();
        assert!(decode_response(&f[4..]).is_err());
        // A size count far beyond the frame.
        let mut e = Enc::new(1, 4);
        e.u64(0);
        e.u32(u32::MAX);
        let f = e.finish();
        assert!(decode_response(&f[4..]).is_err());
        // Truncated, unknown tag, trailing bytes.
        assert!(decode_response(&[1, 2, 3]).is_err());
        let mut e = Enc::new(1, 77);
        e.u64(0);
        assert!(decode_response(&e.finish()[4..]).is_err());
        let mut e = Enc::new(1, 3);
        e.u64(0);
        e.u8(0);
        assert!(decode_response(&e.finish()[4..]).is_err());
        // A frame length over the cap.
        let big = ((MAX_FRAME + 1) as u32).to_le_bytes();
        assert!(read_frame(&mut &big[..]).is_err());
        // Non-finite numbers.
        let mut e = Enc::new(1, 4);
        e.u64(0);
        e.u32(1);
        e.f32(f32::NAN);
        e.f32(1.0);
        assert!(decode_response(&e.finish()[4..]).is_err());
    }
}
