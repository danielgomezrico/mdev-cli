use colored::Colorize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::PurgeArgs;
use crate::logger::Logger;
use crate::runner::Runner;

/// Max walk depth from project root (root = 0). Bounds monorepo scans.
const MAX_DEPTH: usize = 4;

/// A worktree entry parsed from `git worktree list --porcelain`.
///
/// The first entry git prints is always the repository's **main** working
/// tree; every later entry is a linked worktree.
#[derive(Debug, PartialEq)]
struct Worktree {
    path: PathBuf,
    is_main: bool,
    locked: bool,
    bare: bool,
}

/// Per-project git worktree cleanup.
///
/// Discovers targets via the union of:
/// 1. Linked worktrees from `git worktree list --porcelain` (main/locked/bare excluded)
/// 2. Convention-named folders under `root` (iterative FS walk)
///
/// Removal is git-gated only: `git worktree remove --force` for porcelain-removable
/// paths. FS-only matches are listed but not deleted (no free-form `rm -rf`).
///
/// Git-gated: a no-op when `git` is not on PATH.
pub fn run(_args: &PurgeArgs, root: &Path, runner: &dyn Runner, dry_run: bool, verbose: bool) {
    let logger = Logger::new();

    if runner.which("git").is_none() {
        return;
    }

    let root_str = root.to_string_lossy();
    let listed = runner.run(
        "git",
        &["-C", &root_str, "worktree", "list", "--porcelain"],
        None,
    );
    if !listed.is_success() {
        return;
    }

    let worktrees = parse_worktrees(&listed.stdout);
    let git_targets: Vec<PathBuf> = removable(&worktrees)
        .into_iter()
        .map(|w| w.path.clone())
        .collect();

    let fs_candidates = discover_worktree_folders(root);

    let (union, fs_only) = union_targets(&git_targets, &fs_candidates);
    if union.is_empty() {
        return;
    }

    logger.info(&format!("  {} worktree target(s) found:", "git".cyan()));
    for p in &union {
        let tag = if fs_only.iter().any(|f| paths_equal(f, p)) {
            " (fs-only, not git-registered)"
        } else {
            ""
        };
        logger.info(&format!("    {}{}", p.display(), tag));
    }

    if dry_run {
        if !fs_only.is_empty() {
            logger.detail(&format!(
                "  ({} fs-only candidate(s) listed; delete only via git when registered)",
                fs_only.len()
            ));
        }
        logger.detail("  (dry run — skipped)");
        return;
    }

    if git_targets.is_empty() {
        if !fs_only.is_empty() {
            logger.detail(&format!(
                "  {} fs-only worktree-like folder(s) — skipped (not git-registered; no free rm)",
                fs_only.len()
            ));
        }
        return;
    }

    if !logger.confirm(
        &format!("  Delete {} linked worktree(s)?", git_targets.len()),
        false,
    ) {
        return;
    }

    for path in &git_targets {
        let path_str = path.to_string_lossy();
        let removed = runner.run(
            "git",
            &["-C", &root_str, "worktree", "remove", "--force", &path_str],
            None,
        );
        if removed.is_success() {
            logger.success(&format!("  {} Removed {}", "✓".green(), path.display()));
        } else {
            logger.err(&format!(
                "  {} Failed to remove {}: {}",
                "✗".red(),
                path.display(),
                removed.stderr
            ));
            if verbose && !removed.stderr.is_empty() {
                logger.err(&removed.stderr);
            }
        }
    }

    if !fs_only.is_empty() {
        logger.detail(&format!(
            "  {} fs-only worktree-like folder(s) left listed but not deleted",
            fs_only.len()
        ));
        if verbose {
            for p in &fs_only {
                logger.detail(&format!("    skip: {}", p.display()));
            }
        }
    }

    runner.run("git", &["-C", &root_str, "worktree", "prune"], None);
}

/// Parse `git worktree list --porcelain`. Blocks are separated by blank lines;
/// the first block is the main working tree. Recognized attribute lines:
/// `locked` and `bare`.
fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
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

/// Linked worktrees eligible for removal: not the main tree, not locked, not bare.
fn removable(worktrees: &[Worktree]) -> Vec<&Worktree> {
    worktrees
        .iter()
        .filter(|w| !w.is_main && !w.locked && !w.bare)
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

/// Heavy / owned dirs — never descend.
fn is_skip_dir(name: &str) -> bool {
    super::common::is_heavy_or_owned_dir(name)
}

/// Iterative BFS walk under `root` for convention worktree folders.
/// Depth-bounded; skips heavy dirs and symlinks (no out-of-root escape).
fn discover_worktree_folders(root: &Path) -> Vec<PathBuf> {
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
            // Never follow symlinks — prevents escape outside root.
            if ft.is_symlink() {
                continue;
            }
            if !ft.is_dir() {
                continue;
            }
            if is_skip_dir(name) {
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
        if ft.is_symlink() || !ft.is_dir() || is_skip_dir(name) {
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

/// Union git removable paths with FS candidates; return (sorted union, fs-only).
fn union_targets(
    git_targets: &[PathBuf],
    fs_candidates: &[PathBuf],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut union: Vec<PathBuf> = Vec::new();
    let mut fs_only: Vec<PathBuf> = Vec::new();

    for p in git_targets {
        let key = path_key(p);
        if seen.insert(key) {
            union.push(p.clone());
        }
    }
    for p in fs_candidates {
        let key = path_key(p);
        if seen.insert(key) {
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

fn paths_equal(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

#[cfg(test)]
mod tests {
    use super::*;
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
            assert_eq!(
                is_worktree_basename(name),
                want,
                "is_worktree_basename({name:?}) expected {want}"
            );
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
        // non-matches
        mkdir(root, "src/lib");
        mkdir(root, "node_modules/pkg");

        let found = discover_worktree_folders(root);
        let got = rels(root, &found);

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

        assert!(
            !got.iter().any(|g| g == "src" || g.starts_with("src/")),
            "must not match src: {got:?}"
        );
        assert!(
            !got.iter()
                .any(|g| g == "node_modules" || g.starts_with("node_modules/")),
            "must not match node_modules: {got:?}"
        );
    }

    #[test]
    fn discover_depth_bound_skips_deep_decoy() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // depth chain: a/b/c/d/worktree — worktree at depth 5 > MAX_DEPTH(4)
        // walk visits dirs at depth < MAX_DEPTH; entry at depth MAX_DEPTH is not entered
        mkdir(root, "a/b/c/d/worktree");
        // shallow hit still found
        mkdir(root, "feature-worktree");

        let found = discover_worktree_folders(root);
        let got = rels(root, &found);

        assert!(
            got.iter().any(|g| g == "feature-worktree"),
            "shallow candidate missing: {got:?}"
        );
        assert!(
            !got.iter().any(|g| g.contains("a/b/c/d")),
            "deep decoy must not be discovered: {got:?}"
        );
    }

    #[test]
    fn discover_skips_symlink_escape() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let outside = TempDir::new().unwrap();
        mkdir(outside.path(), "secret-worktree");

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.join("escape-link");
            symlink(outside.path(), &link).unwrap();
        }
        #[cfg(not(unix))]
        {
            // No symlink API guarantee — still assert in-root discovery works.
            mkdir(root, "feature-worktree");
            let found = discover_worktree_folders(root);
            assert!(!found.is_empty());
            return;
        }

        mkdir(root, "feature-worktree");
        let found = discover_worktree_folders(root);
        let got = rels(root, &found);

        assert!(got.iter().any(|g| g == "feature-worktree"));
        assert!(
            !got.iter().any(|g| g.contains("secret")),
            "must not follow symlink outside root: {got:?}"
        );
        for p in &found {
            assert!(
                p.starts_with(root),
                "candidate escaped root: {}",
                p.display()
            );
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
        assert!(union.contains(&PathBuf::from("/repo/.agents/wt/feature")));
        assert!(union.contains(&PathBuf::from("/repo/feature-worktree")));
        assert_eq!(fs_only.len(), 2);
        assert!(fs_only.contains(&PathBuf::from("/repo/feature-worktree")));
        assert!(fs_only.contains(&PathBuf::from("/repo/worktrees/orphan")));
        assert!(!fs_only.contains(&PathBuf::from("/repo/.agents/wt/feature")));
    }

    #[test]
    fn max_depth_constant_is_bounded() {
        assert!(MAX_DEPTH <= 4);
        assert!(MAX_DEPTH >= 1);
    }
}
