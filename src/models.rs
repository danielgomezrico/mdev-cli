/// Project type detected from the working directory.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectType {
    Flutter,
    Android,
    Ios,
    Unknown,
}

/// Information about the current app project.
#[derive(Debug, Clone, PartialEq)]
pub struct AppInfo {
    pub android_package_id: Option<String>,
    pub ios_bundle_id: Option<String>,
    /// pubspec.yaml `name` field; empty string for non-Flutter projects.
    pub flutter_name: String,
    pub project_type: ProjectType,
}

impl AppInfo {
    pub fn new(
        flutter_name: String,
        project_type: ProjectType,
        android_package_id: Option<String>,
        ios_bundle_id: Option<String>,
    ) -> Self {
        Self {
            android_package_id,
            ios_bundle_id,
            flutter_name,
            project_type,
        }
    }
}

impl std::fmt::Display for AppInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AppInfo(type: {:?}, android: {:?}, ios: {:?}, name: {})",
            self.project_type, self.android_package_id, self.ios_bundle_id, self.flutter_name
        )
    }
}

/// The platform a device runs on.
#[derive(Debug, Clone, PartialEq)]
pub enum DevicePlatform {
    Android,
    Ios,
}

/// Whether a device is an emulator/simulator or physical hardware.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceType {
    Emulator,
    Simulator,
    Physical,
}

/// A connected or available device.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: DevicePlatform,
    pub device_type: DeviceType,
}

impl Device {
    pub fn new(
        id: String,
        name: String,
        platform: DevicePlatform,
        device_type: DeviceType,
    ) -> Self {
        Self {
            id,
            name,
            platform,
            device_type,
        }
    }
}

impl std::fmt::Display for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Device(id: {}, name: {}, platform: {:?}, type: {:?})",
            self.id, self.name, self.platform, self.device_type
        )
    }
}
