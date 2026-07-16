use colored::Colorize;
use std::path::{Path, PathBuf};

use crate::commands::purge::PurgeArgs;
use crate::commands::purge::common::{self, delete_entry, EntryKind};
use crate::logger::Logger;

/// Per-project Ruby/Rails cleanup.
///
/// Rails-ness is recovered from disk (`config/application.rb`) rather than
/// threaded through the dispatch signature, so the four-arg `run` contract
/// matches the other per-project-type modules.
pub fn run(_args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();

    // Always-on Ruby paths.
    let ruby_paths: [PathBuf; 2] = [root.join("vendor").join("bundle"), root.join(".bundle")];
    for p in &ruby_paths {
        delete_entry(p, Some("ruby"), EntryKind::Dir, dry_run, verbose, &logger);
    }

    // Rails-only extras.
    let is_rails = root.join("config").join("application.rb").exists();
    if !is_rails {
        return;
    }

    let tmp_cache = root.join("tmp").join("cache");
    delete_entry(
        &tmp_cache,
        Some("rails"),
        EntryKind::Dir,
        dry_run,
        verbose,
        &logger,
    );

    // Rotate Rails logs: every `log/*.log`. Skip the whole block if `log/`
    // is absent so we don't print noise.
    let log_dir = root.join("log");
    if !log_dir.is_dir() {
        return;
    }

    let entries = match std::fs::read_dir(&log_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("log") {
            continue;
        }
        delete_entry(
            &path,
            Some("rails"),
            EntryKind::File,
            dry_run,
            verbose,
            &logger,
        );
    }
}

/// Globals for Ruby — gated by `common::confirm`. Wired into the dispatcher
/// in `mod.rs` behind the `--ruby-global` flag.
pub fn run_global(_args: &PurgeArgs, dry_run: bool, verbose: bool) {
    let logger = Logger::new();
    let home = dirs::home_dir().unwrap_or_default();

    let candidates: [PathBuf; 2] = [
        home.join(".bundle").join("cache"),
        home.join(".gem").join("cache"),
    ];

    let existing: Vec<&PathBuf> = candidates.iter().filter(|p| p.exists()).collect();
    if existing.is_empty() {
        return;
    }

    logger.info(&format!("\n{}", "Ruby global caches:".cyan()));
    for p in &existing {
        logger.info(&format!("  [ruby] {}", p.display()));
    }

    if dry_run {
        for p in &existing {
            logger.detail(&format!("  [ruby] would delete {}", p.display()));
        }
        return;
    }

    if !common::confirm(&logger, "  Delete Ruby global caches?", false) {
        return;
    }

    for p in &existing {
        delete_entry(p, Some("ruby"), EntryKind::Dir, false, verbose, &logger);
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
    fn dry_run_does_not_delete_ruby_targets() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let vendor_bundle = root.join("vendor").join("bundle");
        let dot_bundle = root.join(".bundle");
        fs::create_dir_all(&vendor_bundle).unwrap();
        fs::create_dir_all(&dot_bundle).unwrap();

        run(&args(), root, true, false);

        assert!(
            vendor_bundle.exists(),
            "dry-run must not delete vendor/bundle/"
        );
        assert!(dot_bundle.exists(), "dry-run must not delete .bundle/");
    }

    #[test]
    fn dry_run_rails_branch_preserves_tmp_cache_and_log() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Mark as Rails.
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config").join("application.rb"), b"# rails\n").unwrap();
        // Common Ruby + Rails targets.
        let vendor_bundle = root.join("vendor").join("bundle");
        let tmp_cache = root.join("tmp").join("cache");
        let log_dir = root.join("log");
        let log_file = log_dir.join("development.log");
        fs::create_dir_all(&vendor_bundle).unwrap();
        fs::create_dir_all(&tmp_cache).unwrap();
        fs::create_dir_all(&log_dir).unwrap();
        fs::write(&log_file, b"line\n").unwrap();

        run(&args(), root, true, false);

        assert!(tmp_cache.exists(), "dry-run must not delete tmp/cache/");
        assert!(log_file.exists(), "dry-run must not delete log/*.log");
        assert!(
            vendor_bundle.exists(),
            "dry-run must not delete vendor/bundle/"
        );
    }
}
