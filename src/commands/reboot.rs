use std::path::PathBuf;

use clap::Args;
use colored::Colorize;

use crate::app_detector::AppDetector;
use crate::commands::kill;
use crate::device_manager::DeviceManager;
use crate::logger::Logger;
use crate::models::{AppInfo, DevicePlatform, NodePm, ProjectType, PyFw};
use crate::runner::{RunResult, Runner};

#[derive(Args, Debug)]
pub struct RebootArgs {
    /// Target a specific device by id (mobile projects only)
    #[arg(short = 'd', long)]
    pub device: Option<String>,

    /// Verbose output
    #[arg(short = 'v', long)]
    pub verbose: bool,
}

pub fn run(args: &RebootArgs, runner: &dyn Runner) -> i32 {
    let logger = Logger::new();
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let (app_info, root) = AppDetector::new().detect_with_root(&current_dir);
    if app_info.project_type == ProjectType::Unknown {
        logger.err("Could not detect a supported project in current directory.");
        return 1;
    }

    match app_info.project_type.clone() {
        // Mobile: force-stop then relaunch the app on devices/simulators.
        ProjectType::Flutter | ProjectType::Android | ProjectType::Ios => {
            reboot_mobile(runner, &app_info, args, &logger)
        }
        // Server-style projects: kill the running dev server, then print the
        // command to start it again (mdev runs and exits, so it doesn't keep
        // a long-lived server alive itself).
        other => {
            let root = root.unwrap_or(current_dir);
            let code = kill::kill_server(runner, &root, &logger, args.verbose);
            match start_command(&other) {
                Some(cmd) => logger.info(&format!("Start it again with: {}", cmd.cyan())),
                None => logger.info("Start it again with your project's usual dev server command."),
            }
            code
        }
    }
}

// ---------------------------------------------------------------------------
// Mobile: force-stop then relaunch the app on connected devices.
// ---------------------------------------------------------------------------

fn reboot_mobile(
    runner: &dyn Runner,
    app_info: &AppInfo,
    args: &RebootArgs,
    logger: &Logger,
) -> i32 {
    if let Some(ref device_id) = args.device {
        let platform = infer_platform(device_id);
        return match restart_on(runner, app_info, platform, Some(device_id), logger, args.verbose) {
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
        if !try_platform(runner, app_info, DevicePlatform::Android, logger, args.verbose) {
            any_fail = true;
        }
    }

    if has_ios {
        any_attempt = true;
        if !try_platform(runner, app_info, DevicePlatform::Ios, logger, args.verbose) {
            any_fail = true;
        }
    }

    if !any_attempt {
        logger.err("No Android package ID or iOS bundle ID detected.");
        return 1;
    }

    if any_fail { 1 } else { 0 }
}

fn try_platform(
    runner: &dyn Runner,
    app_info: &AppInfo,
    platform: DevicePlatform,
    logger: &Logger,
    verbose: bool,
) -> bool {
    match restart_on(runner, app_info, platform.clone(), None, logger, verbose) {
        Some(true) => return true,
        Some(false) => return false,
        None => {}
    }

    let devices = DeviceManager::new(runner).list_running_devices();
    let targets: Vec<_> = devices.iter().filter(|d| d.platform == platform).collect();

    if targets.is_empty() {
        logger.warn(&format!("No running {} devices found.", platform_label(&platform)));
        return false;
    }

    let mut ok = 0usize;
    for d in &targets {
        if let Some(true) =
            restart_on(runner, app_info, platform.clone(), Some(&d.id), logger, verbose)
        {
            ok += 1;
        }
    }
    ok == targets.len()
}

/// Returns Some(true) on success, Some(false) on real failure,
/// None on device-ambiguity (caller enumerates).
fn restart_on(
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
            let pb = logger.progress(&format!("Restarting app on {}...", label));

            let stop = if let Some(id) = device_id {
                runner.run("adb", &["-s", id, "shell", "am", "force-stop", &pkg], None)
            } else {
                runner.run("adb", &["shell", "am", "force-stop", &pkg], None)
            };
            if !stop.is_success() {
                if device_id.is_none() && is_multi_device_error(&stop) {
                    pb.finish_and_clear();
                    return None;
                }
                let err = error_text(&stop);
                pb.finish_with_message(format!("{} Failed to stop: {} — {}", "✗".red(), label, err));
                if verbose {
                    logger.err(err);
                }
                return Some(false);
            }

            let launch = if let Some(id) = device_id {
                runner.run(
                    "adb",
                    &[
                        "-s", id, "shell", "monkey", "-p", &pkg, "-c",
                        "android.intent.category.LAUNCHER", "1",
                    ],
                    None,
                )
            } else {
                runner.run(
                    "adb",
                    &[
                        "shell", "monkey", "-p", &pkg, "-c",
                        "android.intent.category.LAUNCHER", "1",
                    ],
                    None,
                )
            };
            if launch.is_success() {
                pb.finish_with_message(format!("{} Restarted app on {}", "✓".green(), label));
                Some(true)
            } else {
                let err = error_text(&launch);
                pb.finish_with_message(format!(
                    "{} Stopped but failed to launch: {} — {}",
                    "✗".red(),
                    label,
                    err
                ));
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
            let pb = logger.progress(&format!("Restarting app on {}...", label));

            let term = runner.run("xcrun", &["simctl", "terminate", target, &bundle], None);
            if !term.is_success() {
                if device_id.is_none() && is_no_booted_error(&term) {
                    pb.finish_and_clear();
                    return None;
                }
                // "not running" is fine — we're about to launch it anyway.
                if !is_not_running_error(&term) {
                    let err = error_text(&term);
                    pb.finish_with_message(format!(
                        "{} Failed to stop: {} — {}",
                        "✗".red(),
                        label,
                        err
                    ));
                    if verbose {
                        logger.err(err);
                    }
                    return Some(false);
                }
            }

            let launch = runner.run("xcrun", &["simctl", "launch", target, &bundle], None);
            if launch.is_success() {
                pb.finish_with_message(format!("{} Restarted app on {}", "✓".green(), label));
                Some(true)
            } else {
                let err = error_text(&launch);
                pb.finish_with_message(format!(
                    "{} Stopped but failed to launch: {} — {}",
                    "✗".red(),
                    label,
                    err
                ));
                if verbose {
                    logger.err(err);
                }
                Some(false)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Server: best-effort start command per ecosystem (printed, not run).
// ---------------------------------------------------------------------------

/// The conventional dev-server start command for a project type, when there is
/// an unambiguous one. `None` means we can't guess reliably.
fn start_command(pt: &ProjectType) -> Option<&'static str> {
    match pt {
        ProjectType::Node { manager } => Some(match manager {
            NodePm::Npm => "npm run dev",
            NodePm::Pnpm => "pnpm dev",
            NodePm::Yarn => "yarn dev",
            NodePm::Bun => "bun dev",
        }),
        ProjectType::Rust => Some("cargo run"),
        ProjectType::Go => Some("go run ."),
        ProjectType::Ruby { rails: true } => Some("bin/rails server"),
        ProjectType::Python { framework } => match framework {
            Some(PyFw::Django) => Some("python manage.py runserver"),
            Some(PyFw::FastAPI) => Some("uvicorn app:app --reload"),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers.
// ---------------------------------------------------------------------------

fn error_text(r: &RunResult) -> &str {
    if !r.stderr.is_empty() { &r.stderr } else { &r.stdout }
}

fn is_multi_device_error(r: &RunResult) -> bool {
    let t = format!("{}\n{}", r.stderr, r.stdout).to_lowercase();
    t.contains("more than one device")
        || t.contains("more than one emulator")
        || t.contains("multiple devices")
}

fn is_no_booted_error(r: &RunResult) -> bool {
    let t = format!("{}\n{}", r.stderr, r.stdout).to_lowercase();
    t.contains("no devices are booted")
        || t.contains("unable to find")
        || t.contains("no matching")
        || t.contains("invalid device")
}

fn is_not_running_error(r: &RunResult) -> bool {
    let t = format!("{}\n{}", r.stderr, r.stdout).to_lowercase();
    t.contains("found nothing to terminate")
        || t.contains("no such process")
        || t.contains("not running")
}

fn platform_label(p: &DevicePlatform) -> &'static str {
    match p {
        DevicePlatform::Android => "Android",
        DevicePlatform::Ios => "iOS",
    }
}

fn infer_platform(device_id: &str) -> DevicePlatform {
    let looks_ios = device_id.len() == 36 && device_id.matches('-').count() == 4;
    if looks_ios { DevicePlatform::Ios } else { DevicePlatform::Android }
}
