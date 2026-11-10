//! Last line of defence before anything is removed. Every path target must
//! pass `assert_deletable`, regardless of which scanner produced it.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct SafetyCtx {
    pub home: PathBuf,
    pub scan_roots: Vec<PathBuf>,
    pub project_roots: Vec<PathBuf>,
    pub tmp_roots: Vec<PathBuf>,
}

const FORBIDDEN: &[&str] = &[
    "/",
    "/Users",
    "/home",
    "/private",
    "/var",
    "/tmp",
    "/etc",
    "/usr",
    "/bin",
    "/sbin",
    "/opt",
    "/System",
    "/Library",
    "/Applications",
    "/Volumes",
];

/// Ok when `path` is absolute, exists, is not a protected location, is not a
/// scan/project/tmp root or the home itself, and lives (also after resolving
/// its parent's symlinks) under the home, a scan root, or a tmp root.
pub fn assert_deletable(path: &Path, ctx: &SafetyCtx) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{}: not an absolute path", path.display()));
    }
    if FORBIDDEN.iter().any(|f| Path::new(f) == path) {
        return Err(format!("{}: protected system location", path.display()));
    }
    if is_volume_root(path) {
        return Err(format!("{}: volume root", path.display()));
    }
    if path == ctx.home {
        return Err(format!("{}: home directory", path.display()));
    }
    for (what, roots) in [
        ("scan root", &ctx.scan_roots),
        ("project root", &ctx.project_roots),
        ("temp root", &ctx.tmp_roots),
    ] {
        if roots.iter().any(|r| r == path) {
            return Err(format!("{}: is a {}", path.display(), what));
        }
    }
    // Resolve the parent so a symlinked ancestor cannot lead outside the roots.
    let resolved = path
        .parent()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .zip(path.file_name())
        .map(|(parent, name)| parent.join(name))
        .ok_or_else(|| format!("{}: cannot resolve parent directory", path.display()))?;
    let allowed = std::iter::once(&ctx.home)
        .chain(ctx.scan_roots.iter())
        .chain(ctx.tmp_roots.iter())
        .filter(|root| !root.as_os_str().is_empty())
        .any(|root| {
            path.starts_with(root)
                && resolved
                    .starts_with(std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()))
        });
    if !allowed {
        return Err(format!(
            "{}: outside home, scan roots and temp roots",
            path.display()
        ));
    }
    if std::fs::symlink_metadata(path).is_err() {
        return Err(format!("{}: does not exist", path.display()));
    }
    Ok(())
}

/// `/Volumes/<name>` exactly.
fn is_volume_root(path: &Path) -> bool {
    let comps: Vec<Component> = path.components().collect();
    comps.len() == 3 && matches!(comps[0], Component::RootDir) && comps[1].as_os_str() == "Volumes"
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn ctx(home: &Path, scan: &Path, tmp: &Path) -> SafetyCtx {
        SafetyCtx {
            home: home.to_path_buf(),
            scan_roots: vec![scan.to_path_buf()],
            project_roots: vec![scan.join("proj")],
            tmp_roots: vec![tmp.to_path_buf()],
        }
    }

    #[test]
    fn refuses_protected_and_root_locations() {
        let home = TempDir::new().unwrap();
        let scan = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let c = ctx(home.path(), scan.path(), tmp.path());
        for p in ["/", "/Users", "/Volumes/corsair", "/private", "/tmp"] {
            assert!(assert_deletable(Path::new(p), &c).is_err(), "{p}");
        }
        assert!(assert_deletable(home.path(), &c).is_err());
        assert!(assert_deletable(scan.path(), &c).is_err());
        assert!(assert_deletable(tmp.path(), &c).is_err());
        assert!(assert_deletable(Path::new("relative/x"), &c).is_err());
    }

    #[test]
    fn refuses_project_root_and_outside_paths() {
        let home = TempDir::new().unwrap();
        let scan = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let c = ctx(home.path(), scan.path(), tmp.path());
        std::fs::create_dir_all(scan.path().join("proj")).unwrap();
        assert!(assert_deletable(&scan.path().join("proj"), &c).is_err());
        std::fs::create_dir_all(elsewhere.path().join("x")).unwrap();
        assert!(assert_deletable(&elsewhere.path().join("x"), &c).is_err());
    }

    #[test]
    fn accepts_existing_paths_under_allowed_roots() {
        let home = TempDir::new().unwrap();
        let scan = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let c = ctx(home.path(), scan.path(), tmp.path());
        for p in [
            home.path().join(".npm"),
            scan.path().join("proj/node_modules"),
            tmp.path().join("old-thing"),
        ] {
            std::fs::create_dir_all(&p).unwrap();
            assert!(assert_deletable(&p, &c).is_ok(), "{}", p.display());
        }
        assert!(assert_deletable(&home.path().join("missing"), &c).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_paths_reached_through_a_symlinked_parent() {
        let home = TempDir::new().unwrap();
        let scan = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let c = ctx(home.path(), scan.path(), tmp.path());
        std::fs::create_dir_all(elsewhere.path().join("target")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), scan.path().join("link")).unwrap();
        assert!(assert_deletable(&scan.path().join("link/target"), &c).is_err());
    }

    #[test]
    fn refuses_when_parent_cannot_be_resolved() {
        let home = TempDir::new().unwrap();
        let scan = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let c = ctx(home.path(), scan.path(), tmp.path());
        assert!(assert_deletable(&scan.path().join("gone/x"), &c).is_err());
    }
}
