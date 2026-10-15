//! Pasted images (M2 task 10, user decision A): a clipboard image becomes a
//! PNG in `assets/` next to the Markdown document, named
//! `image-<yyyymmdd>-<hash8>.png` and deduplicated by content, and the
//! frontend inserts a relative link to it (one undo step, a paste).

use std::io::{self, Write as _};
use std::path::Path;

use xxhash_rust::xxh3::xxh3_64;

/// Larger clipboard images are refused.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
const ASSETS: &str = "assets";

/// The image as PNG bytes: PNG kept as it is, other formats converted.
pub fn to_png(bytes: &[u8]) -> io::Result<Vec<u8>> {
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "image too large",
        ));
    }
    let format = image::guess_format(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not an image"))?;
    if format == image::ImageFormat::Png {
        // Decodable, not just named PNG.
        image::load_from_memory_with_format(bytes, format).map_err(io::Error::other)?;
        return Ok(bytes.to_vec());
    }
    let img = image::load_from_memory_with_format(bytes, format).map_err(io::Error::other)?;
    let mut out = Vec::new();
    img.write_to(&mut io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(io::Error::other)?;
    Ok(out)
}

/// Stores `png` under `<dir>/assets/` and returns the link to insert
/// (`assets/image-….png`). An identical image already there is reused,
/// whatever its date.
pub fn store(dir: &Path, png: &[u8], date: &str) -> io::Result<String> {
    let hash = format!("{:016x}", xxh3_64(png));
    let hash8 = &hash[..8];
    let assets = dir.join(ASSETS);
    std::fs::create_dir_all(&assets)?;
    let suffix = format!("-{hash8}.png");
    for e in std::fs::read_dir(&assets)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("image-")
            && name.ends_with(&suffix)
            && std::fs::read(e.path()).is_ok_and(|b| b == png)
        {
            return Ok(format!("{ASSETS}/{name}"));
        }
    }
    let mut name = format!("image-{date}{suffix}");
    // The same name with other bytes (a hash8 collision): add a counter.
    let mut n = 2;
    while assets.join(&name).exists() {
        name = format!("image-{date}-{hash8}-{n}.png");
        n += 1;
    }
    let tmp = assets.join(format!(".{name}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(png)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, assets.join(&name)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(format!("{ASSETS}/{name}"))
}

/// Today's local date as `yyyymmdd`.
pub fn today() -> String {
    #[cfg(windows)]
    {
        // SAFETY: GetLocalTime fills the struct it is given.
        let t = unsafe {
            let mut t = std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>();
            windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
            t
        };
        format!("{:04}{:02}{:02}", t.wYear, t.wMonth, t.wDay)
    }
    #[cfg(not(windows))]
    {
        // UTC (civil date from days since the epoch).
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let z = (secs / 86_400) as i64 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}{m:02}{d:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, c: u8) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([c, 0, 0, 255]));
        let mut out = Vec::new();
        img.write_to(&mut io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn stores_once_per_content() {
        let dir = std::env::temp_dir().join(format!("quire-paste-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = png(4, 4, 1);
        let b = png(4, 4, 2);
        let la = store(&dir, &a, "20261015").unwrap();
        assert!(
            la.starts_with("assets/image-20261015-") && la.ends_with(".png"),
            "{la}"
        );
        assert_eq!(
            store(&dir, &a, "20261016").unwrap(),
            la,
            "same bytes: same file"
        );
        let lb = store(&dir, &b, "20261015").unwrap();
        assert_ne!(lb, la);
        assert_eq!(std::fs::read(dir.join(&la)).unwrap(), a);
        let names: Vec<_> = std::fs::read_dir(dir.join("assets"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(names.len(), 2, "no temporary files left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn converts_and_refuses() {
        let p = png(2, 2, 3);
        assert_eq!(to_png(&p).unwrap(), p, "PNG kept as it is");
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]));
        let mut bmp = Vec::new();
        img.write_to(&mut io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .unwrap();
        let out = to_png(&bmp).unwrap();
        assert_eq!(image::guess_format(&out).unwrap(), image::ImageFormat::Png);
        assert!(to_png(b"not an image").is_err());
        let mut broken = p.clone();
        broken.truncate(20);
        assert!(to_png(&broken).is_err());
    }

    #[test]
    fn today_is_a_date() {
        let d = today();
        assert_eq!(d.len(), 8);
        assert!(d.starts_with("20"));
    }
}
