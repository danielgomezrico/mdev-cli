use crate::device_manager::DeviceManager;
use crate::logger::Logger;
use crate::models::{AppInfo, DevicePlatform};
use crate::runner::Runner;

/// Apply `op` to every connected device of `platform`. First tries `op` with no
/// explicit device (fast path); on `None` (device-ambiguity) it enumerates the
/// running devices of that platform and runs `op` on each. Returns true iff every
/// targeted device succeeded. `op(runner, app_info, platform, device_id, logger, verbose)`.
pub fn run_on_platform<F>(
    runner: &dyn Runner,
    app_info: &AppInfo,
    platform: DevicePlatform,
    logger: &Logger,
    verbose: bool,
    op: F,
) -> bool
where
    F: Fn(&dyn Runner, &AppInfo, DevicePlatform, Option<&str>, &Logger, bool) -> Option<bool>,
{
    match op(runner, app_info, platform.clone(), None, logger, verbose) {
        Some(true) => return true,
        Some(false) => return false,
        None => {}
    }

    let devices = DeviceManager::new(runner).list_running_devices();
    let targets: Vec<_> = devices.iter().filter(|d| d.platform == platform).collect();

    if targets.is_empty() {
        logger.warn(&format!("No running {} devices found.", platform.label()));
        return false;
    }

    let mut ok = 0usize;
    for d in &targets {
        if let Some(true) = op(runner, app_info, platform.clone(), Some(&d.id), logger, verbose) {
            ok += 1;
        }
    }
    ok == targets.len()
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

        assert!(result, "expected true when op returns Some(true)");
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

        assert!(!result, "expected false when op returns Some(false)");
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

        assert!(!result, "expected false when no devices found");
        // op is called once (the initial device_id=None attempt), then enumeration
        // finds no devices so op is not called again.
        assert_eq!(call_count.get(), 1, "op called once for the initial attempt");
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

        assert!(result, "single matching device all-success must return true");
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

        assert!(result, "two matching devices both succeeding must return true");
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

        assert!(!result, "partial failure (ok=1, targets=2) must return false");
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

        assert!(
            !result,
            "wrong-platform devices must be filtered out, yielding false"
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

        assert!(result);
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
