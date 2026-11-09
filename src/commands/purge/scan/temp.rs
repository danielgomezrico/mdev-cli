//! Stale entries directly under the system temp roots, plus zombie worktree
//! checkouts anywhere below them.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::super::candidate::{display_path, Candidate, Category, Size, Target, Tier};
use super::worktrees::{discover_tmp_zombies, prune_covered_descendants, worktree_gitdir};
use super::{ScanContext, Scanner};

pub struct TempScanner;

impl Scanner for TempScanner {
    fn category(&self) -> Category {
        Category::Temp
    }

    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
        let mut out = Vec::new();

        for z in prune_covered_descendants(&discover_tmp_zombies(&ctx.tmp_roots)) {
            out.push(Candidate::new(
                Category::Temp,
                "zombie worktrees",
                display_path(&z, &ctx.home),
                Target::dir(z),
                Size::Unknown,
                Tier::Local,
            ));
        }

        let owner = owner_uid(&ctx.home);
        for root in &ctx.tmp_roots {
            let group = display_path(root, &ctx.home);
            for (path, is_dir) in stale_entries(root, ctx.now, ctx.tmp_age, owner) {
                let label = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let target = if is_dir {
                    Target::dir(path)
                } else {
                    Target::file(path)
                };
                out.push(Candidate::new(
                    Category::Temp,
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

/// Names that belong to the OS, to live sessions, or to running agents
/// (lock files) — never listed.
pub fn is_protected_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.starts_with("com.apple.")
        || lower.contains("lock")
        || name.ends_with(".sock")
        || name.starts_with(".X11-unix")
        || name.starts_with(".ICE-unix")
        || name.starts_with(".font-unix")
        || name.starts_with(".XIM-unix")
        || name.starts_with("tmux-")
        || name.starts_with("ssh-")
        || matches!(name, "powerlog" | "sessions" | "KSOutOfProcessFetcher")
}

/// Direct children of `root` owned by `owner` (when known), not sockets /
/// FIFOs / symlinks, not protected names, not live git worktrees, with a
/// modification time older than `age`. `(path, is_dir)` sorted by path.
pub fn stale_entries(
    root: &Path,
    now: SystemTime,
    age: std::time::Duration,
    owner: Option<u32>,
) -> Vec<(PathBuf, bool)> {
    let cutoff = now.checked_sub(age).unwrap_or(SystemTime::UNIX_EPOCH);
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if is_protected_name(name) {
            continue;
        }
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let ft = meta.file_type();
        if ft.is_symlink() || is_special(&ft) {
            continue;
        }
        if let (Some(owner), Some(uid)) = (owner, uid_of(&meta)) {
            if uid != owner {
                continue;
            }
        }
        if ft.is_dir() {
            if let Some(gitdir) = worktree_gitdir(&path) {
                if gitdir.exists() {
                    continue;
                }
            }
        }
        match meta.modified() {
            Ok(m) if m < cutoff => out.push((path, ft.is_dir())),
            _ => {}
        }
    }
    out.sort();
    out
}

#[cfg(unix)]
fn is_special(ft: &std::fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    ft.is_socket() || ft.is_fifo() || ft.is_block_device() || ft.is_char_device()
}

#[cfg(not(unix))]
fn is_special(_ft: &std::fs::FileType) -> bool {
    false
}

#[cfg(unix)]
fn uid_of(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.uid())
}

#[cfg(not(unix))]
fn uid_of(_meta: &std::fs::Metadata) -> Option<u32> {
    None
}

fn owner_uid(home: &Path) -> Option<u32> {
    std::fs::metadata(home).ok().and_then(|m| uid_of(&m))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, FakeRunner};
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tempfile::TempDir;

    fn age(path: &Path, days: u64) {
        let when = SystemTime::now() - Duration::from_secs(days * 24 * 3600);
        let f = fs::File::open(path).unwrap();
        f.set_modified(when).unwrap();
    }

    #[test]
    fn lists_only_old_owned_plain_entries() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("old-dir/inner")).unwrap();
        fs::write(root.join("old-dir/inner/f"), b"x").unwrap();
        age(&root.join("old-dir"), 10);
        fs::write(root.join("old.txt"), b"x").unwrap();
        age(&root.join("old.txt"), 10);
        fs::write(root.join("fresh.txt"), b"x").unwrap();
        fs::create_dir_all(root.join("com.apple.launchd.abc")).unwrap();
        age(&root.join("com.apple.launchd.abc"), 10);
        fs::create_dir_all(root.join("powerlog")).unwrap();
        age(&root.join("powerlog"), 10);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("old.txt"), root.join("link")).unwrap();
        }

        let got = stale_entries(
            root,
            SystemTime::now(),
            Duration::from_secs(3 * 86_400),
            None,
        );
        let names: Vec<String> = got
            .iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["old-dir", "old.txt"]);
        assert!(got[0].1 && !got[1].1);
    }

    #[test]
    fn live_worktrees_are_skipped_but_zombies_listed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let live_gitdir = root.join("repo/.git/worktrees/live");
        fs::create_dir_all(&live_gitdir).unwrap();
        fs::create_dir_all(root.join("live")).unwrap();
        fs::write(
            root.join("live/.git"),
            format!("gitdir: {}\n", live_gitdir.display()),
        )
        .unwrap();
        age(&root.join("live"), 30);
        fs::create_dir_all(root.join("zombie")).unwrap();
        fs::write(root.join("zombie/.git"), "gitdir: /nonexistent/x\n").unwrap();
        let runner = FakeRunner::new(&[]);
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], tmp.path(), vec![root.clone()], &runner);

        let found = TempScanner.scan(&c);

        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].group, "zombie worktrees");
        assert!(found[0].label.ends_with("zombie"));
    }

    #[test]
    fn protected_names_table() {
        for n in [
            "com.apple.launchd.9jkZ",
            "com.apple.CoreSimulator.SimDevice.ABC",
            "agent.sock",
            "powerlog",
            "sessions",
            "tmux-501",
            "logioptionsplus_agent_instance_lock",
            "foo.LOCK",
        ] {
            assert!(is_protected_name(n), "{n}");
        }
        for n in ["claude-501", "pip-build-abc", "foo.txt"] {
            assert!(!is_protected_name(n), "{n}");
        }
    }
}
