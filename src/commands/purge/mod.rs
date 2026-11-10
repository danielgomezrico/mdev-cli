//! `mdev purge` — scan everything purgeable, pick in a tree, delete in parallel.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use clap::Args;
use colored::Colorize;

use crate::app_detector::AppDetector;
use crate::fs_size::{human_size, SizeMode};
use crate::logger::{Logger, Progress};
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

use candidate::{display_path, outermost, Category, Tier};
use executor::ExecOptions;
use scan::ScanEvent;
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

    let scanners: Vec<Box<dyn scan::Scanner>> = scan::all_scanners()
        .into_iter()
        .filter(|s| enabled(s.category(), &args.only, &args.skip))
        .collect();
    let needs_projects = scanners.iter().any(|s| {
        matches!(
            s.category(),
            Category::Projects | Category::Extras | Category::Worktrees
        )
    });

    // Phase 1: project discovery (sequential walk; slow on big trees).
    let projects: Vec<(AppInfo, PathBuf)> = if needs_projects {
        let spinner = logger.progress("Discovering projects…");
        let mut visited: usize = 0;
        let found: Vec<(AppInfo, PathBuf)> = roots
            .iter()
            .flat_map(|r| {
                collect_projects(r, &mut |dir| {
                    visited += 1;
                    spinner.set_message(format!(
                        "Discovering projects… {visited} dirs · {}",
                        display_path(dir, &home)
                    ));
                })
            })
            .collect();
        spinner.finish_and_clear();
        logger.detail(&format!(
            "{} {} project(s) under {} root(s), {} dirs walked ({})",
            "✓".green(),
            found.len(),
            roots.len(),
            visited,
            elapsed_text(started.elapsed())
        ));
        found
    } else {
        Vec::new()
    };

    let ctx = scan::ScanContext {
        roots: &roots,
        projects: &projects,
        home,
        tmp_roots: scan::tmp_roots(),
        tmp_age: Duration::from_secs(args.tmp_age * 24 * 3600),
        now: SystemTime::now(),
        runner,
        size_mode: SizeMode::Disk,
        progress: scan::silent(),
    };

    // Phase 2: one live line per category, then a sizing bar.
    let scan_started = Instant::now();
    let group = logger.progress_group();
    let lines: BTreeMap<Category, Progress> = scanners
        .iter()
        .map(|s| {
            (
                s.category(),
                group.spinner(s.category().label(), "waiting…"),
            )
        })
        .collect();
    let sizing = group.bar(0, "");
    let candidates = scan::run_all(&scanners, &ctx, &|ev| match ev {
        ScanEvent::Note(cat, msg) => {
            if let Some(p) = lines.get(&cat) {
                p.set_message(msg);
            }
        }
        ScanEvent::Done(cat, found) => {
            if let Some(p) = lines.get(&cat) {
                p.finish_with_message(format!("{found} found"));
            }
        }
        ScanEvent::Sized { done, total } => {
            sizing.set_length(total as u64);
            sizing.set_position(done as u64);
            sizing.set_message(format!("sizing {total} items in parallel…"));
        }
    });
    sizing.finish_and_clear();
    group.clear();

    let mut tree = SelectionTree::build(candidates);
    let t = tree.totals();
    logger.detail(&format!(
        "{} {} item(s), {} scanned in {}",
        "✓".green(),
        t.total,
        human_size(t.total_bytes),
        elapsed_text(scan_started.elapsed())
    ));
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
        safety: ctx.safety(),
        size_mode: SizeMode::Disk,
    };
    let bar = logger.progress_bar(items.len() as u64, "deleting…");
    let verbose = args.verbose;
    let outcomes = executor::execute(&tree, &items, runner, &opts, &|o| {
        bar.inc(1);
        let cand = tree.candidate(o.item);
        bar.set_message(format!("deleted {}", cand.label));
        if verbose {
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

fn elapsed_text(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 {
        format!("{:.0} ms", s * 1000.0)
    } else if s < 60.0 {
        format!("{s:.1} s")
    } else {
        format!("{}m {:02}s", (s / 60.0) as u64, (s % 60.0) as u64)
    }
}

fn enabled(cat: Category, only: &[Category], skip: &[Category]) -> bool {
    if !only.is_empty() {
        return only.contains(&cat);
    }
    !skip.contains(&cat)
}

/// Canonical scan roots, sorted, without roots nested under another (the
/// outer walk covers them); cwd when none given.
fn scan_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let given: Vec<PathBuf> = if paths.is_empty() {
        vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
    } else {
        paths.to_vec()
    };
    outermost(
        given
            .into_iter()
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .collect(),
    )
}

/// Discover every project under `start_dir` — downward only. A repo-root
/// scan root is listed together with its sub-projects.
pub(crate) fn collect_projects(
    start_dir: &Path,
    on_visit: &mut dyn FnMut(&Path),
) -> Vec<(AppInfo, PathBuf)> {
    let mut found = AppDetector::new().discover_projects_with(start_dir, on_visit);
    found.dedup_by(|a, b| a.1 == b.1);
    found
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
    fn elapsed_text_picks_unit() {
        assert_eq!(elapsed_text(Duration::from_millis(250)), "250 ms");
        assert_eq!(elapsed_text(Duration::from_millis(13_600)), "13.6 s");
        assert_eq!(elapsed_text(Duration::from_secs(125)), "2m 05s");
    }

    #[test]
    fn collect_projects_reports_visited_dirs() {
        let tmp = TempDir::new().unwrap();
        touch(
            &tmp.path().join("a/Cargo.toml"),
            "[package]\nname = \"a\"\n",
        );
        touch(&tmp.path().join("b/c/go.mod"), "module x\n");
        let mut seen: Vec<PathBuf> = Vec::new();
        let got = collect_projects(tmp.path(), &mut |d| seen.push(d.to_path_buf()));
        assert_eq!(got.len(), 2);
        assert!(seen.contains(&tmp.path().to_path_buf()));
        assert!(seen.contains(&tmp.path().join("b/c")));
    }

    #[test]
    fn scan_roots_dedupes_and_defaults_to_cwd() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().to_path_buf();
        let roots = scan_roots(&[p.clone(), p.clone()]);
        assert_eq!(roots.len(), 1);
        assert_eq!(scan_roots(&[]).len(), 1);
    }

    #[test]
    fn scan_roots_drop_roots_nested_under_another() {
        let tmp = TempDir::new().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        for d in ["a/sub", "ab"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        let roots = scan_roots(&[
            tmp.path().join("a/sub"),
            tmp.path().join("ab"),
            tmp.path().join("a"),
        ]);
        assert_eq!(roots, vec![base.join("a"), base.join("ab")]);
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
        let got = collect_projects(&repo, &mut |_| {});
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
        let got = collect_projects(tmp.path(), &mut |_| {});
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
    fn collect_projects_keeps_repo_root_and_its_sub_projects() {
        let tmp = TempDir::new().unwrap();
        touch(&tmp.path().join(".git/HEAD"), "");
        touch(&tmp.path().join("pubspec.yaml"), "name: plugin\n");
        touch(&tmp.path().join("example/pubspec.yaml"), "name: example\n");
        let got = collect_projects(tmp.path(), &mut |_| {});
        assert_eq!(rel_paths(&got, tmp.path()), vec!["", "example"]);
    }

    #[test]
    fn collect_projects_single_repo_at_scan_root() {
        let tmp = TempDir::new().unwrap();
        touch(
            &tmp.path().join("Cargo.toml"),
            "[package]\nname = \"solo\"\n",
        );
        let got = collect_projects(tmp.path(), &mut |_| {});
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, tmp.path().to_path_buf());
    }
}
