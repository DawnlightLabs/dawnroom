//! Log files that rotate by size when they're opened (Windows can't rename a file that's open,
//! so each writer rotates its own log before opening it): `x.log` → `x.log.1` → … → `x.log.KEEP`.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// 10 MB per log, three old ones kept (must-fix 10).
pub const MAX_BYTES: u64 = 10_000_000;
pub const KEEP: u32 = 3;

fn numbered(path: &Path, n: u32) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

/// Shifts the old logs along if `path` has grown past `max`. Errors are ignored: a log that can't
/// rotate just keeps growing, which is better than not starting.
pub fn rotate(path: &Path, max: u64, keep: u32) {
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) <= max {
        return;
    }
    let _ = std::fs::remove_file(numbered(path, keep));
    for n in (1..keep).rev() {
        let _ = std::fs::rename(numbered(path, n), numbered(path, n + 1));
    }
    let _ = std::fs::rename(path, numbered(path, 1));
}

/// Rotates if needed, then opens for appending (creating the folder and file).
pub fn open_append(path: &Path) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    rotate(path, MAX_BYTES, KEEP);
    OpenOptions::new().create(true).append(true).open(path)
}

/// Rotated logs in `dir` (`*.log.N`): rebuildable, so "Clear cache" may remove them.
pub fn rotated(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            n.rsplit_once(".log.").is_some_and(|(_, tail)| !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_by_size_and_keeps_a_few() {
        let dir = std::env::temp_dir().join(format!("logs-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("sd-server.log");
        for round in 0..5 {
            std::fs::write(&log, vec![b'x'; 20 + round]).unwrap();
            rotate(&log, 10, 3);
        }
        assert!(!log.exists(), "the big log moved to .1");
        let mut kept: Vec<String> = rotated(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        kept.sort();
        assert_eq!(kept, ["sd-server.log.1", "sd-server.log.2", "sd-server.log.3"]);
        assert_eq!(std::fs::metadata(dir.join("sd-server.log.1")).unwrap().len(), 24, "newest is .1");
        std::fs::write(&log, b"small").unwrap();
        rotate(&log, 10, 3);
        assert!(log.exists(), "a small log stays");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
