//! Scanners discover purge candidates. One per `Category`; all run in
//! parallel and never delete anything.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::fs_size::{dir_size, SizeMode};
use crate::models::AppInfo;
use crate::parallel;
use crate::runner::Runner;

use super::candidate::{dedupe_covered, Candidate, Category, Size, Target};

pub mod android_sdk;
pub mod docker;
pub mod extras;
pub mod logs;
pub mod package_caches;
pub mod projects;
pub mod temp;
pub mod worktrees;
pub mod xcode;

/// Everything a scanner may need. Paths are injected so tests run on
/// temp fixtures instead of the real home directory.
pub struct ScanContext<'a> {
    /// `mdev purge [PATH...]` — where project artifacts are looked for.
    pub roots: &'a [PathBuf],
    pub projects: &'a [(AppInfo, PathBuf)],
    pub home: PathBuf,
    pub tmp_roots: Vec<PathBuf>,
    pub tmp_age: Duration,
    pub now: SystemTime,
    pub runner: &'a (dyn Runner + Sync),
    pub size_mode: SizeMode,
}

impl<'a> ScanContext<'a> {
    pub fn size_of(&self, path: &Path) -> Size {
        Size::Bytes(dir_size(path, self.size_mode))
    }
}

pub trait Scanner: Sync {
    fn category(&self) -> Category;
    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate>;
}

pub fn all_scanners() -> Vec<Box<dyn Scanner>> {
    vec![
        Box::new(projects::ProjectsScanner),
        Box::new(extras::ExtrasScanner),
        Box::new(worktrees::WorktreesScanner),
        Box::new(temp::TempScanner),
        Box::new(logs::LogsScanner),
        Box::new(xcode::XcodeScanner),
        Box::new(android_sdk::AndroidSdkScanner),
        Box::new(package_caches::PackageCachesScanner),
        Box::new(docker::DockerScanner),
    ]
}

/// Run every scanner in parallel, size path candidates that came back
/// unsized, drop nested duplicates. `on_done` fires as each scanner finishes.
pub fn run_all(
    scanners: &[Box<dyn Scanner>],
    ctx: &ScanContext,
    on_done: &(dyn Fn(Category) + Sync),
) -> Vec<Candidate> {
    let per_scanner: Vec<Vec<Candidate>> = parallel::map(scanners.iter().collect(), |s| {
        let found = s.scan(ctx);
        on_done(s.category());
        found
    });
    let flat: Vec<Candidate> = per_scanner.into_iter().flatten().collect();
    let deduped = dedupe_covered(flat);
    parallel::map(deduped, |mut c| {
        if c.size == Size::Unknown {
            if let Target::Path { path, .. } = &c.target {
                c.size = ctx.size_of(path);
            }
        }
        c
    })
}

/// Shared skip set for FS walks: dependency/build/VCS trees that other
/// scanners own or that never host sibling artifacts we should descend into.
pub fn is_heavy_or_owned_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | ".git"
            | "target"
            | "build"
            | "dist"
            | "vendor"
            | ".dart_tool"
            | "Pods"
            | "DerivedData"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".gradle"
    )
}

/// Existing, deduplicated (canonical) temp roots on this machine.
pub fn tmp_roots() -> Vec<PathBuf> {
    let candidates = [
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/var/tmp"),
        std::env::temp_dir(),
    ];
    let mut seen: HashSet<PathBuf> = HashSet::new();
    candidates
        .into_iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Direct child directories of `dir`, skipping symlinks. Empty on error.
pub fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| {
                e.file_type()
                    .map(|t| t.is_dir() && !t.is_symlink())
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// Keep only paths that exist (without following a dangling symlink).
pub fn existing(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|p| std::fs::symlink_metadata(p).is_ok())
        .collect()
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::runner::RunResult;
    use std::sync::Mutex;

    /// Records every call; answers `which` from `tools` and `run` from
    /// `responses` (matched by executable name, first match wins).
    pub struct FakeRunner {
        pub tools: Vec<&'static str>,
        pub responses: Vec<(&'static str, RunResult)>,
        pub calls: Mutex<Vec<Vec<String>>>,
    }

    impl FakeRunner {
        pub fn new(tools: &[&'static str]) -> Self {
            Self {
                tools: tools.to_vec(),
                responses: Vec::new(),
                calls: Mutex::new(Vec::new()),
            }
        }

        pub fn respond(mut self, exe: &'static str, result: RunResult) -> Self {
            self.responses.push((exe, result));
            self
        }

        pub fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, executable: &str, args: &[&str], _cwd: Option<&str>) -> RunResult {
            let mut call = vec![executable.to_string()];
            call.extend(args.iter().map(|a| a.to_string()));
            self.calls.lock().unwrap().push(call);
            self.responses
                .iter()
                .find(|(exe, _)| *exe == executable)
                .map(|(_, r)| r.clone())
                .unwrap_or_else(|| RunResult::failure(127, format!("{executable}: not scripted")))
        }

        fn which(&self, executable: &str) -> Option<String> {
            self.tools
                .contains(&executable)
                .then(|| format!("/usr/bin/{executable}"))
        }

        fn spawn_detached(
            &self,
            _executable: &str,
            _args: &[&str],
            _log_path: &Path,
        ) -> Option<Box<dyn crate::runner::BackgroundProcess>> {
            None
        }
    }

    pub fn ctx<'a>(
        roots: &'a [PathBuf],
        projects: &'a [(AppInfo, PathBuf)],
        home: &Path,
        tmp_roots: Vec<PathBuf>,
        runner: &'a (dyn Runner + Sync),
    ) -> ScanContext<'a> {
        ScanContext {
            roots,
            projects,
            home: home.to_path_buf(),
            tmp_roots,
            tmp_age: Duration::from_secs(3 * 24 * 3600),
            now: SystemTime::now(),
            runner,
            size_mode: SizeMode::Apparent,
        }
    }
}
