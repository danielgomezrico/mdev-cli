use colored::Colorize;
use std::path::{Path, PathBuf};

use crate::commands::purge::PurgeArgs;
use crate::logger::Logger;

/// Shared cleaner contract for per-project-type modules.
///
/// Wave-2 workers may implement this trait on a zero-sized struct per language
/// module; today the per-project dispatchers expose a free `pub fn run(...)`
/// with the same four-argument shape.
#[allow(dead_code)]
pub trait Cleaner {
    fn clean(&self, args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool);
}

/// Confirm prompt wrapper — keeps a single place to swap behavior later
/// (e.g. force-yes, non-interactive CI).
#[allow(dead_code)]
pub fn confirm(logger: &Logger, msg: &str, default_value: bool) -> bool {
    logger.confirm(msg, default_value)
}

/// How to remove a path: directory tree, single file, or auto-detect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
    /// `remove_dir_all` when `path.is_dir()`, else `remove_file`.
    #[allow(dead_code)] // public API for callers with mixed file/dir targets
    Auto,
}

/// Delete each existing path. In dry-run mode, print a `~` line instead.
pub fn delete_paths(paths: &[PathBuf], dry_run: bool, verbose: bool, logger: &Logger) {
    for p in paths {
        delete_entry(p, None, EntryKind::Dir, dry_run, verbose, logger);
    }
}

/// Recursively delete `path` (directory), printing success or failure. No-op if missing.
pub fn delete_path_verbose(path: &Path, verbose: bool, logger: &Logger) {
    delete_entry(path, None, EntryKind::Dir, false, verbose, logger);
}

/// Paths from `candidates` that currently exist on disk.
pub fn existing_paths(candidates: &[PathBuf]) -> Vec<&Path> {
    candidates
        .iter()
        .filter(|p| p.exists())
        .map(PathBuf::as_path)
        .collect()
}

/// Global-cache flow: dry-run lists via `delete_entry`, else one default-No
/// confirm then bulk delete. Callers log headers; this owns the gate + remove.
///
/// Returns the number of paths dry-run listed or deleted (0 if empty or cancel).
pub fn delete_existing_with_confirm(
    existing: &[&Path],
    dry_run: bool,
    verbose: bool,
    logger: &Logger,
    tag: Option<&str>,
    confirm_msg: &str,
) -> usize {
    if existing.is_empty() {
        return 0;
    }
    if dry_run {
        for p in existing {
            delete_entry(p, tag, EntryKind::Dir, true, verbose, logger);
        }
        return existing.len();
    }
    if !confirm(logger, confirm_msg, false) {
        return 0;
    }
    for p in existing {
        delete_entry(p, tag, EntryKind::Dir, false, verbose, logger);
    }
    existing.len()
}

/// Single entry-point for dry-run-aware file/dir deletion used by purge modules.
///
/// - Missing paths are no-ops.
/// - `tag` prefixes log lines as `[tag]` when `Some`.
/// - `kind` chooses remove_dir_all vs remove_file vs auto.
pub fn delete_entry(
    path: &Path,
    tag: Option<&str>,
    kind: EntryKind,
    dry_run: bool,
    verbose: bool,
    logger: &Logger,
) {
    if !path.exists() {
        return;
    }

    if dry_run {
        logger.detail(&format_dry_run(path, tag));
        return;
    }

    let result = match kind {
        EntryKind::Dir => std::fs::remove_dir_all(path),
        EntryKind::File => std::fs::remove_file(path),
        EntryKind::Auto => {
            if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            }
        }
    };

    match result {
        Ok(()) => logger.success(&format_success(path, tag)),
        Err(e) => {
            logger.err(&format_fail(path, tag, &e));
            if verbose {
                logger.err(&e.to_string());
            }
        }
    }
}

fn format_dry_run(path: &Path, tag: Option<&str>) -> String {
    match tag {
        Some(t) => format!(
            "  {} [{}] would delete {}",
            "~".cyan(),
            t,
            path.display()
        ),
        None => format!("  {} {}", "~".cyan(), path.display()),
    }
}

fn format_success(path: &Path, tag: Option<&str>) -> String {
    match tag {
        Some(t) => format!(
            "  {} [{}] Deleted {}",
            "✓".green(),
            t,
            path.display()
        ),
        None => format!("  {} Deleted {}", "✓".green(), path.display()),
    }
}

fn format_fail(path: &Path, tag: Option<&str>, e: &std::io::Error) -> String {
    match tag {
        Some(t) => format!(
            "  {} [{}] Failed to delete {}: {}",
            "✗".red(),
            t,
            path.display(),
            e
        ),
        None => format!(
            "  {} Failed to delete {}: {}",
            "✗".red(),
            path.display(),
            e
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Logger is interactive; unit-test the pure remove path via dry_run + exists.

    #[test]
    fn delete_entry_dry_run_preserves_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("cache");
        fs::create_dir_all(&dir).unwrap();
        let logger = Logger::new();
        delete_entry(&dir, Some("rust"), EntryKind::Dir, true, false, &logger);
        assert!(dir.exists());
    }

    #[test]
    fn delete_entry_removes_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("cache");
        fs::create_dir_all(dir.join("nested")).unwrap();
        let logger = Logger::new();
        delete_entry(&dir, None, EntryKind::Dir, false, false, &logger);
        assert!(!dir.exists());
    }

    #[test]
    fn delete_entry_removes_file() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("x.log");
        fs::write(&f, b"x").unwrap();
        let logger = Logger::new();
        delete_entry(&f, Some("rails"), EntryKind::File, false, false, &logger);
        assert!(!f.exists());
    }

    #[test]
    fn delete_entry_auto_picks_file_vs_dir() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("a.txt");
        let d = tmp.path().join("b");
        fs::write(&f, b"x").unwrap();
        fs::create_dir_all(&d).unwrap();
        let logger = Logger::new();
        delete_entry(&f, None, EntryKind::Auto, false, false, &logger);
        delete_entry(&d, None, EntryKind::Auto, false, false, &logger);
        assert!(!f.exists());
        assert!(!d.exists());
    }

    #[test]
    fn delete_entry_missing_is_noop() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope");
        let logger = Logger::new();
        delete_entry(&missing, None, EntryKind::Dir, false, false, &logger);
        // no panic
    }

    #[test]
    fn delete_paths_dry_run_preserves_all() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let logger = Logger::new();
        delete_paths(&[a.clone(), b.clone()], true, false, &logger);
        assert!(a.exists() && b.exists());
    }

    // --- TDD harden R1: kind/path mismatches must not clobber ---

    #[test]
    fn kind_dir_on_file_leaves_file() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("notadir");
        fs::write(&f, b"data").unwrap();
        let logger = Logger::new();
        delete_entry(&f, None, EntryKind::Dir, false, false, &logger);
        assert!(f.exists(), "Dir kind on file must not remove the file");
        assert_eq!(fs::read(&f).unwrap(), b"data");
    }

    #[test]
    fn kind_file_on_dir_leaves_dir() {
        let tmp = TempDir::new().unwrap();
        let d = tmp.path().join("adir");
        fs::create_dir_all(d.join("nested")).unwrap();
        let logger = Logger::new();
        delete_entry(&d, None, EntryKind::File, false, false, &logger);
        assert!(d.exists(), "File kind on dir must not remove the directory");
        assert!(d.join("nested").exists());
    }

    // --- TDD harden R2: Auto + nested + empty dir ---

    #[test]
    fn auto_removes_nested_tree() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("tree");
        fs::create_dir_all(root.join("a").join("b")).unwrap();
        fs::write(root.join("a").join("b").join("c.txt"), b"x").unwrap();
        let logger = Logger::new();
        delete_entry(&root, Some("t"), EntryKind::Auto, false, false, &logger);
        assert!(!root.exists());
    }

    #[test]
    fn auto_removes_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let d = tmp.path().join("empty");
        fs::create_dir_all(&d).unwrap();
        let logger = Logger::new();
        delete_entry(&d, None, EntryKind::Auto, false, false, &logger);
        assert!(!d.exists());
    }

    #[test]
    fn existing_paths_filters_missing() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("missing");
        fs::create_dir_all(&a).unwrap();
        let cands = vec![a.clone(), b];
        let got = existing_paths(&cands);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], a.as_path());
    }

    #[test]
    fn confirm_delete_dry_run_preserves_and_counts() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let logger = Logger::new();
        let existing = [a.as_path(), b.as_path()];
        let n = delete_existing_with_confirm(
            &existing,
            true,
            false,
            &logger,
            Some("t"),
            "Delete?",
        );
        assert_eq!(n, 2);
        assert!(a.exists() && b.exists());
    }

    #[test]
    fn confirm_delete_empty_is_zero() {
        let logger = Logger::new();
        let n = delete_existing_with_confirm(&[], false, false, &logger, None, "x");
        assert_eq!(n, 0);
    }
}
