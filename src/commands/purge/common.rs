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

/// Delete each existing path. In dry-run mode, print a `~` line instead.
pub fn delete_paths(paths: &[PathBuf], dry_run: bool, verbose: bool, logger: &Logger) {
    for p in paths {
        if p.exists() {
            if dry_run {
                logger.detail(&format!("  {} {}", "~".cyan(), p.display()));
            } else {
                delete_path_verbose(p, verbose, logger);
            }
        }
    }
}

/// Recursively delete `path`, printing success or failure. No-op if missing.
pub fn delete_path_verbose(path: &Path, verbose: bool, logger: &Logger) {
    if !path.exists() {
        return;
    }
    match std::fs::remove_dir_all(path) {
        Ok(_) => logger.success(&format!("  {} Deleted {}", "✓".green(), path.display())),
        Err(e) => {
            logger.err(&format!(
                "  {} Failed to delete {}: {}",
                "✗".red(),
                path.display(),
                e
            ));
            if verbose {
                logger.err(&e.to_string());
            }
        }
    }
}
