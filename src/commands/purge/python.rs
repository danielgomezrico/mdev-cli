use colored::Colorize;
use std::path::{Path, PathBuf};

use crate::commands::purge::PurgeArgs;
use crate::commands::purge::common::{
    self, delete_entry, delete_existing_with_confirm, existing_paths, EntryKind,
};
use crate::logger::Logger;

/// Max recursion depth for the project-local walk. Mirrors the detector's
/// search depth so we stay bounded even on pathological trees.
const MAX_DEPTH: usize = 10;

/// Extra names beyond `common::is_heavy_or_owned_dir` that the Python walk
/// must not descend into (`env` is a common non-venv folder name).
const SKIP_DIRS_EXTRA: &[&str] = &["env"];

/// Cache directory names we collect (and later delete) at any depth.
const PY_CACHE_DIRS: &[&str] = &[
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
];

/// Per-project Python cleanup. Walks `root` recursively (bounded depth, skip
/// list applied), deleting Python-cache directories at any depth. A couple of
/// root-only artifacts (`.coverage`, `htmlcov/`) and Django-specific
/// `staticfiles/` are handled separately.
pub fn run(_args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();

    // Recursive cache-dir sweep.
    walk_and_delete(root, PY_CACHE_DIRS, dry_run, verbose, &logger);

    // Root-only `.coverage` file.
    let coverage = root.join(".coverage");
    if coverage.exists() {
        delete_entry(
            &coverage,
            Some("python"),
            EntryKind::File,
            dry_run,
            verbose,
            &logger,
        );
    }

    // Root-only `htmlcov/` directory.
    let htmlcov = root.join("htmlcov");
    if htmlcov.exists() {
        delete_entry(
            &htmlcov,
            Some("python"),
            EntryKind::Dir,
            dry_run,
            verbose,
            &logger,
        );
    }

    // Django-only: presence of `manage.py` at the root marks this as Django,
    // re-derived from the filesystem so we don't change the dispatch signature.
    if root.join("manage.py").exists() {
        let staticfiles = root.join("staticfiles");
        if staticfiles.exists() {
            delete_entry(
                &staticfiles,
                Some("django"),
                EntryKind::Dir,
                dry_run,
                verbose,
                &logger,
            );
        }
    }
}

/// Global Python caches. Wired into the dispatcher in `mod.rs` behind the
/// `--python-global` flag. OS-guarded; pip & poetry live in different caches
/// on macOS vs Linux.
pub fn run_global(_args: &PurgeArgs, dry_run: bool, verbose: bool) {
    let logger = Logger::new();
    let home = dirs::home_dir().unwrap_or_default();

    let mut paths: Vec<PathBuf> = Vec::new();

    // pip & poetry — OS-specific cache locations.
    if cfg!(target_os = "macos") {
        paths.push(home.join("Library").join("Caches").join("pip"));
        paths.push(home.join("Library").join("Caches").join("pypoetry"));
    } else if cfg!(target_os = "linux") {
        paths.push(home.join(".cache").join("pip"));
        paths.push(home.join(".cache").join("pypoetry"));
    }

    // Cross-platform caches.
    paths.push(home.join(".cache").join("uv"));
    paths.push(home.join(".local").join("share").join("virtualenvs"));

    let existing = existing_paths(&paths);
    if existing.is_empty() {
        return;
    }

    logger.info(&format!("\n{}", "Global Python caches to delete:".cyan()));
    for p in &existing {
        logger.info(&format!("  {}", p.display()));
    }

    delete_existing_with_confirm(
        &existing,
        dry_run,
        verbose,
        &logger,
        Some("python"),
        "  Delete global Python caches?",
    );
}

/// Per-project venv cleanup. Wired into the dispatcher in `mod.rs` behind
/// the `--python-venv` flag. Deletes top-level virtualenv directories.
/// Confirms each one because these are common working-dir mistakes (devs
/// name folders `env/` for unrelated reasons).
pub fn run_venv(_args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();

    for name in &[".venv", "venv", "env"] {
        let p = root.join(name);
        if !p.is_dir() {
            continue;
        }

        if dry_run {
            delete_entry(&p, Some("python"), EntryKind::Dir, true, verbose, &logger);
            continue;
        }

        let prompt = format!("  Delete {} ?", p.display());
        if common::confirm(&logger, &prompt, false) {
            delete_entry(&p, Some("python"), EntryKind::Dir, false, verbose, &logger);
        }
    }
}

/// Recursively walk `root` (bounded by `MAX_DEPTH`), deleting any directory
/// whose name matches `dir_names`. Skip-list (see `SKIP_DIRS`) prunes the
/// descent. When a match is hit we delete the whole subtree and stop
/// descending into it.
fn walk_and_delete(
    root: &Path,
    dir_names: &[&str],
    dry_run: bool,
    verbose: bool,
    logger: &Logger,
) {
    walk_inner(root, dir_names, 0, dry_run, verbose, logger);
}

fn walk_inner(
    dir: &Path,
    dir_names: &[&str],
    depth: usize,
    dry_run: bool,
    verbose: bool,
    logger: &Logger,
) {
    if depth > MAX_DEPTH {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if !file_type.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        if common::is_heavy_or_owned_dir(name) || SKIP_DIRS_EXTRA.contains(&name) {
            continue;
        }

        if dir_names.contains(&name) {
            delete_entry(
                &path,
                Some("python"),
                EntryKind::Dir,
                dry_run,
                verbose,
                logger,
            );
            // Don't descend into a directory we just deleted (or queued for delete).
            continue;
        }

        walk_inner(&path, dir_names, depth + 1, dry_run, verbose, logger);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn args() -> PurgeArgs {
        PurgeArgs { dry_run: true, ..Default::default() }
    }

    #[test]
    fn dry_run_does_not_delete_python_caches() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let pycache = root.join("__pycache__");
        let pytest = root.join(".pytest_cache");
        let coverage = root.join(".coverage");
        let htmlcov = root.join("htmlcov");
        fs::create_dir_all(&pycache).unwrap();
        fs::create_dir_all(&pytest).unwrap();
        fs::write(&coverage, b"").unwrap();
        fs::create_dir_all(&htmlcov).unwrap();

        run(&args(), root, true, false);

        assert!(pycache.exists(), "dry-run must not delete __pycache__/");
        assert!(pytest.exists(), "dry-run must not delete .pytest_cache/");
        assert!(coverage.exists(), "dry-run must not delete .coverage");
        assert!(htmlcov.exists(), "dry-run must not delete htmlcov/");
    }
}
