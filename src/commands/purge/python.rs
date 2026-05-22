use colored::Colorize;
use std::path::{Path, PathBuf};

use crate::commands::purge::PurgeArgs;
use crate::commands::purge::common::{self, delete_path_verbose};
use crate::logger::Logger;

/// Max recursion depth for the project-local walk. Mirrors the detector's
/// search depth so we stay bounded even on pathological trees.
const MAX_DEPTH: usize = 10;

/// Directory names we never recurse into when walking a Python project.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".venv",
    "venv",
    "env",
    "target",
];

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
        delete_file_or_dir(&coverage, "python", dry_run, verbose, &logger);
    }

    // Root-only `htmlcov/` directory.
    let htmlcov = root.join("htmlcov");
    if htmlcov.exists() {
        delete_file_or_dir(&htmlcov, "python", dry_run, verbose, &logger);
    }

    // Django-only: presence of `manage.py` at the root marks this as Django,
    // re-derived from the filesystem so we don't change the dispatch signature.
    if root.join("manage.py").exists() {
        let staticfiles = root.join("staticfiles");
        if staticfiles.exists() {
            delete_file_or_dir(&staticfiles, "django", dry_run, verbose, &logger);
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

    let existing: Vec<&PathBuf> = paths.iter().filter(|p| p.exists()).collect();
    if existing.is_empty() {
        return;
    }

    logger.info(&format!("\n{}", "Global Python caches to delete:".cyan()));
    for p in &existing {
        logger.info(&format!("  {}", p.display()));
    }

    if dry_run {
        for p in &existing {
            logger.detail(&format!("  {} [python] would delete {}", "~".cyan(), p.display()));
        }
        return;
    }

    if !common::confirm(&logger, "  Delete global Python caches?", false) {
        return;
    }

    for p in &existing {
        delete_path_verbose(p, verbose, &logger);
    }
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
            logger.detail(&format!(
                "  {} [python] would delete {}",
                "~".cyan(),
                p.display()
            ));
            continue;
        }

        let prompt = format!("  Delete {} ?", p.display());
        if common::confirm(&logger, &prompt, false) {
            delete_path_verbose(&p, verbose, &logger);
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

        if SKIP_DIRS.contains(&name) {
            continue;
        }

        if dir_names.contains(&name) {
            delete_file_or_dir(&path, "python", dry_run, verbose, logger);
            // Don't descend into a directory we just deleted (or queued for delete).
            continue;
        }

        walk_inner(&path, dir_names, depth + 1, dry_run, verbose, logger);
    }
}

/// Delete a file or directory, with a `[tag] would delete <path>` line in
/// dry-run mode. `tag` is `"python"` for generic Python artifacts and
/// `"django"` for Django-specific ones.
fn delete_file_or_dir(
    path: &Path,
    tag: &str,
    dry_run: bool,
    verbose: bool,
    logger: &Logger,
) {
    if dry_run {
        logger.detail(&format!(
            "  {} [{}] would delete {}",
            "~".cyan(),
            tag,
            path.display()
        ));
        return;
    }

    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };

    match result {
        Ok(_) => logger.success(&format!(
            "  {} [{}] Deleted {}",
            "✓".green(),
            tag,
            path.display()
        )),
        Err(e) => {
            logger.err(&format!(
                "  {} [{}] Failed to delete {}: {}",
                "✗".red(),
                tag,
                path.display(),
                e
            ));
            if verbose {
                logger.err(&e.to_string());
            }
        }
    }
}
