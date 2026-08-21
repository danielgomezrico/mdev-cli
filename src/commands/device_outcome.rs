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
    t.contains("delete_failed_internal_error")
        || t.contains("unknown package")
        || t.contains("failed to clear application data")
        || t.trim() == "failed"
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
            "Failure [DELETE_FAILED_INTERNAL_ERROR]",
            ""
        )));
        assert!(is_not_installed_error(&r(
            "",
            "Unknown package: com.example.app"
        )));
        assert!(is_not_installed_error(&r("Failed", "")));
        assert!(is_not_installed_error(&r(
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
