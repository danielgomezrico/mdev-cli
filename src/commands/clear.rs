use clap::Args;
use colored::Colorize;
use std::io::{self, BufRead};

use crate::app_detector::AppDetector;
use crate::device_manager::DeviceManager;
use crate::logger::Logger;
use crate::models::{Device, DevicePlatform, ProjectType};
use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct ClearArgs {
    /// Target a specific device by id
    #[arg(short = 'd', long)]
    pub device: Option<String>,

    /// Clear on all connected devices
    #[arg(short = 'a', long)]
    pub all: bool,

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

    let device_manager = DeviceManager::new(runner);
    let devices = device_manager.list_running_devices();
    if devices.is_empty() {
        logger.warn("No running devices or emulators found.");
        return 1;
    }

    let selected = select_devices(&devices, args, &logger);
    if selected.is_empty() {
        logger.warn("No devices selected.");
        return 1;
    }

    let total = selected.len();
    let mut success_count = 0usize;

    for device in &selected {
        match device.platform {
            DevicePlatform::Ios => {
                if cfg!(target_os = "linux") {
                    logger.warn(&format!(
                        "Skipping iOS device '{}' — not supported on Linux.",
                        device.name
                    ));
                    continue;
                }
                let bundle_id = match &app_info.ios_bundle_id {
                    Some(id) => id.clone(),
                    None => {
                        logger.err(&format!(
                            "No iOS bundle ID detected, cannot clear '{}'.",
                            device.name
                        ));
                        continue;
                    }
                };
                let pb = logger.progress(&format!("Clearing {}...", device.name));
                // Get container path
                let container_result = runner.run(
                    "xcrun",
                    &["simctl", "get_app_container", &device.id, &bundle_id, "data"],
                    None,
                );
                if !container_result.is_success() {
                    let err = if !container_result.stderr.is_empty() {
                        &container_result.stderr
                    } else {
                        &container_result.stdout
                    };
                    pb.finish_with_message(format!(
                        "{} Failed to get container: {} — {}",
                        "✗".red(),
                        device.name,
                        err
                    ));
                    continue;
                }
                let container_path = container_result.stdout.trim().to_string();
                // Delete container contents and recreate
                let container = std::path::Path::new(&container_path);
                let mut cleared = false;
                if container.exists() {
                    if std::fs::remove_dir_all(container).is_ok() {
                        let _ = std::fs::create_dir_all(container);
                        cleared = true;
                    }
                } else {
                    cleared = true; // nothing to clear
                }

                if cleared {
                    // Relaunch the app
                    let launch_result = runner.run(
                        "xcrun",
                        &["simctl", "launch", &device.id, &bundle_id],
                        None,
                    );
                    if launch_result.is_success() {
                        pb.finish_with_message(format!(
                            "{} Cleared and restarted {}",
                            "✓".green(),
                            device.name
                        ));
                        success_count += 1;
                    } else {
                        let err = if !launch_result.stderr.is_empty() {
                            &launch_result.stderr
                        } else {
                            &launch_result.stdout
                        };
                        pb.finish_with_message(format!(
                            "{} Cleared but failed to launch: {} — {}",
                            "✗".red(),
                            device.name,
                            err
                        ));
                        if args.verbose {
                            logger.err(err);
                        }
                    }
                } else {
                    pb.finish_with_message(format!(
                        "{} Failed to clear container: {}",
                        "✗".red(),
                        device.name
                    ));
                }
            }
            DevicePlatform::Android => {
                let package_id = match &app_info.android_package_id {
                    Some(id) => id.clone(),
                    None => {
                        logger.err(&format!(
                            "No Android package ID detected, cannot clear '{}'.",
                            device.name
                        ));
                        continue;
                    }
                };
                let pb = logger.progress(&format!("Clearing {}...", device.name));
                let clear_result = runner.run(
                    "adb",
                    &["-s", &device.id, "shell", "pm", "clear", &package_id],
                    None,
                );
                if !clear_result.is_success() {
                    let err = if !clear_result.stderr.is_empty() {
                        &clear_result.stderr
                    } else {
                        &clear_result.stdout
                    };
                    pb.finish_with_message(format!(
                        "{} Failed to clear: {} — {}",
                        "✗".red(),
                        device.name,
                        err
                    ));
                    if args.verbose {
                        logger.err(err);
                    }
                    continue;
                }
                // Launch app via monkey
                let launch_result = runner.run(
                    "adb",
                    &[
                        "-s",
                        &device.id,
                        "shell",
                        "monkey",
                        "-p",
                        &package_id,
                        "-c",
                        "android.intent.category.LAUNCHER",
                        "1",
                    ],
                    None,
                );
                if launch_result.is_success() {
                    pb.finish_with_message(format!(
                        "{} Cleared and restarted {}",
                        "✓".green(),
                        device.name
                    ));
                    success_count += 1;
                } else {
                    let err = if !launch_result.stderr.is_empty() {
                        &launch_result.stderr
                    } else {
                        &launch_result.stdout
                    };
                    pb.finish_with_message(format!(
                        "{} Cleared but failed to launch: {} — {}",
                        "✗".red(),
                        device.name,
                        err
                    ));
                    if args.verbose {
                        logger.err(err);
                    }
                }
            }
        }
    }

    logger.info(&format!(
        "Cleared and restarted {}/{} devices.",
        success_count, total
    ));
    if success_count < total { 1 } else { 0 }
}

fn select_devices<'a>(
    devices: &'a [Device],
    args: &ClearArgs,
    logger: &Logger,
) -> Vec<&'a Device> {
    if let Some(ref device_id) = args.device {
        let found: Vec<&Device> = devices.iter().filter(|d| d.id == *device_id).collect();
        if found.is_empty() {
            logger.warn(&format!("Device '{}' not found in running devices.", device_id));
        }
        return found;
    }

    if args.all {
        return devices.iter().collect();
    }

    // Interactive selection
    logger.info("Select devices:");
    logger.info("  0. All devices");
    for (i, d) in devices.iter().enumerate() {
        logger.info(&format!("  {}. {} ({})", i + 1, d.name, d.id));
    }
    logger.info("Enter numbers separated by commas (e.g. 1,3):");

    let stdin = io::stdin();
    let line = stdin
        .lock()
        .lines()
        .next()
        .unwrap_or(Ok(String::new()))
        .unwrap_or_default();

    let mut selected = Vec::new();
    for token in line.split(',') {
        let token = token.trim();
        match token.parse::<usize>() {
            Ok(0) => return devices.iter().collect(),
            Ok(n) if n >= 1 && n <= devices.len() => {
                selected.push(&devices[n - 1]);
            }
            _ => {}
        }
    }
    selected
}
