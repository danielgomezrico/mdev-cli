//! Git worktrees: linked worktrees registered with git, convention-named
//! worktree folders on disk, and zombie checkouts in temp dirs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::super::candidate::{display_path, Candidate, Category, Size, Target, Tier};
use super::{is_heavy_or_owned_dir, ScanContext, Scanner};

/// Max walk depth from a root (root = 0). Bounds monorepo scans.
const MAX_DEPTH: usize = 4;
/// Max walk depth under each temp root (root = 0).
const TMP_MAX_DEPTH: usize = 3;

pub struct WorktreesScanner;

impl Scanner for WorktreesScanner {
    fn category(&self) -> Category {
        Category::Worktrees
    }

    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
        let has_git = ctx.runner.which("git").is_some();
        let mut out = Vec::new();
        for root in repo_roots(ctx) {
            let group = display_path(&root, &ctx.home);
            ctx.note(format!("git worktree list {group}"));
            let root_str = root.to_string_lossy().into_owned();

            let mut git_targets: Vec<PathBuf> = Vec::new();
            if has_git {
                let listed = ctx.runner.run(
                    "git",
                    &["-C", &root_str, "worktree", "list", "--porcelain"],
                    None,
                );
                if listed.is_success() {
                    for wt in linked(&parse_worktrees(&listed.stdout)) {
                        if std::fs::symlink_metadata(&wt.path).is_err() {
                            continue;
                        }
                        git_targets.push(wt.path.clone());
                        let dirty = is_dirty(ctx, &wt.path);
                        let tier = if dirty || wt.locked {
                            Tier::Global
                        } else {
                            Tier::Local
                        };
                        let mut note = Vec::new();
                        if wt.locked {
                            note.push("locked");
                        }
                        if dirty {
                            note.push("uncommitted changes");
                        }
                        let label = if note.is_empty() {
                            display_path(&wt.path, &ctx.home)
                        } else {
                            format!(
                                "{} ({})",
                                display_path(&wt.path, &ctx.home),
                                note.join(", ")
                            )
                        };
                        let wt_str = wt.path.to_string_lossy().into_owned();
                        let mut args =
                            vec!["-C", root_str.as_str(), "worktree", "remove", "--force"];
                        if wt.locked {
                            args.push("--force");
                        }
                        args.push(wt_str.as_str());
                        out.push(Candidate::new(
                            Category::Worktrees,
                            group.clone(),
                            label,
                            Target::action("git", &args, None, vec![wt.path.clone()]),
                            Size::Skipped,
                            tier,
                        ));
                    }
                }
            }

            let fs_candidates = discover_worktree_folders(&root);
            let (_, fs_only) = union_targets(&git_targets, &fs_candidates);
            for p in prune_covered_descendants(&fs_only) {
                out.push(Candidate::new(
                    Category::Worktrees,
                    group.clone(),
                    relative_label(&p, &root, &ctx.home),
                    Target::dir(p),
                    Size::Skipped,
                    Tier::Local,
                ));
            }
        }
        out
    }
}

/// `path` relative to `root` when nested under it, else `~`-abbreviated.
fn relative_label(path: &Path, root: &Path, home: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => display_path(path, home),
    }
}

/// Scan roots plus detected project roots, deduplicated.
fn repo_roots(ctx: &ScanContext) -> Vec<PathBuf> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();
    for r in ctx.roots.iter().chain(ctx.projects.iter().map(|(_, r)| r)) {
        let key = std::fs::canonicalize(r).unwrap_or_else(|_| r.clone());
        if seen.insert(key) {
            out.push(r.clone());
        }
    }
    out
}

fn is_dirty(ctx: &ScanContext, worktree: &Path) -> bool {
    ctx.note(format!("git status {}", display_path(worktree, &ctx.home)));
    let wt = worktree.to_string_lossy();
    let status = ctx
        .runner
        .run("git", &["-C", &wt, "status", "--porcelain"], None);
    status.is_success() && !status.stdout.trim().is_empty()
}

/// A worktree entry parsed from `git worktree list --porcelain`. The first
/// entry git prints is always the repository's main working tree.
#[derive(Debug, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub is_main: bool,
    pub locked: bool,
    pub bare: bool,
}

/// Blocks are separated by blank lines; recognized attribute lines are
/// `locked` and `bare`.
pub fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.push(Worktree {
                path: PathBuf::from(path.trim()),
                is_main: out.is_empty(),
                locked: false,
                bare: false,
            });
        } else if line == "locked" || line.starts_with("locked ") {
            if let Some(w) = out.last_mut() {
                w.locked = true;
            }
        } else if line == "bare" {
            if let Some(w) = out.last_mut() {
                w.bare = true;
            }
        }
    }
    out
}

/// Linked worktrees: not the main tree, not bare. Locked ones included.
pub fn linked(worktrees: &[Worktree]) -> Vec<&Worktree> {
    worktrees.iter().filter(|w| !w.is_main && !w.bare).collect()
}

/// Linked worktrees that are also not locked.
#[cfg(test)]
pub fn removable(worktrees: &[Worktree]) -> Vec<&Worktree> {
    linked(worktrees)
        .into_iter()
        .filter(|w| !w.locked)
        .collect()
}

/// Basename matches a known worktree layout name.
fn is_worktree_basename(name: &str) -> bool {
    matches!(name, "worktree" | "worktrees" | ".worktree" | ".worktrees")
        || name.ends_with("-worktree")
        || (name.ends_with(".worktree") && name != ".worktree")
}

/// Multi-worktree parent: children of these dirs are also candidates.
fn is_multi_worktree_parent(name: &str, rel: &Path) -> bool {
    matches!(name, "worktree" | "worktrees" | ".worktree" | ".worktrees")
        || rel == Path::new(".agents/wt")
        || rel.ends_with(Path::new(".agents/wt"))
}

/// Iterative BFS walk under `root` for convention worktree folders.
/// Depth-bounded; skips heavy dirs and symlinks (no out-of-root escape).
pub fn discover_worktree_folders(root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut queue: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];

    while let Some((dir, depth)) = queue.pop() {
        if depth >= MAX_DEPTH {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
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
            if ft.is_symlink() || !ft.is_dir() || is_heavy_or_owned_dir(name) {
                continue;
            }
            let rel = match path.strip_prefix(root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if is_worktree_basename(name) {
                push_unique(&mut found, &mut seen, path.clone());
            }
            if is_multi_worktree_parent(name, rel) {
                push_multi_parent_children(&path, root, &mut found, &mut seen);
            }
            queue.push((path, depth + 1));
        }
    }

    found.sort();
    found
}

fn push_multi_parent_children(
    parent: &Path,
    root: &Path,
    found: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) {
    let entries = match std::fs::read_dir(parent) {
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
        if ft.is_symlink() || !ft.is_dir() || is_heavy_or_owned_dir(name) {
            continue;
        }
        if path.strip_prefix(root).is_err() {
            continue;
        }
        push_unique(found, seen, path);
    }
}

fn push_unique(found: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, path: PathBuf) {
    if seen.insert(path.clone()) {
        found.push(path);
    }
}

/// Union git paths with FS candidates; return (sorted union, fs-only).
pub fn union_targets(
    git_targets: &[PathBuf],
    fs_candidates: &[PathBuf],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut union: Vec<PathBuf> = Vec::new();
    let mut fs_only: Vec<PathBuf> = Vec::new();
    for p in git_targets {
        if seen.insert(path_key(p)) {
            union.push(p.clone());
        }
    }
    for p in fs_candidates {
        if seen.insert(path_key(p)) {
            union.push(p.clone());
            fs_only.push(p.clone());
        }
    }
    union.sort();
    fs_only.sort();
    (union, fs_only)
}

fn path_key(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches('/').to_string()
}

/// Keep shallowest ancestors only — a parent wipe covers nested children.
pub fn prune_covered_descendants(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut sorted: Vec<PathBuf> = paths.to_vec();
    sorted.sort();
    let mut out: Vec<PathBuf> = Vec::new();
    for p in sorted {
        if out.iter().any(|parent| p.starts_with(parent)) {
            continue;
        }
        out.push(p);
    }
    out
}

/// Gitdir a linked-worktree `.git` file points to; `None` if `dir/.git` is
/// not a file or has no `gitdir:` line.
pub fn worktree_gitdir(dir: &Path) -> Option<PathBuf> {
    let git = dir.join(".git");
    if !git.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(&git).ok()?;
    let target = content.lines().find_map(|l| l.strip_prefix("gitdir:"))?;
    let target = PathBuf::from(target.trim());
    Some(if target.is_relative() {
        dir.join(target)
    } else {
        target
    })
}

/// Walk `roots` for linked-worktree checkouts whose gitdir is gone (repo
/// deleted or worktree pruned). Depth-bounded; skips symlinks; never
/// descends into a worktree.
pub fn discover_tmp_zombies(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut queue: Vec<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();

    while let Some((dir, depth)) = queue.pop() {
        if depth >= TMP_MAX_DEPTH {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_symlink() || !ft.is_dir() {
                continue;
            }
            let path = entry.path();
            match worktree_gitdir(&path) {
                Some(gitdir) if !gitdir.exists() => found.push(path),
                Some(_) => {}
                None => queue.push((path, depth + 1)),
            }
        }
    }

    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, FakeRunner};
    use super::*;
    use crate::runner::RunResult;
    use std::fs;
    use tempfile::TempDir;

    const SAMPLE: &str = "\
worktree /repo
HEAD aaaa
branch refs/heads/main

worktree /repo/.agents/wt/feature
HEAD bbbb
branch refs/heads/feature

worktree /repo/.claude/worktrees/agent-1
HEAD cccc
detached
locked harness-managed

worktree /repo/.agents/wt/dirty
HEAD dddd
branch refs/heads/dirty
";

    #[test]
    fn skips_main_and_locked_worktrees() {
        let parsed = parse_worktrees(SAMPLE);
        assert_eq!(parsed.len(), 4);
        assert!(parsed[0].is_main);
        assert!(parsed[2].locked);
        let targets: Vec<&Path> = removable(&parsed)
            .iter()
            .map(|w| w.path.as_path())
            .collect();
        assert_eq!(
            targets,
            vec![
                Path::new("/repo/.agents/wt/feature"),
                Path::new("/repo/.agents/wt/dirty"),
            ]
        );
        assert_eq!(linked(&parsed).len(), 3);
    }

    #[test]
    fn empty_output_yields_no_targets() {
        assert!(parse_worktrees("").is_empty());
        assert!(removable(&[]).is_empty());
    }

    #[test]
    fn lone_main_worktree_has_no_removable_targets() {
        let parsed = parse_worktrees("worktree /repo\nHEAD aaaa\nbranch refs/heads/main\n");
        assert_eq!(parsed.len(), 1);
        assert!(removable(&parsed).is_empty());
    }

    #[test]
    fn bare_main_repo_is_not_removable() {
        let parsed = parse_worktrees(
            "worktree /repo.git\nbare\n\nworktree /repo/wt\nHEAD b\nbranch refs/heads/x\n",
        );
        let targets = removable(&parsed);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].path, PathBuf::from("/repo/wt"));
    }

    #[test]
    fn worktree_basename_matcher_table() {
        let cases: &[(&str, bool)] = &[
            ("worktree", true),
            ("worktrees", true),
            (".worktree", true),
            (".worktrees", true),
            ("feature-worktree", true),
            ("my-feature-worktree", true),
            ("agent.worktree", true),
            ("src", false),
            ("node_modules", false),
            ("worktreex", false),
            ("myworktree", false),
            ("work-tree", false),
            (".git", false),
            ("wt", false),
        ];
        for &(name, want) in cases {
            assert_eq!(is_worktree_basename(name), want, "{name:?}");
        }
    }

    #[test]
    fn multi_parent_matcher_table() {
        assert!(is_multi_worktree_parent(
            "worktrees",
            Path::new("worktrees")
        ));
        assert!(is_multi_worktree_parent(
            "worktree",
            Path::new(".claude/worktree")
        ));
        assert!(is_multi_worktree_parent(
            "worktrees",
            Path::new(".claude/worktrees")
        ));
        assert!(is_multi_worktree_parent("wt", Path::new(".agents/wt")));
        assert!(!is_multi_worktree_parent("wt", Path::new("other/wt")));
        assert!(!is_multi_worktree_parent("src", Path::new("src")));
    }

    fn mkdir(root: &Path, rel: &str) {
        fs::create_dir_all(root.join(rel)).unwrap();
    }

    fn rels(root: &Path, paths: &[PathBuf]) -> Vec<String> {
        let mut v: Vec<String> = paths
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn discover_finds_convention_layouts() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        mkdir(root, "worktree");
        mkdir(root, "worktrees/feature-a");
        mkdir(root, "feature-worktree");
        mkdir(root, ".worktree");
        mkdir(root, ".claude/worktree/agent-x");
        mkdir(root, ".claude/worktrees/agent-1");
        mkdir(root, ".agents/wt/feature");
        mkdir(root, "src/lib");
        mkdir(root, "node_modules/pkg");

        let got = rels(root, &discover_worktree_folders(root));
        for expected in [
            "worktree",
            "worktrees",
            "worktrees/feature-a",
            "feature-worktree",
            ".worktree",
            ".claude/worktree",
            ".claude/worktree/agent-x",
            ".claude/worktrees",
            ".claude/worktrees/agent-1",
            ".agents/wt/feature",
        ] {
            assert!(
                got.iter().any(|g| g == expected),
                "missing {expected:?} in {got:?}"
            );
        }
        assert!(!got.iter().any(|g| g == "src" || g.starts_with("src/")));
        assert!(!got.iter().any(|g| g.starts_with("node_modules")));
    }

    #[test]
    fn discover_depth_bound_skips_deep_decoy() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        mkdir(root, "a/b/c/d/worktree");
        mkdir(root, "feature-worktree");
        let got = rels(root, &discover_worktree_folders(root));
        assert!(got.iter().any(|g| g == "feature-worktree"));
        assert!(!got.iter().any(|g| g.contains("a/b/c/d")));
    }

    #[cfg(unix)]
    #[test]
    fn discover_skips_symlink_escape() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let outside = TempDir::new().unwrap();
        mkdir(outside.path(), "secret-worktree");
        std::os::unix::fs::symlink(outside.path(), root.join("escape-link")).unwrap();
        mkdir(root, "feature-worktree");
        let found = discover_worktree_folders(root);
        let got = rels(root, &found);
        assert!(got.iter().any(|g| g == "feature-worktree"));
        assert!(!got.iter().any(|g| g.contains("secret")));
        for p in &found {
            assert!(p.starts_with(root));
        }
    }

    #[test]
    fn union_dedupes_fs_matching_porcelain() {
        let git = vec![
            PathBuf::from("/repo/.agents/wt/feature"),
            PathBuf::from("/repo/.agents/wt/dirty"),
        ];
        let fs = vec![
            PathBuf::from("/repo/.agents/wt/feature"),
            PathBuf::from("/repo/feature-worktree"),
            PathBuf::from("/repo/worktrees/orphan"),
        ];
        let (union, fs_only) = union_targets(&git, &fs);
        assert_eq!(union.len(), 4);
        assert_eq!(fs_only.len(), 2);
        assert!(!fs_only.contains(&PathBuf::from("/repo/.agents/wt/feature")));
    }

    fn write_git_file(dir: &Path, gitdir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    }

    #[test]
    fn tmp_zombies_only_those_with_missing_gitdir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let live_gitdir = root.join("repo/.git/worktrees/live");
        fs::create_dir_all(&live_gitdir).unwrap();
        write_git_file(&root.join("live"), &live_gitdir);
        write_git_file(
            &root.join("zombie"),
            &root.join("gone/.git/worktrees/zombie"),
        );
        write_git_file(
            &root.join("nested/deep-zombie"),
            &root.join("gone/.git/worktrees/deep"),
        );
        mkdir(root, "plain/sub");
        mkdir(root, "regular/.git");
        let got = rels(root, &discover_tmp_zombies(&[root.to_path_buf()]));
        assert_eq!(got, vec!["nested/deep-zombie", "zombie"]);
    }

    #[test]
    fn tmp_zombies_resolve_relative_gitdir_and_skip_symlinks() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let outside = TempDir::new().unwrap();
        write_git_file(&outside.path().join("z"), Path::new("/nonexistent/gitdir"));
        mkdir(root, "rel");
        fs::write(root.join("rel/.git"), "gitdir: ../missing\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("z"), root.join("link")).unwrap();
        let got = rels(root, &discover_tmp_zombies(&[root.to_path_buf()]));
        assert_eq!(got, vec!["rel"]);
    }

    #[test]
    fn prune_covered_descendants_keeps_shallowest() {
        let paths = vec![
            PathBuf::from("/repo/.agents/worktree/child-a"),
            PathBuf::from("/repo/.agents/worktree"),
            PathBuf::from("/repo/.claude/worktrees/agent-1"),
            PathBuf::from("/repo/.claude/worktrees"),
            PathBuf::from("/repo/worktrees/feature"),
            PathBuf::from("/repo/orphan-worktree"),
        ];
        assert_eq!(
            prune_covered_descendants(&paths),
            vec![
                PathBuf::from("/repo/.agents/worktree"),
                PathBuf::from("/repo/.claude/worktrees"),
                PathBuf::from("/repo/orphan-worktree"),
                PathBuf::from("/repo/worktrees/feature"),
            ]
        );
    }

    #[test]
    fn prune_does_not_collapse_sibling_prefixes() {
        let paths = vec![
            PathBuf::from("/repo/worktree"),
            PathBuf::from("/repo/worktrees"),
            PathBuf::from("/repo/worktrees/feature"),
        ];
        assert_eq!(
            prune_covered_descendants(&paths),
            vec![
                PathBuf::from("/repo/worktree"),
                PathBuf::from("/repo/worktrees")
            ]
        );
    }

    #[test]
    fn scanner_marks_dirty_and_locked_worktrees_global_and_lists_fs_only() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        mkdir(&root, "wt/clean");
        mkdir(&root, "wt/dirty");
        mkdir(&root, "wt/locked");
        mkdir(&root, "feature-worktree");
        let porcelain = format!(
            "worktree {r}\nHEAD a\n\nworktree {r}/wt/clean\nHEAD b\n\nworktree {r}/wt/dirty\nHEAD c\n\nworktree {r}/wt/locked\nHEAD d\nlocked\n",
            r = root.display()
        );
        // `git status` answers: first call (clean) empty, then dirty, then locked clean.
        let runner = FakeRunner::new(&["git"]).respond("git", RunResult::success(porcelain));
        let roots = vec![root.clone()];
        let c = ctx(&roots, &[], tmp.path(), vec![], &runner);

        let found = WorktreesScanner.scan(&c);

        let by_label = |needle: &str| {
            found
                .iter()
                .find(|c| c.label.contains(needle))
                .unwrap_or_else(|| panic!("missing {needle}: {found:?}"))
        };
        assert!(matches!(by_label("wt/clean").target, Target::Action { .. }));
        assert_eq!(by_label("wt/locked").tier, Tier::Global);
        assert!(by_label("wt/locked").label.contains("locked"));
        if let Target::Action { args, .. } = &by_label("wt/locked").target {
            assert_eq!(args.iter().filter(|a| *a == "--force").count(), 2);
        }
        assert!(matches!(
            by_label("feature-worktree").target,
            Target::Path { .. }
        ));
        assert_eq!(by_label("feature-worktree").tier, Tier::Local);
        assert!(root.join("wt/clean").exists());
        assert!(found.iter().all(|c| c.size == Size::Skipped), "{found:?}");
    }

    #[test]
    fn scanner_without_git_still_lists_fs_folders() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        mkdir(&root, ".worktree/x");
        let runner = FakeRunner::new(&[]);
        let roots = vec![root.clone()];
        let c = ctx(&roots, &[], tmp.path(), vec![], &runner);
        let found = WorktreesScanner.scan(&c);
        assert_eq!(found.len(), 1);
        assert!(found[0].label.ends_with(".worktree"));
        assert!(runner.calls().is_empty());
    }
}
