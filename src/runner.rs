use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, Stdio};

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

    pub fn success(stdout: String) -> Self {
        Self::new(0, stdout, String::new())
    }

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
