use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// Result of a subprocess execution.
#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl RunResult {
    pub fn new(exit_code: i32, stdout: String, stderr: String) -> Self {
        Self {
            exit_code,
            stdout,
            stderr,
        }
    }

    #[cfg(test)]
    pub fn success(stdout: String) -> Self {
        Self::new(0, stdout, String::new())
    }

    #[cfg(test)]
    pub fn failure(exit_code: i32, stderr: String) -> Self {
        Self::new(exit_code, String::new(), stderr)
    }

    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Handle to a process left running in the background.
pub trait BackgroundProcess {
    /// `Some(exit_code)` once the process has exited, `None` while it still runs.
    fn exit_code(&mut self) -> Option<i32>;
}

/// Abstract subprocess runner — allows real and mock implementations.
/// Object-safe: all methods take `&self` and return owned values.
pub trait Runner {
    fn run(&self, executable: &str, args: &[&str], working_dir: Option<&str>) -> RunResult;
    fn which(&self, executable: &str) -> Option<String>;

    /// [`Runner::run`] that gives up after `timeout`: the process is killed
    /// and a failure returned. Stdin is closed so prompts cannot block.
    fn run_timeout(
        &self,
        executable: &str,
        args: &[&str],
        working_dir: Option<&str>,
        _timeout: Duration,
    ) -> RunResult {
        self.run(executable, args, working_dir)
    }

    /// Start `executable` in the background with stdout and stderr appended to
    /// `log_path`. The process outlives this one. `None` when the runner cannot
    /// spawn (test doubles) or the spawn failed.
    fn spawn_detached(
        &self,
        _executable: &str,
        _args: &[&str],
        _log_path: &Path,
    ) -> Option<Box<dyn BackgroundProcess>> {
        None
    }
}

/// Live `std::process::Child`. Dropping it leaves the process running, which is
/// what callers such as the Android emulator launcher want. `try_wait` reaps the
/// child so a crashed process is reported as exited instead of lingering as a
/// zombie that still answers to signals.
struct ChildProcess(Child);

impl BackgroundProcess for ChildProcess {
    fn exit_code(&mut self) -> Option<i32> {
        match self.0.try_wait() {
            Ok(Some(status)) => Some(status.code().unwrap_or(1)),
            Ok(None) => None,
            Err(_) => Some(1),
        }
    }
}

/// Every `run` bounded by `timeout` — for calls that must never hang the
/// caller (scan-time probes). Other calls pass through.
pub struct Timed<'a> {
    inner: &'a (dyn Runner + Sync),
    timeout: Duration,
}

impl<'a> Timed<'a> {
    pub fn new(inner: &'a (dyn Runner + Sync), timeout: Duration) -> Self {
        Self { inner, timeout }
    }
}

impl Runner for Timed<'_> {
    fn run(&self, executable: &str, args: &[&str], working_dir: Option<&str>) -> RunResult {
        self.inner
            .run_timeout(executable, args, working_dir, self.timeout)
    }

    fn run_timeout(
        &self,
        executable: &str,
        args: &[&str],
        working_dir: Option<&str>,
        timeout: Duration,
    ) -> RunResult {
        self.inner
            .run_timeout(executable, args, working_dir, timeout)
    }

    fn which(&self, executable: &str) -> Option<String> {
        self.inner.which(executable)
    }

    fn spawn_detached(
        &self,
        executable: &str,
        args: &[&str],
        log_path: &Path,
    ) -> Option<Box<dyn BackgroundProcess>> {
        self.inner.spawn_detached(executable, args, log_path)
    }
}

/// Read `r` to the end on its own thread so a full pipe never blocks the child.
fn drain(mut r: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        let _ = tx.send(String::from_utf8_lossy(&buf).trim().to_string());
    });
    rx
}

/// Drained output, or empty if a grandchild still holds the pipe at `deadline`
/// (its reader thread is then left behind rather than blocking the caller).
fn collect(rx: Option<Receiver<String>>, deadline: Instant) -> String {
    rx.and_then(|rx| {
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()
    })
    .unwrap_or_default()
}

/// Production runner using `std::process::Command`.
pub struct ProcessRunner;

impl ProcessRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for ProcessRunner {
    fn run(&self, executable: &str, args: &[&str], working_dir: Option<&str>) -> RunResult {
        let mut cmd = Command::new(executable);
        cmd.args(args);
        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        match cmd.output() {
            Ok(output) => {
                let exit_code = output.status.code().unwrap_or(1);
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                RunResult::new(exit_code, stdout, stderr)
            }
            Err(e) => RunResult::new(1, String::new(), e.to_string()),
        }
    }

    fn run_timeout(
        &self,
        executable: &str,
        args: &[&str],
        working_dir: Option<&str>,
        timeout: Duration,
    ) -> RunResult {
        let mut cmd = Command::new(executable);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return RunResult::new(1, String::new(), e.to_string()),
        };
        let out = child.stdout.take().map(drain);
        let err = child.stderr.take().map(drain);
        let deadline = Instant::now() + timeout;
        let mut poll = Duration::from_millis(1);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let grace = deadline.max(Instant::now() + Duration::from_secs(1));
                    return RunResult::new(
                        status.code().unwrap_or(1),
                        collect(out, grace),
                        collect(err, grace),
                    );
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(poll);
                    poll = (poll * 2).min(Duration::from_millis(50));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return RunResult::new(
                        1,
                        String::new(),
                        format!("{executable}: timed out after {timeout:?}"),
                    );
                }
            }
        }
    }

    fn spawn_detached(
        &self,
        executable: &str,
        args: &[&str],
        log_path: &Path,
    ) -> Option<Box<dyn BackgroundProcess>> {
        let log = File::create(log_path).ok()?;
        let log_err = log.try_clone().ok()?;
        let child = Command::new(executable)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .spawn()
            .ok()?;
        Some(Box::new(ChildProcess(child)))
    }

    fn which(&self, executable: &str) -> Option<String> {
        let which_cmd = if cfg!(target_os = "windows") {
            "where"
        } else {
            "which"
        };

        match Command::new(which_cmd).arg(executable).output() {
            Ok(output) if output.status.success() => {
                let raw = String::from_utf8_lossy(&output.stdout);
                raw.lines().next().map(|l| l.trim().to_string())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn run_timeout_kills_a_hung_process() {
        let started = Instant::now();
        let r = ProcessRunner.run_timeout("sleep", &["5"], None, Duration::from_millis(200));
        assert!(!r.is_success());
        assert!(r.stderr.contains("timed out"), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn run_timeout_returns_output_of_fast_process() {
        let r = ProcessRunner.run_timeout(
            "sh",
            &["-c", "head -c 300000 /dev/zero; echo err >&2; exit 3"],
            None,
            Duration::from_secs(10),
        );
        assert_eq!(r.exit_code, 3);
        assert_eq!(r.stdout.len(), 300_000, "large output must not deadlock");
        assert_eq!(r.stderr, "err");
    }

    #[test]
    fn run_timeout_reports_spawn_failure() {
        let r = ProcessRunner.run_timeout("mdev-no-such-exe", &[], None, Duration::from_secs(1));
        assert!(!r.is_success());
    }
}
