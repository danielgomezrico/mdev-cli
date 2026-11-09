//! Purgeable things discovered by scanners, before any selection or deletion.

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
    Unknown,
}

impl Size {
    pub fn bytes(self) -> u64 {
        match self {
            Size::Bytes(b) => b,
            Size::Unknown => 0,
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

    /// Shell-like rendering of an action, e.g. `$ docker system prune -f`.
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
            size,
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
    let mut roots: Vec<PathBuf> = cands
        .iter()
        .filter_map(|c| c.target.path().map(Path::to_path_buf))
        .collect();
    roots.sort();
    roots.dedup();
    let mut keep_roots: Vec<PathBuf> = Vec::new();
    for p in roots {
        if !keep_roots.iter().any(|parent| p.starts_with(parent)) {
            keep_roots.push(p);
        }
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    cands
        .into_iter()
        .filter(|c| match c.target.path() {
            None => true,
            Some(p) => {
                let kept = keep_roots.iter().any(|k| k == p);
                if kept && !seen.iter().any(|s| s == p) {
                    seen.push(p.to_path_buf());
                    true
                } else {
                    false
                }
            }
        })
        .collect()
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
