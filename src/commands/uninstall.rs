use clap::Args;
use colored::Colorize;
use std::io::{self, BufRead};

use crate::app_detector::AppDetector;
use crate::device_manager::DeviceManager;
use crate::logger::Logger;
use crate::models::{Device, DevicePlatform, ProjectType};
use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Target a specific device by id
    #[arg(short = 'd', long)]
    pub device: Option<String>,

    /// Uninstall from all connected devices
    #[arg(short = 'a', long)]
    pub all: bool,

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
                            "No iOS bundle ID detected, cannot uninstall from '{}'.",
                            device.name
                        ));
                        continue;
                    }
                };
                let pb = logger.progress(&format!("Uninstalling from {}...", device.name));
                let result = runner.run(
                    "xcrun",
                    &["simctl", "uninstall", &device.id, &bundle_id],
                    None,
                );
                if result.is_success() {
                    pb.finish_with_message(format!(
                        "{} Uninstalled from {}",
                        "✓".green(),
                        device.name
                    ));
                    success_count += 1;
                } else {
                    let err = if !result.stderr.is_empty() { &result.stderr } else { &result.stdout };
                    pb.finish_with_message(format!(
                        "{} Failed: {} — {}",
                        "✗".red(),
                        device.name,
                        err
                    ));
                    if args.verbose {
                        logger.err(err);
                    }
                }
            }
            DevicePlatform::Android => {
                let package_id = match &app_info.android_package_id {
                    Some(id) => id.clone(),
                    None => {
                        logger.err(&format!(
                            "No Android package ID detected, cannot uninstall from '{}'.",
                            device.name
                        ));
                        continue;
                    }
                };
                let pb = logger.progress(&format!("Uninstalling from {}...", device.name));
                let result = runner.run(
                    "adb",
                    &["-s", &device.id, "uninstall", &package_id],
                    None,
                );
                if result.is_success() {
                    pb.finish_with_message(format!(
                        "{} Uninstalled from {}",
                        "✓".green(),
                        device.name
                    ));
                    success_count += 1;
                } else {
                    let err = if !result.stderr.is_empty() { &result.stderr } else { &result.stdout };
                    pb.finish_with_message(format!(
                        "{} Failed: {} — {}",
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

    logger.info(&format!("Uninstalled from {}/{} devices.", success_count, total));
    if success_count < total { 1 } else { 0 }
}

fn select_devices<'a>(
    devices: &'a [Device],
    args: &UninstallArgs,
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
