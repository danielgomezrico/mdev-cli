/// Node.js package manager variant.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodePm {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

/// Python framework variant.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyFw {
    Django,
    FastAPI,
    Generic,
}

/// Project type detected from the working directory.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectType {
    Flutter,
    Android,
    Ios,
    Node { manager: NodePm },
    Rust,
    Go,
    Ruby { rails: bool },
    Python { framework: Option<PyFw> },
    Unknown,
}

impl ProjectType {
    /// Returns true for mobile project types (Flutter, Android, iOS).
    #[allow(dead_code)]
    pub fn is_mobile(&self) -> bool {
        matches!(
            self,
            ProjectType::Flutter | ProjectType::Android | ProjectType::Ios
        )
    }

    /// Returns true only for iOS (which requires macOS to clean).
    #[allow(dead_code)]
    pub fn needs_macos(&self) -> bool {
        matches!(self, ProjectType::Ios)
    }

    /// Stable, lowercase label per variant.
    #[allow(dead_code)]
    pub fn name(&self) -> &'static str {
        match self {
            ProjectType::Flutter => "flutter",
            ProjectType::Android => "android",
            ProjectType::Ios => "ios",
            ProjectType::Node { .. } => "node",
            ProjectType::Rust => "rust",
            ProjectType::Go => "go",
            ProjectType::Ruby { rails: true } => "rails",
            ProjectType::Ruby { rails: false } => "ruby",
            ProjectType::Python {
                framework: Some(PyFw::Django),
            } => "django",
            ProjectType::Python {
                framework: Some(PyFw::FastAPI),
            } => "fastapi",
            ProjectType::Python { .. } => "python",
            ProjectType::Unknown => "unknown",
        }
    }
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

/// Human-readable label for a `ProjectType`.
fn project_type_label(pt: &ProjectType) -> String {
    match pt {
        ProjectType::Flutter => "Flutter".to_string(),
        ProjectType::Android => "Android".to_string(),
        ProjectType::Ios => "iOS".to_string(),
        ProjectType::Node { manager } => {
            let pm = match manager {
                NodePm::Npm => "npm",
                NodePm::Pnpm => "pnpm",
                NodePm::Yarn => "yarn",
                NodePm::Bun => "bun",
            };
            format!("Node ({pm})")
        }
        ProjectType::Rust => "Rust".to_string(),
        ProjectType::Go => "Go".to_string(),
        ProjectType::Ruby { rails: true } => "Ruby on Rails".to_string(),
        ProjectType::Ruby { rails: false } => "Ruby".to_string(),
        ProjectType::Python { framework } => match framework {
            Some(PyFw::Django) => "Python (Django)".to_string(),
            Some(PyFw::FastAPI) => "Python (FastAPI)".to_string(),
            Some(PyFw::Generic) | None => "Python".to_string(),
        },
        ProjectType::Unknown => "Unknown".to_string(),
    }
}

impl std::fmt::Display for AppInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AppInfo(type: {}, android: {:?}, ios: {:?}, name: {})",
            project_type_label(&self.project_type),
            self.android_package_id,
            self.ios_bundle_id,
            self.flutter_name
        )
    }
}

/// The platform a device runs on.
#[derive(Debug, Clone, PartialEq)]
pub enum DevicePlatform {
    Android,
    Ios,
}

impl DevicePlatform {
    pub fn label(&self) -> &'static str {
        match self {
            DevicePlatform::Android => "Android",
            DevicePlatform::Ios => "iOS",
        }
    }

    pub fn from_device_id(device_id: &str) -> Self {
        let looks_ios = device_id.len() == 36 && device_id.matches('-').count() == 4;
        if looks_ios {
            DevicePlatform::Ios
        } else {
            DevicePlatform::Android
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_platform_label_android() {
        assert_eq!(DevicePlatform::Android.label(), "Android");
    }

    #[test]
    fn device_platform_label_ios() {
        assert_eq!(DevicePlatform::Ios.label(), "iOS");
    }

    #[test]
    fn device_platform_from_device_id_ios_uuid() {
        let uuid = "12345678-1234-1234-1234-123456789012";
        assert_eq!(DevicePlatform::from_device_id(uuid), DevicePlatform::Ios);
    }

    #[test]
    fn device_platform_from_device_id_android() {
        assert_eq!(
            DevicePlatform::from_device_id("emulator-5554"),
            DevicePlatform::Android
        );
    }

    // GUARD: label strings are exactly "Android" and "iOS" (case-sensitive pin)
    #[test]
    fn label_android_exact_string() {
        assert_eq!(DevicePlatform::Android.label(), "Android");
    }

    #[test]
    fn label_ios_exact_string() {
        assert_eq!(DevicePlatform::Ios.label(), "iOS");
    }

    // GUARD: empty string → len 0 ≠ 36 → Android
    #[test]
    fn from_device_id_empty_string() {
        assert_eq!(DevicePlatform::from_device_id(""), DevicePlatform::Android);
    }

    // GUARD: 36-char string with wrong dash count (3 dashes) → Android
    #[test]
    fn from_device_id_36_chars_wrong_dash_count() {
        // len==36, but only 3 dashes (not 4) → Android
        // Replace the 4th dash with 'X': "12345678-1234-1234-1234X123456789012"
        let id = "12345678-1234-1234-1234X123456789012";
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 3);
        assert_eq!(DevicePlatform::from_device_id(id), DevicePlatform::Android);
    }

    // GUARD: short UUID-ish string (len < 36) → Android regardless of dash count
    #[test]
    fn from_device_id_short_uuid_ish() {
        let id = "abc-def-ghi-jkl-m"; // len < 36, has dashes
        assert!(id.len() < 36);
        assert_eq!(DevicePlatform::from_device_id(id), DevicePlatform::Android);
    }
}
