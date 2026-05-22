use std::path::Path;

use crate::commands::purge::PurgeArgs;
use crate::commands::purge::common::{confirm, delete_path_verbose, delete_paths};
use crate::logger::Logger;

/// Per-project Node/frontend cache cleanup. Deletes well-known build,
/// bundler, and dependency caches under `root` if present. Uses the shared
/// `delete_paths` helper so dry-run output stays byte-identical to the
/// Flutter/Android/iOS modules.
pub fn run(_args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();

    let targets = [
        root.join("node_modules"),
        root.join(".next"),
        root.join(".nuxt"),
        root.join(".turbo"),
        root.join(".vite"),
        root.join(".parcel-cache"),
        root.join("dist"),
        root.join("build"),
        root.join(".svelte-kit"),
        root.join(".astro"),
        root.join("coverage"),
    ];

    delete_paths(&targets, dry_run, verbose, &logger);
}

/// Global Node/frontend cache cleanup. Wired into the dispatcher in `mod.rs`
/// behind the `--node-global` flag. Prompts before each deletion (skipped
/// when `dry_run`).
pub fn run_global(_args: &PurgeArgs, dry_run: bool, verbose: bool) {
    let logger = Logger::new();
    let home = dirs::home_dir().unwrap_or_default();

    let mut candidates: Vec<std::path::PathBuf> = vec![
        home.join(".npm"),
        home.join(".pnpm-store"),
    ];
    if cfg!(target_os = "linux") {
        candidates.push(home.join(".cache").join("yarn"));
    }
    if cfg!(target_os = "macos") {
        candidates.push(home.join("Library").join("Caches").join("Yarn"));
    }
    candidates.push(home.join(".bun").join("install").join("cache"));

    for path in candidates {
        if !path.exists() {
            continue;
        }
        if dry_run {
            logger.info(&format!("[node] would delete {}", path.display()));
            continue;
        }
        let prompt = format!("[node] Delete {}?", path.display());
        if confirm(&logger, &prompt, false) {
            delete_path_verbose(&path, verbose, &logger);
        }
    }
}
