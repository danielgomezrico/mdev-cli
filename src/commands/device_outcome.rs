use crate::runner::RunResult;

pub fn error_text(r: &RunResult) -> &str {
    if !r.stderr.is_empty() { &r.stderr } else { &r.stdout }
}

pub fn is_multi_device_error(r: &RunResult) -> bool {
    let t = format!("{}\n{}", r.stderr, r.stdout).to_lowercase();
    t.contains("more than one device")
        || t.contains("more than one emulator")
        || t.contains("multiple devices")
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
