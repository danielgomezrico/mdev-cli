//! `mdev purge` — scan everything purgeable, pick in a tree, delete in parallel.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use clap::Args;
use colored::Colorize;

use crate::app_detector::AppDetector;
use crate::fs_size::{human_size, SizeMode};
use crate::logger::Logger;
use crate::models::AppInfo;
use crate::runner::Runner;
use crate::tui;

pub mod candidate;
pub mod executor;
pub mod report;
pub mod safety;
pub mod scan;
pub mod text_ui;
pub mod tree;

use candidate::{Category, Tier};
use executor::ExecOptions;
use safety::SafetyCtx;
use tree::SelectionTree;

#[derive(Args, Debug, Default)]
pub struct PurgeArgs {
    /// Directories to scan for project artifacts (default: current directory).
    /// Temp dirs, logs, Xcode/Android and package caches are always home-wide.
    pub paths: Vec<PathBuf>,

    /// List everything with sizes and exit — nothing is deleted
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Skip the tree UI and delete the default selection
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Also pre-select the expensive-to-recreate tier (global package caches,
    /// simulator data, archives, venvs, dirty worktrees)
    #[arg(long)]
    pub all: bool,

    /// Only scan these categories (comma separated)
    #[arg(long, value_enum, value_delimiter = ',', conflicts_with = "skip")]
    pub only: Vec<Category>,

    /// Skip these categories (comma separated)
    #[arg(long, value_enum, value_delimiter = ',')]
    pub skip: Vec<Category>,

    /// Temp entries younger than this many days are left alone
    #[arg(long, default_value_t = 3)]
    pub tmp_age: u64,

    /// Verbose output
    #[arg(short = 'v', long)]
    pub verbose: bool,
}

pub fn run(args: &PurgeArgs, runner: &(dyn Runner + Sync)) -> i32 {
    let logger = Logger::new();
    let started = Instant::now();
    let home = dirs::home_dir().unwrap_or_default();
    let roots = scan_roots(&args.paths);
    let projects: Vec<(AppInfo, PathBuf)> =
        roots.iter().flat_map(|r| collect_projects(r)).collect();
    let tmp_roots = scan::tmp_roots();

    let scanners: Vec<Box<dyn scan::Scanner>> = scan::all_scanners()
        .into_iter()
        .filter(|s| enabled(s.category(), &args.only, &args.skip))
        .collect();

    logger.info(&format!(
        "{} scanning {} root(s), {} project(s)…",
        "mdev purge".cyan(),
        roots.len(),
        projects.len()
    ));

    let ctx = scan::ScanContext {
        roots: &roots,
        projects: &projects,
        home: home.clone(),
        tmp_roots: tmp_roots.clone(),
        tmp_age: Duration::from_secs(args.tmp_age * 24 * 3600),
        now: SystemTime::now(),
        runner,
        size_mode: SizeMode::Disk,
    };

    let spinner = logger.progress("Scanning…");
    let pending: Mutex<BTreeSet<Category>> =
        Mutex::new(scanners.iter().map(|s| s.category()).collect());
    spinner.set_message(scanning_msg(&pending.lock().unwrap()));
    let candidates = scan::run_all(&scanners, &ctx, &|cat| {
        let mut p = pending.lock().unwrap();
        p.remove(&cat);
        spinner.set_message(scanning_msg(&p));
    });
    spinner.finish_and_clear();

    let mut tree = SelectionTree::build(candidates);
    if args.all {
        tree.select_tier(Tier::Global, true);
    }
    if tree.is_empty() {
        logger.success("Nothing to purge.");
        return 0;
    }

    let width = tui::terminal_width().unwrap_or(100);
    if args.dry_run {
        print!("{}", text_ui::render(&tree, width));
        let t = tree.totals();
        logger.info(&format!(
            "\n{} selected of {} ({} of {}) — dry run, nothing deleted.",
            t.selected,
            t.total,
            human_size(t.selected_bytes),
            human_size(t.total_bytes)
        ));
        return 0;
    }

    if !args.yes {
        if !tui::stdout_is_tty() {
            print!("{}", text_ui::render(&tree, width));
            logger
                .warn("stdout is not a terminal; re-run with --yes to delete the selection above.");
            return 2;
        }
        match tui::select_tree(&mut tree, "mdev purge") {
            Ok(tui::SelectOutcome::Confirm) => {}
            Ok(tui::SelectOutcome::Abort) => {
                logger.info("Aborted, nothing deleted.");
                return 0;
            }
            Err(e) => {
                logger.err(&format!("terminal error: {e}"));
                return 1;
            }
        }
    }

    let items = tree.selected_items();
    if items.is_empty() {
        logger.info("Nothing selected.");
        return 0;
    }

    let opts = ExecOptions {
        dry_run: false,
        safety: SafetyCtx {
            home,
            scan_roots: roots.clone(),
            project_roots: projects.iter().map(|(_, r)| r.clone()).collect(),
            tmp_roots,
        },
        size_mode: SizeMode::Disk,
    };
    let bar = logger.progress_bar(items.len() as u64, "deleting…");
    let verbose = args.verbose;
    let outcomes = executor::execute(&tree, &items, runner, &opts, &|o| {
        bar.inc(1);
        if verbose {
            let cand = tree.candidate(o.item);
            let line = match &o.error {
                None => format!("  {} {}", "✓".green(), cand.label),
                Some(e) => format!("  {} {}: {}", "✗".red(), cand.label, e),
            };
            bar.suspend(|| println!("{line}"));
        }
    });
    bar.finish_and_clear();

    let summary = report::summarize(&tree, &outcomes);
    print!("{}", report::render(&summary, started.elapsed(), false));
    if summary.failures.is_empty() {
        0
    } else {
        1
    }
}

fn scanning_msg(pending: &BTreeSet<Category>) -> String {
    if pending.is_empty() {
        "Scanning… done".to_string()
    } else {
        let names: Vec<&str> = pending.iter().map(|c| c.label()).collect();
        format!("Scanning… {}", names.join(", "))
    }
}

fn enabled(cat: Category, only: &[Category], skip: &[Category]) -> bool {
    if !only.is_empty() {
        return only.contains(&cat);
    }
    !skip.contains(&cat)
}

/// Canonical, deduplicated scan roots; cwd when none given.
fn scan_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let given: Vec<PathBuf> = if paths.is_empty() {
        vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
    } else {
        paths.to_vec()
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for p in given {
        let canon = std::fs::canonicalize(&p).unwrap_or(p);
        if !out.contains(&canon) {
            out.push(canon);
        }
    }
    out
}

/// Discover every project under `start_dir` — downward only, with
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProjectType;
    use tempfile::TempDir;

    #[test]
    fn only_and_skip_filters() {
        assert!(enabled(Category::Temp, &[], &[]));
        assert!(!enabled(Category::Temp, &[], &[Category::Temp]));
        assert!(enabled(Category::Temp, &[Category::Temp], &[]));
        assert!(!enabled(Category::Logs, &[Category::Temp], &[]));
    }

    #[test]
    fn scan_roots_dedupes_and_defaults_to_cwd() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().to_path_buf();
        let roots = scan_roots(&[p.clone(), p.clone()]);
        assert_eq!(roots.len(), 1);
        assert_eq!(scan_roots(&[]).len(), 1);
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
        assert_eq!(
            rel_paths(&got, tmp.path()),
            vec![
                "projects/open-source/cli-apps-helper".to_string(),
                "projects/other-app".to_string(),
            ]
        );
        assert!(got.iter().all(|(_, p)| p != tmp.path()));
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
    }
}
