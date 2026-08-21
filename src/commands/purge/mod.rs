use clap::Args;
use colored::Colorize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::app_detector::AppDetector;
use crate::logger::Logger;
use crate::models::{AppInfo, ProjectType};
use crate::runner::Runner;

pub mod android;
pub mod common;
pub mod extras;
pub mod flutter;
pub mod go;
pub mod ios;
pub mod node;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod worktrees;

use common::{delete_path_verbose, select_paths_to_delete};

/// Discover every project under `start_dir` for purge — downward only, with
/// nested-path deduplication (a child repo is not listed separately when its
/// parent path is already a detected project root).
pub(crate) fn collect_projects(start_dir: &Path) -> Vec<(AppInfo, PathBuf)> {
    let mut projects: HashMap<String, (AppInfo, PathBuf)> = HashMap::new();
    for (info, root) in AppDetector::new().discover_projects(start_dir) {
        let key = root.to_string_lossy().to_string();
        projects.insert(key, (info, root));
    }

    let keys: Vec<String> = projects.keys().cloned().collect();
    let mut to_remove: Vec<String> = Vec::new();
    for a in &keys {
        for b in &keys {
            if a != b {
                let b_with_sep = format!("{}{}", b, std::path::MAIN_SEPARATOR);
                if a.starts_with(&b_with_sep) {
                    to_remove.push(a.clone());
                    break;
                }
            }
        }
    }
    for key in to_remove {
        projects.remove(&key);
    }

    let mut sorted: Vec<(AppInfo, PathBuf)> = projects.into_values().collect();
    sorted.sort_by(|a, b| a.1.cmp(&b.1));
    sorted
}

#[derive(Args, Debug, Default)]
pub struct PurgeArgs {
    /// Clean Flutter projects
    #[arg(long)]
    pub flutter: bool,

    /// Clean pub cache
    #[arg(long = "pub")]
    pub pub_cache: bool,

    /// Clean Gradle caches
    #[arg(long)]
    pub gradle: bool,

    /// Clean Android projects
    #[arg(long)]
    pub android: bool,

    /// Clean iOS projects
    #[arg(long)]
    pub ios: bool,

    /// Clean Node/frontend projects
    #[arg(long)]
    pub node: bool,

    /// Clean Rust/cargo projects
    #[arg(long)]
    pub rust: bool,

    /// Clean Go projects
    #[arg(long)]
    pub go: bool,

    /// Clean Ruby/Rails projects
    #[arg(long)]
    pub ruby: bool,

    /// Clean Python projects
    #[arg(long)]
    pub python: bool,

    /// Also clean Node global stores (~/.npm, ~/.pnpm-store, etc.)
    #[arg(long = "node-global")]
    pub node_global: bool,

    /// Also clean ~/.cargo/registry caches (destructive)
    #[arg(long = "rust-global")]
    pub rust_global: bool,

    /// Also run `go clean -modcache` and delete go-build cache (destructive)
    #[arg(long = "go-global")]
    pub go_global: bool,

    /// Also clean ~/.bundle/cache and ~/.gem/cache (destructive)
    #[arg(long = "ruby-global")]
    pub ruby_global: bool,

    /// Also clean pip/uv/poetry/pipenv global caches (destructive)
    #[arg(long = "python-global")]
    pub python_global: bool,

    /// Also delete .venv/venv/env directories under each Python project (very destructive — opt-in)
    #[arg(long = "python-venv")]
    pub python_venv: bool,

    /// Skip the cross-platform "extras" scan (JVM heap dumps, crash logs, editor backups, .DS_Store, etc.)
    #[arg(long = "no-extras")]
    pub no_extras: bool,

    /// Dry run — show what would be deleted without deleting
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Verbose output
    #[arg(short = 'v', long)]
    pub verbose: bool,
}

pub fn run(args: &PurgeArgs, runner: &dyn Runner) -> i32 {
    let logger = Logger::new();
    let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let sorted_projects = collect_projects(&current_dir);

    if sorted_projects.is_empty() {
        logger.warn("No recognized projects found in current directory.");
        if !args.no_extras {
            extras::run(args, &current_dir, args.dry_run, args.verbose);
        }
        return 0;
    }

    let has_android = sorted_projects.iter().any(|(info, _)| {
        info.project_type == ProjectType::Android || info.project_type == ProjectType::Flutter
    });
    let has_ios = sorted_projects.iter().any(|(info, _)| {
        info.project_type == ProjectType::Ios || info.project_type == ProjectType::Flutter
    });

    let explicit_flags = args.flutter
        || args.pub_cache
        || args.gradle
        || args.android
        || args.ios
        || args.node
        || args.rust
        || args.go
        || args.ruby
        || args.python;

    // Determine global targets — Flutter caches (pub + SDK bin/cache) are offered even when
    // no Flutter project is detected, because they are global disk hogs.
    let do_pub = if explicit_flags {
        args.pub_cache || args.flutter
    } else {
        true
    };
    let do_flutter_sdk = if explicit_flags { args.flutter } else { true };
    let do_gradle = if explicit_flags {
        args.gradle
    } else {
        has_android
    };
    let do_derived_data = if explicit_flags {
        args.ios && cfg!(target_os = "macos")
    } else {
        has_ios && cfg!(target_os = "macos")
    };
    let do_pod_cache = if explicit_flags {
        args.ios && cfg!(target_os = "macos")
    } else {
        has_ios && cfg!(target_os = "macos")
    };

    logger.info(&format!("{} starting...", "mdev purge".cyan()));
    logger.info(&format!("Found {} project(s).", sorted_projects.len()));
    if args.dry_run {
        logger.warn("Dry run — no files will be deleted.");
    }

    // Per-project local cleanup — dispatched by project type to its module.
    // When no explicit flags are passed, every detected project type runs.
    // With explicit flags, only the matching cleaners run.
    for (info, root) in &sorted_projects {
        let display_root = root.display();
        logger.info(&format!("\n{} {}", "→".cyan(), display_root));

        match info.project_type {
            ProjectType::Flutter => {
                if !explicit_flags || args.flutter || args.android || args.ios {
                    flutter::run(args, root, args.dry_run, args.verbose);
                }
            }
            ProjectType::Android => {
                if !explicit_flags || args.android {
                    android::run(args, root, args.dry_run, args.verbose);
                }
            }
            ProjectType::Ios => {
                if !explicit_flags || args.ios {
                    ios::run(args, root, args.dry_run, args.verbose);
                }
            }
            ProjectType::Node { .. } => {
                if !explicit_flags || args.node {
                    node::run(args, root, args.dry_run, args.verbose);
                }
            }
            ProjectType::Rust => {
                if !explicit_flags || args.rust {
                    rust::run(args, root, args.dry_run, args.verbose);
                }
            }
            ProjectType::Go => {
                if !explicit_flags || args.go {
                    go::run(args, root, args.dry_run, args.verbose);
                }
            }
            ProjectType::Ruby { .. } => {
                if !explicit_flags || args.ruby {
                    ruby::run(args, root, args.dry_run, args.verbose);
                }
            }
            ProjectType::Python { .. } => {
                if !explicit_flags || args.python {
                    python::run(args, root, args.dry_run, args.verbose);
                }
            }
            ProjectType::Unknown => {}
        }
        if !args.no_extras {
            extras::run(args, root, args.dry_run, args.verbose);
        }
        // Git worktrees are a VCS concept, not tied to project type — offer to
        // remove linked worktrees of every detected repo. Self-gated on git
        // availability and a default-No prompt.
        worktrees::run(args, root, runner, args.dry_run, args.verbose);
    }

    // Global caches
    let home = dirs::home_dir().unwrap_or_default();
    let gradle_home = std::env::var("GRADLE_USER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".gradle"));

    let flutter_sdk_caches = locate_flutter_sdk_caches(runner);

    let mut global_paths: Vec<PathBuf> = Vec::new();
    if do_pub {
        global_paths.push(home.join(".pub-cache"));
    }
    if do_flutter_sdk {
        global_paths.extend(flutter_sdk_caches.iter().cloned());
    }
    if do_gradle {
        global_paths.push(gradle_home.join("caches"));
        global_paths.push(gradle_home.join("wrapper").join("dists"));
        global_paths.push(gradle_home.join("daemon"));
        global_paths.push(home.join(".kotlin"));
    }
    if do_derived_data {
        global_paths.push(
            home.join("Library")
                .join("Developer")
                .join("Xcode")
                .join("DerivedData"),
        );
    }
    if do_pod_cache {
        global_paths.push(home.join("Library").join("Caches").join("CocoaPods"));
    }

    let existing_globals: Vec<&Path> = global_paths
        .iter()
        .filter(|p| p.exists())
        .map(PathBuf::as_path)
        .collect();

    if !existing_globals.is_empty() {
        logger.info(&format!("\n{}", "Global caches to delete:".cyan()));
        for p in &existing_globals {
            logger.info(&format!("  {}", p.display()));
        }

        if args.dry_run {
            logger.detail("  (dry run — skipped)");
        } else {
            let selected =
                select_paths_to_delete(&existing_globals, &logger, "  Delete global caches?");
            let pub_cache = home.join(".pub-cache");
            for p in selected {
                if p == pub_cache.as_path() {
                    // Prefer flutter's own cleaner; fall back to rm.
                    let clean_result =
                        runner.run("flutter", &["pub", "cache", "clean", "-f"], None);
                    if clean_result.is_success() {
                        logger.success(&format!("  {} pub cache cleaned", "✓".green()));
                    } else {
                        delete_path_verbose(p, args.verbose, &logger);
                    }
                } else {
                    delete_path_verbose(p, args.verbose, &logger);
                }
            }
        }
    }

    // Opt-in global cleanup for new ecosystems. Each `run_global` handles its
    // own confirmation prompt (skipped automatically in dry-run mode).
    if args.node_global {
        node::run_global(args, args.dry_run, args.verbose);
    }
    if args.rust_global {
        rust::run_global(args, args.dry_run, args.verbose);
    }
    if args.go_global {
        go::run_global(args, args.dry_run, args.verbose);
    }
    if args.ruby_global {
        ruby::run_global(args, args.dry_run, args.verbose);
    }
    if args.python_global {
        python::run_global(args, args.dry_run, args.verbose);
    }
    if args.python_venv {
        // `run_venv` is per-project; loop over detected Python projects.
        for (info, root) in &sorted_projects {
            if matches!(info.project_type, ProjectType::Python { .. }) {
                python::run_venv(args, root, args.dry_run, args.verbose);
            }
        }
    }

    logger.success("\nPurge complete.");
    0
}

/// Resolve every Flutter SDK `bin/cache` directory worth cleaning:
///   - the currently-active SDK (via `$FLUTTER_ROOT` or `which flutter`)
///   - every FVM-managed version under `$FVM_CACHE_PATH/versions/*`,
///     `~/fvm/versions/*`, and `~/.fvm/versions/*`
///   - asdf/mise-managed versions under `~/.asdf/installs/flutter/*`
///
/// Returns deduplicated (canonical) paths so a symlinked active SDK doesn't
/// get listed twice.
fn locate_flutter_sdk_caches(runner: &dyn Runner) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut version_roots: Vec<PathBuf> = Vec::new();
    if let Ok(custom) = std::env::var("FVM_CACHE_PATH") {
        version_roots.push(PathBuf::from(custom).join("versions"));
    }
    version_roots.push(home.join("fvm").join("versions"));
    version_roots.push(home.join(".fvm").join("versions"));
    version_roots.push(home.join(".asdf").join("installs").join("flutter"));

    collect_flutter_sdk_caches(active_flutter_sdk_cache(runner), &version_roots)
}

/// Pure helper: combine an optional active SDK cache with every
/// `<version_root>/<version>/bin/cache` that exists, deduped by canonical
/// path. Extracted so tests can inject synthetic roots.
fn collect_flutter_sdk_caches(active: Option<PathBuf>, version_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(p) = active {
        out.push(p);
    }
    for root in version_roots {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let cache = entry.path().join("bin").join("cache");
            if cache.exists() {
                out.push(cache);
            }
        }
    }
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    out.retain(|p| {
        let key = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        seen.insert(key)
    });
    out
}

fn active_flutter_sdk_cache(runner: &dyn Runner) -> Option<PathBuf> {
    if let Ok(root) = std::env::var("FLUTTER_ROOT") {
        let p = PathBuf::from(root).join("bin").join("cache");
        if p.exists() {
            return Some(p);
        }
    }
    let flutter_bin = runner.which("flutter")?;
    let resolved = std::fs::canonicalize(&flutter_bin).ok()?;
    let cache = resolved.parent()?.join("cache");
    if cache.exists() {
        Some(cache)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn collects_every_fvm_version_and_dedupes_active() {
        let tmp = TempDir::new().unwrap();
        let versions_root = tmp.path().join("versions");
        for v in &["3.41.5", "3.41.6", "3.44.0"] {
            let cache = versions_root.join(v).join("bin").join("cache");
            std::fs::create_dir_all(&cache).unwrap();
        }
        // Active SDK points at the same physical path as one of the versions —
        // dedupe should drop the duplicate.
        let active = Some(versions_root.join("3.44.0").join("bin").join("cache"));

        let caches = collect_flutter_sdk_caches(active, &[versions_root.clone()]);

        assert_eq!(caches.len(), 3);
        let names: Vec<String> = caches
            .iter()
            .filter_map(|p| {
                p.parent()
                    .and_then(|b| b.parent())
                    .and_then(|v| v.file_name())
                    .and_then(|n| n.to_str().map(String::from))
            })
            .collect();
        assert!(names.contains(&"3.41.5".to_string()));
        assert!(names.contains(&"3.41.6".to_string()));
        assert!(names.contains(&"3.44.0".to_string()));
    }

    #[test]
    fn missing_version_root_is_ignored() {
        let caches = collect_flutter_sdk_caches(None, &[PathBuf::from("/nonexistent/path")]);
        assert!(caches.is_empty());
    }

    fn rel_paths(projects: &[(AppInfo, PathBuf)], base: &Path) -> Vec<String> {
        projects
            .iter()
            .map(|(_, p)| {
                p.strip_prefix(base)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    fn touch(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    /// Purge must discover downward from CWD only — never walk up to a parent
    /// container that happens to carry a weak marker file.
    #[test]
    fn collect_projects_does_not_walk_up_to_parent_container() {
        let tmp = TempDir::new().unwrap();
        touch(&tmp.path().join("Gemfile"), "");
        touch(
            &tmp.path().join("repo/Cargo.toml"),
            "[package]\nname = \"x\"\n",
        );

        let repo = tmp.path().join("repo");
        let got = collect_projects(&repo);

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, repo);
        assert_eq!(got[0].0.project_type, ProjectType::Rust);
    }

    /// End-to-end purge discovery path for the `$HOME` + `package.json` layout.
    #[test]
    fn collect_projects_home_layout_finds_nested_repos_only() {
        let tmp = TempDir::new().unwrap();
        touch(&tmp.path().join("package.json"), "{}");
        touch(
            &tmp.path()
                .join("projects/open-source/cli-apps-helper/Cargo.toml"),
            "[package]\nname = \"mdev\"\n",
        );
        touch(
            &tmp.path().join("projects/other-app/go.mod"),
            "module example.com/other\n",
        );
        touch(
            &tmp.path()
                .join("Library/Zed/languages/bash-language-server/package.json"),
            "{}",
        );

        let got = collect_projects(tmp.path());
        let paths = rel_paths(&got, tmp.path());

        assert_eq!(
            paths,
            vec![
                "projects/open-source/cli-apps-helper".to_string(),
                "projects/other-app".to_string(),
            ]
        );
        assert!(
            got.iter().all(|(_, p)| p != tmp.path()),
            "purge must not treat a container scan root as the sole project"
        );
    }

    #[test]
    fn collect_projects_single_repo_at_scan_root() {
        let tmp = TempDir::new().unwrap();
        touch(
            &tmp.path().join("Cargo.toml"),
            "[package]\nname = \"solo\"\n",
        );

        let got = collect_projects(tmp.path());

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, tmp.path().to_path_buf());
        assert_eq!(got[0].0.project_type, ProjectType::Rust);
    }
}
