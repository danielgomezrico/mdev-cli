//! Parallel, symlink-safe directory sizing and human-readable formatting.

use std::path::Path;

use crate::parallel;

/// Below this many entries a directory is summed sequentially — spawning
/// pool work for tiny dirs costs more than it saves.
const PARALLEL_THRESHOLD: usize = 32;

/// How to measure a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeMode {
    /// `metadata.len()` — logical bytes (exact; used by tests).
    #[cfg_attr(not(test), allow(dead_code))]
    Apparent,
    /// Allocated blocks × 512 on unix (what `du` reports); `Apparent` elsewhere.
    Disk,
}

/// Total bytes under `path`. Symlinks are never followed (a link counts 0);
/// unreadable entries count 0; a plain file returns its own size; a missing
/// path returns 0.
pub fn dir_size(path: &Path, mode: SizeMode) -> u64 {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let ft = meta.file_type();
    if ft.is_symlink() {
        return 0;
    }
    if ft.is_dir() {
        return sum_dir(path, mode);
    }
    file_size(&meta, mode)
}

fn sum_dir(dir: &Path, mode: SizeMode) -> u64 {
    let entries: Vec<std::fs::DirEntry> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(_) => return 0,
    };
    let size_of = |entry: std::fs::DirEntry| -> u64 {
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => return 0,
        };
        if ft.is_symlink() {
            0
        } else if ft.is_dir() {
            sum_dir(&entry.path(), mode)
        } else {
            entry.metadata().map(|m| file_size(&m, mode)).unwrap_or(0)
        }
    };
    if entries.len() < PARALLEL_THRESHOLD {
        entries
            .into_iter()
            .fold(0u64, |acc, e| acc.saturating_add(size_of(e)))
    } else {
        parallel::map(entries, size_of)
            .into_iter()
            .fold(0u64, |acc, s| acc.saturating_add(s))
    }
}

#[cfg(unix)]
fn file_size(meta: &std::fs::Metadata, mode: SizeMode) -> u64 {
    use std::os::unix::fs::MetadataExt;
    match mode {
        SizeMode::Apparent => meta.len(),
        SizeMode::Disk => meta.blocks().saturating_mul(512),
    }
}

#[cfg(not(unix))]
fn file_size(meta: &std::fs::Metadata, _mode: SizeMode) -> u64 {
    meta.len()
}

/// `1.23 GB` / `4.50 MB` / `12.0 KB` / `512 B`.
pub fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn sums_nested_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("x"), vec![0u8; 100]).unwrap();
        fs::write(root.join("a/y"), vec![0u8; 200]).unwrap();
        fs::write(root.join("a/b/z"), vec![0u8; 300]).unwrap();
        assert_eq!(dir_size(root, SizeMode::Apparent), 600);
    }

    #[test]
    fn wide_dir_uses_parallel_path_and_still_sums() {
        let tmp = TempDir::new().unwrap();
        for i in 0..(PARALLEL_THRESHOLD + 10) {
            fs::write(tmp.path().join(format!("f{i}")), vec![0u8; 10]).unwrap();
        }
        assert_eq!(
            dir_size(tmp.path(), SizeMode::Apparent),
            10 * (PARALLEL_THRESHOLD as u64 + 10)
        );
    }

    #[test]
    fn file_root_returns_its_size() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("f");
        fs::write(&f, vec![0u8; 42]).unwrap();
        assert_eq!(dir_size(&f, SizeMode::Apparent), 42);
    }

    #[test]
    fn missing_path_is_zero() {
        assert_eq!(
            dir_size(Path::new("/definitely/not/here"), SizeMode::Apparent),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("big"), vec![0u8; 10_000]).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        fs::write(tmp.path().join("own"), vec![0u8; 5]).unwrap();
        assert_eq!(dir_size(tmp.path(), SizeMode::Apparent), 5);
        assert_eq!(dir_size(&tmp.path().join("link"), SizeMode::Apparent), 0);
    }

    #[cfg(unix)]
    #[test]
    fn disk_mode_counts_blocks() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("f"), vec![0u8; 1]).unwrap();
        let disk = dir_size(tmp.path(), SizeMode::Disk);
        assert!(
            disk == 0 || disk.is_multiple_of(512),
            "disk size {disk} not block-aligned"
        );
    }

    #[test]
    fn human_size_table() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.00 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.00 GB");
    }
}
