#![allow(dead_code)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::models::{AppInfo, NodePm, ProjectType, PyFw};

const MAX_LEVELS: usize = 10;
/// Maximum bytes to scan when looking for the FastAPI substring in
/// requirement / project files. 64 KiB is plenty for `pyproject.toml`
/// and `requirements.txt` while bounding cost.
const SCAN_BYTES: usize = 64 * 1024;

// Kotlin DSL: applicationId = "com.example.app"
fn kotlin_dsl_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"applicationId\s*=\s*"([^"]+)""#).unwrap())
}

// Groovy DSL: applicationId 'com.example.app'
fn groovy_dsl_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"applicationId\s+'([^']+)'").unwrap())
}

// AndroidManifest.xml: package="com.example.app"
fn manifest_package_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"package="([^"]+)""#).unwrap())
}

// project.pbxproj: PRODUCT_BUNDLE_IDENTIFIER = com.example.app;
fn bundle_id_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"PRODUCT_BUNDLE_IDENTIFIER\s*=\s*([^;]+);").unwrap())
}

pub struct AppDetector;

impl AppDetector {
    pub fn new() -> Self {
        Self
    }

    pub fn detect(&self, start_dir: &Path) -> AppInfo {
        let (info, _) = self.detect_with_root(start_dir);
        info
    }

    pub fn detect_with_root(&self, start_dir: &Path) -> (AppInfo, Option<PathBuf>) {
        // Walk up to MAX_LEVELS, asking classify_dir for each candidate
        // directory. First non-Unknown match wins (deepest-first walk
        // preserves the previous Flutter/Android/iOS behaviour and
        // gives nested projects priority over their parents).
        let mut current = start_dir.to_path_buf();
        for _ in 0..MAX_LEVELS {
            let pt = classify_dir(&current);
            if pt != ProjectType::Unknown {
                let info = build_app_info(&current, &pt);
                return (info, Some(current));
            }
            // Special case for "we are inside android/" pointing at a
            // pure-Android project root one level up.
            if (current.join("build.gradle.kts").exists()
                || current.join("build.gradle").exists())
                && !current.join("app").join("build.gradle.kts").exists()
                && !current.join("app").join("build.gradle").exists()
            {
                if let Some(parent) = current.parent() {
                    let parent = parent.to_path_buf();
                    let pt = classify_dir(&parent);
                    if pt != ProjectType::Unknown {
                        let info = build_app_info(&parent, &pt);
                        return (info, Some(parent));
                    }
                }
            }
            let parent = match current.parent() {
                Some(p) if p != current => p.to_path_buf(),
                _ => break,
            };
            current = parent;
        }

        (
            AppInfo::new(String::new(), ProjectType::Unknown, None, None),
            None,
        )
    }
}

/// Build an `AppInfo` for a detected project root + type. Carries the
/// flutter name + android/ios identifiers for the legacy variants;
/// other variants get an empty/None payload.
fn build_app_info(root: &Path, pt: &ProjectType) -> AppInfo {
    match pt {
        ProjectType::Flutter => detect_flutter_project(root),
        ProjectType::Android => {
            let android_id = detect_android_id(root);
            AppInfo::new(String::new(), ProjectType::Android, android_id, None)
        }
        ProjectType::Ios => {
            let bundle_id = detect_ios_bundle_id(root);
            AppInfo::new(String::new(), ProjectType::Ios, None, bundle_id)
        }
        other => AppInfo::new(String::new(), other.clone(), None, None),
    }
}

/// Classify a single directory by checking anchor files in the
/// documented precedence order. First match wins; falls back to
/// `ProjectType::Unknown`.
fn classify_dir(dir: &Path) -> ProjectType {
    // 1. Flutter
    if dir.join("pubspec.yaml").exists() {
        return ProjectType::Flutter;
    }
    // 2. Rails (Gemfile + config/application.rb)
    let has_gemfile = dir.join("Gemfile").exists();
    if has_gemfile && dir.join("config").join("application.rb").exists() {
        return ProjectType::Ruby { rails: true };
    }
    // 3. Django (manage.py + python project marker)
    if dir.join("manage.py").exists()
        && (dir.join("pyproject.toml").exists()
            || dir.join("requirements.txt").exists()
            || dir.join("Pipfile").exists())
    {
        return ProjectType::Python {
            framework: Some(PyFw::Django),
        };
    }
    // 4. Python FastAPI — substring scan of pyproject.toml /
    //    requirements.txt (first 64 KiB, case-insensitive).
    let pyproject = dir.join("pyproject.toml");
    let requirements = dir.join("requirements.txt");
    let pyproject_exists = pyproject.exists();
    let requirements_exists = requirements.exists();
    if (pyproject_exists && file_head_contains_ci(&pyproject, "fastapi"))
        || (requirements_exists && file_head_contains_ci(&requirements, "fastapi"))
    {
        return ProjectType::Python {
            framework: Some(PyFw::FastAPI),
        };
    }
    // 5. Python (generic)
    if pyproject_exists
        || requirements_exists
        || dir.join("Pipfile").exists()
        || dir.join("uv.lock").exists()
        || dir.join("poetry.lock").exists()
    {
        return ProjectType::Python {
            framework: Some(PyFw::Generic),
        };
    }
    // 6. Node
    if dir.join("package.json").exists() {
        let manager = if dir.join("pnpm-lock.yaml").exists() {
            NodePm::Pnpm
        } else if dir.join("yarn.lock").exists() {
            NodePm::Yarn
        } else if dir.join("bun.lockb").exists() {
            NodePm::Bun
        } else {
            NodePm::Npm
        };
        return ProjectType::Node { manager };
    }
    // 7. Rust
    if dir.join("Cargo.toml").exists() {
        return ProjectType::Rust;
    }
    // 8. Go
    if dir.join("go.mod").exists() {
        return ProjectType::Go;
    }
    // 9. Ruby (non-Rails) — Gemfile only
    if has_gemfile {
        return ProjectType::Ruby { rails: false };
    }
    // 10. Android (app/build.gradle{,.kts})
    if dir.join("app").join("build.gradle.kts").exists()
        || dir.join("app").join("build.gradle").exists()
    {
        return ProjectType::Android;
    }
    // 11. iOS (*.xcodeproj direct child)
    if dir_has_xcodeproj(dir) {
        return ProjectType::Ios;
    }
    ProjectType::Unknown
}

/// True if `dir` directly contains a `*.xcodeproj` directory.
fn dir_has_xcodeproj(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(ext) = path.extension() {
                if ext == "xcodeproj" {
                    return true;
                }
            }
        }
    }
    false
}

/// Read the first `SCAN_BYTES` of `path` (lossy UTF-8) and return
/// true iff `needle` (already lowercase) is found case-insensitively.
/// Returns false on any I/O error.
fn file_head_contains_ci(path: &Path, needle: &str) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; SCAN_BYTES];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let text = String::from_utf8_lossy(&buf[..n]);
    text.to_lowercase().contains(needle)
}

impl Default for AppDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn detect_flutter_project(root: &Path) -> AppInfo {
    let flutter_name = read_flutter_name(root).unwrap_or_default();
    let android_id = detect_android_id_in_flutter_project(root);
    let bundle_id = detect_ios_bundle_id(root);
    AppInfo::new(flutter_name, ProjectType::Flutter, android_id, bundle_id)
}

fn read_flutter_name(root: &Path) -> Option<String> {
    let content = fs::read_to_string(root.join("pubspec.yaml")).ok()?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
    yaml.get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn detect_android_id_in_flutter_project(root: &Path) -> Option<String> {
    // 1. Kotlin DSL: android/app/build.gradle.kts
    let kts_path = root.join("android").join("app").join("build.gradle.kts");
    if kts_path.exists() {
        if let Some(id) = extract_application_id_from_gradle(&kts_path, true) {
            return Some(id);
        }
    }
    // 2. Groovy DSL: android/app/build.gradle
    let groovy_path = root.join("android").join("app").join("build.gradle");
    if groovy_path.exists() {
        if let Some(id) = extract_application_id_from_gradle(&groovy_path, false) {
            return Some(id);
        }
    }
    // 3. AndroidManifest.xml
    let manifest_path = root
        .join("android")
        .join("app")
        .join("src")
        .join("main")
        .join("AndroidManifest.xml");
    if manifest_path.exists() {
        return extract_package_from_manifest(&manifest_path);
    }
    None
}

fn detect_android_id(android_root: &Path) -> Option<String> {
    // 1. app/build.gradle.kts
    let kts_path = android_root.join("app").join("build.gradle.kts");
    if kts_path.exists() {
        if let Some(id) = extract_application_id_from_gradle(&kts_path, true) {
            return Some(id);
        }
    }
    // 2. app/build.gradle
    let groovy_path = android_root.join("app").join("build.gradle");
    if groovy_path.exists() {
        if let Some(id) = extract_application_id_from_gradle(&groovy_path, false) {
            return Some(id);
        }
    }
    None
}

fn extract_application_id_from_gradle(path: &Path, is_kts: bool) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    if is_kts {
        kotlin_dsl_pattern()
            .captures(&content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    } else {
        groovy_dsl_pattern()
            .captures(&content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    }
}

fn extract_package_from_manifest(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    manifest_package_pattern()
        .captures(&content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

fn detect_ios_bundle_id(root: &Path) -> Option<String> {
    // Try ios/Runner.xcodeproj/project.pbxproj first
    let preferred = root
        .join("ios")
        .join("Runner.xcodeproj")
        .join("project.pbxproj");
    if preferred.exists() {
        if let Ok(content) = fs::read_to_string(&preferred) {
            return parse_bundle_id_from_pbxproj(&content);
        }
    }

    // Fall back to any *.xcodeproj in root
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(ext) = path.extension() {
                    if ext == "xcodeproj" {
                        let pbxproj = path.join("project.pbxproj");
                        if pbxproj.exists() {
                            if let Ok(content) = fs::read_to_string(&pbxproj) {
                                return parse_bundle_id_from_pbxproj(&content);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn parse_bundle_id_from_pbxproj(content: &str) -> Option<String> {
    let release_markers = ["name = Release;", r#"name = "Release";"#, "/* Release */"];
    let debug_markers = ["name = Debug;", r#"name = "Debug";"#, "/* Debug */"];

    let mut in_release_block = false;
    let mut release_value: Option<String> = None;
    let mut fallback_value: Option<String> = None;

    for line in content.lines() {
        // Detect block transitions
        let is_release = release_markers.iter().any(|m| line.contains(m));
        let is_debug = debug_markers.iter().any(|m| line.contains(m));
        if is_release {
            in_release_block = true;
        } else if is_debug {
            in_release_block = false;
        }

        if let Some(caps) = bundle_id_pattern().captures(line) {
            if let Some(m) = caps.get(1) {
                let value = m.as_str().trim().to_string();
                // Skip variable substitutions like $(PRODUCT_BUNDLE_IDENTIFIER)
                if value.starts_with("$(") {
                    continue;
                }
                if in_release_block && release_value.is_none() {
                    release_value = Some(value.clone());
                }
                if fallback_value.is_none() {
                    fallback_value = Some(value);
                }
            }
        }
    }

    release_value.or(fallback_value)
}
