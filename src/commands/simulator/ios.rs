use clap::Args;
use serde_json::Value;

use crate::logger::Logger;
use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct IosArgs {
    /// Case-insensitive substring of the simulator name to boot.
    #[arg(short = 'd', long, default_value = "iPhone")]
    pub device: String,
}

/// A bootable simulator plus the runtime it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct SimDevice {
    pub udid: String,
    pub name: String,
    pub runtime: String,
    pub booted: bool,
}

pub fn run(args: &IosArgs, runner: &dyn Runner) -> i32 {
    let logger = Logger::new();

    if !cfg!(target_os = "macos") {
        logger.err("iOS simulators require macOS with Xcode");
        return 1;
    }

    let listed = runner.run(
        "xcrun",
        &["simctl", "list", "devices", "available", "--json"],
        None,
    );
    if !listed.is_success() {
        logger.err("Could not list simulators — is Xcode installed?");
        if !listed.stderr.is_empty() {
            logger.detail(&listed.stderr);
        }
        return 1;
    }

    let device = match select_device(&listed.stdout, &args.device) {
        Some(d) => d,
        None => {
            logger.err(&format!(
                "No available iOS simulator matching '{}'",
                args.device
            ));
            logger.detail("List them with: xcrun simctl list devices available");
            return 1;
        }
    };

    if device.booted {
        logger.info(&format!(
            "{} ({}) already booted",
            device.name, device.runtime
        ));
    } else {
        logger.info(&format!("Booting {} ({})", device.name, device.runtime));
    }

    // `bootstatus -b` boots the device when needed and blocks until boot finishes;
    // it is safe to call on an already-booted device.
    let progress = logger.progress(&format!("Waiting for {} to boot", device.name));
    let boot = runner.run("xcrun", &["simctl", "bootstatus", &device.udid, "-b"], None);
    progress.finish_and_clear();

    if !boot.is_success() {
        logger.err(&format!("Failed to boot {}", device.name));
        let reason = if boot.stderr.is_empty() {
            &boot.stdout
        } else {
            &boot.stderr
        };
        if !reason.is_empty() {
            logger.detail(reason);
        }
        return 1;
    }

    // Open Simulator.app *after* booting: `open --args -CurrentDeviceUDID` is
    // ignored when the app already runs, but it always shows booted devices.
    runner.run("open", &["-a", "Simulator"], None);

    logger.success(&format!(
        "Simulator ready: {} ({}) — {}",
        device.name, device.runtime, device.udid
    ));
    0
}

/// Picks, from `xcrun simctl list devices available --json`, an already-booted
/// match first, otherwise the first match under the newest iOS runtime.
pub fn select_device(json: &str, want: &str) -> Option<SimDevice> {
    let parsed: Value = serde_json::from_str(json).ok()?;
    let runtimes = parsed.get("devices")?.as_object()?;
    let want = want.to_lowercase();

    let mut booted: Option<SimDevice> = None;
    let mut newest: Option<((u32, u32), SimDevice)> = None;

    for (runtime_id, devices) in runtimes {
        let Some(version) = ios_runtime_version(runtime_id) else {
            continue;
        };
        let Some(devices) = devices.as_array() else {
            continue;
        };
        let runtime_label = format!("iOS {}.{}", version.0, version.1);

        for item in devices {
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let udid = item
                .get("udid")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let available = item
                .get("isAvailable")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            if udid.is_empty() || !available || !name.to_lowercase().contains(&want) {
                continue;
            }

            let device = SimDevice {
                udid: udid.to_string(),
                name: name.to_string(),
                runtime: runtime_label.clone(),
                booted: item.get("state").and_then(|v| v.as_str()) == Some("Booted"),
            };

            if device.booted && booted.is_none() {
                booted = Some(device.clone());
            }
            // Keep the first device of the highest runtime seen so far.
            if newest.as_ref().is_none_or(|(best, _)| version > *best) {
                newest = Some((version, device));
            }
        }
    }

    booted.or(newest.map(|(_, device)| device))
}

/// `(major, minor)` for an iOS runtime key, `None` for tvOS/watchOS/xrOS.
/// Handles both `com.apple.CoreSimulator.SimRuntime.iOS-26-5` and `iOS 26.5`.
fn ios_runtime_version(runtime_id: &str) -> Option<(u32, u32)> {
    let tail = runtime_id.rsplit('.').next()?;
    let version = tail
        .strip_prefix("iOS-")
        .or_else(|| runtime_id.strip_prefix("iOS "))?;
    let mut parts = version.split(['-', '.']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(udid: &str, name: &str, state: &str) -> serde_json::Value {
        serde_json::json!({
            "udid": udid,
            "name": name,
            "state": state,
            "isAvailable": true,
        })
    }

    fn listing(runtimes: serde_json::Value) -> String {
        serde_json::json!({ "devices": runtimes }).to_string()
    }

    const IOS_26_3: &str = "com.apple.CoreSimulator.SimRuntime.iOS-26-3";
    const IOS_26_5: &str = "com.apple.CoreSimulator.SimRuntime.iOS-26-5";

    #[test]
    fn picks_first_match_of_newest_runtime() {
        let json = listing(serde_json::json!({
            IOS_26_3: [device("OLD", "iPhone 17 Pro", "Shutdown")],
            IOS_26_5: [
                device("NEW", "iPhone 17 Pro", "Shutdown"),
                device("NEW2", "iPhone 17", "Shutdown"),
            ],
        }));

        let picked = select_device(&json, "iPhone").unwrap();
        assert_eq!(picked.udid, "NEW");
        assert_eq!(picked.runtime, "iOS 26.5");
    }

    // GUARD: a booted device wins even when it lives on an older runtime.
    #[test]
    fn prefers_booted_device_over_newer_runtime() {
        let json = listing(serde_json::json!({
            IOS_26_3: [device("OLD", "iPhone 17 Pro", "Booted")],
            IOS_26_5: [device("NEW", "iPhone 17 Pro", "Shutdown")],
        }));

        let picked = select_device(&json, "iPhone").unwrap();
        assert_eq!(picked.udid, "OLD");
        assert!(picked.booted);
    }

    #[test]
    fn matches_name_case_insensitively() {
        let json = listing(serde_json::json!({
            IOS_26_5: [device("A", "iPad Pro 13-inch (M5)", "Shutdown")],
        }));

        assert_eq!(select_device(&json, "ipad pro").unwrap().udid, "A");
    }

    #[test]
    fn no_match_returns_none() {
        let json = listing(serde_json::json!({
            IOS_26_5: [device("A", "iPhone 17 Pro", "Shutdown")],
        }));

        assert_eq!(select_device(&json, "Nokia"), None);
    }

    // GUARD: non-iOS runtimes must never be booted by this command.
    #[test]
    fn skips_non_ios_runtimes() {
        let json = listing(serde_json::json!({
            "com.apple.CoreSimulator.SimRuntime.watchOS-26-0": [
                device("W", "Apple Watch Ultra 3", "Shutdown"),
            ],
            "com.apple.CoreSimulator.SimRuntime.tvOS-26-0": [
                device("T", "Apple TV 4K", "Shutdown"),
            ],
        }));

        assert_eq!(select_device(&json, "Apple"), None);
    }

    // GUARD: unavailable devices cannot boot, so they must be skipped.
    #[test]
    fn skips_unavailable_devices() {
        let json = listing(serde_json::json!({
            IOS_26_5: [{
                "udid": "A",
                "name": "iPhone 17 Pro",
                "state": "Shutdown",
                "isAvailable": false,
            }],
        }));

        assert_eq!(select_device(&json, "iPhone"), None);
    }

    // GUARD: 9.x must not sort above 26.x (lexical comparison would).
    #[test]
    fn compares_versions_numerically_not_lexically() {
        let json = listing(serde_json::json!({
            "com.apple.CoreSimulator.SimRuntime.iOS-9-3": [device("NINE", "iPhone", "Shutdown")],
            "com.apple.CoreSimulator.SimRuntime.iOS-26-0": [device("TWENTYSIX", "iPhone", "Shutdown")],
        }));

        assert_eq!(select_device(&json, "iPhone").unwrap().udid, "TWENTYSIX");
    }

    #[test]
    fn parses_legacy_runtime_key_format() {
        assert_eq!(ios_runtime_version("iOS 13.7"), Some((13, 7)));
        assert_eq!(ios_runtime_version("iOS-26-5"), Some((26, 5)));
        assert_eq!(ios_runtime_version(IOS_26_5), Some((26, 5)));
        assert_eq!(ios_runtime_version("iOS-18"), Some((18, 0)));
    }

    #[test]
    fn malformed_json_returns_none() {
        assert_eq!(select_device("not json", "iPhone"), None);
        assert_eq!(select_device("{}", "iPhone"), None);
    }

    // GUARD: a runtime whose value is not an array must be skipped, not panic.
    #[test]
    fn non_array_runtime_value_is_skipped() {
        let json = listing(serde_json::json!({
            IOS_26_3: "unexpected",
            IOS_26_5: [device("A", "iPhone 17", "Shutdown")],
        }));

        assert_eq!(select_device(&json, "iPhone").unwrap().udid, "A");
    }
}
