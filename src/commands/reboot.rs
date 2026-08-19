use std::path::PathBuf;

use clap::Args;
use colored::Colorize;

use crate::app_detector::AppDetector;
use crate::commands::device_op;
use crate::commands::device_outcome;
use crate::commands::kill;
use crate::logger::Logger;
use crate::models::{AppInfo, DevicePlatform, NodePm, ProjectType, PyFw};
use crate::runner::Runner;

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
        let platform = DevicePlatform::from_device_id(device_id);
        return match restart_on(runner, app_info, platform, Some(device_id), logger, args.verbose) {
            Some(true) => 0,
            _ => 1,
        };
    }

    device_op::run_on_all_platforms(runner, app_info, logger, args.verbose, restart_on)
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
                if device_id.is_none() && device_outcome::should_enumerate(&stop) {
                    pb.finish_and_clear();
                    return None;
                }
                let err = device_outcome::error_text(&stop);
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
                let err = device_outcome::error_text(&launch);
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
                if device_id.is_none() && device_outcome::is_no_booted_error(&term) {
                    pb.finish_and_clear();
                    return None;
                }
                // "not running" is fine — we're about to launch it anyway.
                if !device_outcome::is_not_running_error(&term) {
                    let err = device_outcome::error_text(&term);
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
                let err = device_outcome::error_text(&launch);
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

