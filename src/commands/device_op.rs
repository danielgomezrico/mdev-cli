use crate::device_manager::DeviceManager;
use crate::logger::Logger;
use crate::models::{AppInfo, DevicePlatform};
use crate::runner::Runner;

/// Result of running an operation across every device of one platform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlatformOutcome {
    /// Every targeted device succeeded.
    AllOk,
    /// At least one targeted device failed.
    Failed,
    /// The platform has no running devices — nothing was attempted.
    NoDevices,
}

/// Apply `op` to every connected device of `platform`. First tries `op` with no
/// explicit device (fast path); on `None` (op couldn't pick a device on its own)
/// it enumerates the running devices of that platform and runs `op` on each.
/// `op(runner, app_info, platform, device_id, logger, verbose)`.
pub fn run_on_platform<F>(
    runner: &dyn Runner,
    app_info: &AppInfo,
    platform: DevicePlatform,
    logger: &Logger,
    verbose: bool,
    op: F,
) -> PlatformOutcome
where
    F: Fn(&dyn Runner, &AppInfo, DevicePlatform, Option<&str>, &Logger, bool) -> Option<bool>,
{
    match op(runner, app_info, platform.clone(), None, logger, verbose) {
        Some(true) => return PlatformOutcome::AllOk,
        Some(false) => return PlatformOutcome::Failed,
        None => {}
    }

    let devices = DeviceManager::new(runner).list_running_devices();
    let targets: Vec<_> = devices.iter().filter(|d| d.platform == platform).collect();

    if targets.is_empty() {
        return PlatformOutcome::NoDevices;
    }

    let mut ok = 0usize;
    for d in &targets {
        if let Some(true) = op(runner, app_info, platform.clone(), Some(&d.id), logger, verbose) {
            ok += 1;
        }
    }
    if ok == targets.len() { PlatformOutcome::AllOk } else { PlatformOutcome::Failed }
}

/// Run `op` on every platform the project targets (Android when it has a
/// package id, iOS when it has a bundle id and we're not on Linux) and turn the
/// per-platform outcomes into a process exit code.
///
/// A platform with no running devices is skipped, not failed: a Flutter project
/// with only a booted simulator must not report an Android error. Only when no
/// platform has any device does the whole run report a problem.
pub fn run_on_all_platforms<F>(
    runner: &dyn Runner,
    app_info: &AppInfo,
    logger: &Logger,
    verbose: bool,
    op: F,
) -> i32
where
    F: Fn(&dyn Runner, &AppInfo, DevicePlatform, Option<&str>, &Logger, bool) -> Option<bool> + Copy,
{
    let mut platforms: Vec<DevicePlatform> = Vec::new();
    if app_info.android_package_id.is_some() {
        platforms.push(DevicePlatform::Android);
    }
    if app_info.ios_bundle_id.is_some() && !cfg!(target_os = "linux") {
        platforms.push(DevicePlatform::Ios);
    }

    if platforms.is_empty() {
        logger.err("No Android package ID or iOS bundle ID detected.");
        return 1;
    }

    let outcomes: Vec<(DevicePlatform, PlatformOutcome)> = platforms
        .into_iter()
        .map(|p| {
            let outcome = run_on_platform(runner, app_info, p.clone(), logger, verbose, op);
            (p, outcome)
        })
        .collect();

    let any_device = outcomes
        .iter()
        .any(|(_, o)| *o != PlatformOutcome::NoDevices);

    if !any_device {
        let labels: Vec<&str> = outcomes.iter().map(|(p, _)| p.label()).collect();
        logger.warn(&format!("No running {} devices found.", labels.join(" or ")));
        return 1;
    }

    for (platform, outcome) in &outcomes {
        if *outcome == PlatformOutcome::NoDevices {
            logger.detail(&format!("No running {} devices — skipped.", platform.label()));
        }
    }

    if outcomes.iter().any(|(_, o)| *o == PlatformOutcome::Failed) { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProjectType;
    use crate::runner::RunResult;
    use std::cell::{Cell, RefCell};

    struct MockRunner {
        run_result: RunResult,
    }

    impl Runner for MockRunner {
        fn run(&self, _executable: &str, _args: &[&str], _working_dir: Option<&str>) -> RunResult {
            self.run_result.clone()
        }

        fn which(&self, _executable: &str) -> Option<String> {
            None
        }
    }

    /// Returns `flutter_json` for `flutter devices --machine` and fails for all
    /// other commands (including `xcrun simctl …`) so only the crafted JSON
    /// contributes devices.
    struct FlutterDevicesMockRunner {
        flutter_json: String,
    }

    impl FlutterDevicesMockRunner {
        fn new(flutter_json: impl Into<String>) -> Self {
            Self {
                flutter_json: flutter_json.into(),
            }
        }

        fn android_device(id: &str, name: &str) -> serde_json::Value {
            serde_json::json!({
                "id": id,
                "name": name,
                "targetPlatform": "android-x64",
                "emulator": true
            })
        }

        fn ios_device(id: &str, name: &str) -> serde_json::Value {
            serde_json::json!({
                "id": id,
                "name": name,
                "targetPlatform": "ios",
                "emulator": false
            })
        }
    }

    impl Runner for FlutterDevicesMockRunner {
        fn run(
            &self,
            executable: &str,
            _args: &[&str],
            _working_dir: Option<&str>,
        ) -> RunResult {
            if executable == "flutter" {
                RunResult::new(0, self.flutter_json.clone(), String::new())
            } else {
                // xcrun simctl and everything else → failure so no extra devices
                RunResult::new(1, String::new(), String::new())
            }
        }

        fn which(&self, _executable: &str) -> Option<String> {
            None
        }
    }

    fn make_app_info() -> AppInfo {
        AppInfo::new(
            String::new(),
            ProjectType::Android,
            Some("com.example.app".to_string()),
            Some("com.example.app".to_string()),
        )
    }

    #[test]
    fn op_returns_some_true_on_first_call_returns_true() {
        let runner = MockRunner {
            run_result: RunResult::new(0, String::new(), String::new()),
        };
        let app_info = make_app_info();
        let logger = Logger::new();
        let call_count = Cell::new(0u32);

        let result = run_on_platform(
            &runner,
            &app_info,
            DevicePlatform::Android,
            &logger,
            false,
            |_runner, _app, _platform, _device_id, _logger, _verbose| {
                call_count.set(call_count.get() + 1);
                Some(true)
            },
        );

        assert_eq!(result, PlatformOutcome::AllOk, "expected AllOk when op returns Some(true)");
        assert_eq!(call_count.get(), 1, "op must be called exactly once");
    }

    #[test]
    fn op_returns_some_false_on_first_call_returns_false() {
        let runner = MockRunner {
            run_result: RunResult::new(0, String::new(), String::new()),
        };
        let app_info = make_app_info();
        let logger = Logger::new();
        let call_count = Cell::new(0u32);

        let result = run_on_platform(
            &runner,
            &app_info,
            DevicePlatform::Android,
            &logger,
            false,
            |_runner, _app, _platform, _device_id, _logger, _verbose| {
                call_count.set(call_count.get() + 1);
                Some(false)
            },
        );

        assert_eq!(result, PlatformOutcome::Failed, "expected Failed when op returns Some(false)");
        assert_eq!(call_count.get(), 1, "op must be called exactly once");
    }

    #[test]
    fn op_returns_none_and_no_devices_returns_false() {
        // Runner returns failure for all commands → DeviceManager yields empty list.
        let runner = MockRunner {
            run_result: RunResult::new(1, String::new(), String::new()),
        };
        let app_info = make_app_info();
        let logger = Logger::new();
        let call_count = Cell::new(0u32);

        let result = run_on_platform(
            &runner,
            &app_info,
            DevicePlatform::Android,
            &logger,
            false,
            |_runner, _app, _platform, _device_id, _logger, _verbose| {
                call_count.set(call_count.get() + 1);
                None
            },
        );

        assert_eq!(result, PlatformOutcome::NoDevices, "expected NoDevices when none are running");
        // op is called once (the initial device_id=None attempt), then enumeration
        // finds no devices so op is not called again.
        assert_eq!(call_count.get(), 1, "op called once for the initial attempt");
    }

    // ── run_on_all_platforms: a device-less platform is skipped, not failed ──

    fn dead_runner() -> MockRunner {
        // Every command fails → DeviceManager enumerates nothing.
        MockRunner {
            run_result: RunResult::new(1, String::new(), String::new()),
        }
    }

    fn app_info_with(android: Option<&str>, ios: Option<&str>) -> AppInfo {
        AppInfo::new(
            String::new(),
            ProjectType::Flutter,
            android.map(|s| s.to_string()),
            ios.map(|s| s.to_string()),
        )
    }

    #[test]
    fn all_platforms_no_ids_errors() {
        let runner = dead_runner();
        let logger = Logger::new();
        let code = run_on_all_platforms(
            &runner,
            &app_info_with(None, None),
            &logger,
            false,
            |_r, _a, _p, _d, _l, _v| Some(true),
        );
        assert_eq!(code, 1, "no package/bundle id must be an error");
    }

    #[test]
    fn all_platforms_no_devices_anywhere_returns_error_code() {
        let runner = dead_runner();
        let logger = Logger::new();
        let code = run_on_all_platforms(
            &runner,
            &app_info_with(Some("com.example.app"), None),
            &logger,
            false,
            |_r, _a, _p, _d, _l, _v| None,
        );
        assert_eq!(code, 1, "nothing was attempted anywhere → error");
    }

    #[test]
    fn all_platforms_android_without_devices_does_not_fail_ios_success() {
        // The reported bug: a Flutter project with only a booted simulator
        // printed an Android failure and exited 1.
        let runner = dead_runner();
        let logger = Logger::new();
        let code = run_on_all_platforms(
            &runner,
            &app_info_with(Some("com.example.app"), Some("com.example.app")),
            &logger,
            false,
            |_r, _a, platform, _d, _l, _v| match platform {
                DevicePlatform::Android => None, // no Android device → enumerate → none
                DevicePlatform::Ios => Some(true),
            },
        );
        assert_eq!(code, 0, "device-less Android must be skipped, not failed");
    }

    #[test]
    fn all_platforms_real_failure_still_returns_error_code() {
        let runner = dead_runner();
        let logger = Logger::new();
        let code = run_on_all_platforms(
            &runner,
            &app_info_with(Some("com.example.app"), Some("com.example.app")),
            &logger,
            false,
            |_r, _a, platform, _d, _l, _v| match platform {
                DevicePlatform::Android => Some(false), // real failure on a live device
                DevicePlatform::Ios => Some(true),
            },
        );
        assert_eq!(code, 1, "a real per-device failure must still exit non-zero");
    }

    // ── Enumeration-path regression locks ────────────────────────────────────

    #[test]
    fn none_one_matching_device_op_true_returns_true() {
        let json =
            serde_json::json!([FlutterDevicesMockRunner::android_device("emulator-5554", "Pixel")])
                .to_string();
        let runner = FlutterDevicesMockRunner::new(json);
        let app_info = make_app_info();
        let logger = Logger::new();
        let call_count = Cell::new(0u32);

        let result = run_on_platform(
            &runner,
            &app_info,
            DevicePlatform::Android,
            &logger,
            false,
            |_r, _a, _p, _device_id, _l, _v| {
                call_count.set(call_count.get() + 1);
                // First call (device_id=None) → None to trigger enumeration.
                // Subsequent calls (with a device id) → Some(true).
                if call_count.get() == 1 {
                    None
                } else {
                    Some(true)
                }
            },
        );

        assert_eq!(result, PlatformOutcome::AllOk, "single matching device all-success must be AllOk");
        assert_eq!(call_count.get(), 2, "op called twice: None first, then device id");
    }

    #[test]
    fn none_two_matching_devices_all_true_returns_true() {
        let json = serde_json::json!([
            FlutterDevicesMockRunner::android_device("emulator-5554", "Pixel"),
            FlutterDevicesMockRunner::android_device("emulator-5556", "Nexus"),
        ])
        .to_string();
        let runner = FlutterDevicesMockRunner::new(json);
        let app_info = make_app_info();
        let logger = Logger::new();
        let call_count = Cell::new(0u32);

        let result = run_on_platform(
            &runner,
            &app_info,
            DevicePlatform::Android,
            &logger,
            false,
            |_r, _a, _p, _device_id, _l, _v| {
                call_count.set(call_count.get() + 1);
                if call_count.get() == 1 {
                    None
                } else {
                    Some(true)
                }
            },
        );

        assert_eq!(result, PlatformOutcome::AllOk, "two matching devices both succeeding must be AllOk");
        assert_eq!(call_count.get(), 3, "op called 3×: None + two device ids");
    }

    #[test]
    fn none_two_matching_devices_partial_failure_returns_false() {
        let json = serde_json::json!([
            FlutterDevicesMockRunner::android_device("emulator-5554", "Pixel"),
            FlutterDevicesMockRunner::android_device("emulator-5556", "Nexus"),
        ])
        .to_string();
        let runner = FlutterDevicesMockRunner::new(json);
        let app_info = make_app_info();
        let logger = Logger::new();
        let call_count = Cell::new(0u32);

        let result = run_on_platform(
            &runner,
            &app_info,
            DevicePlatform::Android,
            &logger,
            false,
            |_r, _a, _p, _device_id, _l, _v| {
                call_count.set(call_count.get() + 1);
                match call_count.get() {
                    1 => None,          // trigger enumeration
                    2 => Some(true),    // first device succeeds
                    _ => Some(false),   // second device fails → partial failure
                }
            },
        );

        assert_eq!(result, PlatformOutcome::Failed, "partial failure (ok=1, targets=2) must be Failed");
    }

    #[test]
    fn none_wrong_platform_devices_only_returns_false() {
        // Only an iOS device present; requesting Android → targets empty after filter.
        let json = serde_json::json!([
            FlutterDevicesMockRunner::ios_device("00008101-000A1234", "iPhone 14"),
        ])
        .to_string();
        let runner = FlutterDevicesMockRunner::new(json);
        let app_info = make_app_info();
        let logger = Logger::new();
        let call_count = Cell::new(0u32);

        let result = run_on_platform(
            &runner,
            &app_info,
            DevicePlatform::Android,
            &logger,
            false,
            |_r, _a, _p, _device_id, _l, _v| {
                call_count.set(call_count.get() + 1);
                if call_count.get() == 1 {
                    None
                } else {
                    Some(true)
                }
            },
        );

        assert_eq!(
            result,
            PlatformOutcome::NoDevices,
            "wrong-platform devices must be filtered out, yielding NoDevices"
        );
        // op called once (None probe), then no matching device → not called again
        assert_eq!(call_count.get(), 1, "op not called a second time when no targets");
    }

    #[test]
    fn none_single_device_op_receives_correct_device_id() {
        let json =
            serde_json::json!([FlutterDevicesMockRunner::android_device("emulator-5554", "Pixel")])
                .to_string();
        let runner = FlutterDevicesMockRunner::new(json);
        let app_info = make_app_info();
        let logger = Logger::new();

        let call_count = Cell::new(0u32);
        let received_ids: RefCell<Vec<Option<String>>> = RefCell::new(Vec::new());

        let result = run_on_platform(
            &runner,
            &app_info,
            DevicePlatform::Android,
            &logger,
            false,
            |_r, _a, _p, device_id, _l, _v| {
                call_count.set(call_count.get() + 1);
                received_ids
                    .borrow_mut()
                    .push(device_id.map(|s| s.to_owned()));
                if call_count.get() == 1 {
                    None
                } else {
                    Some(true)
                }
            },
        );

        assert_eq!(result, PlatformOutcome::AllOk);
        let ids = received_ids.borrow();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], None, "first call must pass None (fast-path probe)");
        assert_eq!(
            ids[1].as_deref(),
            Some("emulator-5554"),
            "second call must pass the device id from flutter JSON"
        );
    }
}
