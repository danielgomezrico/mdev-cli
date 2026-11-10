//! Scanners discover purge candidates. One per `Category`; all run in
//! parallel and never delete anything.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use crate::fs_size::{dir_size, SizeMode};
use crate::models::AppInfo;
use crate::parallel;
use crate::runner::{Runner, Timed};

use super::candidate::{dedupe_covered, Candidate, Category, Size, Target, Tier};
use super::safety::{assert_deletable, SafetyCtx};

pub mod android_sdk;
pub mod docker;
pub mod extras;
pub mod logs;
pub mod package_caches;
pub mod projects;
pub mod temp;
pub mod worktrees;
pub mod xcode;

/// Longest any scan-time command may run (docker daemon starting, `simctl`
/// after an Xcode update, CLT install dialog…) before it counts as failed.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(10);

/// Something a scanner is doing right now, for the progress display.
pub type ProgressSink<'a> = dyn Fn(&str) + Sync + 'a;

/// Sink that drops every note (tests, non-interactive callers).
pub fn silent() -> &'static ProgressSink<'static> {
    &|_: &str| {}
}

/// What `run_all` reports while scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanEvent {
    /// A scanner is working on `msg` (a path, a command…).
    Note(Category, String),
    /// A scanner finished with `found` candidates (unsized ones not yet sized).
    Done(Category, usize),
    /// `done` of `total` unsized candidates have been sized.
    Sized { done: usize, total: usize },
}

/// Everything a scanner may need. Paths are injected so tests run on
/// temp fixtures instead of the real home directory.
#[derive(Clone)]
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
    /// Receives what this scanner is doing right now.
    pub progress: &'a ProgressSink<'a>,
}

impl<'a> ScanContext<'a> {
    /// Report the current activity (shown next to the category spinner).
    pub fn note(&self, msg: impl AsRef<str>) {
        (self.progress)(msg.as_ref());
    }

    /// Delete-time safety rules for this scan's roots.
    pub fn safety(&self) -> SafetyCtx {
        SafetyCtx {
            home: self.home.clone(),
            scan_roots: self.roots.to_vec(),
            project_roots: self.projects.iter().map(|(_, r)| r.clone()).collect(),
            tmp_roots: self.tmp_roots.clone(),
        }
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

/// Run every scanner in parallel — their commands bounded by `SCAN_TIMEOUT` —
/// drop path candidates the executor would refuse and nested duplicates,
/// size path candidates that came back `Unknown` (`Skipped` stay unmeasured).
/// `on_event` receives live progress.
pub fn run_all(
    scanners: &[Box<dyn Scanner>],
    ctx: &ScanContext,
    on_event: &(dyn Fn(ScanEvent) + Sync),
) -> Vec<Candidate> {
    let timed = Timed::new(ctx.runner, SCAN_TIMEOUT);
    let safety = ctx.safety();
    let per_scanner: Vec<Vec<Candidate>> = parallel::map(scanners.iter().collect(), |s| {
        let cat = s.category();
        let sink = move |msg: &str| on_event(ScanEvent::Note(cat, msg.to_string()));
        let local = ScanContext {
            runner: &timed,
            progress: &sink,
            ..ctx.clone()
        };
        let found: Vec<Candidate> = s
            .scan(&local)
            .into_iter()
            .filter(|c| {
                c.target
                    .path()
                    .is_none_or(|p| assert_deletable(p, &safety).is_ok())
            })
            .collect();
        on_event(ScanEvent::Done(cat, found.len()));
        found
    });
    let flat: Vec<Candidate> = per_scanner.into_iter().flatten().collect();
    let deduped = dedupe_covered(flat);

    let total = deduped
        .iter()
        .filter(|c| c.size == Size::Unknown && matches!(c.target, Target::Path { .. }))
        .count();
    on_event(ScanEvent::Sized { done: 0, total });
    let done = AtomicUsize::new(0);
    parallel::map(deduped, |mut c| {
        if c.size == Size::Unknown {
            if let Target::Path { path, .. } = &c.target {
                c.size = Size::Bytes(dir_size(path, ctx.size_mode));
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                on_event(ScanEvent::Sized { done: n, total });
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

/// Whether a process whose command line matches `pattern` (ERE) is running.
/// Unknown (no `pgrep`) counts as running, so callers stay cautious.
pub fn process_running(ctx: &ScanContext, pattern: &str) -> bool {
    ctx.runner.which("pgrep").is_none()
        || ctx.runner.run("pgrep", &["-f", pattern], None).is_success()
}

/// `Global` while `busy`, so in-use data is never deleted by default.
pub fn tier_unless_busy(busy: bool) -> Tier {
    if busy {
        Tier::Global
    } else {
        Tier::Local
    }
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
        /// Working directory of every `run` call, in call order.
        pub cwds: Mutex<Vec<Option<String>>>,
        /// Timeout of every `run_timeout` call, in call order.
        pub timeouts: Mutex<Vec<Duration>>,
    }

    impl FakeRunner {
        pub fn new(tools: &[&'static str]) -> Self {
            Self {
                tools: tools.to_vec(),
                responses: Vec::new(),
                calls: Mutex::new(Vec::new()),
                cwds: Mutex::new(Vec::new()),
                timeouts: Mutex::new(Vec::new()),
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
        fn run(&self, executable: &str, args: &[&str], cwd: Option<&str>) -> RunResult {
            let mut call = vec![executable.to_string()];
            call.extend(args.iter().map(|a| a.to_string()));
            self.calls.lock().unwrap().push(call);
            self.cwds.lock().unwrap().push(cwd.map(str::to_string));
            self.responses
                .iter()
                .find(|(exe, _)| *exe == executable)
                .map(|(_, r)| r.clone())
                .unwrap_or_else(|| RunResult::failure(127, format!("{executable}: not scripted")))
        }

        fn run_timeout(
            &self,
            executable: &str,
            args: &[&str],
            cwd: Option<&str>,
            timeout: Duration,
        ) -> RunResult {
            self.timeouts.lock().unwrap().push(timeout);
            self.run(executable, args, cwd)
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
            progress: silent(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{ctx, FakeRunner};
    use super::*;
    use crate::commands::purge::candidate::Freed;
    use crate::fs_size::SizeMode;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct Probe;

    impl Scanner for Probe {
        fn category(&self) -> Category {
            Category::Docker
        }
        fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
            ctx.runner.run("docker", &["info"], None);
            Vec::new()
        }
    }

    #[test]
    fn scanner_commands_run_with_a_timeout() {
        let runner = FakeRunner::new(&[]);
        let roots: Vec<PathBuf> = vec![];
        let tmp = TempDir::new().unwrap();
        let c = ctx(&roots, &[], tmp.path(), vec![], &runner);
        run_all(&[Box::new(Probe) as Box<dyn Scanner>], &c, &|_| {});
        assert_eq!(*runner.timeouts.lock().unwrap(), vec![SCAN_TIMEOUT]);
    }

    struct Fixed(Vec<Candidate>);

    impl Scanner for Fixed {
        fn category(&self) -> Category {
            Category::Logs
        }
        fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
            ctx.note("looking");
            self.0.clone()
        }
    }

    #[test]
    fn run_all_reports_notes_done_and_sizing() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("a");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("f"), b"12345").unwrap();
        let cand = Candidate::new(
            Category::Logs,
            "g",
            "a",
            Target::dir(dir),
            Size::Unknown,
            Tier::Local,
        );
        let scanners: Vec<Box<dyn Scanner>> = vec![Box::new(Fixed(vec![cand]))];
        let runner = FakeRunner::new(&[]);
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], tmp.path(), vec![], &runner);
        let events: Mutex<Vec<ScanEvent>> = Mutex::new(Vec::new());
        let out = run_all(&scanners, &c, &|e| events.lock().unwrap().push(e));

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].size, Size::Bytes(5));
        assert_eq!(c.size_mode, SizeMode::Apparent);
        let ev = events.into_inner().unwrap();
        assert_eq!(ev[0], ScanEvent::Note(Category::Logs, "looking".into()));
        assert_eq!(ev[1], ScanEvent::Done(Category::Logs, 1));
        assert_eq!(ev[2], ScanEvent::Sized { done: 0, total: 1 });
        assert_eq!(ev[3], ScanEvent::Sized { done: 1, total: 1 });
    }

    #[test]
    fn run_all_drops_path_candidates_that_would_be_refused() {
        let home = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let inside = home.path().join("cache");
        let outside = elsewhere.path().join("sdk");
        std::fs::create_dir(&inside).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let cand = |p: &Path| {
            Candidate::new(
                Category::Logs,
                "g",
                p.display().to_string(),
                Target::dir(p.to_path_buf()),
                Size::Bytes(1),
                Tier::Local,
            )
        };
        let action = Candidate::new(
            Category::Logs,
            "g",
            "act",
            Target::action("x", &[], None, Freed::Reported),
            Size::Bytes(1),
            Tier::Local,
        );
        let scanners: Vec<Box<dyn Scanner>> = vec![Box::new(Fixed(vec![
            cand(&inside),
            cand(&outside),
            cand(home.path()),
            action,
        ]))];
        let runner = FakeRunner::new(&[]);
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], home.path(), vec![], &runner);
        let labels: Vec<String> = run_all(&scanners, &c, &|_| {})
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(labels, vec![inside.display().to_string(), "act".into()]);
    }
}
