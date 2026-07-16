use std::path::{Path, PathBuf};

use super::PurgeArgs;
use super::common::{self, delete_entry, EntryKind};
use crate::logger::Logger;

/// Per-project Rust/Cargo cleanup.
///
/// Deletes `target/` if present. Never touches `Cargo.lock`.
pub fn run(_args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();
    let paths: [PathBuf; 1] = [root.join("target")];
    for path in &paths {
        delete_entry(path, Some("rust"), EntryKind::Dir, dry_run, verbose, &logger);
    }
}

/// Global Cargo cache cleanup. Prompts the user once before deleting. Wired
/// into the dispatcher in `mod.rs` behind the `--rust-global` flag.
pub fn run_global(_args: &PurgeArgs, dry_run: bool, verbose: bool) {
    let logger = Logger::new();
    let home = dirs::home_dir().unwrap_or_default();
    let candidates: [PathBuf; 3] = [
        home.join(".cargo").join("registry").join("cache"),
        home.join(".cargo").join("registry").join("src"),
        home.join(".cargo").join("git").join("db"),
    ];

    let existing: Vec<&PathBuf> = candidates.iter().filter(|p| p.exists()).collect();
    if existing.is_empty() {
        return;
    }

    logger.info("[rust] global caches to delete:");
    for p in &existing {
        logger.info(&format!("  {}", p.display()));
    }

    if dry_run {
        for p in &existing {
            delete_entry(p, Some("rust"), EntryKind::Dir, true, verbose, &logger);
        }
        return;
    }

    if !common::confirm(&logger, "Delete global Rust/Cargo caches?", false) {
        return;
    }

    for p in &existing {
        delete_entry(p, Some("rust"), EntryKind::Dir, false, verbose, &logger);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn args() -> PurgeArgs {
        PurgeArgs {
            dry_run: true,
            ..Default::default()
        }
    }

    #[test]
    fn dry_run_does_not_delete_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let target = root.join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        let sentinel = target.join("debug").join("placeholder");
        fs::write(&sentinel, b"x").unwrap();

        run(&args(), root, true, false);

        assert!(target.exists(), "dry-run must not delete target/");
        assert!(
            sentinel.exists(),
            "dry-run must leave files inside target/ untouched"
        );
    }
}
