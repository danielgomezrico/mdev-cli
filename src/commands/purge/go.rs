//! Go cache purging.
//!
//! Two cleaners:
//!   - `run`: per-project — wipes `bin/` and `pkg/` when the project root
//!     contains a `go.mod` (safety belt: the dispatch already implies Go).
//!   - `run_global`: cross-machine — invokes `go clean -modcache` (if `go` is
//!     on `PATH`) and removes the platform's go-build cache directory. Guarded
//!     by `common::confirm` before any destructive action.

use std::path::Path;

use super::common;
use super::PurgeArgs;
use crate::logger::Logger;
use crate::runner::{ProcessRunner, Runner};

/// Per-project Go cleanup. Removes `bin/` and `pkg/` when `go.mod` is present
/// at `root`. Safety belt: even though dispatch implies Go, re-verify.
pub fn run(_args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();

    if !root.join("go.mod").exists() {
        if verbose {
            logger.detail(&format!(
                "[go] skip {} — no go.mod",
                root.display()
            ));
        }
        return;
    }

    for sub in &["bin", "pkg"] {
        let path = root.join(sub);
        if !path.exists() {
            continue;
        }
        if dry_run {
            logger.info(&format!("[go] would delete {}", path.display()));
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(_) => logger.success(&format!("[go] deleted {}", path.display())),
            Err(e) => logger.err(&format!(
                "[go] failed to delete {}: {}",
                path.display(),
                e
            )),
        }
    }
}

/// Global Go cleanup. Runs `go clean -modcache` (if `go` is available) and
/// removes the OS-specific go-build cache. All destructive actions are
/// guarded by `common::confirm`. Wired into the dispatcher in `mod.rs`
/// behind the `--go-global` flag.
pub fn run_global(_args: &PurgeArgs, dry_run: bool, verbose: bool) {
    let logger = Logger::new();
    let runner = ProcessRunner::new();

    let go_available = runner.which("go").is_some();
    let home = dirs::home_dir().unwrap_or_default();

    let build_cache = if cfg!(target_os = "macos") {
        Some(home.join("Library").join("Caches").join("go-build"))
    } else if cfg!(target_os = "linux") {
        Some(home.join(".cache").join("go-build"))
    } else {
        None
    };

    // Nothing to do: no `go` binary and no build cache directory.
    let build_cache_present = build_cache.as_ref().is_some_and(|p| p.exists());
    if !go_available && !build_cache_present {
        if verbose {
            logger.detail("[go] nothing to purge globally");
        }
        return;
    }

    logger.info("[go] global caches:");
    if go_available {
        logger.info("  go clean -modcache");
    }
    if let Some(p) = &build_cache {
        if p.exists() {
            logger.info(&format!("  {}", p.display()));
        }
    }

    if dry_run {
        if go_available {
            logger.info("[go] would run `go clean -modcache`");
        }
        if let Some(p) = &build_cache {
            if p.exists() {
                logger.info(&format!("[go] would delete {}", p.display()));
            }
        }
        return;
    }

    if !common::confirm(&logger, "  Delete Go global caches?", false) {
        logger.detail("[go] skipped");
        return;
    }

    if go_available {
        let result = runner.run("go", &["clean", "-modcache"], None);
        if result.is_success() {
            logger.success("[go] go clean -modcache");
        } else {
            logger.err(&format!(
                "[go] go clean -modcache failed: {}",
                result.stderr
            ));
        }
    }

    if let Some(p) = build_cache {
        if p.exists() {
            match std::fs::remove_dir_all(&p) {
                Ok(_) => logger.success(&format!("[go] deleted {}", p.display())),
                Err(e) => logger.err(&format!(
                    "[go] failed to delete {}: {}",
                    p.display(),
                    e
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn args() -> PurgeArgs {
        PurgeArgs {
            flutter: false,
            pub_cache: false,
            gradle: false,
            android: false,
            ios: false,
            dry_run: true,
            verbose: false,
        }
    }

    #[test]
    fn dry_run_does_not_delete_bin_or_pkg() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Safety belt requires go.mod to be present.
        fs::write(root.join("go.mod"), "module example.test\n").unwrap();
        let bin = root.join("bin");
        let pkg = root.join("pkg");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&pkg).unwrap();

        run(&args(), root, true, false);

        assert!(bin.exists(), "dry-run must not delete bin/");
        assert!(pkg.exists(), "dry-run must not delete pkg/");
    }
}
