//! Platform-agnostic junk under each scanned root: JVM heap dumps and crash
//! logs, OS core dumps, package-manager debug logs, editor backups, Finder
//! metadata, linter caches.

use std::path::{Path, PathBuf};

use super::super::candidate::{display_path, Candidate, Category, Size, Target, Tier};
use super::{is_heavy_or_owned_dir, ScanContext, Scanner};

pub struct ExtrasScanner;

impl Scanner for ExtrasScanner {
    fn category(&self) -> Category {
        Category::Extras
    }

    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
        let roots: Vec<&PathBuf> = if ctx.projects.is_empty() {
            ctx.roots.iter().collect()
        } else {
            ctx.projects.iter().map(|(_, r)| r).collect()
        };
        let mut out = Vec::new();
        for root in roots {
            let group = display_path(root, &ctx.home);
            ctx.note(format!("walking {group}"));
            for (path, is_dir) in find_extras(root) {
                let label = path
                    .strip_prefix(root)
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string());
                let target = if is_dir {
                    Target::dir(path)
                } else {
                    Target::file(path)
                };
                out.push(Candidate::new(
                    Category::Extras,
                    group.clone(),
                    label,
                    target,
                    Size::Unknown,
                    Tier::Local,
                ));
            }
        }
        out
    }
}

/// Matches by exact name, prefix, suffix, or a single substring rule.
pub fn is_extra_file(name: &str) -> bool {
    matches!(
        name,
        ".DS_Store" | "Thumbs.db" | ".eslintcache" | ".stylelintcache" | "core" | "nohup.out"
    ) || matches_pid_pattern(name, "java_pid", ".hprof")
        || matches_pid_pattern(name, "hs_err_pid", ".log")
        || name.ends_with(".dmp")
        || (name.starts_with("core.")
            && name[5..].chars().all(|c| c.is_ascii_digit())
            && name.len() > 5)
        || name.starts_with("npm-debug.log")
        || name.starts_with("yarn-debug.log")
        || name.starts_with("yarn-error.log")
        || name.starts_with("lerna-debug.log")
        || (name.ends_with('~') && name.len() > 1)
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

pub fn is_extra_dir(name: &str) -> bool {
    matches!(name, ".nyc_output")
}

/// Directories other scanners own — skip so paths are not double-listed —
/// plus macOS `Library` (huge, privacy-prompting, never holds dev junk).
fn is_skip_dir(name: &str) -> bool {
    is_heavy_or_owned_dir(name)
        || matches!(
            name,
            "Library"
                | ".next"
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

/// `(path, is_dir)` for every extra under `root`, depth-bounded, symlinks skipped.
pub fn find_extras(root: &Path) -> Vec<(PathBuf, bool)> {
    let mut found = Vec::new();
    walk(root, 0, &mut found);
    found.sort();
    found
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, bool)>) {
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
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if is_extra_dir(name) {
                out.push((path, true));
                continue;
            }
            if is_skip_dir(name) {
                continue;
            }
            walk(&path, depth + 1, out);
        } else if ft.is_file() && is_extra_file(name) {
            out.push((path, false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, FakeRunner};
    use super::*;
    use std::fs;
    use tempfile::TempDir;

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
        assert!(!is_extra_file("~"));
    }

    #[test]
    fn skips_owned_subdirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let nm = root.join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join(".DS_Store"), b"x").unwrap();
        fs::write(root.join(".DS_Store"), b"y").unwrap();

        let found = find_extras(root);
        assert_eq!(found.len(), 1);
        assert!(found[0].0.ends_with(".DS_Store"));
        assert!(!found[0].0.to_string_lossy().contains("node_modules"));
    }

    #[test]
    fn never_walks_macos_library() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("Library/Caches")).unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("Library/Caches/.DS_Store"), b"x").unwrap();
        fs::write(root.join("a/.DS_Store"), b"x").unwrap();
        let found = find_extras(root);
        assert_eq!(found, vec![(root.join("a/.DS_Store"), false)]);
    }

    #[test]
    fn scanner_uses_scan_roots_when_no_projects() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("sub/.nyc_output")).unwrap();
        fs::write(root.join("java_pid42.hprof"), b"x".repeat(1024)).unwrap();
        let roots = vec![root.clone()];
        let runner = FakeRunner::new(&[]);
        let c = ctx(&roots, &[], tmp.path(), vec![], &runner);

        let found = ExtrasScanner.scan(&c);

        let mut labels: Vec<&str> = found.iter().map(|c| c.label.as_str()).collect();
        labels.sort();
        assert_eq!(labels, vec!["java_pid42.hprof", "sub/.nyc_output"]);
        assert!(root.join("java_pid42.hprof").exists());
    }
}
