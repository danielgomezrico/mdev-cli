use clap::Args;
use colored::Colorize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::app_detector::AppDetector;
use crate::logger::Logger;
use crate::models::{AppInfo, ProjectType};
use crate::runner::Runner;

pub mod common;
pub mod flutter;
pub mod android;
pub mod ios;
pub mod node;
pub mod rust;
pub mod go;
pub mod ruby;
pub mod python;

use common::delete_path_verbose;

#[derive(Args, Debug)]
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

    // Discover projects
    let mut projects: HashMap<String, (AppInfo, PathBuf)> = HashMap::new();

    // Current dir
    let (info, root_opt) = AppDetector::new().detect_with_root(&current_dir);
    if info.project_type != ProjectType::Unknown {
        if let Some(root) = root_opt {
            projects.insert(root.to_string_lossy().to_string(), (info, root));
        }
    }

    // Direct subdirs
    if let Ok(entries) = std::fs::read_dir(&current_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let (sub_info, sub_root_opt) = AppDetector::new().detect_with_root(&path);
                if sub_info.project_type != ProjectType::Unknown {
                    if let Some(sub_root) = sub_root_opt {
                        let key = sub_root.to_string_lossy().to_string();
                        projects.entry(key).or_insert((sub_info, sub_root));
                    }
                }
            }
        }
    }

    // Remove sub-paths: if path A starts with path B + separator and B != A, remove A
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

    // Sort by path
    let mut sorted_projects: Vec<(AppInfo, PathBuf)> = projects.into_values().collect();
    sorted_projects.sort_by(|a, b| a.1.cmp(&b.1));

    if sorted_projects.is_empty() {
        logger.warn("No Flutter/Android/iOS projects found.");
        return 0;
    }

    let has_android = sorted_projects.iter().any(|(info, _)| {
        info.project_type == ProjectType::Android || info.project_type == ProjectType::Flutter
    });
    let has_ios = sorted_projects.iter().any(|(info, _)| {
        info.project_type == ProjectType::Ios || info.project_type == ProjectType::Flutter
    });

    let explicit_flags = args.flutter || args.pub_cache || args.gradle || args.android || args.ios
        || args.node || args.rust || args.go || args.ruby || args.python;

    // Determine global targets — Flutter caches (pub + SDK bin/cache) are offered even when
    // no Flutter project is detected, because they are global disk hogs.
    let do_pub = if explicit_flags {
        args.pub_cache || args.flutter
    } else {
        true
    };
    let do_flutter_sdk = if explicit_flags {
        args.flutter
    } else {
        true
    };
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
    }

    // Global caches
    let home = dirs::home_dir().unwrap_or_default();
    let gradle_home = std::env::var("GRADLE_USER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".gradle"));

    let flutter_sdk_cache = locate_flutter_sdk_cache(runner);

    let mut global_paths: Vec<PathBuf> = Vec::new();
    if do_pub {
        global_paths.push(home.join(".pub-cache"));
    }
    if do_flutter_sdk {
        if let Some(p) = &flutter_sdk_cache {
            global_paths.push(p.clone());
        }
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

    let existing_globals: Vec<&PathBuf> = global_paths.iter().filter(|p| p.exists()).collect();

    if !existing_globals.is_empty() {
        logger.info(&format!("\n{}", "Global caches to delete:".cyan()));
        for p in &existing_globals {
            logger.info(&format!("  {}", p.display()));
        }

        let confirmed = args.dry_run || logger.confirm("  Delete global caches?", false);

        if confirmed && !args.dry_run {
            if do_pub {
                // Try flutter pub cache clean -f first
                let clean_result = runner.run("flutter", &["pub", "cache", "clean", "-f"], None);
                if !clean_result.is_success() {
                    let pub_cache = home.join(".pub-cache");
                    if pub_cache.exists() {
                        match std::fs::remove_dir_all(&pub_cache) {
                            Ok(_) => logger.success(&format!(
                                "  {} Deleted {}",
                                "✓".green(),
                                pub_cache.display()
                            )),
                            Err(e) => logger.err(&format!(
                                "  {} Failed to delete {}: {}",
                                "✗".red(),
                                pub_cache.display(),
                                e
                            )),
                        }
                    }
                } else {
                    logger.success(&format!("  {} pub cache cleaned", "✓".green()));
                }
            }
            if do_flutter_sdk {
                if let Some(p) = &flutter_sdk_cache {
                    delete_path_verbose(p, args.verbose, &logger);
                }
            }
            if do_gradle {
                for p in &[
                    gradle_home.join("caches"),
                    gradle_home.join("wrapper").join("dists"),
                    gradle_home.join("daemon"),
                    home.join(".kotlin"),
                ] {
                    delete_path_verbose(p, args.verbose, &logger);
                }
            }
            if do_derived_data {
                let p = home
                    .join("Library")
                    .join("Developer")
                    .join("Xcode")
                    .join("DerivedData");
                delete_path_verbose(&p, args.verbose, &logger);
            }
            if do_pod_cache {
                let p = home.join("Library").join("Caches").join("CocoaPods");
                delete_path_verbose(&p, args.verbose, &logger);
            }
        } else if args.dry_run {
            logger.detail("  (dry run — skipped)");
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

/// Resolve the Flutter SDK's `bin/cache` directory, preferring `$FLUTTER_ROOT`
/// and falling back to `which flutter` (canonicalizing symlinks so e.g. fvm
/// shims resolve to the real SDK).
fn locate_flutter_sdk_cache(runner: &dyn Runner) -> Option<PathBuf> {
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
