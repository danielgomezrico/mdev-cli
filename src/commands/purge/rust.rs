use std::path::{Path, PathBuf};

use super::PurgeArgs;
use super::common;
use crate::logger::Logger;

/// Per-project Rust/Cargo cleanup.
///
/// Deletes `target/` if present. Never touches `Cargo.lock`.
pub fn run(_args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();
    let paths: [PathBuf; 1] = [root.join("target")];
    for path in &paths {
        if path.exists() {
            delete(path, dry_run, verbose, &logger);
        }
    }
}

/// Global Cargo cache cleanup. Prompts the user once before deleting.
///
/// Not yet wired from `mod.rs`; B9 will gate this behind a CLI flag.
#[allow(dead_code)]
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
            logger.info(&format!("[rust] would delete {}", p.display()));
        }
        return;
    }

    if !common::confirm("Delete global Rust/Cargo caches?", false) {
        return;
    }

    for p in &existing {
        delete(p, dry_run, verbose, &logger);
    }
}

fn delete(path: &Path, dry_run: bool, verbose: bool, logger: &Logger) {
    if dry_run {
        logger.info(&format!("[rust] would delete {}", path.display()));
        return;
    }
    match std::fs::remove_dir_all(path) {
        Ok(_) => logger.success(&format!("[rust] deleted {}", path.display())),
        Err(e) => {
            logger.err(&format!("[rust] failed to delete {}: {}", path.display(), e));
            if verbose {
                logger.err(&e.to_string());
            }
        }
    }
}
