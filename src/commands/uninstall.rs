use clap::Args;
use colored::Colorize;

use crate::app_detector::AppDetector;
use crate::commands::device_op;
use crate::commands::device_outcome;
use crate::logger::Logger;
use crate::models::{AppInfo, DevicePlatform, ProjectType};
use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Target a specific device by id
    #[arg(short = 'd', long)]
    pub device: Option<String>,

    /// Verbose output
    #[arg(short = 'v', long)]
    pub verbose: bool,
}

pub fn run(args: &UninstallArgs, runner: &dyn Runner) -> i32 {
    let logger = Logger::new();
    let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let app_info = AppDetector::new().detect(&current_dir);
    if app_info.project_type == ProjectType::Unknown {
        logger.err("Could not detect Flutter/Android/iOS project in current directory.");
        return 1;
    }

    // If a specific device was requested, run against it directly.
    if let Some(ref device_id) = args.device {
        let platform = DevicePlatform::from_device_id(device_id);
        return match uninstall_on(
            runner,
            &app_info,
            platform,
            Some(device_id),
            &logger,
            args.verbose,
        ) {
            Some(true) => 0,
            _ => 1,
        };
    }

    device_op::run_on_all_platforms(runner, &app_info, &logger, args.verbose, uninstall_on)
}

/// Run a single uninstall. Returns:
///   Some(true)  = success
///   Some(false) = failed for a reason other than device ambiguity
///   None        = ambiguous (multiple devices / no booted) — caller should enumerate
fn uninstall_on(
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
            let label = device_id.unwrap_or("android");
            let pb = logger.progress(&format!("Uninstalling from {}...", label));
            let result = if let Some(id) = device_id {
                runner.run("adb", &["-s", id, "uninstall", &pkg], None)
            } else {
                runner.run("adb", &["uninstall", &pkg], None)
            };
            if result.is_success() {
                pb.finish_with_message(format!("{} Uninstalled from {}", "✓".green(), label));
                Some(true)
            } else if device_id.is_none() && device_outcome::should_enumerate(&result) {
                pb.finish_and_clear();
                None
            } else if device_outcome::is_not_installed_error(&result) {
                pb.finish_with_message(format!("{} Not installed on {}", "✓".green(), label));
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
            let label = device_id.unwrap_or("booted simulator");
            let pb = logger.progress(&format!("Uninstalling from {}...", label));
            let result = runner.run("xcrun", &["simctl", "uninstall", target, &bundle], None);
            if result.is_success() {
                pb.finish_with_message(format!("{} Uninstalled from {}", "✓".green(), label));
                Some(true)
            } else if device_id.is_none() && device_outcome::is_no_booted_error(&result) {
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
    }
}
