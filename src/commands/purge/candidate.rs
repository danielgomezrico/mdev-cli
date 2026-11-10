//! Purgeable things discovered by scanners, before any selection or deletion.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use clap::ValueEnum;

/// Top level of the selection tree. Declaration order is display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, ValueEnum)]
pub enum Category {
    Projects,
    Extras,
    Worktrees,
    Temp,
    Logs,
    Xcode,
    Android,
    PackageCaches,
    Docker,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::Projects,
        Category::Extras,
        Category::Worktrees,
        Category::Temp,
        Category::Logs,
        Category::Xcode,
        Category::Android,
        Category::PackageCaches,
        Category::Docker,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Projects => "Projects",
            Category::Extras => "Extras",
            Category::Worktrees => "Git worktrees",
            Category::Temp => "Temp dirs",
            Category::Logs => "Logs",
            Category::Xcode => "Xcode",
            Category::Android => "Android SDK",
            Category::PackageCaches => "Package caches",
            Category::Docker => "Docker",
        }
    }
}

/// `Global` = expensive to recreate (package caches, simulator data, dirty
/// worktrees…) — listed but unchecked by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Local,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Bytes(u64),
    /// Not measured yet; the scan post-pass sizes it.
    Unknown,
    /// Never measured on purpose (too slow to walk, e.g. worktrees).
    Skipped,
}

impl Size {
    pub fn bytes(self) -> u64 {
        match self {
            Size::Bytes(b) => b,
            Size::Unknown | Size::Skipped => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A file or directory removed with `std::fs`.
    Path { path: PathBuf, kind: EntryKind },
    /// A command that frees space. `affects` are paths sized before and
    /// after the command to estimate bytes freed.
    Action {
        exe: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        affects: Vec<PathBuf>,
    },
}

impl Target {
    pub fn dir(path: PathBuf) -> Self {
        Target::Path {
            path,
            kind: EntryKind::Dir,
        }
    }

    pub fn file(path: PathBuf) -> Self {
        Target::Path {
            path,
            kind: EntryKind::File,
        }
    }

    pub fn action(exe: &str, args: &[&str], cwd: Option<PathBuf>, affects: Vec<PathBuf>) -> Self {
        Target::Action {
            exe: exe.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd,
            affects,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Target::Path { path, .. } => Some(path),
            Target::Action { .. } => None,
        }
    }

    /// Shell-like rendering of an action, e.g. `$ docker builder prune -f`.
    pub fn command_line(&self) -> Option<String> {
        match self {
            Target::Action { exe, args, .. } => Some(format!("$ {} {}", exe, args.join(" "))),
            Target::Path { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub category: Category,
    /// Second level of the tree (project root, "DerivedData", temp root…).
    pub group: String,
    pub label: String,
    pub target: Target,
    pub size: Size,
    pub tier: Tier,
}

impl Candidate {
    /// `Global` candidates are never measured (`Size::Skipped`): they are
    /// unchecked by default, so walking them only slows the scan.
    pub fn new(
        category: Category,
        group: impl Into<String>,
        label: impl Into<String>,
        target: Target,
        size: Size,
        tier: Tier,
    ) -> Self {
        Self {
            category,
            group: group.into(),
            label: label.into(),
            target,
            size: if tier == Tier::Global {
                Size::Skipped
            } else {
                size
            },
            tier,
        }
    }

    pub fn default_selected(&self) -> bool {
        self.tier == Tier::Local
    }
}

/// Stable index of a candidate inside the scan result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub usize);

/// Drop `Path` candidates nested under another `Path` candidate — the parent
/// wipe covers them. Component-wise (`/a/b` does not cover `/a/bc`). Actions
/// are always kept. Output keeps input order.
pub fn dedupe_covered(cands: Vec<Candidate>) -> Vec<Candidate> {
    let paths: Vec<&Path> = cands.iter().filter_map(|c| c.target.path()).collect();
    let mut keep: HashSet<PathBuf> = outermost(paths)
        .into_iter()
        .map(Path::to_path_buf)
        .collect();
    cands
        .into_iter()
        .filter(|c| c.target.path().is_none_or(|p| keep.remove(p)))
        .collect()
}

/// Sorted, deduplicated paths not nested under another one. Component-wise
/// ordering puts every descendant right after its ancestor, so one sweep
/// against the last kept path suffices.
pub fn outermost<P: AsRef<Path>>(mut paths: Vec<P>) -> Vec<P> {
    paths.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
    let mut out: Vec<P> = Vec::with_capacity(paths.len());
    for p in paths {
        if !out
            .last()
            .is_some_and(|l| p.as_ref().starts_with(l.as_ref()))
        {
            out.push(p);
        }
    }
    out
}

/// `~`-abbreviated path for display.
pub fn display_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) if !home.as_os_str().is_empty() => {
            if rel.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rel.display())
            }
        }
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_cand(p: &str) -> Candidate {
        Candidate::new(
            Category::Projects,
            "g",
            p,
            Target::dir(PathBuf::from(p)),
            Size::Bytes(1),
            Tier::Local,
        )
    }

    #[test]
    fn dedupe_drops_nested_but_not_sibling_prefixes() {
        let cands = vec![
            path_cand("/repo/worktrees/feature"),
            path_cand("/repo/worktree"),
            path_cand("/repo/worktrees"),
            path_cand("/repo/worktrees/feature"),
        ];
        let got: Vec<String> = dedupe_covered(cands).into_iter().map(|c| c.label).collect();
        assert_eq!(got, vec!["/repo/worktree", "/repo/worktrees"]);
    }

    #[test]
    fn global_candidates_are_never_sized() {
        let c = Candidate::new(
            Category::PackageCaches,
            "g",
            "x",
            Target::dir(PathBuf::from("/a")),
            Size::Bytes(9),
            Tier::Global,
        );
        assert_eq!(c.size, Size::Skipped);
        assert_eq!(path_cand("/b").size, Size::Bytes(1));
    }

    #[test]
    fn dedupe_is_not_quadratic() {
        let cands: Vec<Candidate> = (0..40_000)
            .map(|i| path_cand(&format!("/r/p{i}")))
            .collect();
        let started = std::time::Instant::now();
        assert_eq!(dedupe_covered(cands).len(), 40_000);
        assert!(started.elapsed().as_secs() < 3, "{:?}", started.elapsed());
    }

    #[test]
    fn dedupe_keeps_actions() {
        let action = Candidate::new(
            Category::Docker,
            "g",
            "prune",
            Target::action("docker", &["system", "prune"], None, vec![]),
            Size::Unknown,
            Tier::Local,
        );
        let got = dedupe_covered(vec![action.clone(), path_cand("/a")]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], action);
    }

    #[test]
    fn display_path_abbreviates_home() {
        let home = Path::new("/Users/me");
        assert_eq!(display_path(Path::new("/Users/me/x/y"), home), "~/x/y");
        assert_eq!(display_path(Path::new("/Users/me"), home), "~");
        assert_eq!(display_path(Path::new("/tmp/z"), home), "/tmp/z");
    }

    #[test]
    fn command_line_renders_action() {
        let t = Target::action("go", &["clean", "-modcache"], None, vec![]);
        assert_eq!(t.command_line().unwrap(), "$ go clean -modcache");
        assert!(Target::dir(PathBuf::from("/a")).command_line().is_none());
    }

    #[test]
    fn category_all_is_in_declaration_order() {
        assert_eq!(Category::ALL[0], Category::Projects);
        assert_eq!(Category::ALL[8], Category::Docker);
        assert_eq!(Category::ALL.len(), 9);
    }
}
