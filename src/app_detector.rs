#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::models::{AppInfo, ProjectType};

const MAX_LEVELS: usize = 10;

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
        // Try Flutter project (pubspec.yaml)
        if let Some(root) = find_project_root(start_dir) {
            let info = detect_flutter_project(&root);
            return (info, Some(root));
        }

        // Pure Android
        if let Some(android_root) = find_android_root(start_dir) {
            let android_id = detect_android_id(&android_root);
            let info = AppInfo::new(String::new(), ProjectType::Android, android_id, None);
            return (info, Some(android_root));
        }

        // Pure iOS
        if let Some(ios_root) = find_ios_root(start_dir) {
            let bundle_id = detect_ios_bundle_id(&ios_root);
            let info = AppInfo::new(String::new(), ProjectType::Ios, None, bundle_id);
            return (info, Some(ios_root));
        }

        (AppInfo::new(String::new(), ProjectType::Unknown, None, None), None)
    }
}

impl Default for AppDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn find_project_root(dir: &Path) -> Option<PathBuf> {
    let mut current = dir.to_path_buf();
    for _ in 0..MAX_LEVELS {
        if current.join("pubspec.yaml").exists() {
            return Some(current);
        }
        let parent = match current.parent() {
            Some(p) if p != current => p.to_path_buf(),
            _ => break,
        };
        current = parent;
    }
    None
}

fn find_android_root(dir: &Path) -> Option<PathBuf> {
    let mut current = dir.to_path_buf();
    for _ in 0..MAX_LEVELS {
        // Check for app/build.gradle.kts or app/build.gradle
        if current.join("app").join("build.gradle.kts").exists()
            || current.join("app").join("build.gradle").exists()
        {
            return Some(current);
        }
        // Check if we're inside android/ folder (build.gradle.kts or build.gradle at current)
        if current.join("build.gradle.kts").exists() || current.join("build.gradle").exists() {
            return current.parent().map(|p| p.to_path_buf());
        }
        let parent = match current.parent() {
            Some(p) if p != current => p.to_path_buf(),
            _ => break,
        };
        current = parent;
    }
    None
}

fn find_ios_root(dir: &Path) -> Option<PathBuf> {
    let mut current = dir.to_path_buf();
    for _ in 0..MAX_LEVELS {
        if let Ok(entries) = fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(ext) = path.extension() {
                        if ext == "xcodeproj" {
                            return Some(current);
                        }
                    }
                }
            }
        }
        let parent = match current.parent() {
            Some(p) if p != current => p.to_path_buf(),
            _ => break,
        };
        current = parent;
    }
    None
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
