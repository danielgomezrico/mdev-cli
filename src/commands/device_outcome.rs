use crate::runner::RunResult;

pub fn error_text(r: &RunResult) -> &str {
    if !r.stderr.is_empty() {
        &r.stderr
    } else {
        &r.stdout
    }
}

pub fn is_multi_device_error(r: &RunResult) -> bool {
    let t = format!("{}\n{}", r.stderr, r.stdout).to_lowercase();
    t.contains("more than one device")
        || t.contains("more than one emulator")
        || t.contains("multiple devices")
}

/// No Android device is attached at all (`adb: no devices/emulators found`,
/// `error: device not found`, `device offline`).
pub fn is_no_devices_error(r: &RunResult) -> bool {
    let t = format!("{}\n{}", r.stderr, r.stdout).to_lowercase();
    t.contains("no devices/emulators found")
        || t.contains("no devices found")
        || t.contains("no emulators found")
        || t.contains("device not found")
        || t.contains("device offline")
        || t.contains("device unauthorized")
}

/// The command couldn't pick a device on its own: either several are attached
/// or none is. Both cases are resolved by enumerating devices and retrying.
pub fn should_enumerate(r: &RunResult) -> bool {
    is_multi_device_error(r) || is_no_devices_error(r)
}

/// The app simply isn't installed on the device — not a real failure for
/// uninstall/clear, which both want the app gone.
pub fn is_not_installed_error(r: &RunResult) -> bool {
    let t = format!("{}\n{}", r.stderr, r.stdout).to_lowercase();
    // `pm clear` of a missing package prints a bare "Failed".
    t.contains("unknown package") || r.stdout.trim().eq_ignore_ascii_case("failed")
}

/// ADB / `pm` printed a sole success token (install/uninstall/clear).
pub fn stdout_is_success(r: &RunResult) -> bool {
    r.stdout.trim().eq_ignore_ascii_case("success")
}

/// Start the app's launcher activity. `adb shell monkey` exits 251 even when
/// the activity starts, and `am start -p <pkg>` cannot resolve the launcher
/// intent, so resolve the component and start that.
pub fn launch_android(
    runner: &dyn crate::runner::Runner,
    serial: Option<&str>,
    pkg: &str,
) -> crate::runner::RunResult {
    let resolved = adb(
        runner,
        serial,
        &[
            "shell",
            "cmd",
            "package",
            "resolve-activity",
            "--brief",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.LAUNCHER",
            pkg,
        ],
    );
    let component = resolved
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| line.contains('/'))
        .map(str::to_string);
    let Some(component) = component else {
        return crate::runner::RunResult::new(1, resolved.stdout, resolved.stderr);
    };
    adb(runner, serial, &["shell", "am", "start", "-n", &component])
}

fn adb(
    runner: &dyn crate::runner::Runner,
    serial: Option<&str>,
    args: &[&str],
) -> crate::runner::RunResult {
    let mut full: Vec<&str> = Vec::new();
    if let Some(serial) = serial {
        full.extend(["-s", serial]);
    }
    full.extend(args);
    runner.run("adb", &full, None)
}

/// `pm clear` and `pm uninstall` only succeed while the cell radio is off.
/// Enable airplane mode on this serial before either command. Already-on is a
/// no-op. A missing serial means adb has not picked a device yet — skip, so
/// the following command can still report "enumerate" instead of us doing it.
pub fn ensure_airplane_mode(runner: &dyn crate::runner::Runner, serial: Option<&str>) {
    let Some(serial) = serial else {
        return;
    };
    let current = runner.run(
        "adb",
        &[
            "-s",
            serial,
            "shell",
            "settings",
            "get",
            "global",
            "airplane_mode_on",
        ],
        None,
    );
    if current.stdout.replace('\r', "").trim() == "1" {
        return;
    }
    runner.run(
        "adb",
        &[
            "-s",
            serial,
            "shell",
            "cmd",
            "connectivity",
            "airplane-mode",
            "enable",
        ],
        None,
    );
}

pub fn is_no_booted_error(r: &RunResult) -> bool {
    let t = format!("{}\n{}", r.stderr, r.stdout).to_lowercase();
    t.contains("no devices are booted")
        || t.contains("unable to find")
        || t.contains("no matching")
        || t.contains("invalid device")
}

pub fn is_not_running_error(r: &RunResult) -> bool {
    let t = format!("{}\n{}", r.stderr, r.stdout).to_lowercase();
    t.contains("found nothing to terminate")
        || t.contains("no such process")
        || t.contains("not running")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RunResult;

    fn r(stdout: &str, stderr: &str) -> RunResult {
        RunResult::new(0, stdout.to_string(), stderr.to_string())
    }

    #[test]
    fn error_text_prefers_stderr() {
        let res = r("out", "err");
        assert_eq!(error_text(&res), "err");
    }

    #[test]
    fn error_text_falls_back_to_stdout_when_stderr_empty() {
        let res = r("out", "");
        assert_eq!(error_text(&res), "out");
    }

    // GUARD: both fields empty → empty string, no panic
    #[test]
    fn error_text_both_empty() {
        let res = r("", "");
        assert_eq!(error_text(&res), "");
    }

    // GUARD: stderr non-empty AND stdout set → prefers stderr
    #[test]
    fn error_text_prefers_stderr_when_stdout_also_set() {
        let res = r("stdout content", "stderr content");
        assert_eq!(error_text(&res), "stderr content");
    }

    #[test]
    fn is_multi_device_error_true() {
        let res = r("", "error: more than one device attached");
        assert!(is_multi_device_error(&res));
    }

    #[test]
    fn is_multi_device_error_false() {
        let res = r("", "device not found");
        assert!(!is_multi_device_error(&res));
    }

    // GUARD: phrase in stdout only → still matches
    #[test]
    fn is_multi_device_error_detected_in_stdout_only() {
        let res = r("more than one device attached", "");
        assert!(is_multi_device_error(&res));
    }

    // GUARD: UPPERCASE phrase → lowercasing makes it match
    #[test]
    fn is_multi_device_error_case_insensitive() {
        let res = r("", "ERROR: MORE THAN ONE DEVICE ATTACHED");
        assert!(is_multi_device_error(&res));
    }

    // GUARD: phrase embedded inside a larger token → substring match still triggers
    #[test]
    fn is_multi_device_error_phrase_embedded_in_token() {
        let res = r("", "[sdk] more than one device detected at startup");
        assert!(is_multi_device_error(&res));
    }

    // GUARD: empty RunResult → false, no panic
    #[test]
    fn is_multi_device_error_empty_run_result() {
        let res = r("", "");
        assert!(!is_multi_device_error(&res));
    }

    #[test]
    fn is_no_devices_error_matches_adb_phrasing() {
        assert!(is_no_devices_error(&r(
            "",
            "adb: no devices/emulators found"
        )));
        assert!(is_no_devices_error(&r("", "error: device not found")));
        assert!(is_no_devices_error(&r("", "error: device offline")));
    }

    #[test]
    fn is_no_devices_error_false_for_other_failures() {
        assert!(!is_no_devices_error(&r(
            "",
            "Failure [DELETE_FAILED_INTERNAL_ERROR]"
        )));
    }

    #[test]
    fn should_enumerate_covers_both_multi_and_none() {
        assert!(should_enumerate(&r(
            "",
            "adb: more than one device/emulator"
        )));
        assert!(should_enumerate(&r("", "adb: no devices/emulators found")));
        assert!(!should_enumerate(&r(
            "",
            "Failure [DELETE_FAILED_INTERNAL_ERROR]"
        )));
    }

    #[test]
    fn is_not_installed_error_matches_uninstall_and_clear_output() {
        assert!(is_not_installed_error(&r(
            "",
            "Unknown package: com.example.app"
        )));
    }

    #[test]
    fn is_not_installed_error_true_for_unknown_package() {
        assert!(is_not_installed_error(&r(
            "",
            "Unknown package: com.example.app"
        )));
    }

    #[test]
    fn is_not_installed_error_false_for_delete_failed_internal_error() {
        assert!(!is_not_installed_error(&r(
            "Failure [DELETE_FAILED_INTERNAL_ERROR]",
            ""
        )));
    }

    #[test]
    fn is_not_installed_error_true_for_bare_failed() {
        // `pm clear` of a package that is not on the device.
        assert!(is_not_installed_error(&r("Failed", "")));
    }

    #[test]
    fn is_not_installed_error_false_for_failed_to_clear_application_data() {
        assert!(!is_not_installed_error(&r(
            "",
            "Error: Failed to clear application data"
        )));
    }

    #[test]
    fn is_not_installed_error_false_for_device_problems() {
        assert!(!is_not_installed_error(&r(
            "",
            "adb: no devices/emulators found"
        )));
    }

    #[test]
    fn launch_android_starts_resolved_component() {
        use crate::runner::Runner;
        use std::cell::RefCell;

        struct Rec {
            calls: RefCell<Vec<Vec<String>>>,
        }
        impl Runner for Rec {
            fn run(&self, _: &str, args: &[&str], _: Option<&str>) -> RunResult {
                self.calls
                    .borrow_mut()
                    .push(args.iter().map(|a| (*a).to_string()).collect());
                if args.iter().any(|a| *a == "resolve-activity") {
                    return RunResult::new(
                        0,
                        "priority=0\ncom.example.app/.Main".into(),
                        String::new(),
                    );
                }
                RunResult::new(0, "Starting".into(), String::new())
            }
            fn which(&self, _: &str) -> Option<String> {
                None
            }
        }
        let rec = Rec {
            calls: RefCell::new(Vec::new()),
        };
        let result = launch_android(&rec, Some("emulator-5554"), "com.example.app");
        assert!(result.is_success());
        let calls = rec.calls.borrow();
        let start = calls
            .iter()
            .find(|c| c.windows(2).any(|w| w == ["am", "start"]))
            .expect("am start");
        assert!(start.iter().any(|a| a == "com.example.app/.Main"));
    }

    #[test]
    fn launch_android_fails_when_no_component() {
        use crate::runner::Runner;

        struct Rec;
        impl Runner for Rec {
            fn run(&self, _: &str, _: &[&str], _: Option<&str>) -> RunResult {
                RunResult::new(0, "No activity found".into(), String::new())
            }
            fn which(&self, _: &str) -> Option<String> {
                None
            }
        }
        let result = launch_android(&Rec, None, "com.missing");
        assert!(!result.is_success());
    }

    fn ensure_airplane_mode_enables_when_off() {
        use crate::runner::Runner;
        use std::cell::RefCell;

        struct Rec {
            calls: RefCell<Vec<Vec<String>>>,
        }
        impl Runner for Rec {
            fn run(&self, _: &str, args: &[&str], _: Option<&str>) -> RunResult {
                self.calls
                    .borrow_mut()
                    .push(args.iter().map(|a| (*a).to_string()).collect());
                if args.iter().any(|a| *a == "airplane_mode_on") {
                    return RunResult::new(0, "0".into(), String::new());
                }
                RunResult::new(0, String::new(), String::new())
            }
            fn which(&self, _: &str) -> Option<String> {
                None
            }
        }
        let rec = Rec {
            calls: RefCell::new(Vec::new()),
        };
        ensure_airplane_mode(&rec, Some("emulator-5554"));
        let calls = rec.calls.borrow();
        assert!(calls.iter().any(|c| c.iter().any(|a| a == "enable")));
        assert!(calls[0].iter().any(|a| a == "emulator-5554"));
    }

    #[test]
    fn ensure_airplane_mode_skips_enable_when_already_on() {
        use crate::runner::Runner;
        use std::cell::RefCell;

        struct Rec {
            calls: RefCell<Vec<Vec<String>>>,
        }
        impl Runner for Rec {
            fn run(&self, _: &str, args: &[&str], _: Option<&str>) -> RunResult {
                self.calls
                    .borrow_mut()
                    .push(args.iter().map(|a| (*a).to_string()).collect());
                RunResult::new(0, "1".into(), String::new())
            }
            fn which(&self, _: &str) -> Option<String> {
                None
            }
        }
        let rec = Rec {
            calls: RefCell::new(Vec::new()),
        };
        ensure_airplane_mode(&rec, Some("emulator-5554"));
        assert!(rec
            .calls
            .borrow()
            .iter()
            .all(|c| !c.iter().any(|a| a == "enable")));
    }

    #[test]
    fn ensure_airplane_mode_without_serial_is_a_noop() {
        use crate::runner::Runner;
        use std::cell::Cell;

        struct Rec {
            calls: Cell<u32>,
        }
        impl Runner for Rec {
            fn run(&self, _: &str, _: &[&str], _: Option<&str>) -> RunResult {
                self.calls.set(self.calls.get() + 1);
                RunResult::new(0, String::new(), String::new())
            }
            fn which(&self, _: &str) -> Option<String> {
                None
            }
        }
        let rec = Rec {
            calls: Cell::new(0),
        };
        ensure_airplane_mode(&rec, None);
        assert_eq!(rec.calls.get(), 0);
    }

    fn stdout_is_success_true_for_success_ci() {
        assert!(stdout_is_success(&r("Success", "")));
        assert!(stdout_is_success(&r("success", "")));
        assert!(stdout_is_success(&r("SUCCESS", "")));
    }

    #[test]
    fn stdout_is_success_false_for_failure_bracket() {
        assert!(!stdout_is_success(&r(
            "Failure [DELETE_FAILED_INTERNAL_ERROR]",
            ""
        )));
    }

    #[test]
    fn stdout_is_success_false_for_failed() {
        assert!(!stdout_is_success(&r("Failed", "")));
    }

    #[test]
    fn stdout_is_success_false_for_empty() {
        assert!(!stdout_is_success(&r("", "")));
    }

    #[test]
    fn is_no_booted_error_true() {
        let res = r("", "No devices are booted");
        assert!(is_no_booted_error(&res));
    }

    #[test]
    fn is_no_booted_error_false() {
        let res = r("", "device connected");
        assert!(!is_no_booted_error(&res));
    }

    // GUARD: phrase in stdout only
    #[test]
    fn is_no_booted_error_detected_in_stdout_only() {
        let res = r("no devices are booted", "");
        assert!(is_no_booted_error(&res));
    }

    // GUARD: UPPERCASE input → lowercasing makes it match
    #[test]
    fn is_no_booted_error_case_insensitive() {
        let res = r("", "NO DEVICES ARE BOOTED");
        assert!(is_no_booted_error(&res));
    }

    // GUARD: phrase embedded in a larger string
    #[test]
    fn is_no_booted_error_phrase_embedded_in_token() {
        let res = r("", "simulator: no devices are booted right now");
        assert!(is_no_booted_error(&res));
    }

    // GUARD: empty RunResult → false, no panic
    #[test]
    fn is_no_booted_error_empty_run_result() {
        let res = r("", "");
        assert!(!is_no_booted_error(&res));
    }

    #[test]
    fn is_not_running_error_true() {
        let res = r("", "found nothing to terminate");
        assert!(is_not_running_error(&res));
    }

    #[test]
    fn is_not_running_error_false() {
        let res = r("", "process running fine");
        assert!(!is_not_running_error(&res));
    }

    // GUARD: phrase in stdout only
    #[test]
    fn is_not_running_error_detected_in_stdout_only() {
        let res = r("not running", "");
        assert!(is_not_running_error(&res));
    }

    // GUARD: UPPERCASE input → lowercasing makes it match
    #[test]
    fn is_not_running_error_case_insensitive() {
        let res = r("", "NOT RUNNING");
        assert!(is_not_running_error(&res));
    }

    // GUARD: phrase embedded in larger token
    #[test]
    fn is_not_running_error_phrase_embedded_in_token() {
        let res = r("", "app is not running on this device");
        assert!(is_not_running_error(&res));
    }

    // GUARD: empty RunResult → false, no panic
    #[test]
    fn is_not_running_error_empty_run_result() {
        let res = r("", "");
        assert!(!is_not_running_error(&res));
    }
}
