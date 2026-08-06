use clap::Args;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::commands::tool_locator;
use crate::logger::Logger;
use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct AndroidArgs {
    /// AVD to start (default: the first one from `emulator -list-avds`).
    #[arg(short = 'a', long)]
    pub avd: Option<String>,
    /// Stop the emulator instead of starting it (every running one when --avd is omitted).
    #[arg(short = 'o', long)]
    pub off: bool,
}

/// How often, and how many times, to re-check a pending state.
pub struct PollConfig {
    pub attempts: u32,
    pub interval: Duration,
}

impl PollConfig {
    /// Waiting for a freshly launched emulator to register with adb.
    fn serial() -> Self {
        Self {
            attempts: 60,
            interval: Duration::from_secs(2),
        }
    }

    /// Waiting for a registered emulator to finish booting.
    fn boot() -> Self {
        Self {
            attempts: 90,
            interval: Duration::from_secs(2),
        }
    }
}

pub fn run(args: &AndroidArgs, runner: &dyn Runner) -> i32 {
    if args.off {
        return stop(args, runner);
    }
    start(args, runner, &PollConfig::serial(), &PollConfig::boot())
}

/// Kills the emulator hosting `--avd`, or every running emulator when it is omitted.
fn stop(args: &AndroidArgs, runner: &dyn Runner) -> i32 {
    let logger = Logger::new();

    let Some(adb) = tool_locator::adb(runner) else {
        logger.err("adb not found — install the Android SDK platform-tools or set ANDROID_HOME");
        return 1;
    };

    let serials = match &args.avd {
        Some(avd) => match serial_for_avd(&adb, runner, avd) {
            Some(serial) => vec![serial],
            None => {
                logger.info(&format!("AVD {} is not running", avd));
                return 0;
            }
        },
        None => {
            let listed = runner.run(&adb, &["devices"], None);
            parse_emulator_serials(&listed.stdout)
        }
    };

    if serials.is_empty() {
        logger.info("No running emulators");
        return 0;
    }

    let mut exit_code = 0;
    for serial in &serials {
        let killed = runner.run(&adb, &["-s", serial, "emu", "kill"], None);
        if killed.is_success() {
            logger.success(&format!("Emulator off: {}", serial));
        } else {
            exit_code = 1;
            logger.err(&format!("Failed to stop {}", serial));
            let reason = if killed.stderr.is_empty() {
                &killed.stdout
            } else {
                &killed.stderr
            };
            if !reason.is_empty() {
                logger.detail(reason.trim());
            }
        }
    }
    exit_code
}

fn start(
    args: &AndroidArgs,
    runner: &dyn Runner,
    serial_poll: &PollConfig,
    boot_poll: &PollConfig,
) -> i32 {
    let logger = Logger::new();

    let Some(emulator) = tool_locator::emulator(runner) else {
        logger.err("emulator not found — install the Android SDK or set ANDROID_HOME");
        return 1;
    };
    let Some(adb) = tool_locator::adb(runner) else {
        logger.err("adb not found — install the Android SDK platform-tools or set ANDROID_HOME");
        return 1;
    };

    let avd = match args.avd.clone() {
        Some(avd) => avd,
        None => {
            let listed = runner.run(&emulator, &["-list-avds"], None);
            match parse_avd_names(&listed.stdout).into_iter().next() {
                Some(avd) => avd,
                None => {
                    logger.err("No AVDs found — create one with avdmanager or Android Studio");
                    return 1;
                }
            }
        }
    };

    // Starting the server first keeps the later device queries from racing it.
    runner.run(&adb, &["start-server"], None);

    let serial = match serial_for_avd(&adb, runner, &avd) {
        Some(serial) => {
            logger.info(&format!("AVD {} already running ({})", avd, serial));
            serial
        }
        None => {
            let log_path = log_path_for(&avd);
            logger.info(&format!(
                "Starting AVD {} (log: {})",
                avd,
                log_path.display()
            ));
            match launch(&emulator, &adb, runner, &avd, &log_path, serial_poll) {
                Ok(serial) => serial,
                Err(message) => {
                    logger.err(&message);
                    if let Ok(log) = std::fs::read_to_string(&log_path) {
                        logger.detail(log.trim());
                    }
                    return 1;
                }
            }
        }
    };

    runner.run(&adb, &["-s", &serial, "wait-for-device"], None);

    let progress = logger.progress(&format!("Waiting for {} to finish booting", avd));
    let booted = wait_for_boot(&adb, runner, &serial, boot_poll);
    progress.finish_and_clear();

    if !booted {
        logger.err(&format!(
            "Timed out waiting for {} ({}) to finish booting",
            avd, serial
        ));
        return 1;
    }

    logger.success(&format!("Emulator ready: {} ({})", avd, serial));
    0
}

/// Spawns the emulator and waits for it to register with adb, failing fast when
/// the process dies first (bad AVD name, stale lock, broken SDK path).
fn launch(
    emulator: &str,
    adb: &str,
    runner: &dyn Runner,
    avd: &str,
    log_path: &Path,
    poll: &PollConfig,
) -> Result<String, String> {
    let Some(mut process) = runner.spawn_detached(emulator, &["-avd", avd], log_path) else {
        return Err(format!(
            "Could not start the emulator binary at {}",
            emulator
        ));
    };

    for attempt in 0..poll.attempts {
        if let Some(code) = process.exit_code() {
            return Err(format!(
                "emulator exited with code {} before registering with adb",
                code
            ));
        }
        // The emulator picks its own console port, so ask each running emulator
        // which AVD it hosts instead of assuming emulator-5554.
        if let Some(serial) = serial_for_avd(adb, runner, avd) {
            return Ok(serial);
        }
        if attempt + 1 < poll.attempts {
            thread::sleep(poll.interval);
        }
    }

    Err(format!(
        "emulator did not register with adb after {:?}",
        poll.interval * poll.attempts
    ))
}

fn wait_for_boot(adb: &str, runner: &dyn Runner, serial: &str, poll: &PollConfig) -> bool {
    for attempt in 0..poll.attempts {
        let boot_completed = getprop(adb, runner, serial, "sys.boot_completed");
        let bootanim = getprop(adb, runner, serial, "init.svc.bootanim");
        if boot_completed == "1" && bootanim == "stopped" {
            return true;
        }
        if attempt + 1 < poll.attempts {
            thread::sleep(poll.interval);
        }
    }
    false
}

fn getprop(adb: &str, runner: &dyn Runner, serial: &str, property: &str) -> String {
    let result = runner.run(adb, &["-s", serial, "shell", "getprop", property], None);
    result.stdout.replace('\r', "").trim().to_string()
}

/// Serial of the running emulator hosting `avd`, if any.
fn serial_for_avd(adb: &str, runner: &dyn Runner, avd: &str) -> Option<String> {
    let listed = runner.run(adb, &["devices"], None);
    parse_emulator_serials(&listed.stdout)
        .into_iter()
        .find(|serial| {
            let reply = runner.run(adb, &["-s", serial, "emu", "avd", "name"], None);
            parse_avd_reply(&reply.stdout).as_deref() == Some(avd)
        })
}

fn log_path_for(avd: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mdev-emulator-{}.log", avd))
}

/// AVD names from `emulator -list-avds`, dropping the log lines it interleaves.
fn parse_avd_names(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(|line| line.trim())
        .filter(|line| {
            !line.is_empty()
                && !line.contains(char::is_whitespace)
                && !line.contains('|')
                && !line.contains(':')
        })
        .map(|line| line.to_string())
        .collect()
}

/// Emulator serials from `adb devices` (physical and network devices excluded).
fn parse_emulator_serials(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|serial| serial.starts_with("emulator-"))
        .map(|serial| serial.to_string())
        .collect()
}

/// `adb emu avd name` answers with the name followed by an `OK` status line.
fn parse_avd_reply(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(|line| line.replace('\r', "").trim().to_string())
        .find(|line| !line.is_empty() && line != "OK")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{BackgroundProcess, RunResult};
    use std::cell::{Cell, RefCell};

    #[test]
    fn parses_avd_names_ignoring_log_lines() {
        let stdout =
            "INFO | storing crashdata\nPixel_9\nPixel_7_API_34\n\nWARNING: something: bad\n";
        assert_eq!(
            parse_avd_names(stdout),
            vec!["Pixel_9".to_string(), "Pixel_7_API_34".to_string()]
        );
    }

    #[test]
    fn parses_emulator_serials_only() {
        let stdout = "List of devices attached\nemulator-5554\tdevice\n39021FDJH00BQK\tdevice\nadb-abc123-xyz._adb-tls-connect._tcp\tdevice\nemulator-5556\toffline";
        assert_eq!(
            parse_emulator_serials(stdout),
            vec!["emulator-5554".to_string(), "emulator-5556".to_string()]
        );
    }

    #[test]
    fn parses_avd_reply_without_ok_line() {
        assert_eq!(parse_avd_reply("Pixel_9\r\nOK\r\n"), Some("Pixel_9".into()));
        assert_eq!(parse_avd_reply("OK\n"), None);
        assert_eq!(parse_avd_reply(""), None);
    }

    /// Emulator process whose liveness the test controls.
    struct FakeProcess {
        exit_code: Option<i32>,
    }

    impl BackgroundProcess for FakeProcess {
        fn exit_code(&mut self) -> Option<i32> {
            self.exit_code
        }
    }

    #[derive(Default)]
    struct ScriptedRunner {
        /// Serial reported by `adb devices` once the emulator has registered.
        registered_serial: RefCell<Option<String>>,
        /// AVD hosted by `registered_serial`.
        hosted_avd: String,
        /// Registration happens on this `adb devices` call (0 = already running).
        register_after_calls: u32,
        devices_calls: Cell<u32>,
        /// Exit code the spawned emulator reports, if it dies.
        spawn_exit_code: Option<i32>,
        /// `None` makes `spawn_detached` fail.
        spawn_succeeds: bool,
        boot_completed: RefCell<String>,
        bootanim: RefCell<String>,
        calls: RefCell<Vec<String>>,
    }

    impl ScriptedRunner {
        fn running(avd: &str) -> Self {
            Self {
                registered_serial: RefCell::new(Some("emulator-5554".into())),
                hosted_avd: avd.to_string(),
                spawn_succeeds: true,
                boot_completed: RefCell::new("1".into()),
                bootanim: RefCell::new("stopped".into()),
                ..Default::default()
            }
        }

        fn launching(avd: &str, register_after_calls: u32) -> Self {
            Self {
                register_after_calls,
                ..Self::running(avd)
            }
        }

        fn called(&self, needle: &str) -> bool {
            self.calls.borrow().iter().any(|c| c.contains(needle))
        }
    }

    impl Runner for ScriptedRunner {
        fn run(&self, executable: &str, args: &[&str], _working_dir: Option<&str>) -> RunResult {
            let command = format!("{} {}", executable, args.join(" "));
            self.calls.borrow_mut().push(command.clone());

            if command.contains("-list-avds") {
                return RunResult::success(self.hosted_avd.clone());
            }
            if args == ["devices"] {
                let seen = self.devices_calls.get();
                self.devices_calls.set(seen + 1);
                if seen < self.register_after_calls {
                    return RunResult::success("List of devices attached".into());
                }
                let listing = match self.registered_serial.borrow().as_ref() {
                    Some(serial) => format!("List of devices attached\n{}\tdevice", serial),
                    None => "List of devices attached".into(),
                };
                return RunResult::success(listing);
            }
            if command.contains("emu avd name") {
                return RunResult::success(format!("{}\nOK", self.hosted_avd));
            }
            if command.contains("getprop sys.boot_completed") {
                return RunResult::success(self.boot_completed.borrow().clone());
            }
            if command.contains("getprop init.svc.bootanim") {
                return RunResult::success(self.bootanim.borrow().clone());
            }
            RunResult::success(String::new())
        }

        fn which(&self, executable: &str) -> Option<String> {
            Some(format!("/fake/sdk/{}", executable))
        }

        fn spawn_detached(
            &self,
            executable: &str,
            args: &[&str],
            _log_path: &Path,
        ) -> Option<Box<dyn BackgroundProcess>> {
            self.calls
                .borrow_mut()
                .push(format!("spawn {} {}", executable, args.join(" ")));
            if !self.spawn_succeeds {
                return None;
            }
            Some(Box::new(FakeProcess {
                exit_code: self.spawn_exit_code,
            }))
        }
    }

    fn instant_poll(attempts: u32) -> PollConfig {
        PollConfig {
            attempts,
            interval: Duration::from_millis(0),
        }
    }

    fn args_for(avd: &str) -> AndroidArgs {
        AndroidArgs {
            avd: Some(avd.to_string()),
            off: false,
        }
    }

    fn off_args(avd: Option<&str>) -> AndroidArgs {
        AndroidArgs {
            avd: avd.map(|a| a.to_string()),
            off: true,
        }
    }

    #[test]
    fn off_kills_the_emulator_hosting_the_avd() {
        let runner = ScriptedRunner::running("Pixel_9");
        let code = stop(&off_args(Some("Pixel_9")), &runner);

        assert_eq!(code, 0);
        assert!(runner.called("-s emulator-5554 emu kill"));
    }

    // GUARD: without --avd every running emulator goes down, not just the first AVD.
    #[test]
    fn off_without_avd_kills_every_running_emulator() {
        let runner = ScriptedRunner::running("Pixel_9");
        let code = stop(&off_args(None), &runner);

        assert_eq!(code, 0);
        assert!(runner.called("-s emulator-5554 emu kill"));
        assert!(!runner.called("emu avd name"), "must not resolve AVD names");
    }

    #[test]
    fn off_is_a_no_op_when_the_avd_is_not_running() {
        let runner = ScriptedRunner::running("Pixel_9");
        let code = stop(&off_args(Some("Other_AVD")), &runner);

        assert_eq!(code, 0);
        assert!(!runner.called("emu kill"));
    }

    #[test]
    fn running_avd_is_reused_without_spawning() {
        let runner = ScriptedRunner::running("Pixel_9");
        let code = start(
            &args_for("Pixel_9"),
            &runner,
            &instant_poll(3),
            &instant_poll(3),
        );

        assert_eq!(code, 0);
        assert!(
            !runner.called("spawn "),
            "must not launch a second emulator"
        );
    }

    #[test]
    fn launches_and_waits_for_registration() {
        let runner = ScriptedRunner::launching("Pixel_9", 2);
        let code = start(
            &args_for("Pixel_9"),
            &runner,
            &instant_poll(5),
            &instant_poll(3),
        );

        assert_eq!(code, 0);
        // Path-agnostic: the emulator binary resolves against the real SDK layout.
        assert!(runner.called("spawn "));
        assert!(runner.called("-avd Pixel_9"));
        assert!(runner.called("wait-for-device"));
    }

    // GUARD: a dead emulator must fail fast instead of waiting on adb forever.
    #[test]
    fn emulator_exiting_early_fails_instead_of_hanging() {
        let runner = ScriptedRunner {
            spawn_exit_code: Some(1),
            register_after_calls: u32::MAX,
            ..ScriptedRunner::running("Pixel_9")
        };
        let code = start(
            &args_for("Pixel_9"),
            &runner,
            &instant_poll(30),
            &instant_poll(3),
        );

        assert_eq!(code, 1);
        assert!(!runner.called("wait-for-device"));
    }

    #[test]
    fn never_registering_emulator_times_out() {
        let runner = ScriptedRunner {
            register_after_calls: u32::MAX,
            ..ScriptedRunner::running("Pixel_9")
        };
        let code = start(
            &args_for("Pixel_9"),
            &runner,
            &instant_poll(3),
            &instant_poll(3),
        );

        assert_eq!(code, 1);
    }

    // GUARD: boot_completed alone is not enough — the boot animation must stop too.
    #[test]
    fn boot_animation_still_running_times_out() {
        let runner = ScriptedRunner {
            bootanim: RefCell::new("running".into()),
            ..ScriptedRunner::running("Pixel_9")
        };
        let code = start(
            &args_for("Pixel_9"),
            &runner,
            &instant_poll(3),
            &instant_poll(3),
        );

        assert_eq!(code, 1);
    }

    #[test]
    fn defaults_to_first_listed_avd() {
        let runner = ScriptedRunner::running("Pixel_9");
        let code = start(
            &AndroidArgs {
                avd: None,
                off: false,
            },
            &runner,
            &instant_poll(3),
            &instant_poll(3),
        );

        assert_eq!(code, 0);
        assert!(runner.called("-list-avds"));
    }

    #[test]
    fn spawn_failure_is_reported() {
        let runner = ScriptedRunner {
            spawn_succeeds: false,
            register_after_calls: u32::MAX,
            ..ScriptedRunner::running("Pixel_9")
        };
        let code = start(
            &args_for("Pixel_9"),
            &runner,
            &instant_poll(3),
            &instant_poll(3),
        );

        assert_eq!(code, 1);
    }
}
