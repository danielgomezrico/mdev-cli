use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clap::Args;
use colored::Colorize;

use crate::app_detector::AppDetector;
use crate::commands::device_op;
use crate::commands::device_outcome;
use crate::logger::Logger;
use crate::models::{AppInfo, DevicePlatform, ProjectType};
use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct KillArgs {
    /// Target a specific device by id (mobile projects only)
    #[arg(short = 'd', long)]
    pub device: Option<String>,

    /// Verbose output
    #[arg(short = 'v', long)]
    pub verbose: bool,
}

pub fn run(args: &KillArgs, runner: &dyn Runner) -> i32 {
    let logger = Logger::new();
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let (app_info, root) = AppDetector::new().detect_with_root(&current_dir);
    if app_info.project_type == ProjectType::Unknown {
        logger.err("Could not detect a supported project in current directory.");
        return 1;
    }

    match app_info.project_type {
        // Mobile: force-stop the running app process on devices/simulators.
        ProjectType::Flutter | ProjectType::Android | ProjectType::Ios => {
            kill_mobile(runner, &app_info, args, &logger)
        }
        // Everything else is a "server"-style project: kill the dev server
        // process that's listening on a port from this project directory.
        _ => {
            let root = root.unwrap_or(current_dir);
            kill_server(runner, &root, &logger, args.verbose)
        }
    }
}

// ---------------------------------------------------------------------------
// Mobile: force-stop the app on connected Android/iOS devices.
// ---------------------------------------------------------------------------

fn kill_mobile(
    runner: &dyn Runner,
    app_info: &AppInfo,
    args: &KillArgs,
    logger: &Logger,
) -> i32 {
    if let Some(ref device_id) = args.device {
        let platform = DevicePlatform::from_device_id(device_id);
        return match force_stop_on(runner, app_info, platform, Some(device_id), logger, args.verbose)
        {
            Some(true) => 0,
            _ => 1,
        };
    }

    let has_android = app_info.android_package_id.is_some();
    let has_ios = app_info.ios_bundle_id.is_some() && !cfg!(target_os = "linux");

    let mut any_attempt = false;
    let mut any_fail = false;

    if has_android {
        any_attempt = true;
        if !device_op::run_on_platform(runner, app_info, DevicePlatform::Android, logger, args.verbose, force_stop_on) {
            any_fail = true;
        }
    }

    if has_ios {
        any_attempt = true;
        if !device_op::run_on_platform(runner, app_info, DevicePlatform::Ios, logger, args.verbose, force_stop_on) {
            any_fail = true;
        }
    }

    if !any_attempt {
        logger.err("No Android package ID or iOS bundle ID detected.");
        return 1;
    }

    if any_fail { 1 } else { 0 }
}

/// Returns Some(true) on success (app stopped or already not running),
/// Some(false) on real failure, None on device-ambiguity (caller enumerates).
fn force_stop_on(
    runner: &dyn Runner,
    app_info: &AppInfo,
    platform: DevicePlatform,
    device_id: Option<&str>,
    logger: &Logger,
    verbose: bool,
) -> Option<bool> {
    match platform {
        DevicePlatform::Android => {
            let pkg = match &app_info.android_package_id {
                Some(id) => id.clone(),
                None => {
                    logger.err("No Android package ID detected.");
                    return Some(false);
                }
            };
            let label = device_id.unwrap_or("android").to_string();
            let pb = logger.progress(&format!("Stopping app on {}...", label));
            let result = if let Some(id) = device_id {
                runner.run("adb", &["-s", id, "shell", "am", "force-stop", &pkg], None)
            } else {
                runner.run("adb", &["shell", "am", "force-stop", &pkg], None)
            };
            if result.is_success() {
                pb.finish_with_message(format!("{} Stopped app on {}", "✓".green(), label));
                Some(true)
            } else if device_id.is_none() && device_outcome::is_multi_device_error(&result) {
                pb.finish_and_clear();
                None
            } else {
                let err = device_outcome::error_text(&result);
                pb.finish_with_message(format!("{} Failed: {} — {}", "✗".red(), label, err));
                if verbose {
                    logger.err(err);
                }
                Some(false)
            }
        }
        DevicePlatform::Ios => {
            if cfg!(target_os = "linux") {
                return Some(false);
            }
            let bundle = match &app_info.ios_bundle_id {
                Some(id) => id.clone(),
                None => {
                    logger.err("No iOS bundle ID detected.");
                    return Some(false);
                }
            };
            let target = device_id.unwrap_or("booted");
            let label = device_id.unwrap_or("booted simulator").to_string();
            let pb = logger.progress(&format!("Stopping app on {}...", label));
            let result = runner.run("xcrun", &["simctl", "terminate", target, &bundle], None);
            if result.is_success() {
                pb.finish_with_message(format!("{} Stopped app on {}", "✓".green(), label));
                Some(true)
            } else if device_id.is_none() && device_outcome::is_no_booted_error(&result) {
                pb.finish_and_clear();
                None
            } else if device_outcome::is_not_running_error(&result) {
                pb.finish_with_message(format!("{} App not running on {}", "•".yellow(), label));
                Some(true)
            } else {
                let err = device_outcome::error_text(&result);
                pb.finish_with_message(format!("{} Failed: {} — {}", "✗".red(), label, err));
                if verbose {
                    logger.err(err);
                }
                Some(false)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Server: kill the dev-server process listening on a port from this project.
// ---------------------------------------------------------------------------

/// A listening process owned by the current project.
struct Server {
    pid: u32,
    command: String,
    port: String,
}

pub(crate) fn kill_server(runner: &dyn Runner, root: &Path, logger: &Logger, verbose: bool) -> i32 {
    if cfg!(target_os = "windows") {
        logger.err("`mdev kill` for server projects is not supported on Windows yet.");
        return 1;
    }

    let pb = logger.progress("Finding running server for this project...");
    let servers = find_project_servers(runner, root);
    pb.finish_and_clear();

    if servers.is_empty() {
        logger.warn("No running server found for this project.");
        return 0;
    }

    let mut any_fail = false;
    for s in &servers {
        let label = format!("{} (pid {}, port {})", s.command, s.pid, s.port);
        let pid = s.pid.to_string();
        let result = runner.run("kill", &["-TERM", &pid], None);
        if result.is_success() {
            logger.success(&format!("{} Killed {}", "✓".green(), label));
        } else {
            let err = device_outcome::error_text(&result);
            logger.err(&format!("{} Failed to kill {} — {}", "✗".red(), label, err));
            if verbose {
                logger.err(err);
            }
            any_fail = true;
        }
    }

    if any_fail { 1 } else { 0 }
}

/// Find every process LISTENing on a TCP port whose working directory is
/// inside `root`. Uses `lsof` (no-op returning empty when `lsof` is absent).
fn find_project_servers(runner: &dyn Runner, root: &Path) -> Vec<Server> {
    let res = runner.run("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"], None);
    if !res.is_success() {
        return Vec::new();
    }

    // Parse lsof -F output: `p<pid>` and `c<command>` are per-process, each
    // following `n<host:port>` is a listening socket of that process.
    let mut entries: Vec<(u32, String, String)> = Vec::new();
    let mut pid: Option<u32> = None;
    let mut command = String::new();
    for line in res.stdout.lines() {
        let Some(tag) = line.chars().next() else {
            continue;
        };
        let val = &line[1..];
        match tag {
            'p' => {
                pid = val.parse().ok();
                command.clear();
            }
            'c' => command = val.to_string(),
            'n' => {
                if let (Some(pid), Some(port)) = (pid, port_from_name(val)) {
                    entries.push((pid, command.clone(), port));
                }
            }
            _ => {}
        }
    }

    let mut cwd_ok: HashMap<u32, bool> = HashMap::new();
    let mut servers: Vec<Server> = Vec::new();
    for (pid, command, port) in entries {
        let ok = *cwd_ok
            .entry(pid)
            .or_insert_with(|| cwd_under(runner, pid, root));
        if ok && !servers.iter().any(|s| s.pid == pid && s.port == port) {
            servers.push(Server { pid, command, port });
        }
    }
    servers
}

/// True if process `pid`'s current working directory is at or below `root`.
fn cwd_under(runner: &dyn Runner, pid: u32, root: &Path) -> bool {
    let res = runner.run("lsof", &["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"], None);
    if !res.is_success() {
        return false;
    }
    res.stdout
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .any(|path| Path::new(path).starts_with(root))
}

/// Extract the numeric port from an lsof socket name like `*:3000`,
/// `127.0.0.1:8000`, or `[::1]:3000`.
fn port_from_name(name: &str) -> Option<String> {
    name.rsplit(':')
        .next()
        .filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        .map(|p| p.to_string())
}

