//! Global package-manager and SDK caches. All `Tier::Global`: safe to delete,
//! expensive to re-download.

use std::path::PathBuf;

use super::super::candidate::{display_path, Candidate, Category, Size, Target, Tier};
use super::{existing, ScanContext, Scanner};

pub struct PackageCachesScanner;

impl Scanner for PackageCachesScanner {
    fn category(&self) -> Category {
        Category::PackageCaches
    }

    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
        let home = &ctx.home;
        let mut out = Vec::new();
        let mut push_paths = |group: &str, paths: Vec<PathBuf>| {
            for p in existing(paths) {
                out.push(Candidate::new(
                    Category::PackageCaches,
                    group,
                    display_path(&p, home),
                    Target::dir(p),
                    Size::Unknown,
                    Tier::Global,
                ));
            }
        };

        let mut node = vec![
            home.join(".npm"),
            home.join(".pnpm-store"),
            home.join(".bun").join("install").join("cache"),
        ];
        if cfg!(target_os = "macos") {
            node.push(home.join("Library").join("Caches").join("Yarn"));
        } else {
            node.push(home.join(".cache").join("yarn"));
        }
        push_paths("Node (npm, pnpm, yarn, bun)", node);

        let mut flutter = vec![home.join(".pub-cache")];
        flutter.extend(flutter_sdk_caches(ctx));
        push_paths("Flutter / Dart", flutter);

        push_paths(
            "Rust (cargo)",
            vec![
                home.join(".cargo").join("registry").join("cache"),
                home.join(".cargo").join("registry").join("src"),
                home.join(".cargo").join("git").join("db"),
            ],
        );

        let gradle_home = std::env::var("GRADLE_USER_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".gradle"));
        push_paths(
            "Gradle / Kotlin",
            vec![
                gradle_home.join("caches"),
                gradle_home.join("wrapper").join("dists"),
                home.join(".kotlin"),
            ],
        );

        let mut python = Vec::new();
        if cfg!(target_os = "macos") {
            python.push(home.join("Library").join("Caches").join("pip"));
            python.push(home.join("Library").join("Caches").join("pypoetry"));
        } else {
            python.push(home.join(".cache").join("pip"));
            python.push(home.join(".cache").join("pypoetry"));
        }
        python.push(home.join(".cache").join("uv"));
        python.push(home.join(".local").join("share").join("virtualenvs"));
        push_paths("Python (pip, poetry, uv, pipenv)", python);

        push_paths(
            "Ruby (bundler, gem)",
            vec![
                home.join(".bundle").join("cache"),
                home.join(".gem").join("cache"),
            ],
        );

        if cfg!(target_os = "macos") {
            push_paths(
                "CocoaPods",
                vec![home.join("Library").join("Caches").join("CocoaPods")],
            );
            push_paths(
                "Homebrew",
                vec![home.join("Library").join("Caches").join("Homebrew")],
            );
        }

        let go_build = if cfg!(target_os = "macos") {
            home.join("Library").join("Caches").join("go-build")
        } else {
            home.join(".cache").join("go-build")
        };
        push_paths("Go", vec![go_build]);
        if ctx.runner.which("go").is_some() {
            let modcache = ctx.runner.run("go", &["env", "GOMODCACHE"], None);
            if modcache.is_success() && !modcache.stdout.trim().is_empty() {
                let p = PathBuf::from(modcache.stdout.trim());
                if p.exists() {
                    out.push(Candidate::new(
                        Category::PackageCaches,
                        "Go",
                        format!("{} (go clean -modcache)", display_path(&p, home)),
                        Target::action("go", &["clean", "-modcache"], None, vec![p]),
                        Size::Skipped,
                        Tier::Global,
                    ));
                }
            }
        }

        out
    }
}

/// Every Flutter SDK `bin/cache` worth cleaning: the active SDK (via
/// `$FLUTTER_ROOT` or `which flutter`), FVM versions, asdf/mise installs.
fn flutter_sdk_caches(ctx: &ScanContext) -> Vec<PathBuf> {
    let home = &ctx.home;
    let mut version_roots: Vec<PathBuf> = Vec::new();
    if let Ok(custom) = std::env::var("FVM_CACHE_PATH") {
        version_roots.push(PathBuf::from(custom).join("versions"));
    }
    version_roots.push(home.join("fvm").join("versions"));
    version_roots.push(home.join(".fvm").join("versions"));
    version_roots.push(home.join(".asdf").join("installs").join("flutter"));
    collect_flutter_sdk_caches(active_flutter_sdk_cache(ctx), &version_roots)
}

/// Combine an optional active SDK cache with every
/// `<version_root>/<version>/bin/cache` that exists, deduped by canonical path.
pub fn collect_flutter_sdk_caches(
    active: Option<PathBuf>,
    version_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(p) = active {
        out.push(p);
    }
    for root in version_roots {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let cache = entry.path().join("bin").join("cache");
            if cache.exists() {
                out.push(cache);
            }
        }
    }
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    out.retain(|p| {
        let key = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        seen.insert(key)
    });
    out.sort();
    out
}

fn active_flutter_sdk_cache(ctx: &ScanContext) -> Option<PathBuf> {
    if let Ok(root) = std::env::var("FLUTTER_ROOT") {
        let p = PathBuf::from(root).join("bin").join("cache");
        if p.exists() {
            return Some(p);
        }
    }
    let flutter_bin = ctx.runner.which("flutter")?;
    let resolved = std::fs::canonicalize(flutter_bin).ok()?;
    let cache = resolved.parent()?.join("cache");
    cache.exists().then_some(cache)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, FakeRunner};
    use super::*;
    use crate::runner::RunResult;
    use tempfile::TempDir;

    #[test]
    fn collects_every_fvm_version_and_dedupes_active() {
        let tmp = TempDir::new().unwrap();
        let versions_root = tmp.path().join("versions");
        for v in &["3.41.5", "3.41.6", "3.44.0"] {
            std::fs::create_dir_all(versions_root.join(v).join("bin").join("cache")).unwrap();
        }
        let active = Some(versions_root.join("3.44.0").join("bin").join("cache"));
        let caches = collect_flutter_sdk_caches(active, std::slice::from_ref(&versions_root));
        assert_eq!(caches.len(), 3);
    }

    #[test]
    fn missing_version_root_is_ignored() {
        assert!(collect_flutter_sdk_caches(None, &[PathBuf::from("/nonexistent/path")]).is_empty());
    }

    #[test]
    fn lists_existing_caches_as_global_and_go_modcache_as_action() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".npm/_cacache")).unwrap();
        std::fs::create_dir_all(home.join(".cargo/registry/cache")).unwrap();
        std::fs::create_dir_all(home.join("go/pkg/mod")).unwrap();
        let runner = FakeRunner::new(&["go"]).respond(
            "go",
            RunResult::success(home.join("go/pkg/mod").display().to_string()),
        );
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], home, vec![], &runner);

        let found = PackageCachesScanner.scan(&c);

        assert!(found.iter().all(|c| c.tier == Tier::Global));
        assert!(found.iter().any(|c| c.label == "~/.npm"));
        assert!(found.iter().any(|c| c.label == "~/.cargo/registry/cache"));
        assert!(!found.iter().any(|c| c.label.contains(".pub-cache")));
        let go = found.iter().find(|c| c.group == "Go").unwrap();
        assert!(matches!(go.target, Target::Action { .. }));
        assert!(go.label.contains("go clean -modcache"));
    }
}
