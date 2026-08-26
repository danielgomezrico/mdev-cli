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
            if device_id.is_none() && device_outcome::should_enumerate(&result) {
                pb.finish_and_clear();
                None
            } else if device_outcome::stdout_is_success(&result) {
                pb.finish_with_message(format!("{} Uninstalled from {}", "✓".green(), label));
                Some(true)
            } else if device_outcome::is_not_installed_error(&result) {
                pb.finish_with_message(format!("{} Not installed on {}", "✓".green(), label));
                Some(true)
            } else {
                let path_result = if let Some(id) = device_id {
                    runner.run("adb", &["-s", id, "shell", "pm", "path", &pkg], None)
                } else {
                    runner.run("adb", &["shell", "pm", "path", &pkg], None)
                };
                if device_id.is_none() && device_outcome::should_enumerate(&path_result) {
                    pb.finish_and_clear();
                    return None;
                }
                if device_outcome::should_enumerate(&path_result) {
                    let err = device_outcome::error_text(&path_result);
                    pb.finish_with_message(format!("{} Failed: {} — {}", "✗".red(), label, err));
                    if verbose {
                        logger.err(err);
                    }
                    return Some(false);
                }
                if path_result.stdout.to_lowercase().contains("package:") {
                    let err = device_outcome::error_text(&result);
                    pb.finish_with_message(format!("{} Failed: {} — {}", "✗".red(), label, err));
                    if verbose {
                        logger.err(err);
                    }
                    Some(false)
                } else {
                    pb.finish_with_message(format!("{} Not installed on {}", "✓".green(), label));
                    Some(true)
                }
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

    fn failure_stdout() -> RunResult {
        RunResult::new(
            0,
            "Failure [DELETE_FAILED_INTERNAL_ERROR]".into(),
            String::new(),
        )
    }

    fn pm_path_present() -> RunResult {
        RunResult::new(
            0,
            "package:/data/app/com.example.app/base.apk".into(),
            String::new(),
        )
    }

    #[test]
    fn uninstall_on_exit0_failure_stdout_is_not_success() {
        let runner = MockRunner {
            uninstall: failure_stdout(),
            clear: RunResult::new(1, String::new(), "unexpected".into()),
            pm_path: Some(pm_path_present()),
            monkey: None,
        };
        let got = uninstall_on(
            &runner,
            &app(ProjectType::Android),
            DevicePlatform::Android,
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(false));
    }

    #[test]
    fn uninstall_on_exit1_delete_failed_is_not_success() {
        let runner = MockRunner {
            uninstall: RunResult::new(
                1,
                "Failure [DELETE_FAILED_INTERNAL_ERROR]".into(),
                String::new(),
            ),
            clear: RunResult::new(1, String::new(), "unexpected".into()),
            pm_path: Some(pm_path_present()),
            monkey: None,
        };
        let got = uninstall_on(
            &runner,
            &app(ProjectType::Android),
            DevicePlatform::Android,
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(false));
    }

    #[test]
    fn uninstall_on_flutter_appinfo_same_android_arm() {
        let runner = MockRunner {
            uninstall: failure_stdout(),
            clear: RunResult::new(1, String::new(), "unexpected".into()),
            pm_path: Some(pm_path_present()),
            monkey: None,
        };
        let got = uninstall_on(
            &runner,
            &app(ProjectType::Flutter),
            DevicePlatform::Android,
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(false));
    }

    #[test]
    fn uninstall_on_stdout_success_exit0_is_ok() {
        let runner = MockRunner {
            uninstall: RunResult::new(0, "Success".into(), String::new()),
            clear: RunResult::new(1, String::new(), "unexpected".into()),
            pm_path: None,
            monkey: None,
        };
        let got = uninstall_on(
            &runner,
            &app(ProjectType::Android),
            DevicePlatform::Android,
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(true));
    }

    #[test]
    fn uninstall_on_unknown_package_is_idempotent_ok() {
        let runner = MockRunner {
            uninstall: RunResult::new(1, String::new(), "Unknown package: com.example.app".into()),
            clear: RunResult::new(1, String::new(), "unexpected".into()),
            pm_path: None,
            monkey: None,
        };
        let got = uninstall_on(
            &runner,
            &app(ProjectType::Android),
            DevicePlatform::Android,
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(true));
    }

    #[test]
    fn uninstall_on_nonsuccess_pm_path_empty_is_ok() {
        let runner = MockRunner {
            uninstall: failure_stdout(),
            clear: RunResult::new(1, String::new(), "unexpected".into()),
            pm_path: Some(RunResult::new(1, String::new(), String::new())),
            monkey: None,
        };
        let got = uninstall_on(
            &runner,
            &app(ProjectType::Android),
            DevicePlatform::Android,
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(true));
    }

    #[test]
    fn uninstall_on_nonsuccess_pm_path_present_is_fail() {
        let runner = MockRunner {
            uninstall: failure_stdout(),
            clear: RunResult::new(1, String::new(), "unexpected".into()),
            pm_path: Some(pm_path_present()),
            monkey: None,
        };
        let got = uninstall_on(
            &runner,
            &app(ProjectType::Android),
            DevicePlatform::Android,
            Some("emulator-5554"),
            &Logger::new(),
            false,
        );
        assert_eq!(got, Some(false));
    }

    #[test]
    fn uninstall_on_should_enumerate_returns_none() {
        let runner = MockRunner {
            uninstall: RunResult::new(
                1,
                String::new(),
                "error: more than one device attached".into(),
            ),
            clear: RunResult::new(1, String::new(), "unexpected".into()),
            pm_path: None,
            monkey: None,
        };
        let got = uninstall_on(
            &runner,
            &app(ProjectType::Android),
            DevicePlatform::Android,
            None,
            &Logger::new(),
            false,
        );
        assert_eq!(got, None);
    }
}
