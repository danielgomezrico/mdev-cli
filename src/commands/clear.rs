use clap::Args;
use colored::Colorize;

use crate::app_detector::AppDetector;
use crate::device_manager::DeviceManager;
use crate::logger::Logger;
use crate::models::{AppInfo, DevicePlatform, ProjectType};
use crate::runner::{RunResult, Runner};

#[derive(Args, Debug)]
pub struct ClearArgs {
    /// Target a specific device by id
    #[arg(short = 'd', long)]
    pub device: Option<String>,

    /// Verbose output
    #[arg(short = 'v', long)]
    pub verbose: bool,
}

pub fn run(args: &ClearArgs, runner: &dyn Runner) -> i32 {
    let logger = Logger::new();
    let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let app_info = AppDetector::new().detect(&current_dir);
    if app_info.project_type == ProjectType::Unknown {
        logger.err("Could not detect Flutter/Android/iOS project in current directory.");
        return 1;
    }

    if let Some(ref device_id) = args.device {
        let platform = infer_platform(device_id);
        return match clear_on(runner, &app_info, platform, Some(device_id), &logger, args.verbose) {
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

fn try_platform(
    runner: &dyn Runner,
    app_info: &AppInfo,
    platform: DevicePlatform,
    logger: &Logger,
    verbose: bool,
) -> bool {
    let first = clear_on(runner, app_info, platform.clone(), None, logger, verbose);
    match first {
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
        if let Some(true) = clear_on(runner, app_info, platform.clone(), Some(&d.id), logger, verbose) {
            ok += 1;
        }
    }
    ok == targets.len()
}

/// Returns Some(true) on success, Some(false) on real failure,
/// None on device-ambiguity error (caller should enumerate).
fn clear_on(
    runner: &dyn Runner,
    app_info: &AppInfo,
    platform: DevicePlatform,
    device_id: Option<&str>,
    logger: &Logger,
    verbose: bool,
) -> Option<bool> {
    match platform {
        DevicePlatform::Android => clear_android(runner, app_info, device_id, logger, verbose),
        DevicePlatform::Ios => clear_ios(runner, app_info, device_id, logger, verbose),
    }
}

fn clear_android(
    runner: &dyn Runner,
    app_info: &AppInfo,
    device_id: Option<&str>,
    logger: &Logger,
    verbose: bool,
) -> Option<bool> {
    let pkg = match &app_info.android_package_id {
        Some(id) => id.clone(),
        None => {
            logger.err("No Android package ID detected.");
            return Some(false);
        }
    };
    let label = device_id.unwrap_or("android").to_string();
    let pb = logger.progress(&format!("Clearing {}...", label));

    let clear_result = if let Some(id) = device_id {
        runner.run("adb", &["-s", id, "shell", "pm", "clear", &pkg], None)
    } else {
        runner.run("adb", &["shell", "pm", "clear", &pkg], None)
    };
    if !clear_result.is_success() {
        if device_id.is_none() && is_multi_device_error(&clear_result) {
            pb.finish_and_clear();
            return None;
        }
        let err = error_text(&clear_result);
        pb.finish_with_message(format!("{} Failed to clear: {} — {}", "✗".red(), label, err));
        if verbose {
            logger.err(err);
        }
        return Some(false);
    }

    let launch_result = if let Some(id) = device_id {
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
    if launch_result.is_success() {
        pb.finish_with_message(format!("{} Cleared and restarted {}", "✓".green(), label));
        Some(true)
    } else {
        let err = error_text(&launch_result);
        pb.finish_with_message(format!(
            "{} Cleared but failed to launch: {} — {}",
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

fn clear_ios(
    runner: &dyn Runner,
    app_info: &AppInfo,
    device_id: Option<&str>,
    logger: &Logger,
    verbose: bool,
) -> Option<bool> {
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
    let pb = logger.progress(&format!("Clearing {}...", label));

    let container_result = runner.run(
        "xcrun",
        &["simctl", "get_app_container", target, &bundle, "data"],
        None,
    );
    if !container_result.is_success() {
        if device_id.is_none() && is_no_booted_error(&container_result) {
            pb.finish_and_clear();
            return None;
        }
        let err = error_text(&container_result);
        pb.finish_with_message(format!(
            "{} Failed to get container: {} — {}",
            "✗".red(),
            label,
            err
        ));
        if verbose {
            logger.err(err);
        }
        return Some(false);
    }

    let container_path = container_result.stdout.trim().to_string();
    let container = std::path::Path::new(&container_path);
    let mut cleared = false;
    if container.exists() {
        if std::fs::remove_dir_all(container).is_ok() {
            let _ = std::fs::create_dir_all(container);
            cleared = true;
        }
    } else {
        cleared = true;
    }

    if !cleared {
        pb.finish_with_message(format!("{} Failed to clear container: {}", "✗".red(), label));
        return Some(false);
    }

    let launch_result = runner.run("xcrun", &["simctl", "launch", target, &bundle], None);
    if launch_result.is_success() {
        pb.finish_with_message(format!("{} Cleared and restarted {}", "✓".green(), label));
        Some(true)
    } else {
        let err = error_text(&launch_result);
        pb.finish_with_message(format!(
            "{} Cleared but failed to launch: {} — {}",
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
    let looks_ios = device_id.len() == 36 && device_id.matches('-').count() == 4;
    if looks_ios { DevicePlatform::Ios } else { DevicePlatform::Android }
}
