use clap::Args;
use colored::Colorize;

use crate::app_detector::AppDetector;
use crate::commands::device_op;
use crate::commands::device_outcome;
use crate::logger::Logger;
use crate::models::{AppInfo, DevicePlatform, ProjectType};
use crate::runner::Runner;

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
        let platform = DevicePlatform::from_device_id(device_id);
        return match clear_on(
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

    device_op::run_on_all_platforms(runner, &app_info, &logger, args.verbose, clear_on)
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
    if device_id.is_none() && device_outcome::should_enumerate(&clear_result) {
        pb.finish_and_clear();
        return None;
    }
    if !device_outcome::stdout_is_success(&clear_result) {
        if device_outcome::is_not_installed_error(&clear_result) {
            pb.finish_with_message(format!("{} Not installed on {}", "✓".green(), label));
            return Some(true);
        }
        let err = device_outcome::error_text(&clear_result);
        pb.finish_with_message(format!(
            "{} Failed to clear: {} — {}",
            "✗".red(),
            label,
            err
        ));
        if verbose {
            logger.err(err);
        }
        return Some(false);
    }

    let launch_result = if let Some(id) = device_id {
        runner.run(
            "adb",
            &[
                "-s",
                id,
                "shell",
                "monkey",
                "-p",
                &pkg,
                "-c",
                "android.intent.category.LAUNCHER",
                "1",
            ],
            None,
        )
    } else {
        runner.run(
            "adb",
            &[
                "shell",
                "monkey",
                "-p",
                &pkg,
                "-c",
                "android.intent.category.LAUNCHER",
                "1",
            ],
            None,
        )
    };
    if launch_result.is_success() {
        pb.finish_with_message(format!("{} Cleared and restarted {}", "✓".green(), label));
        Some(true)
    } else {
        let err = device_outcome::error_text(&launch_result);
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
        if device_id.is_none() && device_outcome::is_no_booted_error(&container_result) {
            pb.finish_and_clear();
            return None;
        }
        let err = device_outcome::error_text(&container_result);
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
        pb.finish_with_message(format!(
            "{} Failed to clear container: {}",
            "✗".red(),
            label
        ));
        return Some(false);
    }

    let launch_result = runner.run("xcrun", &["simctl", "launch", target, &bundle], None);
    if launch_result.is_success() {
        pb.finish_with_message(format!("{} Cleared and restarted {}", "✓".green(), label));
        Some(true)
    } else {
        let err = device_outcome::error_text(&launch_result);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RunResult;

    struct MockRunner {
        uninstall: RunResult,
        clear: RunResult,
        pm_path: Option<RunResult>,
        monkey: Option<RunResult>,
    }

    impl Runner for MockRunner {
        fn run(&self, _exe: &str, args: &[&str], _: Option<&str>) -> RunResult {
            if args.iter().any(|a| *a == "uninstall") {
                return self.uninstall.clone();
            }
            if args.iter().any(|a| *a == "path") {
                return self.pm_path.clone().expect("pm path must not be called");
            }
            if args.windows(2).any(|w| w == ["pm", "clear"]) {
                return self.clear.clone();
            }
            if args.iter().any(|a| *a == "monkey") {
                return self.monkey.clone().expect("monkey must not be called");
            }
            RunResult::new(1, String::new(), "unexpected".into())
        }
        fn which(&self, _: &str) -> Option<String> {
            None
        }
    }

    fn app(pt: ProjectType) -> AppInfo {
        AppInfo::new(String::new(), pt, Some("com.example.app".into()), None)
    }

    #[test]
    fn clear_android_exit1_failed_is_not_success() {
        let runner = MockRunner {
            uninstall: RunResult::new(1, String::new(), "unexpected".into()),
            clear: RunResult::new(1, "Failed".into(), String::new()),
            pm_path: None,
            monkey: None,
        };
        let got = clear_android(
            &runner,
            &app(ProjectType::Android),
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(false));
    }

    #[test]
    fn clear_android_exit0_failed_stdout_is_not_success() {
        let runner = MockRunner {
            uninstall: RunResult::new(1, String::new(), "unexpected".into()),
            clear: RunResult::new(0, "Failed".into(), String::new()),
            pm_path: None,
            monkey: None,
        };
        let got = clear_android(
            &runner,
            &app(ProjectType::Android),
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(false));
    }

    #[test]
    fn clear_android_stdout_success_then_monkey_ok() {
        let runner = MockRunner {
            uninstall: RunResult::new(1, String::new(), "unexpected".into()),
            clear: RunResult::new(0, "Success".into(), String::new()),
            pm_path: None,
            monkey: Some(RunResult::new(0, String::new(), String::new())),
        };
        let got = clear_android(
            &runner,
            &app(ProjectType::Android),
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(true));
    }

    #[test]
    fn clear_android_unknown_package_is_ok() {
        let runner = MockRunner {
            uninstall: RunResult::new(1, String::new(), "unexpected".into()),
            clear: RunResult::new(1, String::new(), "Unknown package: com.example.app".into()),
            pm_path: None,
            monkey: None,
        };
        let got = clear_android(
            &runner,
            &app(ProjectType::Android),
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(true));
    }

    #[test]
    fn clear_android_failed_to_clear_application_data_is_fail() {
        let runner = MockRunner {
            uninstall: RunResult::new(1, String::new(), "unexpected".into()),
            clear: RunResult::new(
                1,
                String::new(),
                "Error: Failed to clear application data".into(),
            ),
            pm_path: None,
            monkey: None,
        };
        let got = clear_android(
            &runner,
            &app(ProjectType::Android),
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(false));
    }

    #[test]
    fn clear_android_should_enumerate_returns_none() {
        let runner = MockRunner {
            uninstall: RunResult::new(1, String::new(), "unexpected".into()),
            clear: RunResult::new(1, String::new(), "adb: no devices/emulators found".into()),
            pm_path: None,
            monkey: None,
        };
        let got = clear_android(
            &runner,
            &app(ProjectType::Android),
            None,
            &Logger::new(),
            false,
        );
        assert_eq!(got, None);
    }
}
