use std::process::Command;

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

/// Abstract subprocess runner — allows real and mock implementations.
/// Object-safe: all methods take `&self` and return owned values.
pub trait Runner {
    fn run(&self, executable: &str, args: &[&str], working_dir: Option<&str>) -> RunResult;
    fn which(&self, executable: &str) -> Option<String>;
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

    fn which(&self, executable: &str) -> Option<String> {
        let which_cmd = if cfg!(target_os = "windows") { "where" } else { "which" };

        match Command::new(which_cmd).arg(executable).output() {
            Ok(output) if output.status.success() => {
                let raw = String::from_utf8_lossy(&output.stdout);
                raw.lines().next().map(|l| l.trim().to_string())
            }
            _ => None,
        }
    }
}
