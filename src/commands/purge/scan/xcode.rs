//! Xcode and iOS simulator leftovers. Only yields candidates when
//! `~/Library/Developer` exists (macOS).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::super::candidate::{display_path, Candidate, Category, Freed, Size, Target, Tier};
use super::{child_dirs, existing, process_running, tier_unless_busy, ScanContext, Scanner};

pub struct XcodeScanner;

/// How many newest DeviceSupport versions stay unchecked by default.
const KEEP_DEVICE_SUPPORT: usize = 2;
/// Processes that write DerivedData.
const XCODE_PROCESS: &str = "Xcode.app/Contents/MacOS/Xcode|xcodebuild";

impl Scanner for XcodeScanner {
    fn category(&self) -> Category {
        Category::Xcode
    }

    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
        let dev = ctx.home.join("Library").join("Developer");
        if !dev.is_dir() {
            return Vec::new();
        }
        let xcode = dev.join("Xcode");
        let mut out = Vec::new();
        let name_of = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };

        let derived = child_dirs(&xcode.join("DerivedData"));
        if !derived.is_empty() {
            let tier = tier_unless_busy(process_running(ctx, XCODE_PROCESS));
            for p in derived {
                out.push(Candidate::new(
                    Category::Xcode,
                    "DerivedData",
                    name_of(&p),
                    Target::dir(p),
                    Size::Unknown,
                    tier,
                ));
            }
        }

        for p in child_dirs(&xcode.join("Archives")) {
            out.push(Candidate::new(
                Category::Xcode,
                "Archives",
                name_of(&p),
                Target::dir(p),
                Size::Unknown,
                Tier::Global,
            ));
        }

        for platform in ["iOS", "watchOS", "tvOS", "visionOS"] {
            let group = format!("{platform} DeviceSupport");
            let dir = xcode.join(&group);
            let dirs = child_dirs(&dir);
            let names: Vec<String> = dirs.iter().map(|p| name_of(p)).collect();
            let keep = newest_versions(&names, KEEP_DEVICE_SUPPORT);
            for (p, name) in dirs.into_iter().zip(names) {
                let tier = if keep.contains(&name) {
                    Tier::Global
                } else {
                    Tier::Local
                };
                out.push(Candidate::new(
                    Category::Xcode,
                    group.clone(),
                    name,
                    Target::dir(p),
                    Size::Unknown,
                    tier,
                ));
            }
        }

        let mut misc = vec![
            xcode.join("Products"),
            ctx.home
                .join("Library")
                .join("Caches")
                .join("com.apple.dt.Xcode"),
        ];
        let sim = dev.join("CoreSimulator");
        misc.push(sim.join("Caches"));
        for p in existing(misc) {
            out.push(Candidate::new(
                Category::Xcode,
                "Xcode caches",
                display_path(&p, &ctx.home),
                Target::dir(p),
                Size::Unknown,
                Tier::Local,
            ));
        }

        out.extend(simulators(ctx, &sim.join("Devices")));
        out
    }
}

/// Names whose leading `major.minor[.patch]` version ranks among the `keep`
/// newest. Names without a version are never kept.
pub fn newest_versions(names: &[String], keep: usize) -> Vec<String> {
    let mut versioned: Vec<(Vec<u32>, &String)> = names
        .iter()
        .filter_map(|n| leading_version(n).map(|v| (v, n)))
        .collect();
    versioned.sort_by(|a, b| b.0.cmp(&a.0));
    versioned
        .into_iter()
        .take(keep)
        .map(|(_, n)| n.clone())
        .collect()
}

fn leading_version(name: &str) -> Option<Vec<u32>> {
    let head: String = name
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if head.is_empty() {
        return None;
    }
    let parts: Vec<u32> = head
        .split('.')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    (!parts.is_empty()).then_some(parts)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimDevice {
    pub udid: String,
    pub name: String,
    pub runtime: String,
    pub available: bool,
    pub booted: bool,
}

/// Parse `xcrun simctl list devices -j`.
pub fn parse_simctl(json: &str) -> Vec<SimDevice> {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Some(devices) = v.get("devices").and_then(|d| d.as_object()) {
        for (runtime, list) in devices {
            let runtime_label = runtime
                .rsplit('.')
                .next()
                .unwrap_or(runtime)
                .replace('-', ".");
            for d in list.as_array().into_iter().flatten() {
                let udid = d.get("udid").and_then(|u| u.as_str()).unwrap_or("");
                if udid.is_empty() {
                    continue;
                }
                out.push(SimDevice {
                    udid: udid.to_string(),
                    name: d
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string(),
                    runtime: runtime_label.clone(),
                    available: d
                        .get("isAvailable")
                        .and_then(|a| a.as_bool())
                        .unwrap_or(true),
                    booted: d.get("state").and_then(|s| s.as_str()) == Some("Booted"),
                });
            }
        }
    }
    out
}

fn simulators(ctx: &ScanContext, devices_dir: &Path) -> Vec<Candidate> {
    let dirs = child_dirs(devices_dir);
    if dirs.is_empty() {
        return Vec::new();
    }
    let known: HashMap<String, SimDevice> = if ctx.runner.which("xcrun").is_some() {
        ctx.note("xcrun simctl list devices");
        let listed = ctx
            .runner
            .run("xcrun", &["simctl", "list", "devices", "-j"], None);
        if listed.is_success() {
            parse_simctl(&listed.stdout)
                .into_iter()
                .map(|d| (d.udid.clone(), d))
                .collect()
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };

    let mut out = Vec::new();
    let mut unavailable: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let udid = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let data = dir.join("data");
        match known.get(&udid) {
            Some(d) if d.booted => {}
            Some(d) if !d.available => unavailable.push(dir),
            Some(d) if data.is_dir() => {
                out.push(sim_data(data, format!("{} ({})", d.name, d.runtime)))
            }
            Some(_) => {}
            None if known.is_empty() => {
                if data.is_dir() {
                    out.push(sim_data(data, udid));
                }
            }
            // Not known to simctl: `delete unavailable` would not touch it.
            None => out.push(Candidate::new(
                Category::Xcode,
                "Simulators",
                format!("{udid} (orphaned)"),
                Target::dir(dir),
                Size::Unknown,
                Tier::Global,
            )),
        }
    }
    if !unavailable.is_empty() {
        // Global: sims of a runtime that is merely unmounted show as unavailable.
        out.push(Candidate::new(
            Category::Xcode,
            "Simulators",
            format!("{} unavailable simulator(s)", unavailable.len()),
            Target::action(
                "xcrun",
                &["simctl", "delete", "unavailable"],
                None,
                Freed::Paths(unavailable),
            ),
            Size::Skipped,
            Tier::Global,
        ));
    }
    out
}

fn sim_data(data: PathBuf, label: String) -> Candidate {
    Candidate::new(
        Category::Xcode,
        "Simulator device data",
        label,
        Target::dir(data),
        Size::Unknown,
        Tier::Global,
    )
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, FakeRunner};
    use super::*;
    use crate::runner::RunResult;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn newest_versions_keeps_two_highest_and_ignores_unversioned() {
        let names: Vec<String> = [
            "16.4 (20E247)",
            "17.0.1 (21A340)",
            "15.5 (19F70)",
            "17.2 (21C62)",
            "Symbols",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let keep = newest_versions(&names, 2);
        assert_eq!(keep, vec!["17.2 (21C62)", "17.0.1 (21A340)"]);
    }

    #[test]
    fn parse_simctl_reads_runtime_and_availability() {
        let json = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-17-2":[
            {"udid":"AAA","name":"iPhone 15","isAvailable":true},
            {"udid":"BBB","name":"Old","isAvailable":false}]}}"#;
        let got = parse_simctl(json);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].runtime, "iOS.17.2");
        assert!(got[0].available);
        assert!(!got[1].available);
        assert!(parse_simctl("not json").is_empty());
    }

    #[test]
    fn scanner_groups_derived_data_device_support_and_simulators() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let xcode = home.join("Library/Developer/Xcode");
        fs::create_dir_all(xcode.join("DerivedData/App-abc")).unwrap();
        fs::create_dir_all(xcode.join("DerivedData/ModuleCache.noindex")).unwrap();
        fs::create_dir_all(xcode.join("Archives/2026-01-01")).unwrap();
        for v in ["15.5 (19F70)", "16.4 (20E247)", "17.2 (21C62)"] {
            fs::create_dir_all(xcode.join("iOS DeviceSupport").join(v)).unwrap();
        }
        let devices = home.join("Library/Developer/CoreSimulator/Devices");
        fs::create_dir_all(devices.join("AAA/data")).unwrap();
        fs::create_dir_all(devices.join("BBB/data")).unwrap();
        fs::write(devices.join("BBB/data/blob"), vec![0u8; 100]).unwrap();
        let json = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-17-2":[
            {"udid":"AAA","name":"iPhone 15","isAvailable":true},
            {"udid":"BBB","name":"Old","isAvailable":false}]}}"#;
        let runner = FakeRunner::new(&["xcrun"]).respond("xcrun", RunResult::success(json.into()));
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], home, vec![], &runner);

        let found = XcodeScanner.scan(&c);

        let find = |label: &str| {
            found
                .iter()
                .find(|c| c.label == label)
                .unwrap_or_else(|| panic!("{label}: {found:?}"))
        };
        assert_eq!(find("App-abc").group, "DerivedData");
        assert_eq!(find("2026-01-01").tier, Tier::Global);
        assert_eq!(find("17.2 (21C62)").tier, Tier::Global);
        assert_eq!(find("16.4 (20E247)").tier, Tier::Global);
        assert_eq!(find("15.5 (19F70)").tier, Tier::Local);
        assert_eq!(find("iPhone 15 (iOS.17.2)").tier, Tier::Global);
        let unavailable = find("1 unavailable simulator(s)");
        assert_eq!(unavailable.size, Size::Skipped, "Global is never sized");
        assert!(matches!(unavailable.target, Target::Action { .. }));
    }

    fn sim_fixture(home: &Path) {
        let devices = home.join("Library/Developer/CoreSimulator/Devices");
        for udid in ["AAA", "BBB", "CCC", "DDD"] {
            fs::create_dir_all(devices.join(udid).join("data")).unwrap();
        }
    }

    const SIMS: &str = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-17-2":[
        {"udid":"AAA","name":"iPhone 15","isAvailable":true,"state":"Shutdown"},
        {"udid":"BBB","name":"Old","isAvailable":false,"state":"Shutdown"},
        {"udid":"DDD","name":"Live","isAvailable":true,"state":"Booted"}]}}"#;

    #[test]
    fn unavailable_sims_are_global_and_orphan_dirs_listed_on_their_own() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        sim_fixture(home);
        let runner = FakeRunner::new(&["xcrun"]).respond("xcrun", RunResult::success(SIMS.into()));
        let roots: Vec<PathBuf> = vec![];
        let found = XcodeScanner.scan(&ctx(&roots, &[], home, vec![], &runner));

        let action = found
            .iter()
            .find(|c| matches!(c.target, Target::Action { .. }))
            .unwrap_or_else(|| panic!("{found:?}"));
        assert_eq!(action.tier, Tier::Global);
        let Target::Action {
            freed: Freed::Paths(affects),
            ..
        } = &action.target
        else {
            unreachable!()
        };
        assert_eq!(affects.len(), 1);
        assert!(affects[0].ends_with("BBB"));
        let orphan = found
            .iter()
            .find(|c| c.target.path().is_some_and(|p| p.ends_with("CCC")))
            .unwrap_or_else(|| panic!("{found:?}"));
        assert_eq!(orphan.tier, Tier::Global);
    }

    #[test]
    fn booted_simulator_data_is_not_offered() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        sim_fixture(home);
        let runner = FakeRunner::new(&["xcrun"]).respond("xcrun", RunResult::success(SIMS.into()));
        let roots: Vec<PathBuf> = vec![];
        let found = XcodeScanner.scan(&ctx(&roots, &[], home, vec![], &runner));
        assert!(found.iter().any(|c| c.label.starts_with("iPhone 15")));
        assert!(
            !found.iter().any(|c| c.label.starts_with("Live")),
            "{found:?}"
        );
    }

    #[test]
    fn derived_data_is_global_while_xcode_runs() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join("Library/Developer/Xcode/DerivedData/App-abc")).unwrap();
        let roots: Vec<PathBuf> = vec![];
        let tier = |runner: &FakeRunner| {
            XcodeScanner.scan(&ctx(&roots, &[], home, vec![], runner))[0].tier
        };
        let busy = FakeRunner::new(&["pgrep"]).respond("pgrep", RunResult::success("1".into()));
        let idle =
            FakeRunner::new(&["pgrep"]).respond("pgrep", RunResult::failure(1, String::new()));
        assert_eq!(tier(&busy), Tier::Global);
        assert_eq!(tier(&idle), Tier::Local);
        assert_eq!(tier(&FakeRunner::new(&[])), Tier::Global);
    }

    #[test]
    fn no_developer_dir_means_no_candidates() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(&[]);
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], tmp.path(), vec![], &runner);
        assert!(XcodeScanner.scan(&c).is_empty());
    }
}
