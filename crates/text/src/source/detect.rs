//! Detects files that must not be memory-mapped: network drives and cloud
//! placeholders (OneDrive Files On-Demand and similar). A read error behind a
//! mapping of such a file is likely, and costly (§5.5).

use std::path::Path;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteKind {
    /// On a network drive or share.
    Network,
    /// A cloud-sync placeholder whose data may be fetched on access.
    CloudPlaceholder,
}

/// Whether `path` is remote or a placeholder; `None` for ordinary local files.
pub fn remote_kind(path: &Path) -> Option<RemoteKind> {
    imp::remote_kind(path)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
        FILE_ATTRIBUTE_RECALL_ON_OPEN, GetDriveTypeW, GetFileAttributesW, GetVolumePathNameW,
        INVALID_FILE_ATTRIBUTES,
    };

    use super::RemoteKind;

    const DRIVE_REMOTE: u32 = 4;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub fn remote_kind(path: &Path) -> Option<RemoteKind> {
        let s = path.as_os_str().to_string_lossy();
        let unc = (s.starts_with(r"\\") && !s.starts_with(r"\\?\") && !s.starts_with(r"\\.\"))
            || s.starts_with(r"\\?\UNC\");
        // Checked first: touching a share that is down can block for seconds.
        if unc {
            return Some(RemoteKind::Network);
        }
        let w = wide(path);
        // SAFETY: `w` is NUL-terminated.
        let attrs = unsafe { GetFileAttributesW(w.as_ptr()) };
        if attrs != INVALID_FILE_ATTRIBUTES
            && attrs
                & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                    | FILE_ATTRIBUTE_RECALL_ON_OPEN
                    | FILE_ATTRIBUTE_OFFLINE)
                != 0
        {
            return Some(RemoteKind::CloudPlaceholder);
        }
        let mut root = [0u16; 1024];
        // SAFETY: `w` is NUL-terminated and `root` holds `root.len()` units.
        let ok = unsafe { GetVolumePathNameW(w.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
        // SAFETY: on success `root` holds a NUL-terminated volume path.
        if ok != 0 && unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE {
            return Some(RemoteKind::Network);
        }
        None
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use super::RemoteKind;

    // statfs f_type values of network and FUSE file systems.
    const REMOTE_MAGIC: &[i64] = &[
        0x6969,                 // NFS
        0x517B,                 // SMB
        0xFF53_4D42_u32 as i64, // CIFS
        0xFE53_4D42_u32 as i64, // SMB2
        0x6573_5546,            // FUSE (sshfs, rclone, cloud clients)
        0x0102_1997,            // 9P
        0x7375_7245,            // Coda
        0x5346_414F,            // AFS
    ];

    pub fn remote_kind(path: &Path) -> Option<RemoteKind> {
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: zeroed statfs is a valid out-parameter.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is NUL-terminated; `st` is writable.
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        #[allow(clippy::unnecessary_cast)]
        let magic = st.f_type as i64;
        REMOTE_MAGIC.contains(&magic).then_some(RemoteKind::Network)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use super::RemoteKind;

    pub fn remote_kind(path: &Path) -> Option<RemoteKind> {
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: zeroed statfs is a valid out-parameter.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is NUL-terminated; `st` is writable.
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        (st.f_flags & libc::MNT_LOCAL as u32 == 0).then_some(RemoteKind::Network)
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use std::path::Path;

    use super::RemoteKind;

    pub fn remote_kind(_: &Path) -> Option<RemoteKind> {
        None
    }
}
