#![allow(dead_code)]

use serde_json::Value;

use crate::models::{Device, DevicePlatform, DeviceType};
use crate::runner::Runner;

pub struct DeviceManager<'a> {
    runner: &'a dyn Runner,
}

impl<'a> DeviceManager<'a> {
    pub fn new(runner: &'a dyn Runner) -> Self {
        Self { runner }
    }

    pub fn list_running_devices(&self) -> Vec<Device> {
        let mut devices: Vec<Device> = Vec::new();

        // Run: flutter devices --machine (returns JSON array)
        let result = self.runner.run("flutter", &["devices", "--machine"], None);
        if result.is_success() && !result.stdout.is_empty() {
            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&result.stdout) {
                for item in arr {
                    if let Value::Object(map) = item {
                        if let Some(device) = parse_flutter_device(&map) {
                            devices.push(device);
                        }
                    }
                }
            }
        }

        // On macOS: also list iOS simulators and merge (dedup by id)
        if cfg!(target_os = "macos") {
            for sim in self.list_ios_simulators() {
                if !devices.iter().any(|d| d.id == sim.id) {
                    devices.push(sim);
                }
            }
        }

        devices
    }

    pub fn list_ios_simulators(&self) -> Vec<Device> {
        let mut simulators: Vec<Device> = Vec::new();

        let result = self
            .runner
            .run("xcrun", &["simctl", "list", "devices", "--json"], None);
        if !result.is_success() || result.stdout.is_empty() {
            return simulators;
        }

        let json: Value = match serde_json::from_str(&result.stdout) {
            Ok(v) => v,
            Err(_) => return simulators,
        };

        // Structure: { "devices": { "<runtime-key>": [ { udid, name, state } ] } }
        let devices_map = match json.get("devices").and_then(|v| v.as_object()) {
            Some(m) => m,
            None => return simulators,
        };

        for (runtime_key, runtime_devices) in devices_map {
            // Only process iOS runtimes
            if !runtime_key.contains("iOS") {
                continue;
            }
            if let Some(arr) = runtime_devices.as_array() {
                for item in arr {
                    let state = item
                        .get("state")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    if state != "Booted" {
                        continue;
                    }
                    let udid = item
                        .get("udid")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if udid.is_empty() {
                        continue;
                    }
                    simulators.push(Device::new(
                        udid,
                        name,
                        DevicePlatform::Ios,
                        DeviceType::Simulator,
                    ));
                }
            }
        }

        simulators
    }
}

fn parse_flutter_device(map: &serde_json::Map<String, Value>) -> Option<Device> {
    let id = map.get("id")?.as_str()?.to_string();
    let name = map.get("name")?.as_str()?.to_string();
    let target_platform = map
        .get("targetPlatform")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let is_emulator = map
        .get("emulator")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if target_platform.contains("android") {
        let device_type = if is_emulator {
            DeviceType::Emulator
        } else {
            DeviceType::Physical
        };
        Some(Device::new(id, name, DevicePlatform::Android, device_type))
    } else if target_platform.contains("ios") {
        let device_type = if is_emulator {
            DeviceType::Simulator
        } else {
            DeviceType::Physical
        };
        Some(Device::new(id, name, DevicePlatform::Ios, device_type))
    } else {
        // Skip web, desktop, etc.
        None
    }
}
