//! Free space on the data drive, and the "will it fit" check before any download.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::backend::BackendError;

/// Kept free on top of what a download needs, so a pull never fills the drive to the last byte.
pub const RESERVE_BYTES: u64 = 500_000_000;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Space {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// Space on the drive holding `path` (which may not exist yet). `DAWNROOM_FAKE_FREE_BYTES`
/// replaces the free number, to test the refusal without filling a disk.
pub fn space(path: &Path) -> Option<Space> {
    let at = existing_ancestor(path)?;
    let mut s = platform_space(&at)?;
    if let Some(fake) = std::env::var("DAWNROOM_FAKE_FREE_BYTES").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        s.free_bytes = fake;
    }
    Some(s)
}

fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().ok()?.join(path) };
    abs.ancestors().find(|p| p.exists()).map(Path::to_path_buf)
}

/// Bytes as people read them: "7.1 GB", "350 MB" (decimal units, like Windows Explorer's sizes on the site).
pub fn human(bytes: u64) -> String {
    if bytes >= 1_000_000_000 { format!("{:.1} GB", bytes as f64 / 1e9) } else { format!("{} MB", (bytes as f64 / 1e6).ceil() as u64) }
}

/// Refuses politely when `needed` bytes (plus the reserve) won't fit next to `path`.
/// Unknown free space (an odd filesystem) doesn't block: the download itself reports a full disk.
pub fn check(path: &Path, needed: u64, what: &str) -> Result<(), BackendError> {
    if needed == 0 {
        return Ok(());
    }
    let Some(s) = space(path) else { return Ok(()) };
    if needed + RESERVE_BYTES <= s.free_bytes {
        return Ok(());
    }
    let drive = existing_ancestor(path).and_then(|p| p.ancestors().last().map(|r| r.display().to_string())).unwrap_or_default();
    let short = (needed + RESERVE_BYTES).saturating_sub(s.free_bytes);
    Err(BackendError::with_message(
        "not_enough_space",
        format!(
            "{what} needs {} but the drive {drive} has only {} free (Dawnroom keeps {} spare). Free up {} and try again.",
            human(needed),
            human(s.free_bytes),
            human(RESERVE_BYTES),
            human(short)
        ),
    ))
}

#[cfg(windows)]
fn platform_space(path: &Path) -> Option<Space> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::HSTRING;
    let (mut free, mut total) = (0u64, 0u64);
    let p = HSTRING::from(path.as_os_str());
    unsafe { GetDiskFreeSpaceExW(&p, Some(&mut free), Some(&mut total), None) }.ok()?;
    Some(Space { free_bytes: free, total_bytes: total })
}

#[cfg(unix)]
fn platform_space(path: &Path) -> Option<Space> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    Some(Space { free_bytes: s.f_bavail as u64 * s.f_frsize as u64, total_bytes: s.f_blocks as u64 * s.f_frsize as u64 })
}

/// A drive the data folder could live on (first run's "Where models live").
#[derive(Debug, Clone, Serialize)]
pub struct Drive {
    /// `C:\` on Windows, the mount point on Linux.
    pub root: String,
    /// The volume label ("Data"), when it has one.
    pub label: Option<String>,
    pub free_bytes: u64,
    pub total_bytes: u64,
    /// Where the data folder would go on this drive: the default folder on the default drive,
    /// else `<root>Dawnroom`.
    pub path: String,
    /// The data folder is on this drive now.
    pub current: bool,
}

/// Fixed drives (no removable, network or optical ones), with their space.
pub fn drives(data_dir: &Path, default_dir: &Path) -> Vec<Drive> {
    let on = |root: &str, p: &Path| existing_ancestor(p).is_some_and(|a| root_of(&a).eq_ignore_ascii_case(root));
    roots()
        .into_iter()
        .filter_map(|(root, label)| {
            let s = platform_space(Path::new(&root))?;
            let current = on(&root, data_dir);
            let path = if current {
                data_dir.display().to_string()
            } else if on(&root, default_dir) {
                default_dir.display().to_string()
            } else {
                Path::new(&root).join(crate::config::PRODUCT_NAME).display().to_string()
            };
            Some(Drive { current, path, label, free_bytes: s.free_bytes, total_bytes: s.total_bytes, root })
        })
        .collect()
}

/// The drive root a path is on: `C:\` on Windows, the deepest listed mount point on Linux.
fn root_of(p: &Path) -> String {
    if cfg!(windows) {
        return p.ancestors().last().map(|r| r.display().to_string()).unwrap_or_default().trim_start_matches(r"\\?\").to_string();
    }
    roots().into_iter().map(|(r, _)| r).filter(|r| p.starts_with(r)).max_by_key(|r| r.len()).unwrap_or_else(|| "/".into())
}

#[cfg(windows)]
fn roots() -> Vec<(String, Option<String>)> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    use windows::core::HSTRING;
    const DRIVE_FIXED: u32 = 3;
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let root = format!("{}:\\", (b'A' + i) as char);
            let h = HSTRING::from(root.as_str());
            if unsafe { GetDriveTypeW(&h) } != DRIVE_FIXED {
                return None;
            }
            let mut name = [0u16; 128];
            let label = unsafe { GetVolumeInformationW(&h, Some(&mut name), None, None, None, None) }.ok().map(|_| String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(0)])).filter(|l| !l.trim().is_empty());
            Some((root, label))
        })
        .collect()
}

#[cfg(unix)]
fn roots() -> Vec<(String, Option<String>)> {
    const REAL: &[&str] = &["ext4", "ext3", "btrfs", "xfs", "f2fs", "zfs", "ntfs", "ntfs3", "exfat", "vfat"];
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    mounts
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let (dev, at, fs) = (*f.first()?, f.get(1)?.replace("\\040", " "), *f.get(2)?);
            let wanted = at == "/" || ["/home", "/mnt/", "/media/", "/run/media/", "/data"].iter().any(|p| at.starts_with(p));
            (REAL.contains(&fs) && wanted && seen.insert(dev.to_string())).then(|| (at.clone(), None))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_space_for_a_folder_that_does_not_exist_yet() {
        let s = space(&std::env::temp_dir().join("dawnroom-nope").join("deeper")).unwrap();
        assert!(s.total_bytes > 0 && s.free_bytes <= s.total_bytes);
    }

    #[test]
    fn lists_the_drive_the_data_folder_is_on() {
        let tmp = std::env::temp_dir();
        let d = drives(&tmp, &tmp);
        assert!(!d.is_empty());
        let here = d.iter().find(|x| x.current).expect("the temp folder's drive is listed");
        assert_eq!(here.path, tmp.display().to_string(), "the default folder on its own drive");
        assert!(here.free_bytes <= here.total_bytes);
        assert!(d.iter().filter(|x| !x.current).all(|x| x.path.ends_with("Dawnroom")));
    }

    #[test]
    fn refuses_with_both_numbers() {
        // Needs more than any disk has: the message carries what's needed and what's free.
        let e = check(&std::env::temp_dir(), u64::MAX / 4, "This model").unwrap_err();
        assert_eq!(e.code, "not_enough_space");
        assert!(e.message.contains("needs") && e.message.contains("free"), "{}", e.message);
        assert!(check(&std::env::temp_dir(), 1, "x").is_ok());
        assert!(check(&std::env::temp_dir(), 0, "x").is_ok());
        assert_eq!(human(7_100_000_000), "7.1 GB");
        assert_eq!(human(30_023_026), "31 MB");
    }
}
