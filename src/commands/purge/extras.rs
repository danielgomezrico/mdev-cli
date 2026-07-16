use std::path::{Path, PathBuf};

use crate::commands::purge::common;
use crate::commands::purge::PurgeArgs;
use crate::logger::Logger;

/// Filename patterns for platform-agnostic junk that tends to balloon in size:
/// JVM heap dumps and crash logs, OS core dumps, package-manager debug logs,
/// editor backups, OS finder metadata, and linter caches. Matches by exact
/// name, prefix, suffix, or a single substring rule (no full regex).
fn is_extra_file(name: &str) -> bool {
    // Exact names.
    matches!(
        name,
        ".DS_Store"
            | "Thumbs.db"
            | ".eslintcache"
            | ".stylelintcache"
            | "core"
            | "nohup.out"
    )
        // JVM heap dumps: java_pid<NNN>.hprof (require at least one digit)
        || matches_pid_pattern(name, "java_pid", ".hprof")
        // JVM crash logs: hs_err_pid<NNN>.log (require at least one digit)
        || matches_pid_pattern(name, "hs_err_pid", ".log")
        // Generic crash dumps.
        || name.ends_with(".dmp")
        // Linux core dumps with PID suffix.
        || (name.starts_with("core.") && name[5..].chars().all(|c| c.is_ascii_digit()) && name.len() > 5)
        // Package-manager debug logs.
        || name.starts_with("npm-debug.log")
        || name.starts_with("yarn-debug.log")
        || name.starts_with("yarn-error.log")
        || name.starts_with("lerna-debug.log")
        // Editor backups ending with ~ (but not just "~").
        || (name.ends_with('~') && name.len() > 1)
        // Vim swap files: .<filename>.swp / .swo / .swn
        || (name.starts_with('.')
            && (name.ends_with(".swp") || name.ends_with(".swo") || name.ends_with(".swn")))
}

fn matches_pid_pattern(name: &str, prefix: &str, suffix: &str) -> bool {
    if !name.starts_with(prefix) || !name.ends_with(suffix) {
        return false;
    }
    let mid = &name[prefix.len()..name.len() - suffix.len()];
    !mid.is_empty() && mid.chars().all(|c| c.is_ascii_digit())
}

fn is_extra_dir(name: &str) -> bool {
    matches!(name, ".nyc_output")
}

/// Directories that other cleaners own — skip them during the walk so we
/// don't double-account paths that the ecosystem cleaner is about to remove.
fn is_skip_dir(name: &str) -> bool {
    use super::common::is_heavy_or_owned_dir;
    is_heavy_or_owned_dir(name)
        || matches!(
            name,
            ".next"
                | ".nuxt"
                | ".turbo"
                | ".vite"
                | ".parcel-cache"
                | "env"
                | ".tox"
                | ".pytest_cache"
                | ".mypy_cache"
                | ".ruff_cache"
                | ".bundle"
                | ".svelte-kit"
                | ".astro"
                | "coverage"
                | "htmlcov"
                | "staticfiles"
        )
}

const MAX_DEPTH: usize = 6;

fn find_extras(root: &Path) -> Vec<(PathBuf, u64, bool)> {
    let mut found: Vec<(PathBuf, u64, bool)> = Vec::new();
    walk(root, 0, &mut found);
    found.sort_by(|a, b| b.1.cmp(&a.1));
    found
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, u64, bool)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_dir() {
            if is_extra_dir(name) {
                let size = dir_size(&path);
                out.push((path, size, true));
                continue;
            }
            if is_skip_dir(name) {
                continue;
            }
            walk(&path, depth + 1, out);
        } else if ft.is_file() && is_extra_file(name) {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((path, size, false));
        }
    }
}

fn dir_size(p: &Path) -> u64 {
    let mut total: u64 = 0;
    let entries = match std::fs::read_dir(p) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        if let Ok(ft) = entry.file_type() {
            if ft.is_dir() {
                total = total.saturating_add(dir_size(&entry.path()));
            } else if let Ok(m) = entry.metadata() {
                total = total.saturating_add(m.len());
            }
        }
    }
    total
}

fn human_size(bytes: u64) -> String {
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

/// `.DS_Store` is pure macOS Finder metadata with zero recovery value, so it
/// is deleted without a confirmation prompt (unlike the other extras).
fn is_noconfirm_file(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()) == Some(".DS_Store")
}

/// Per-project scan for size-hungry, platform-agnostic junk files (JVM heap
/// dumps, crash logs, npm/yarn debug logs, editor backups, OS metadata, etc.).
/// Prints findings with sizes; in dry-run only lists. `.DS_Store` files are
/// deleted unconditionally; the rest prompt once before deleting.
pub fn run(_args: &PurgeArgs, root: &Path, dry_run: bool, _verbose: bool) {
    let logger = Logger::new();
    let found = find_extras(root);
    if found.is_empty() {
        return;
    }
    let total: u64 = found.iter().map(|(_, s, _)| *s).sum();

    logger.info(&format!(
        "[extras] {} candidate file(s) — {}",
        found.len(),
        human_size(total)
    ));
    for (path, size, _) in &found {
        logger.info(&format!("  {} ({})", path.display(), human_size(*size)));
    }

    if dry_run {
        logger.info(&format!(
            "[extras] would delete {} item(s) — dry run",
            found.len()
        ));
        return;
    }

    let (noconfirm, prompted): (Vec<_>, Vec<_>) =
        found.iter().partition(|(p, _, _)| is_noconfirm_file(p));

    let mut ok: usize = 0;
    let mut err: usize = 0;

    // Auto-delete `.DS_Store` without prompting.
    for (path, _, is_dir) in &noconfirm {
        match delete_one(path, *is_dir) {
            Ok(_) => ok += 1,
            Err(e) => {
                err += 1;
                logger.err(&format!("[extras] failed to delete {}: {}", path.display(), e));
            }
        }
    }

    // Everything else is gated by a single default-No confirmation.
    if !prompted.is_empty() {
        let total_prompted: u64 = prompted.iter().map(|(_, s, _)| *s).sum();
        let prompt = format!(
            "  Delete {} extra file(s) totaling {}?",
            prompted.len(),
            human_size(total_prompted)
        );
        if common::confirm(&logger, &prompt, false) {
            for (path, _, is_dir) in &prompted {
                match delete_one(path, *is_dir) {
                    Ok(_) => ok += 1,
                    Err(e) => {
                        err += 1;
                        logger.err(&format!("[extras] failed to delete {}: {}", path.display(), e));
                    }
                }
            }
        } else {
            logger.detail("[extras] skipped (kept prompted files)");
        }
    }

    if ok > 0 || err > 0 {
        logger.success(&format!(
            "[extras] deleted {} item(s){}",
            ok,
            if err > 0 { format!(", {} failed", err) } else { String::new() }
        ));
    }
}

fn delete_one(path: &Path, is_dir: bool) -> std::io::Result<()> {
    if is_dir {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
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
    fn matches_jvm_heap_dump() {
        assert!(is_extra_file("java_pid31278.hprof"));
        assert!(is_extra_file("java_pid1.hprof"));
        assert!(!is_extra_file("java_pid.hprof"));
        assert!(!is_extra_file("heap.hprof"));
    }

    #[test]
    fn matches_jvm_crash_log() {
        assert!(is_extra_file("hs_err_pid9999.log"));
        assert!(!is_extra_file("error.log"));
    }

    #[test]
    fn matches_core_dump_variants() {
        assert!(is_extra_file("core"));
        assert!(is_extra_file("core.123"));
        assert!(!is_extra_file("core.txt"));
        assert!(!is_extra_file("scorecard.md"));
    }

    #[test]
    fn matches_pkg_manager_logs() {
        assert!(is_extra_file("npm-debug.log"));
        assert!(is_extra_file("npm-debug.log.0"));
        assert!(is_extra_file("yarn-error.log"));
        assert!(is_extra_file("lerna-debug.log"));
    }

    #[test]
    fn matches_editor_and_os_junk() {
        assert!(is_extra_file(".DS_Store"));
        assert!(is_extra_file("Thumbs.db"));
        assert!(is_extra_file(".eslintcache"));
        assert!(is_extra_file("main.rs~"));
        assert!(is_extra_file(".main.rs.swp"));
        assert!(!is_extra_file("~")); // single char shouldn't trigger
    }

    #[test]
    fn dry_run_does_not_delete_extras() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let heap = root.join("java_pid42.hprof");
        let ds_store = root.join(".DS_Store");
        fs::write(&heap, b"x".repeat(1024)).unwrap();
        fs::write(&ds_store, b"y").unwrap();

        run(&args(), root, true, false);

        assert!(heap.exists());
        assert!(ds_store.exists());
    }

    #[test]
    fn skips_owned_subdirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // .DS_Store inside node_modules should NOT be reported (node cleaner
        // owns that subtree).
        let nm = root.join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join(".DS_Store"), b"x").unwrap();
        // .DS_Store at root SHOULD be reported.
        fs::write(root.join(".DS_Store"), b"y").unwrap();

        let found = find_extras(root);
        assert_eq!(found.len(), 1);
        assert!(found[0].0.ends_with(".DS_Store"));
        assert!(!found[0].0.to_string_lossy().contains("node_modules"));
    }
}
