use colored::Colorize;
use std::path::PathBuf;

use crate::logger::Logger;
use crate::runner::Runner;

fn check_pass(msg: &str) {
    println!("{} {}", "✓".green(), msg);
}

fn check_fail(msg: &str) {
    eprintln!("{} {}", "✗".red(), msg);
}

fn check_warn(msg: &str) {
    println!("{} {}", "!".yellow(), msg);
}

fn check_skip(msg: &str) {
    println!("{} {}", "~".dimmed(), msg);
}

pub fn run(runner: &dyn Runner) -> i32 {
    let logger = Logger::new();
    logger.info(&format!("{}", "mdev doctor".cyan().bold()));
    logger.info(&"─".repeat(50));
    logger.info("");

    let mut pass_count = 0usize;
    let mut fail_count = 0usize;
    const TOTAL: usize = 10;

    // 1. Flutter version
    {
        let result = runner.run("flutter", &["--version", "--machine"], None);
        if result.is_success() && !result.stdout.is_empty() {
            // Parse frameworkVersion from JSON
            let version = parse_flutter_version(&result.stdout)
                .unwrap_or_else(|| "unknown".to_string());
            check_pass(&format!("Flutter SDK: {}", version));
            pass_count += 1;
        } else {
            check_fail("Flutter SDK not found. Install from https://flutter.dev");
            fail_count += 1;
        }
    }

    // 2. adb version
    {
        let adb = find_adb(runner);
        if let Some(ref adb_path) = adb {
            let result = runner.run(adb_path, &["version"], None);
            if result.is_success() {
                let first_line = result.stdout.lines().next().unwrap_or("").to_string();
                check_pass(&format!("adb: {}", first_line));
                pass_count += 1;
            } else {
                check_fail("adb found but failed to run.");
                fail_count += 1;
            }
        } else {
            check_fail("adb not found. Install Android SDK platform-tools.");
            fail_count += 1;
        }
    }

    // 3. $ANDROID_HOME set and directory exists
    {
        match std::env::var("ANDROID_HOME") {
            Ok(android_home) => {
                let p = PathBuf::from(&android_home);
                if p.exists() && p.is_dir() {
                    check_pass(&format!("$ANDROID_HOME set and exists: {}", android_home));
                    pass_count += 1;
                } else {
                    check_fail(&format!(
                        "$ANDROID_HOME is set to '{}' but directory does not exist.",
                        android_home
                    ));
                    fail_count += 1;
                }
            }
            Err(_) => {
                check_fail("$ANDROID_HOME is not set.");
                fail_count += 1;
            }
        }
    }

    // 4. $ANDROID_HOME/licenses dir exists and non-empty
    {
        match std::env::var("ANDROID_HOME") {
            Ok(android_home) => {
                let licenses_dir = PathBuf::from(&android_home).join("licenses");
                if licenses_dir.exists() && licenses_dir.is_dir() {
                    let count = std::fs::read_dir(&licenses_dir)
                        .map(|rd| rd.count())
                        .unwrap_or(0);
                    if count > 0 {
                        check_pass("Android SDK licenses accepted.");
                        pass_count += 1;
                    } else {
                        check_fail("Android SDK licenses directory is empty. Run: flutter doctor --android-licenses");
                        fail_count += 1;
                    }
                } else {
                    check_fail("Android SDK licenses not accepted. Run: flutter doctor --android-licenses");
                    fail_count += 1;
                }
            }
            Err(_) => {
                // ANDROID_HOME not set — soft pass with warning
                check_warn("$ANDROID_HOME not set, cannot check licenses (soft pass).");
                pass_count += 1;
            }
        }
    }

    // 5. $JAVA_HOME set and directory exists
    {
        match std::env::var("JAVA_HOME") {
            Ok(java_home) => {
                let p = PathBuf::from(&java_home);
                if p.exists() && p.is_dir() {
                    check_pass(&format!("$JAVA_HOME set and exists: {}", java_home));
                    pass_count += 1;
                } else {
                    check_fail(&format!(
                        "$JAVA_HOME is set to '{}' but directory does not exist.",
                        java_home
                    ));
                    fail_count += 1;
                }
            }
            Err(_) => {
                check_fail("$JAVA_HOME is not set.");
                fail_count += 1;
            }
        }
    }

    // 6. keytool
    {
        let keytool = find_keytool(runner);
        if let Some(ref kt) = keytool {
            check_pass(&format!("keytool: {}", kt));
            pass_count += 1;
        } else {
            check_fail("keytool not found. Install a JDK and set $JAVA_HOME.");
            fail_count += 1;
        }
    }

    // 7. xcrun --version (macOS only)
    {
        if cfg!(target_os = "macos") {
            let result = runner.run("xcrun", &["--version"], None);
            if result.is_success() {
                let first_line = result.stdout.lines().next().unwrap_or("").to_string();
                check_pass(&format!("xcrun: {}", first_line));
                pass_count += 1;
            } else {
                check_fail("xcrun not found. Install Xcode command line tools: xcode-select --install");
                fail_count += 1;
            }
        } else {
            check_skip("xcrun: skipped (not macOS)");
            pass_count += 1;
        }
    }

    // 8. xcodebuild -version (macOS only)
    {
        if cfg!(target_os = "macos") {
            let result = runner.run("xcodebuild", &["-version"], None);
            if result.is_success() {
                let first_line = result.stdout.lines().next().unwrap_or("").to_string();
                check_pass(&format!("xcodebuild: {}", first_line));
                pass_count += 1;
            } else {
                check_fail("xcodebuild not found. Install Xcode from the App Store.");
                fail_count += 1;
            }
        } else {
            check_skip("xcodebuild: skipped (not macOS)");
            pass_count += 1;
        }
    }

    // 9. iOS simulator runtimes (macOS only)
    {
        if cfg!(target_os = "macos") {
            let result = runner.run(
                "xcrun",
                &["simctl", "list", "runtimes", "--json"],
                None,
            );
            if result.is_success() && !result.stdout.is_empty() {
                let has_ios = check_ios_runtimes(&result.stdout);
                if has_ios {
                    check_pass("iOS Simulator runtime(s) available.");
                    pass_count += 1;
                } else {
                    check_fail("No iOS Simulator runtimes found. Download via Xcode > Preferences > Components.");
                    fail_count += 1;
                }
            } else {
                check_fail("Could not list simulator runtimes.");
                fail_count += 1;
            }
        } else {
            check_skip("iOS Simulators: skipped (not macOS)");
            pass_count += 1;
        }
    }

    // 10. ~/.pub-cache/bin in $PATH
    {
        let home = dirs::home_dir().unwrap_or_default();
        let pub_bin = home.join(".pub-cache").join("bin");
        let pub_bin_str = pub_bin.to_string_lossy().to_string();
        let path_var = std::env::var("PATH").unwrap_or_default();
        if path_var.split(':').any(|p| p == pub_bin_str) {
            check_pass(&format!("~/.pub-cache/bin in $PATH: {}", pub_bin_str));
        } else {
            check_warn(&format!(
                "~/.pub-cache/bin is not in $PATH. Add it to use globally installed Dart tools."
            ));
        }
        // always soft pass
        pass_count += 1;
    }

    logger.info("");
    if fail_count > 0 {
        logger.err(&format!(
            "{}/{} checks passed ({} issue(s) found)",
            pass_count, TOTAL, fail_count
        ));
        return 1;
    }

    logger.success(&format!("{}/{} checks passed", pass_count, TOTAL));
    0
}

fn find_adb(runner: &dyn Runner) -> Option<String> {
    if let Some(path) = runner.which("adb") {
        return Some(path);
    }
    if let Ok(android_home) = std::env::var("ANDROID_HOME") {
        let candidate = PathBuf::from(android_home)
            .join("platform-tools")
            .join("adb");
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn find_keytool(runner: &dyn Runner) -> Option<String> {
    if let Some(path) = runner.which("keytool") {
        return Some(path);
    }
    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let candidate = PathBuf::from(java_home).join("bin").join("keytool");
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn parse_flutter_version(stdout: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(stdout).ok()?;
    json.get("frameworkVersion")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn check_ios_runtimes(stdout: &str) -> bool {
    let json: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if let Some(runtimes) = json.get("runtimes").and_then(|v| v.as_array()) {
        for runtime in runtimes {
            if let Some(identifier) = runtime.get("identifier").and_then(|v| v.as_str()) {
                if identifier.contains("com.apple.CoreSimulator.SimRuntime.iOS") {
                    return true;
                }
            }
        }
    }
    false
}
