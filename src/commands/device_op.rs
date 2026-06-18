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
    use std::cell::Cell;

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
}
