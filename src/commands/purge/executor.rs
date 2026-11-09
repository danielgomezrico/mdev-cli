//! Removes selected candidates: path targets in parallel, action targets
//! one after another on the calling thread.

use std::path::Path;

use crate::fs_size::{dir_size, SizeMode};
use crate::parallel;
use crate::runner::Runner;

use super::candidate::{EntryKind, ItemId, Target};
use super::safety::{assert_deletable, SafetyCtx};
use super::tree::SelectionTree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub item: ItemId,
    pub bytes_freed: u64,
    pub error: Option<String>,
}

pub struct ExecOptions {
    pub dry_run: bool,
    pub safety: SafetyCtx,
    pub size_mode: SizeMode,
}

/// Delete `items`. Nested path items are skipped (their parent removal
/// covers them). Paths failing the safety check become error outcomes and
/// are never touched. `on_done` is called from worker threads.
pub fn execute(
    tree: &SelectionTree,
    items: &[ItemId],
    runner: &dyn Runner,
    opts: &ExecOptions,
    on_done: &(dyn Fn(&Outcome) + Sync),
) -> Vec<Outcome> {
    let covered = covered_by_parent(tree, items);
    let mut outcomes: Vec<Outcome> = Vec::new();
    let mut paths: Vec<(ItemId, &Path, EntryKind)> = Vec::new();
    let mut actions: Vec<ItemId> = Vec::new();

    for id in items {
        let cand = tree.candidate(*id);
        match &cand.target {
            Target::Path { path, kind } => {
                if covered.contains(id) {
                    outcomes.push(done(*id, 0, None, on_done));
                    continue;
                }
                if let Err(e) = assert_deletable(path, &opts.safety) {
                    outcomes.push(done(*id, 0, Some(format!("refused: {e}")), on_done));
                    continue;
                }
                paths.push((*id, path.as_path(), *kind));
            }
            Target::Action { .. } => actions.push(*id),
        }
    }

    let path_outcomes = parallel::map(paths, |(id, path, kind)| {
        let size = tree.candidate(id).size.bytes();
        if opts.dry_run {
            return done(id, size, None, on_done);
        }
        match remove(path, kind) {
            Ok(()) => done(id, size, None, on_done),
            Err(e) => done(id, 0, Some(e.to_string()), on_done),
        }
    });
    outcomes.extend(path_outcomes);

    for id in actions {
        if let Target::Action {
            exe,
            args,
            cwd,
            affects,
        } = &tree.candidate(id).target
        {
            let size = tree.candidate(id).size.bytes();
            if opts.dry_run {
                outcomes.push(done(id, size, None, on_done));
                continue;
            }
            let before: u64 = affects.iter().map(|p| dir_size(p, opts.size_mode)).sum();
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let cwd_str = cwd.as_ref().map(|p| p.to_string_lossy().into_owned());
            let result = runner.run(exe, &arg_refs, cwd_str.as_deref());
            if result.is_success() {
                let after: u64 = affects.iter().map(|p| dir_size(p, opts.size_mode)).sum();
                let freed = if affects.is_empty() {
                    size
                } else {
                    before.saturating_sub(after)
                };
                outcomes.push(done(id, freed, None, on_done));
            } else {
                let msg = if result.stderr.is_empty() {
                    format!("exit {}", result.exit_code)
                } else {
                    result.stderr.lines().last().unwrap_or("").to_string()
                };
                outcomes.push(done(id, 0, Some(msg), on_done));
            }
        }
    }

    outcomes.sort_by_key(|o| o.item);
    outcomes
}

fn done(
    item: ItemId,
    bytes_freed: u64,
    error: Option<String>,
    on_done: &(dyn Fn(&Outcome) + Sync),
) -> Outcome {
    let o = Outcome {
        item,
        bytes_freed,
        error,
    };
    on_done(&o);
    o
}

fn remove(path: &Path, kind: EntryKind) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || kind == EntryKind::File || !meta.is_dir() {
        std::fs::remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    }
}

/// Path items whose path lies under another selected path item.
fn covered_by_parent(tree: &SelectionTree, items: &[ItemId]) -> Vec<ItemId> {
    let paths: Vec<(ItemId, &Path)> = items
        .iter()
        .filter_map(|id| tree.candidate(*id).target.path().map(|p| (*id, p)))
        .collect();
    paths
        .iter()
        .filter(|(id, p)| {
            paths
                .iter()
                .any(|(other, parent)| other != id && *p != *parent && p.starts_with(parent))
        })
        .map(|(id, _)| *id)
        .collect()
}

/// Thread-safe collector for `on_done` callbacks.
#[cfg(test)]
#[derive(Default)]
pub struct OutcomeLog(std::sync::Mutex<Vec<Outcome>>);

#[cfg(test)]
impl OutcomeLog {
    pub fn push(&self, o: &Outcome) {
        self.0.lock().unwrap().push(o.clone());
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::purge::candidate::{Candidate, Category, Size, Tier};
    use crate::commands::purge::scan::test_support::FakeRunner;
    use crate::runner::RunResult;
    use std::fs;
    use tempfile::TempDir;

    fn opts(home: &Path) -> ExecOptions {
        ExecOptions {
            dry_run: false,
            safety: SafetyCtx {
                home: home.to_path_buf(),
                ..Default::default()
            },
            size_mode: SizeMode::Apparent,
        }
    }

    fn dir_cand(p: &Path, size: u64) -> Candidate {
        Candidate::new(
            Category::Projects,
            "g",
            p.display().to_string(),
            Target::dir(p.to_path_buf()),
            Size::Bytes(size),
            Tier::Local,
        )
    }

    #[test]
    fn deletes_many_dirs_in_parallel_and_reports_sizes() {
        let home = TempDir::new().unwrap();
        let mut cands = Vec::new();
        for i in 0..50 {
            let d = home.path().join(format!("d{i}"));
            fs::create_dir_all(d.join("sub")).unwrap();
            fs::write(d.join("sub/f"), vec![0u8; 10]).unwrap();
            cands.push(dir_cand(&d, 10));
        }
        let tree = SelectionTree::build(cands);
        let items = tree.selected_items();
        let log = OutcomeLog::default();
        let runner = FakeRunner::new(&[]);

        let out = execute(&tree, &items, &runner, &opts(home.path()), &|o| log.push(o));

        assert_eq!(out.len(), 50);
        assert_eq!(log.len(), 50);
        assert!(out.iter().all(|o| o.error.is_none() && o.bytes_freed == 10));
        for i in 0..50 {
            assert!(!home.path().join(format!("d{i}")).exists());
        }
    }

    #[test]
    fn dry_run_deletes_nothing_but_reports_sizes() {
        let home = TempDir::new().unwrap();
        let d = home.path().join("x");
        fs::create_dir_all(&d).unwrap();
        let tree = SelectionTree::build(vec![dir_cand(&d, 7)]);
        let mut o = opts(home.path());
        o.dry_run = true;
        let out = execute(
            &tree,
            &tree.selected_items(),
            &FakeRunner::new(&[]),
            &o,
            &|_| {},
        );
        assert_eq!(out[0].bytes_freed, 7);
        assert!(d.exists());
    }

    #[test]
    fn refuses_unsafe_paths_and_skips_covered_children() {
        let home = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let parent = home.path().join("p");
        fs::create_dir_all(parent.join("child")).unwrap();
        let tree = SelectionTree::build(vec![
            dir_cand(&parent, 1),
            dir_cand(&parent.join("child"), 1),
            dir_cand(elsewhere.path(), 1),
        ]);
        let out = execute(
            &tree,
            &tree.selected_items(),
            &FakeRunner::new(&[]),
            &opts(home.path()),
            &|_| {},
        );
        let by_label = |needle: &str| {
            out.iter()
                .find(|o| tree.candidate(o.item).label.contains(needle))
                .unwrap()
        };
        assert!(by_label("/p").error.is_none());
        assert_eq!(by_label("child").bytes_freed, 0);
        assert!(by_label("child").error.is_none());
        assert!(by_label(&elsewhere.path().display().to_string())
            .error
            .as_deref()
            .unwrap()
            .starts_with("refused"));
        assert!(elsewhere.path().exists());
        assert!(!parent.exists());
    }

    #[test]
    fn file_kind_and_symlink_roots_use_remove_file() {
        let home = TempDir::new().unwrap();
        let f = home.path().join("f.log");
        fs::write(&f, b"x").unwrap();
        let real = home.path().join("real");
        fs::create_dir_all(real.join("keep")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, home.path().join("link")).unwrap();
        let mut cands = vec![Candidate::new(
            Category::Extras,
            "g",
            "f.log",
            Target::file(f.clone()),
            Size::Bytes(1),
            Tier::Local,
        )];
        #[cfg(unix)]
        cands.push(dir_cand(&home.path().join("link"), 1));
        let tree = SelectionTree::build(cands);
        let out = execute(
            &tree,
            &tree.selected_items(),
            &FakeRunner::new(&[]),
            &opts(home.path()),
            &|_| {},
        );
        assert!(out.iter().all(|o| o.error.is_none()), "{out:?}");
        assert!(!f.exists());
        assert!(real.join("keep").exists(), "symlink target must survive");
    }

    #[test]
    fn actions_run_through_runner_and_measure_freed_bytes() {
        let home = TempDir::new().unwrap();
        let cache = home.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("blob"), vec![0u8; 500]).unwrap();
        let ok = Candidate::new(
            Category::PackageCaches,
            "go",
            "mod cache",
            Target::action("go", &["clean", "-modcache"], None, vec![cache.clone()]),
            Size::Unknown,
            Tier::Global,
        );
        let failing = Candidate::new(
            Category::Docker,
            "docker",
            "prune",
            Target::action("docker", &["system", "prune", "-f"], None, vec![]),
            Size::Bytes(99),
            Tier::Local,
        );
        let tree = SelectionTree::build(vec![ok, failing]);
        let mut items = tree.selected_items();
        items.sort();
        // Emulate `go clean` by deleting the blob when the runner is invoked.
        struct GoRunner(std::path::PathBuf);
        impl Runner for GoRunner {
            fn run(&self, exe: &str, _args: &[&str], _cwd: Option<&str>) -> RunResult {
                if exe == "go" {
                    fs::remove_file(self.0.join("blob")).unwrap();
                    RunResult::success(String::new())
                } else {
                    RunResult::failure(1, "daemon not running".into())
                }
            }
            fn which(&self, _exe: &str) -> Option<String> {
                None
            }
        }
        let out = execute(
            &tree,
            &[ItemId(0), ItemId(1)],
            &GoRunner(cache.clone()),
            &opts(home.path()),
            &|_| {},
        );
        assert_eq!(out[0].bytes_freed, 500);
        assert!(out[0].error.is_none());
        assert_eq!(out[1].error.as_deref(), Some("daemon not running"));
    }

    #[test]
    fn delete_failure_is_reported_not_panicked() {
        let home = TempDir::new().unwrap();
        let missing = home.path().join("gone");
        let tree = SelectionTree::build(vec![dir_cand(&missing, 1)]);
        let out = execute(
            &tree,
            &tree.selected_items(),
            &FakeRunner::new(&[]),
            &opts(home.path()),
            &|_| {},
        );
        assert!(out[0].error.is_some());
    }
}
