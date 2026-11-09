//! Per-project build artifacts and dependency trees, grouped by project root.

use std::path::{Path, PathBuf};

use crate::models::ProjectType;

use super::super::candidate::{display_path, Candidate, Category, Size, Target, Tier};
use super::{is_heavy_or_owned_dir, ScanContext, Scanner};

pub struct ProjectsScanner;

impl Scanner for ProjectsScanner {
    fn category(&self) -> Category {
        Category::Projects
    }

    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
        let mut out = Vec::new();
        for (info, root) in ctx.projects {
            let group = display_path(root, &ctx.home);
            for (rel, tier) in project_targets(&info.project_type, root) {
                let path = root.join(&rel);
                if std::fs::symlink_metadata(&path).is_err() {
                    continue;
                }
                let target = if path.is_dir() {
                    Target::dir(path)
                } else {
                    Target::file(path)
                };
                out.push(Candidate::new(
                    Category::Projects,
                    group.clone(),
                    rel.display().to_string(),
                    target,
                    Size::Unknown,
                    tier,
                ));
            }
        }
        out
    }
}

const FLUTTER: &[&str] = &[
    "build",
    ".dart_tool",
    ".flutter-plugins",
    ".flutter-plugins-dependencies",
    "ios/Flutter/ephemeral",
    "macos/Flutter/ephemeral",
    "android/build",
    "android/app/build",
    "android/.gradle",
    "ios/Pods",
    "ios/.symlinks",
    "ios/build",
];
const ANDROID: &[&str] = &["build", "app/build", ".gradle", ".dart_tool"];
const IOS: &[&str] = &["Pods", ".symlinks", "build"];
const NODE: &[&str] = &[
    "node_modules",
    ".next",
    ".nuxt",
    ".turbo",
    ".vite",
    ".parcel-cache",
    "dist",
    "build",
    ".svelte-kit",
    ".astro",
    "coverage",
];
const RUST: &[&str] = &["target"];
const GO: &[&str] = &["bin", "pkg"];
const RUBY: &[&str] = &["vendor/bundle", ".bundle"];
const RAILS: &[&str] = &["tmp/cache"];
const PYTHON_ROOT: &[&str] = &[".coverage", "htmlcov"];
const DJANGO: &[&str] = &["staticfiles"];
const PY_VENVS: &[&str] = &[".venv", "venv", "env"];
const PY_CACHE_DIRS: &[&str] = &[
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
];
const PY_MAX_DEPTH: usize = 10;

/// Relative purge targets for a project of `kind` at `root`, with their tier.
/// Existence is not checked here.
pub fn project_targets(kind: &ProjectType, root: &Path) -> Vec<(PathBuf, Tier)> {
    let local = |list: &[&str]| -> Vec<(PathBuf, Tier)> {
        list.iter()
            .map(|r| (PathBuf::from(r), Tier::Local))
            .collect()
    };
    match kind {
        ProjectType::Flutter => local(FLUTTER),
        ProjectType::Android => local(ANDROID),
        ProjectType::Ios => local(IOS),
        ProjectType::Node { .. } => local(NODE),
        ProjectType::Rust => local(RUST),
        ProjectType::Go => {
            if root.join("go.mod").exists() {
                local(GO)
            } else {
                Vec::new()
            }
        }
        ProjectType::Ruby { .. } => {
            let mut v = local(RUBY);
            if root.join("config").join("application.rb").exists() {
                v.extend(local(RAILS));
                v.extend(rails_logs(root));
            }
            v
        }
        ProjectType::Python { .. } => {
            let mut v = local(PYTHON_ROOT);
            if root.join("manage.py").exists() {
                v.extend(local(DJANGO));
            }
            v.extend(PY_VENVS.iter().map(|n| (PathBuf::from(n), Tier::Global)));
            v.extend(
                python_cache_dirs(root)
                    .into_iter()
                    .map(|p| (p, Tier::Local)),
            );
            v
        }
        ProjectType::Unknown => Vec::new(),
    }
}

fn rails_logs(root: &Path) -> Vec<(PathBuf, Tier)> {
    let log_dir = root.join("log");
    let entries = match std::fs::read_dir(&log_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("log") {
            out.push((PathBuf::from("log").join(entry.file_name()), Tier::Local));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Recursive walk for Python cache directories, relative to `root`. Never
/// descends into a matched dir, heavy dirs, or virtualenvs.
pub fn python_cache_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_py(root, root, 0, &mut out);
    out.sort();
    out
}

fn walk_py(root: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > PY_MAX_DEPTH {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if PY_CACHE_DIRS.contains(&name) {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
            continue;
        }
        if is_heavy_or_owned_dir(name) || PY_VENVS.contains(&name) {
            continue;
        }
        walk_py(root, &path, depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, FakeRunner};
    use super::*;
    use crate::models::{AppInfo, NodePm};
    use std::fs;
    use tempfile::TempDir;

    fn app(kind: ProjectType) -> AppInfo {
        AppInfo::new(String::new(), kind, None, None)
    }

    fn labels(cands: &[Candidate]) -> Vec<String> {
        let mut v: Vec<String> = cands.iter().map(|c| c.label.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn node_project_lists_only_existing_targets_and_deletes_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::write(root.join("node_modules/pkg/i.js"), b"xx").unwrap();
        let projects = vec![(
            app(ProjectType::Node {
                manager: NodePm::Npm,
            }),
            root.clone(),
        )];
        let roots = vec![root.clone()];
        let runner = FakeRunner::new(&[]);
        let c = ctx(&roots, &projects, tmp.path(), vec![], &runner);

        let found = ProjectsScanner.scan(&c);

        assert_eq!(labels(&found), vec!["dist", "node_modules"]);
        assert!(found.iter().all(|c| c.tier == Tier::Local));
        assert!(found.iter().all(|c| c.size == Size::Unknown));
        assert!(root.join("node_modules/pkg/i.js").exists());
    }

    #[test]
    fn python_project_walks_caches_and_marks_venv_global() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("src/pkg/__pycache__")).unwrap();
        fs::create_dir_all(root.join(".pytest_cache")).unwrap();
        fs::create_dir_all(root.join(".venv/lib/__pycache__")).unwrap();
        fs::create_dir_all(root.join("node_modules/x/__pycache__")).unwrap();
        fs::write(root.join(".coverage"), b"1").unwrap();
        fs::write(root.join("manage.py"), b"").unwrap();
        fs::create_dir_all(root.join("staticfiles")).unwrap();
        let projects = vec![(app(ProjectType::Python { framework: None }), root.clone())];
        let roots = vec![root.clone()];
        let runner = FakeRunner::new(&[]);
        let c = ctx(&roots, &projects, tmp.path(), vec![], &runner);

        let found = ProjectsScanner.scan(&c);

        assert_eq!(
            labels(&found),
            vec![
                ".coverage",
                ".pytest_cache",
                ".venv",
                "src/pkg/__pycache__",
                "staticfiles"
            ]
        );
        let venv = found.iter().find(|c| c.label == ".venv").unwrap();
        assert_eq!(venv.tier, Tier::Global);
        let cov = found.iter().find(|c| c.label == ".coverage").unwrap();
        assert!(matches!(
            cov.target,
            Target::Path {
                kind: crate::commands::purge::candidate::EntryKind::File,
                ..
            }
        ));
    }

    #[test]
    fn go_without_go_mod_has_no_targets_and_rails_lists_logs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("bin")).unwrap();
        assert!(project_targets(&ProjectType::Go, root).is_empty());
        fs::write(root.join("go.mod"), b"module x").unwrap();
        assert_eq!(project_targets(&ProjectType::Go, root).len(), 2);

        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config/application.rb"), b"").unwrap();
        fs::create_dir_all(root.join("log")).unwrap();
        fs::write(root.join("log/development.log"), b"").unwrap();
        fs::write(root.join("log/.keep"), b"").unwrap();
        let rails: Vec<PathBuf> = project_targets(&ProjectType::Ruby { rails: true }, root)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert!(rails.contains(&PathBuf::from("tmp/cache")));
        assert!(rails.contains(&PathBuf::from("log/development.log")));
        assert!(!rails.contains(&PathBuf::from("log/.keep")));
    }

    #[test]
    fn group_is_home_abbreviated_root() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let root = home.join("projects/app");
        fs::create_dir_all(root.join("target")).unwrap();
        let projects = vec![(app(ProjectType::Rust), root.clone())];
        let roots = vec![root.clone()];
        let runner = FakeRunner::new(&[]);
        let c = ctx(&roots, &projects, home, vec![], &runner);
        let found = ProjectsScanner.scan(&c);
        assert_eq!(found[0].group, "~/projects/app");
    }
}
