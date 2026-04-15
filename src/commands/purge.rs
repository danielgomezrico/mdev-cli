use clap::Args;
use colored::Colorize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::app_detector::AppDetector;
use crate::logger::Logger;
use crate::models::{AppInfo, ProjectType};
use crate::runner::Runner;

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

    let has_flutter = sorted_projects
        .iter()
        .any(|(info, _)| info.project_type == ProjectType::Flutter);
    let has_android = sorted_projects.iter().any(|(info, _)| {
        info.project_type == ProjectType::Android || info.project_type == ProjectType::Flutter
    });
    let has_ios = sorted_projects.iter().any(|(info, _)| {
        info.project_type == ProjectType::Ios || info.project_type == ProjectType::Flutter
    });

    let explicit_flags = args.flutter || args.pub_cache || args.gradle || args.android || args.ios;

    // Determine global targets
    let do_pub = if explicit_flags {
        args.pub_cache
    } else {
        has_flutter
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

    // Per-project local cleanup
    for (info, root) in &sorted_projects {
        let display_root = root.display();
        logger.info(&format!("\n{} {}", "→".cyan(), display_root));

        match info.project_type {
            ProjectType::Flutter => {
                let do_flutter_clean = !explicit_flags || args.flutter;
                let do_android_build = !explicit_flags || args.android || args.flutter;
                let do_ios_pods =
                    (!explicit_flags || args.ios || args.flutter) && cfg!(target_os = "macos");

                if do_flutter_clean {
                    run_flutter_clean(root, args.dry_run, args.verbose, runner, &logger);
                }
                if do_android_build {
                    delete_paths(
                        &[
                            root.join("android").join("build"),
                            root.join("android").join("app").join("build"),
                            root.join("android").join(".gradle"),
                        ],
                        args.dry_run,
                        args.verbose,
                        &logger,
                    );
                }
                if do_ios_pods {
                    delete_paths(
                        &[
                            root.join("ios").join("Pods"),
                            root.join("ios").join(".symlinks"),
                            root.join("ios").join("build"),
                        ],
                        args.dry_run,
                        args.verbose,
                        &logger,
                    );
                }
            }
            ProjectType::Android => {
                let do_android_build = !explicit_flags || args.android;
                if do_android_build {
                    delete_paths(
                        &[
                            root.join("build"),
                            root.join("app").join("build"),
                            root.join(".gradle"),
                        ],
                        args.dry_run,
                        args.verbose,
                        &logger,
                    );
                }
            }
            ProjectType::Ios => {
                let do_ios_pods = (!explicit_flags || args.ios) && cfg!(target_os = "macos");
                if do_ios_pods {
                    delete_paths(
                        &[
                            root.join("Pods"),
                            root.join(".symlinks"),
                            root.join("build"),
                        ],
                        args.dry_run,
                        args.verbose,
                        &logger,
                    );
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

    let mut global_paths: Vec<PathBuf> = Vec::new();
    if do_pub {
        global_paths.push(home.join(".pub-cache"));
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

    logger.success("\nPurge complete.");
    0
}

fn run_flutter_clean(
    root: &Path,
    dry_run: bool,
    verbose: bool,
    runner: &dyn Runner,
    logger: &Logger,
) {
    let label = format!("flutter clean ({})", root.display());
    if dry_run {
        logger.detail(&format!("  {} {}", "~".cyan(), label));
        return;
    }
    let root_str = root.to_string_lossy().into_owned();
    let result = runner.run("flutter", &["clean"], Some(root_str.as_str()));
    if result.is_success() {
        logger.success(&format!("  {} {}", "✓".green(), label));
    } else {
        logger.err(&format!("  {} Failed: {}", "✗".red(), label));
        if verbose {
            logger.err(&result.stderr);
        }
    }
}

fn delete_paths(paths: &[PathBuf], dry_run: bool, verbose: bool, logger: &Logger) {
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

fn delete_path_verbose(path: &Path, verbose: bool, logger: &Logger) {
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
