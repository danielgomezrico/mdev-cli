use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use clap::Args;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

use crate::logger::Logger;

#[derive(Args, Debug)]
pub struct DoallArgs {
    /// Parent directory whose immediate subfolders receive the command (default: .)
    #[arg(short = 'C', long = "dir", default_value = ".")]
    pub dir: PathBuf,

    /// Include hidden subdirectories (names starting with '.')
    #[arg(long)]
    pub hidden: bool,

    /// Command and arguments to run in each subfolder
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

pub fn run(args: &DoallArgs) -> i32 {
    let logger = Logger::new();

    if args.command.is_empty() {
        logger.err("No command provided. Usage: mdev doall <command> [args...]");
        return 1;
    }

    let parent = match args.dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            logger.err(&format!("Cannot access parent dir '{}': {e}", args.dir.display()));
            return 1;
        }
    };

    let subdirs = match list_subdirs(&parent, args.hidden) {
        Ok(d) if d.is_empty() => {
            logger.err(&format!("No subdirectories under {}", parent.display()));
            return 1;
        }
        Ok(d) => d,
        Err(e) => {
            logger.err(&format!("Failed to list {}: {e}", parent.display()));
            return 1;
        }
    };

    let shell_cmd = join_shell_command(&args.command);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| default_shell().to_string());
    let total = subdirs.len();
    let names: Vec<String> = subdirs
        .iter()
        .map(|d| {
            d.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| d.display().to_string())
        })
        .collect();

    logger.info(&format!(
        "{} {} folder(s) {} in {} · {}",
        "mdev doall".cyan().bold(),
        total,
        "in parallel".dimmed(),
        parent.display().to_string().dimmed(),
        shell_cmd.dimmed()
    ));
    for name in &names {
        logger.detail(&format!("  · {name}"));
    }

    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} [{bar:24.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▓░")
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.enable_steady_tick(Duration::from_millis(80));

    // Folders still running — shared so the bar can show live in-flight names.
    let running: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(names.iter().cloned().collect()));
    pb.set_message(format_running_msg(0, total, &running.lock().unwrap()));

    let (tx, rx) = mpsc::channel::<DirResult>();
    for dir in subdirs {
        let shell = shell.clone();
        let shell_cmd = shell_cmd.clone();
        let tx = tx.clone();
        thread::spawn(move || {
            let result = run_in_dir(&shell, &shell_cmd, &dir);
            let _ = tx.send(result);
        });
    }
    drop(tx);

    let mut results = Vec::with_capacity(total);
    let mut done = 0usize;
    while let Ok(result) = rx.recv() {
        done += 1;
        {
            let mut run = running.lock().unwrap();
            run.remove(&result.name);
            pb.set_position(done as u64);
            pb.set_message(format_running_msg(done, total, &run));
        }
        // suspend so lines aren't eaten when the bar is hidden (non-TTY)
        pb.suspend(|| {
            println!("{}", format_result_line(&result));
            print_result_body(&result);
        });
        results.push(result);
    }
    pb.finish_and_clear();

    results.sort_by(|a, b| a.name.cmp(&b.name));
    let failed: Vec<&DirResult> = results.iter().filter(|r| r.exit_code != 0).collect();
    let ok_count = results.len() - failed.len();

    if failed.is_empty() {
        logger.success(&format!("── {ok_count}/{total} ok ──"));
        0
    } else {
        logger.err(&format!("── {ok_count}/{total} ok · {} failed ──", failed.len()));
        for r in &failed {
            logger.err(&format!("  ✗ {} (exit {})", r.name, r.exit_code));
        }
        1
    }
}

#[derive(Debug)]
struct DirResult {
    name: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn run_in_dir(shell: &str, shell_cmd: &str, dir: &Path) -> DirResult {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string());

    match Command::new(shell)
        .arg("-c")
        .arg(shell_cmd)
        .current_dir(dir)
        .output()
    {
        Ok(output) => DirResult {
            name,
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).trim_end().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim_end().to_string(),
        },
        Err(e) => DirResult {
            name,
            exit_code: 1,
            stdout: String::new(),
            stderr: e.to_string(),
        },
    }
}

fn format_running_msg(done: usize, total: usize, running: &BTreeSet<String>) -> String {
    if running.is_empty() {
        return format!("{done}/{total} complete");
    }
    let list = truncate_names(running, 4);
    format!("{done}/{total} · running: {list}")
}

/// Show up to `max` names; remainder as "+N".
fn truncate_names(names: &BTreeSet<String>, max: usize) -> String {
    let n = names.len();
    if n <= max {
        return names.iter().cloned().collect::<Vec<_>>().join(", ");
    }
    let shown: Vec<_> = names.iter().take(max).cloned().collect();
    format!("{}, +{}", shown.join(", "), n - max)
}

fn format_result_line(result: &DirResult) -> String {
    if result.exit_code == 0 {
        format!("{} {}", "OK".green().bold(), result.name.green())
    } else {
        format!(
            "{} {} (exit {})",
            "FAIL".red().bold(),
            result.name.red(),
            result.exit_code
        )
    }
}

fn print_result_body(result: &DirResult) {
    if !result.stdout.is_empty() {
        for line in result.stdout.lines() {
            println!("  {line}");
        }
    }
    if !result.stderr.is_empty() {
        for line in result.stderr.lines() {
            eprintln!("  {}", line.red());
        }
    }
}

fn list_subdirs(parent: &Path, include_hidden: bool) -> std::io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !include_hidden && name.starts_with('.') {
            continue;
        }
        dirs.push(path);
    }
    dirs.sort();
    Ok(dirs)
}

/// Join argv into a shell-safe command string.
fn join_shell_command(args: &[String]) -> String {
    args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@' | '%' | '+' | '=' | ','))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn default_shell() -> &'static str {
    if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "/bin/sh"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    #[test]
    fn running_msg_lists_in_flight() {
        let mut set = BTreeSet::new();
        set.insert("a".into());
        set.insert("b".into());
        assert_eq!(format_running_msg(0, 2, &set), "0/2 · running: a, b");
        set.clear();
        assert_eq!(format_running_msg(2, 2, &set), "2/2 complete");
    }

    #[test]
    fn truncate_names_adds_remainder() {
        let set: BTreeSet<String> = ["a", "b", "c", "d", "e"].into_iter().map(String::from).collect();
        assert_eq!(truncate_names(&set, 3), "a, b, c, +2");
    }

    #[test]
    fn shell_quote_plain() {
        assert_eq!(shell_quote("nexusindex"), "nexusindex");
        assert_eq!(shell_quote("git"), "git");
    }

    #[test]
    fn shell_quote_spaces() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
    }

    #[test]
    fn shell_quote_single_quote() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn join_command() {
        let args = vec!["git".into(), "status".into(), "-sb".into()];
        assert_eq!(join_shell_command(&args), "git status -sb");
    }

    #[test]
    fn list_subdirs_skips_files_and_hidden() {
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("a")).unwrap();
        fs::create_dir(tmp.path().join("b")).unwrap();
        fs::create_dir(tmp.path().join(".hidden")).unwrap();
        fs::write(tmp.path().join("file.txt"), "x").unwrap();

        let dirs = list_subdirs(tmp.path(), false).unwrap();
        let names: Vec<_> = dirs
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a", "b"]);

        let with_hidden = list_subdirs(tmp.path(), true).unwrap();
        assert_eq!(with_hidden.len(), 3);
    }

    #[test]
    fn runs_command_in_each_subdir_parallel() {
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("one")).unwrap();
        fs::create_dir(tmp.path().join("two")).unwrap();

        let args = DoallArgs {
            dir: tmp.path().to_path_buf(),
            hidden: false,
            command: vec!["sleep".into(), "1".into()],
        };

        let start = Instant::now();
        let code = run(&args);
        let elapsed = start.elapsed();

        assert_eq!(code, 0);
        // Parallel: ~1s, not ~2s
        assert!(
            elapsed < Duration::from_millis(1800),
            "expected parallel (~1s), got {elapsed:?}"
        );
    }

    #[test]
    fn fails_when_any_subdir_fails() {
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("ok")).unwrap();
        fs::create_dir(tmp.path().join("bad")).unwrap();

        // false exits 1 in every dir
        let args = DoallArgs {
            dir: tmp.path().to_path_buf(),
            hidden: false,
            command: vec!["false".into()],
        };
        assert_eq!(run(&args), 1);
    }

    #[test]
    fn runs_cwd_is_subdir() {
        let tmp = tempdir().unwrap();
        let a = tmp.path().join("proj-a");
        fs::create_dir(&a).unwrap();

        let marker = a.join("marker.txt");
        // write marker via the command itself, proving cwd
        let args = DoallArgs {
            dir: tmp.path().to_path_buf(),
            hidden: false,
            command: vec!["touch".into(), "marker.txt".into()],
        };
        assert_eq!(run(&args), 0);
        assert!(marker.exists());
    }
}
