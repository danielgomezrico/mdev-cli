use clap::Args;
use std::thread;
use std::time::Duration;

use crate::commands::simulator::android::{
    self as sim_android, parse_avd_reply, parse_emulator_serials, PollConfig,
};
use crate::commands::tool_locator;
use crate::logger::Logger;
use crate::runner::Runner;

/// Router of the emulator's user-mode network — always reachable while the
/// virtual NIC is up, even when nothing beyond it is.
const GATEWAY: &str = "10.0.2.2";
/// Host that must resolve for the emulator to consider itself online; it is the
/// same endpoint NetworkMonitor probes to decide whether to show the "!" badge.
const PROBE_HOST: &str = "connectivitycheck.gstatic.com";
/// Used when the host has no usable resolver of its own.
const PUBLIC_DNS: &[&str] = &["8.8.8.8", "1.1.1.1"];
/// `emulator -dns-server` accepts at most four addresses.
const MAX_DNS_SERVERS: usize = 4;

#[derive(Args, Debug)]
pub struct AndroidArgs {
    /// Only fix the emulator hosting this AVD (default: every running emulator).
    #[arg(short = 'a', long)]
    pub avd: Option<String>,
    /// DNS servers for the restarted emulator (comma-separated, max 4).
    #[arg(long)]
    pub dns: Option<String>,
    /// Never restart the emulator; print the command to run by hand instead.
    #[arg(long)]
    pub no_restart: bool,
    /// Restart without asking.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// What the emulator's network can and cannot do.
#[derive(Debug, PartialEq)]
enum Health {
    Ok,
    /// Packets flow but names do not resolve — the usual "Wi-Fi with a !" state.
    DnsBroken,
    /// Not even the emulator's own gateway answers.
    Offline,
}

/// How long each repair step waits for the network to come back.
pub struct Timings {
    /// Re-probing after a fix that keeps the emulator running.
    pub recheck: PollConfig,
    /// Waiting for a killed emulator to leave `adb devices`.
    pub shutdown: PollConfig,
    /// Waiting for the restarted emulator to register with adb.
    pub serial: PollConfig,
    /// Waiting for the restarted emulator to finish booting.
    pub boot: PollConfig,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            recheck: PollConfig {
                attempts: 8,
                interval: Duration::from_secs(2),
            },
            shutdown: PollConfig {
                attempts: 15,
                interval: Duration::from_secs(2),
            },
            serial: PollConfig::serial(),
            boot: PollConfig::boot(),
        }
    }
}

pub fn run(args: &AndroidArgs, runner: &dyn Runner) -> i32 {
    fix(args, runner, &Timings::default())
}

fn fix(args: &AndroidArgs, runner: &dyn Runner, timings: &Timings) -> i32 {
    let logger = Logger::new();

    let Some(adb) = tool_locator::adb(runner) else {
        logger.err("adb not found — install the Android SDK platform-tools or set ANDROID_HOME");
        return 1;
    };

    let serials = match &args.avd {
        Some(avd) => match sim_android::serial_for_avd(&adb, runner, avd) {
            Some(serial) => vec![serial],
            None => {
                logger.err(&format!("AVD {} is not running", avd));
                return 1;
            }
        },
        None => parse_emulator_serials(&runner.run(&adb, &["devices"], None).stdout),
    };

    if serials.is_empty() {
        logger.err("No running emulators — start one with `mdev sim a`");
        return 1;
    }

    let mut exit_code = 0;
    for serial in &serials {
        if !fix_device(&adb, runner, serial, args, timings, &logger) {
            exit_code = 1;
        }
    }
    exit_code
}

/// Applies the smallest repair that restores the network, in increasing order of
/// disruption. Returns whether the emulator ended up healthy.
fn fix_device(
    adb: &str,
    runner: &dyn Runner,
    serial: &str,
    args: &AndroidArgs,
    timings: &Timings,
    logger: &Logger,
) -> bool {
    match probe(adb, runner, serial) {
        Health::Ok => {
            logger.success(&format!("{}: network already working", serial));
            return true;
        }
        Health::DnsBroken => logger.warn(&format!(
            "{}: connected but no DNS — {} does not resolve",
            serial, PROBE_HOST
        )),
        Health::Offline => logger.warn(&format!(
            "{}: offline — the emulator gateway {} does not answer",
            serial, GATEWAY
        )),
    }

    if clear_radio_blocks(adb, runner, serial, logger) && wait_healthy(adb, runner, serial, timings)
    {
        logger.success(&format!(
            "{}: network back after re-enabling the radios",
            serial
        ));
        return true;
    }

    logger.info(&format!("{}: toggling Wi-Fi", serial));
    toggle_wifi(adb, runner, serial);
    if wait_healthy(adb, runner, serial, timings) {
        logger.success(&format!("{}: network back after a Wi-Fi toggle", serial));
        return true;
    }

    // Everything above stays inside the guest. What remains is the emulator's own
    // DNS forwarder pointing at host resolvers that no longer answer (host moved
    // network, VPN dropped, resolver on 127.x gone), which only a restart re-reads.
    let servers = dns_servers(args, runner);
    let joined = servers.join(",");

    let Some(avd) = avd_name(adb, runner, serial) else {
        logger.err(&format!(
            "{}: cannot tell which AVD this emulator hosts — restart it with `-dns-server {}`",
            serial, joined
        ));
        return false;
    };

    if args.no_restart {
        logger.warn(&format!(
            "{}: only a restart can renew the emulator's DNS. Run: emulator -avd {} -dns-server {} -no-snapshot-load",
            serial, avd, joined
        ));
        return false;
    }

    if !args.yes && !logger.confirm(&format!("Cold-boot {} with DNS {}?", avd, joined), true) {
        logger.info(&format!("{}: left untouched", serial));
        return false;
    }

    match restart_with_dns(adb, runner, serial, &avd, &servers, timings, logger) {
        Ok(new_serial) => {
            if wait_healthy(adb, runner, &new_serial, timings) {
                logger.success(&format!(
                    "{}: network back after a cold boot with DNS {}",
                    new_serial, joined
                ));
                true
            } else {
                logger.err(&format!(
                    "{}: still no network after the cold boot — check the host's own connection",
                    new_serial
                ));
                false
            }
        }
        Err(message) => {
            logger.err(&format!("{}: {}", serial, message));
            false
        }
    }
}

fn probe(adb: &str, runner: &dyn Runner, serial: &str) -> Health {
    if !ping_reached(&ping(adb, runner, serial, GATEWAY)) {
        return Health::Offline;
    }
    if name_resolved(&ping(adb, runner, serial, PROBE_HOST)) {
        Health::Ok
    } else {
        Health::DnsBroken
    }
}

fn wait_healthy(adb: &str, runner: &dyn Runner, serial: &str, timings: &Timings) -> bool {
    for attempt in 0..timings.recheck.attempts {
        if probe(adb, runner, serial) == Health::Ok {
            return true;
        }
        if attempt + 1 < timings.recheck.attempts {
            thread::sleep(timings.recheck.interval);
        }
    }
    false
}

/// Leaves airplane mode and re-enables Wi-Fi. Returns whether anything changed.
fn clear_radio_blocks(adb: &str, runner: &dyn Runner, serial: &str, logger: &Logger) -> bool {
    let mut changed = false;

    if setting(adb, runner, serial, "airplane_mode_on") == "1" {
        logger.info(&format!("{}: leaving airplane mode", serial));
        shell(
            adb,
            runner,
            serial,
            &["cmd", "connectivity", "airplane-mode", "disable"],
        );
        changed = true;
    }

    if setting(adb, runner, serial, "wifi_on") == "0" {
        logger.info(&format!("{}: enabling Wi-Fi", serial));
        shell(adb, runner, serial, &["svc", "wifi", "enable"]);
        changed = true;
    }

    changed
}

/// Forces ConnectivityService to re-run validation, which is what clears the "!"
/// badge once the network answers again.
fn toggle_wifi(adb: &str, runner: &dyn Runner, serial: &str) {
    shell(adb, runner, serial, &["svc", "wifi", "disable"]);
    shell(adb, runner, serial, &["svc", "wifi", "enable"]);
}

/// Kills the emulator and cold-boots the same AVD with explicit DNS servers.
/// A cold boot matters: a snapshot restore brings the dead resolver state back.
fn restart_with_dns(
    adb: &str,
    runner: &dyn Runner,
    serial: &str,
    avd: &str,
    servers: &[String],
    timings: &Timings,
    logger: &Logger,
) -> Result<String, String> {
    let Some(emulator) = tool_locator::emulator(runner) else {
        return Err("emulator not found — install the Android SDK or set ANDROID_HOME".to_string());
    };

    let killed = runner.run(adb, &["-s", serial, "emu", "kill"], None);
    if !killed.is_success() {
        return Err(format!("could not stop {}", serial));
    }
    if !wait_gone(adb, runner, avd, &timings.shutdown) {
        return Err(format!("{} is still running after `emu kill`", avd));
    }

    let joined = servers.join(",");
    let log_path = sim_android::log_path_for(avd);
    logger.info(&format!(
        "Cold-booting {} with DNS {} (log: {})",
        avd,
        joined,
        log_path.display()
    ));

    let new_serial = sim_android::launch(
        &emulator,
        adb,
        runner,
        avd,
        &["-dns-server", &joined, "-no-snapshot-load"],
        &log_path,
        &timings.serial,
    )?;

    runner.run(adb, &["-s", &new_serial, "wait-for-device"], None);

    let progress = logger.progress(&format!("Waiting for {} to finish booting", avd));
    let booted = sim_android::wait_for_boot(adb, runner, &new_serial, &timings.boot);
    progress.finish_and_clear();

    if !booted {
        return Err(format!("timed out waiting for {} to finish booting", avd));
    }

    Ok(new_serial)
}

fn wait_gone(adb: &str, runner: &dyn Runner, avd: &str, poll: &PollConfig) -> bool {
    for attempt in 0..poll.attempts {
        if sim_android::serial_for_avd(adb, runner, avd).is_none() {
            return true;
        }
        if attempt + 1 < poll.attempts {
            thread::sleep(poll.interval);
        }
    }
    false
}

fn avd_name(adb: &str, runner: &dyn Runner, serial: &str) -> Option<String> {
    let reply = runner.run(adb, &["-s", serial, "emu", "avd", "name"], None);
    parse_avd_reply(&reply.stdout)
}

fn dns_servers(args: &AndroidArgs, runner: &dyn Runner) -> Vec<String> {
    match &args.dns {
        Some(list) => usable_dns(list.split(',').map(|s| s.trim().to_string()).collect()),
        None => usable_dns(host_dns_servers(runner)),
    }
}

/// Resolvers the host itself uses, so internal domains keep working inside the
/// emulator.
fn host_dns_servers(runner: &dyn Runner) -> Vec<String> {
    if cfg!(target_os = "macos") {
        let dns = runner.run("scutil", &["--dns"], None);
        parse_scutil_nameservers(&dns.stdout)
    } else {
        parse_resolv_conf(&std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default())
    }
}

/// Nameservers from `scutil --dns`, in the order macOS queries them.
fn parse_scutil_nameservers(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver["))
        .filter_map(|rest| rest.split(':').nth(1))
        .map(|addr| addr.trim().to_string())
        .collect()
}

fn parse_resolv_conf(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("nameserver "))
        .map(|addr| addr.trim().to_string())
        .collect()
}

/// Drops what the emulator cannot forward to and pads with public resolvers.
/// Loopback addresses are the common trap: the emulator NATs them into the
/// guest, where nothing listens, so every lookup times out.
fn usable_dns(candidates: Vec<String>) -> Vec<String> {
    let mut servers: Vec<String> = Vec::new();
    for candidate in candidates
        .into_iter()
        .chain(PUBLIC_DNS.iter().map(|s| s.to_string()))
    {
        if candidate.is_empty() || is_loopback(&candidate) || servers.contains(&candidate) {
            continue;
        }
        servers.push(candidate);
        if servers.len() == MAX_DNS_SERVERS {
            break;
        }
    }
    servers
}

fn is_loopback(addr: &str) -> bool {
    addr.starts_with("127.") || addr == "::1"
}

fn setting(adb: &str, runner: &dyn Runner, serial: &str, key: &str) -> String {
    shell(adb, runner, serial, &["settings", "get", "global", key])
        .replace('\r', "")
        .trim()
        .to_string()
}

fn ping(adb: &str, runner: &dyn Runner, serial: &str, host: &str) -> String {
    shell(adb, runner, serial, &["ping", "-c", "1", "-W", "2", host])
}

/// stdout and stderr merged: `ping` reports "unknown host" on either stream
/// depending on the image.
fn shell(adb: &str, runner: &dyn Runner, serial: &str, command: &[&str]) -> String {
    let mut args = vec!["-s", serial, "shell"];
    args.extend_from_slice(command);
    let result = runner.run(adb, &args, None);
    format!("{}\n{}", result.stdout, result.stderr)
}

fn ping_reached(output: &str) -> bool {
    output.contains("bytes from") || output.contains(" 0% packet loss")
}

/// `ping` echoes the resolved address as `PING host (1.2.3.4)` before sending
/// anything, so a name resolves even when the ICMP reply never comes.
fn name_resolved(output: &str) -> bool {
    if output.contains("unknown host") {
        return false;
    }
    output
        .lines()
        .any(|line| line.contains('(') && line.contains(')'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{BackgroundProcess, RunResult};
    use std::cell::{Cell, RefCell};
    use std::path::Path;

    #[test]
    fn reads_nameservers_from_scutil_output() {
        let stdout = "DNS configuration\n\nresolver #1\n  nameserver[0] : 192.168.10.22\n  nameserver[1] : 8.8.8.8\n  flags  : Request A records\n";
        assert_eq!(
            parse_scutil_nameservers(stdout),
            vec!["192.168.10.22".to_string(), "8.8.8.8".to_string()]
        );
    }

    #[test]
    fn reads_nameservers_from_resolv_conf() {
        let contents = "# comment\nnameserver 1.1.1.1\nsearch lan\nnameserver 9.9.9.9\n";
        assert_eq!(
            parse_resolv_conf(contents),
            vec!["1.1.1.1".to_string(), "9.9.9.9".to_string()]
        );
    }

    // GUARD: loopback resolvers are exactly what breaks the emulator — dropping
    // them is the whole point of the fix.
    #[test]
    fn drops_loopback_resolvers_and_falls_back_to_public_dns() {
        let servers = usable_dns(vec!["127.0.2.2".into(), "::1".into()]);
        assert_eq!(servers, vec!["8.8.8.8".to_string(), "1.1.1.1".to_string()]);
    }

    #[test]
    fn keeps_host_resolvers_first_and_caps_at_four() {
        let servers = usable_dns(vec![
            "192.168.10.22".into(),
            "192.168.10.22".into(),
            "9.9.9.9".into(),
        ]);
        assert_eq!(
            servers,
            vec![
                "192.168.10.22".to_string(),
                "9.9.9.9".to_string(),
                "8.8.8.8".to_string(),
                "1.1.1.1".to_string()
            ]
        );
    }

    #[test]
    fn unknown_host_means_dns_is_broken() {
        assert!(!name_resolved(
            "ping: unknown host connectivitycheck.gstatic.com\n"
        ));
    }

    // GUARD: a name that resolved but never answered ICMP is still working DNS.
    #[test]
    fn resolved_name_without_icmp_reply_still_counts_as_resolved() {
        let output = "PING connectivitycheck.gstatic.com (142.250.72.163) 56 data bytes\n\n--- statistics ---\n1 packets transmitted, 0 received, 100% packet loss\n";
        assert!(name_resolved(output));
    }

    #[test]
    fn gateway_reply_counts_as_reachable() {
        assert!(ping_reached("64 bytes from 10.0.2.2: icmp_seq=1 ttl=255"));
        assert!(!ping_reached(
            "1 packets transmitted, 0 received, 100% packet loss"
        ));
    }

    struct FakeProcess;

    impl BackgroundProcess for FakeProcess {
        fn exit_code(&mut self) -> Option<i32> {
            None
        }
    }

    /// Emulator whose network state the test drives.
    struct ScriptedRunner {
        dns_ok: Cell<bool>,
        wifi_on: Cell<bool>,
        airplane: Cell<bool>,
        /// DNS starts answering after the Wi-Fi toggle.
        heals_on_wifi_toggle: bool,
        /// DNS starts answering after the emulator cold-boots.
        heals_on_restart: bool,
        /// `adb devices` lists nothing between `emu kill` and the next launch.
        running: Cell<bool>,
        calls: RefCell<Vec<String>>,
    }

    impl ScriptedRunner {
        fn healthy() -> Self {
            Self {
                dns_ok: Cell::new(true),
                wifi_on: Cell::new(true),
                airplane: Cell::new(false),
                heals_on_wifi_toggle: false,
                heals_on_restart: false,
                running: Cell::new(true),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn dns_broken() -> Self {
            Self {
                dns_ok: Cell::new(false),
                ..Self::healthy()
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

            if args == ["devices"] {
                let listing = if self.running.get() {
                    "List of devices attached\nemulator-5554\tdevice"
                } else {
                    "List of devices attached"
                };
                return RunResult::success(listing.into());
            }
            if command.contains("emu avd name") {
                return RunResult::success("Pixel_9\nOK".into());
            }
            if command.contains("emu kill") {
                self.running.set(false);
                return RunResult::success(String::new());
            }
            if command.contains("settings get global airplane_mode_on") {
                return RunResult::success(if self.airplane.get() { "1" } else { "0" }.into());
            }
            if command.contains("settings get global wifi_on") {
                return RunResult::success(if self.wifi_on.get() { "1" } else { "0" }.into());
            }
            if command.contains("airplane-mode disable") {
                self.airplane.set(false);
                return RunResult::success(String::new());
            }
            if command.contains("svc wifi disable") {
                self.wifi_on.set(false);
                return RunResult::success(String::new());
            }
            if command.contains("svc wifi enable") {
                self.wifi_on.set(true);
                if self.heals_on_wifi_toggle {
                    self.dns_ok.set(true);
                }
                return RunResult::success(String::new());
            }
            if command.contains(&format!("ping -c 1 -W 2 {}", GATEWAY)) {
                let reachable = self.wifi_on.get() && !self.airplane.get();
                return RunResult::success(if reachable {
                    "64 bytes from 10.0.2.2: icmp_seq=1 ttl=255".into()
                } else {
                    "1 packets transmitted, 0 received, 100% packet loss".to_string()
                });
            }
            if command.contains(&format!("ping -c 1 -W 2 {}", PROBE_HOST)) {
                return RunResult::success(if self.dns_ok.get() {
                    format!("PING {} (142.250.72.163) 56 data bytes", PROBE_HOST)
                } else {
                    format!("ping: unknown host {}", PROBE_HOST)
                });
            }
            if command.contains("getprop sys.boot_completed") {
                return RunResult::success("1".into());
            }
            if command.contains("getprop init.svc.bootanim") {
                return RunResult::success("stopped".into());
            }
            if command.contains("scutil --dns") {
                return RunResult::success("  nameserver[0] : 127.0.2.2".into());
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
            self.running.set(true);
            if self.heals_on_restart {
                self.dns_ok.set(true);
            }
            Some(Box::new(FakeProcess))
        }
    }

    fn instant_timings() -> Timings {
        let instant = || PollConfig {
            attempts: 2,
            interval: Duration::from_millis(0),
        };
        Timings {
            recheck: instant(),
            shutdown: instant(),
            serial: instant(),
            boot: instant(),
        }
    }

    fn args() -> AndroidArgs {
        AndroidArgs {
            avd: None,
            dns: None,
            no_restart: false,
            yes: true,
        }
    }

    #[test]
    fn healthy_emulator_is_left_alone() {
        let runner = ScriptedRunner::healthy();
        let code = fix(&args(), &runner, &instant_timings());

        assert_eq!(code, 0);
        assert!(!runner.called("svc wifi"));
        assert!(!runner.called("emu kill"));
    }

    #[test]
    fn disabled_wifi_is_re_enabled_without_restarting() {
        let runner = ScriptedRunner {
            wifi_on: Cell::new(false),
            ..ScriptedRunner::healthy()
        };
        let code = fix(&args(), &runner, &instant_timings());

        assert_eq!(code, 0);
        assert!(runner.called("svc wifi enable"));
        assert!(!runner.called("emu kill"));
    }

    #[test]
    fn airplane_mode_is_turned_off_first() {
        let runner = ScriptedRunner {
            airplane: Cell::new(true),
            ..ScriptedRunner::healthy()
        };
        let code = fix(&args(), &runner, &instant_timings());

        assert_eq!(code, 0);
        assert!(runner.called("airplane-mode disable"));
        assert!(!runner.called("emu kill"));
    }

    #[test]
    fn wifi_toggle_is_tried_before_a_restart() {
        let runner = ScriptedRunner {
            heals_on_wifi_toggle: true,
            ..ScriptedRunner::dns_broken()
        };
        let code = fix(&args(), &runner, &instant_timings());

        assert_eq!(code, 0);
        assert!(runner.called("svc wifi disable"));
        assert!(!runner.called("emu kill"));
    }

    // GUARD: the dead-resolver case only clears on a cold boot carrying -dns-server.
    #[test]
    fn dead_resolver_cold_boots_with_explicit_dns() {
        let runner = ScriptedRunner {
            heals_on_restart: true,
            ..ScriptedRunner::dns_broken()
        };
        let code = fix(&args(), &runner, &instant_timings());

        assert_eq!(code, 0);
        assert!(runner.called("emu kill"));
        assert!(runner.called("-avd Pixel_9 -dns-server 8.8.8.8,1.1.1.1 -no-snapshot-load"));
    }

    #[test]
    fn explicit_dns_flag_wins_over_host_resolvers() {
        let runner = ScriptedRunner {
            heals_on_restart: true,
            ..ScriptedRunner::dns_broken()
        };
        let code = fix(
            &AndroidArgs {
                dns: Some("9.9.9.9, 1.0.0.1".into()),
                ..args()
            },
            &runner,
            &instant_timings(),
        );

        assert_eq!(code, 0);
        assert!(runner.called("-dns-server 9.9.9.9,1.0.0.1"));
    }

    #[test]
    fn no_restart_reports_the_manual_command_instead() {
        let runner = ScriptedRunner::dns_broken();
        let code = fix(
            &AndroidArgs {
                no_restart: true,
                ..args()
            },
            &runner,
            &instant_timings(),
        );

        assert_eq!(code, 1);
        assert!(!runner.called("emu kill"));
    }

    #[test]
    fn restart_that_does_not_help_fails() {
        let runner = ScriptedRunner::dns_broken();
        let code = fix(&args(), &runner, &instant_timings());

        assert_eq!(code, 1);
        assert!(runner.called("emu kill"));
    }

    #[test]
    fn missing_emulator_is_reported() {
        let runner = ScriptedRunner {
            running: Cell::new(false),
            ..ScriptedRunner::healthy()
        };
        let code = fix(&args(), &runner, &instant_timings());

        assert_eq!(code, 1);
    }
}
