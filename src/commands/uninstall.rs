use clap::Args;
use colored::Colorize;

use crate::app_detector::AppDetector;
use crate::device_manager::DeviceManager;
use crate::logger::Logger;
use crate::models::{AppInfo, DevicePlatform, ProjectType};
use crate::runner::{RunResult, Runner};

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
        let platform = infer_platform(device_id);
        return match uninstall_on(runner, &app_info, platform, Some(device_id), &logger, args.verbose) {
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
        if !try_platform(runner, &app_info, DevicePlatform::Android, &logger, args.verbose) {
            any_fail = true;
        }
    }

    if has_ios {
        any_attempt = true;
        if !try_platform(runner, &app_info, DevicePlatform::Ios, &logger, args.verbose) {
            any_fail = true;
        }
    }

    if !any_attempt {
        logger.err("No Android package ID or iOS bundle ID detected.");
        return 1;
    }

    if any_fail { 1 } else { 0 }
}

/// Try the direct (no-device) uninstall first. On multi-device error, enumerate
/// and run on each device of that platform. Returns true if at least one target
/// succeeded and none failed.
fn try_platform(
    runner: &dyn Runner,
    app_info: &AppInfo,
    platform: DevicePlatform,
    logger: &Logger,
    verbose: bool,
) -> bool {
    // First attempt: no specific device.
    let first = uninstall_on(runner, app_info, platform.clone(), None, logger, verbose);
    match first {
        Some(true) => return true,
        Some(false) => return false, // real failure, not ambiguity
        None => {} // multi-device / no-booted — fall through to enumerate
    }

    // Enumerate and run on each device of this platform.
    let devices = DeviceManager::new(runner).list_running_devices();
    let targets: Vec<_> = devices
        .iter()
        .filter(|d| d.platform == platform)
        .collect();

    if targets.is_empty() {
        logger.warn(&format!(
            "No running {} devices found.",
            platform_label(&platform)
        ));
        return false;
    }

    let mut ok = 0usize;
    for d in &targets {
        match uninstall_on(runner, app_info, platform.clone(), Some(&d.id), logger, verbose) {
            Some(true) => ok += 1,
            _ => {}
        }
    }
    ok == targets.len()
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
            } else if device_id.is_none() && is_multi_device_error(&result) {
                pb.finish_and_clear();
                None
            } else {
                let err = error_text(&result);
                pb.finish_with_message(format!(
                    "{} Failed: {} — {}",
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
            let label = device_id.unwrap_or("booted simulator");
            let pb = logger.progress(&format!("Uninstalling from {}...", label));
            let result = runner.run("xcrun", &["simctl", "uninstall", target, &bundle], None);
            if result.is_success() {
                pb.finish_with_message(format!("{} Uninstalled from {}", "✓".green(), label));
                Some(true)
            } else if device_id.is_none() && is_no_booted_error(&result) {
                pb.finish_and_clear();
                None
            } else {
                let err = error_text(&result);
                pb.finish_with_message(format!(
                    "{} Failed: {} — {}",
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

fn platform_label(p: &DevicePlatform) -> &'static str {
    match p {
        DevicePlatform::Android => "Android",
        DevicePlatform::Ios => "iOS",
    }
}

fn infer_platform(device_id: &str) -> DevicePlatform {
    // iOS simulator UDIDs are UUID-like: 8-4-4-4-12 hex with dashes.
    let looks_ios = device_id.len() == 36 && device_id.matches('-').count() == 4;
    if looks_ios { DevicePlatform::Ios } else { DevicePlatform::Android }
}
