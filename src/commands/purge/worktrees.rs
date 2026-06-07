use colored::Colorize;
use std::path::{Path, PathBuf};

use super::PurgeArgs;
use crate::logger::Logger;
use crate::runner::Runner;

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
/// Lists every **linked** worktree of the repo at `root` (skipping the main
/// working tree and any locked worktree) and, after a default-No confirmation,
/// removes each via `git worktree remove --force` — which deletes both the
/// admin files and the working directory — then prunes stale metadata.
///
/// Git-gated: a no-op when `git` is not on PATH, honoring "git commands only
/// if available".
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
    let targets = removable(&worktrees);
    if targets.is_empty() {
        return;
    }

    logger.info(&format!("  {} linked worktree(s) to remove:", "git".cyan()));
    for w in &targets {
        logger.info(&format!("    {}", w.path.display()));
    }

    if dry_run {
        logger.detail("  (dry run — skipped)");
        return;
    }

    if !logger.confirm(
        &format!("  Delete {} linked worktree(s)?", targets.len()),
        false,
    ) {
        return;
    }

    for w in &targets {
        let path_str = w.path.to_string_lossy();
        let removed = runner.run(
            "git",
            &["-C", &root_str, "worktree", "remove", "--force", &path_str],
            None,
        );
        if removed.is_success() {
            logger.success(&format!("  {} Removed {}", "✓".green(), w.path.display()));
        } else {
            logger.err(&format!(
                "  {} Failed to remove {}: {}",
                "✗".red(),
                w.path.display(),
                removed.stderr
            ));
            if verbose && !removed.stderr.is_empty() {
                logger.err(&removed.stderr);
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

#[cfg(test)]
mod tests {
    use super::*;

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

        let targets: Vec<&Path> = removable(&parsed).iter().map(|w| w.path.as_path()).collect();
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
        let parsed = parse_worktrees("worktree /repo.git\nbare\n\nworktree /repo/wt\nHEAD b\nbranch refs/heads/x\n");
        let targets = removable(&parsed);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].path, PathBuf::from("/repo/wt"));
    }
}
