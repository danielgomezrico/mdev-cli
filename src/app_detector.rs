#![allow(dead_code)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::models::{AppInfo, NodePm, ProjectType, PyFw};

const MAX_LEVELS: usize = 10;
/// Maximum directory depth descended when recursively discovering projects
/// under a starting directory (e.g. a workspace folder holding many repos at
/// `~/projects/<group>/<repo>`).
const MAX_DISCOVERY_DEPTH: usize = 6;
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

// Indirect Kotlin DSL: applicationId = APPLICATION_ID  (unquoted identifier,
// typically a constant defined in a convention plugin).
fn application_id_ref_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"applicationId\s*=\s*([A-Za-z_][A-Za-z0-9_]*)").unwrap())
}

// AGP namespace literal: namespace = "com.example.app"  (Kotlin or Groovy).
fn namespace_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"namespace\s*=?\s*["']([^"']+)["']"#).unwrap())
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

    /// Recursively discover every project rooted at or below `start_dir`.
    ///
    /// Where [`Self::detect_with_root`] walks *up* to find the single project
    /// that owns `start_dir`, this walks *down*: it classifies `start_dir` and
    /// each descendant, records the first project root found on every branch,
    /// and stops descending once a project is found (its internals belong to
    /// that project). Bounded by [`MAX_DISCOVERY_DEPTH`]; skips
    /// dependency/build/VCS directories. Results are sorted by path.
    pub fn discover_projects(&self, start_dir: &Path) -> Vec<(AppInfo, PathBuf)> {
        let mut found: Vec<(AppInfo, PathBuf)> = Vec::new();
        discover_recursive(start_dir, 0, &mut found, true);
        found.sort_by(|a, b| a.1.cmp(&b.1));
        found
    }
}

/// Directory names never descended into during recursive project discovery:
/// dependency/build/VCS dirs that never host a sibling project we care about
/// and only inflate the walk cost.
fn is_discovery_skip_dir(name: &str) -> bool {
    matches!(
        name,
        // Dependency / build output
        "node_modules"
            | "target"
            | "build"
            | "Pods"
            | ".dart_tool"
            | "DerivedData"
            | "vendor"
            | "__pycache__"
            | ".venv"
            | "venv"
            | "env"
            | ".tox"
            | ".next"
            | ".nuxt"
            | ".turbo"
            // macOS home-folder containers that never hold user repos
            | "Library"
            | "Applications"
            | "Movies"
            | "Music"
            | "Pictures"
            | "Public"
            | "Parallels"
            // Editor / package-manager internals
            | "Caches"
            | "store"
            // SDK / module caches living directly under $HOME
            | "fvm"
            | "go"
            | "pkg"
            | "mod"
    )
}

/// Flutter platform subdirectories are part of the parent app, not separate
/// projects — skip them when descending from a Flutter scan root.
fn is_flutter_platform_subdir(parent_pt: &ProjectType, child_name: &str) -> bool {
    matches!(parent_pt, ProjectType::Flutter)
        && matches!(
            child_name,
            "android" | "ios" | "web" | "linux" | "macos" | "windows"
        )
}

/// Folder-by-folder discovery from `start_dir`:
///   • **Scan root** (the directory the command was invoked in): classify
///     self, then always walk children first. Container dirs like `$HOME`
///     often carry stray `package.json` / `Gemfile` markers — if any child
///     project exists, the root is not recorded. Only when no child project
///     is found is the scan root kept (e.g. you `cd`'d into a single repo).
///   • **Deeper levels**: classify self; on match record and stop (project
///     internals are not separate projects); on unknown recurse into children.
fn discover_recursive(
    dir: &Path,
    depth: usize,
    out: &mut Vec<(AppInfo, PathBuf)>,
    is_scan_root: bool,
) {
    let pt = classify_dir(dir);
    let is_project = pt != ProjectType::Unknown;

    if is_scan_root {
        discover_children(dir, depth, out, &pt, true);
        return;
    }

    if is_project {
        out.push((build_app_info(dir, &pt), dir.to_path_buf()));
        return;
    }

    discover_children(dir, depth, out, &ProjectType::Unknown, false);
}

fn discover_children(
    dir: &Path,
    depth: usize,
    out: &mut Vec<(AppInfo, PathBuf)>,
    parent_pt: &ProjectType,
    is_scan_root: bool,
) {
    if depth >= MAX_DISCOVERY_DEPTH {
        if is_scan_root && *parent_pt != ProjectType::Unknown {
            out.push((build_app_info(dir, parent_pt), dir.to_path_buf()));
        }
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        if is_scan_root && *parent_pt != ProjectType::Unknown {
            out.push((build_app_info(dir, parent_pt), dir.to_path_buf()));
        }
        return;
    };

    let mut child_found = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || is_discovery_skip_dir(name) {
            continue;
        }
        if is_scan_root && is_flutter_platform_subdir(parent_pt, name) {
            continue;
        }
        let before = out.len();
        discover_recursive(&path, depth + 1, out, false);
        if out.len() > before {
            child_found = true;
        }
    }

    if is_scan_root && !child_found && *parent_pt != ProjectType::Unknown {
        out.push((build_app_info(dir, parent_pt), dir.to_path_buf()));
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
    // 9. Android (app/build.gradle{,.kts}) — a strong native anchor must beat
    //    the bare-Gemfile Ruby fallback below, since native projects commonly
    //    carry a fastlane Gemfile at their root.
    if dir.join("app").join("build.gradle.kts").exists()
        || dir.join("app").join("build.gradle").exists()
    {
        return ProjectType::Android;
    }
    // 10. iOS (*.xcodeproj direct child)
    if dir_has_xcodeproj(dir) {
        return ProjectType::Ios;
    }
    // 11. Ruby (non-Rails) — Gemfile only (weak signal; checked last)
    if has_gemfile {
        return ProjectType::Ruby { rails: false };
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
        if let Some(id) = extract_package_from_manifest(&manifest_path) {
            return Some(id);
        }
    }
    // 4. Indirect: convention plugins / const refs / namespace.
    detect_android_id_fallback(
        &[
            root.join("android").join("app"),
            root.join("android").join("build-logic"),
        ],
        &manifest_path,
    )
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
    // 3. Indirect: convention plugins / const refs / namespace / manifest.
    detect_android_id_fallback(
        &[android_root.join("app"), android_root.join("build-logic")],
        &android_root
            .join("app")
            .join("src")
            .join("main")
            .join("AndroidManifest.xml"),
    )
}

/// Resolve an Android application id when no literal `applicationId = "..."`
/// exists in `app/build.gradle{,.kts}` — e.g. projects that set it from a
/// convention plugin via a Kotlin constant (`applicationId = APPLICATION_ID`)
/// or only declare an AGP `namespace`. `scan_roots` are directory trees to
/// search for gradle/kotlin sources; `manifest` is the last-resort
/// `AndroidManifest.xml` carrying a legacy `package="..."`.
fn detect_android_id_fallback(scan_roots: &[PathBuf], manifest: &Path) -> Option<String> {
    let mut files = Vec::new();
    for root in scan_roots {
        collect_gradle_kotlin_files(root, 0, &mut files);
    }

    // 1. A literal applicationId anywhere (commonly inside a convention plugin).
    for f in &files {
        if let Ok(c) = fs::read_to_string(f) {
            if let Some(id) = captured(kotlin_dsl_pattern(), &c)
                .or_else(|| captured(groovy_dsl_pattern(), &c))
            {
                return Some(id);
            }
        }
    }
    // 2. `applicationId = IDENT` resolved against a `val IDENT = "..."` constant.
    for f in &files {
        if let Ok(c) = fs::read_to_string(f) {
            if let Some(ident) = captured(application_id_ref_pattern(), &c) {
                if let Some(id) = resolve_kotlin_string_const(&files, &ident) {
                    return Some(id);
                }
            }
        }
    }
    // 3. AGP `namespace = "..."` literal (defaults to the application id).
    for f in &files {
        if let Ok(c) = fs::read_to_string(f) {
            if let Some(ns) = captured(namespace_pattern(), &c) {
                return Some(ns);
            }
        }
    }
    // 4. Legacy AndroidManifest.xml `package="..."`.
    if manifest.exists() {
        return extract_package_from_manifest(manifest);
    }
    None
}

/// Recursively collect `*.gradle`, `*.gradle.kts`, and `*.kt` files under
/// `dir`, skipping `build` output directories. Bounded by `MAX_LEVELS` depth.
fn collect_gradle_kotlin_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_LEVELS {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("build") {
                continue;
            }
            collect_gradle_kotlin_files(&path, depth + 1, out);
        } else {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".gradle.kts") || name.ends_with(".gradle") || name.ends_with(".kt")
            {
                out.push(path);
            }
        }
    }
}

/// Find a Kotlin `(const) val IDENT = "..."` string constant across `files`.
fn resolve_kotlin_string_const(files: &[PathBuf], ident: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"\bval\s+{}\s*(?::\s*String\s*)?=\s*"([^"]+)""#,
        regex::escape(ident)
    ))
    .ok()?;
    for f in files {
        if let Ok(c) = fs::read_to_string(f) {
            if let Some(id) = captured(&re, &c) {
                return Some(id);
            }
        }
    }
    None
}

/// First capture group of `re` in `text`, as an owned String.
fn captured(re: &Regex, text: &str) -> Option<String> {
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Marker descriptor: relative path + optional file contents. A trailing
    /// `/` in `path` means "create as directory"; everything else is a file.
    struct Marker {
        path: &'static str,
        contents: &'static str,
    }

    impl Marker {
        const fn file(path: &'static str) -> Self {
            Self {
                path,
                contents: "",
            }
        }
        const fn file_with(path: &'static str, contents: &'static str) -> Self {
            Self { path, contents }
        }
        const fn dir(path: &'static str) -> Self {
            // `path` must end with '/'.
            Self {
                path,
                contents: "",
            }
        }
    }

    fn seed(tmp: &TempDir, markers: &[Marker]) {
        for m in markers {
            let p = tmp.path().join(m.path.trim_end_matches('/'));
            if m.path.ends_with('/') {
                fs::create_dir_all(&p).unwrap();
            } else {
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(&p, m.contents).unwrap();
            }
        }
    }

    fn case(markers: &[Marker], expected: ProjectType) {
        let tmp = TempDir::new().unwrap();
        seed(&tmp, markers);
        let got = classify_dir(tmp.path());
        assert_eq!(
            got, expected,
            "classify_dir mismatch for markers {:?}",
            markers.iter().map(|m| m.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn flutter_pubspec() {
        case(&[Marker::file("pubspec.yaml")], ProjectType::Flutter);
    }

    #[test]
    fn android_app_build_gradle_kts() {
        case(
            &[Marker::file("app/build.gradle.kts")],
            ProjectType::Android,
        );
    }

    #[test]
    fn ios_xcodeproj_dir() {
        case(&[Marker::dir("Foo.xcodeproj/")], ProjectType::Ios);
    }

    #[test]
    fn node_pnpm() {
        case(
            &[
                Marker::file("package.json"),
                Marker::file("pnpm-lock.yaml"),
            ],
            ProjectType::Node {
                manager: NodePm::Pnpm,
            },
        );
    }

    #[test]
    fn node_yarn() {
        case(
            &[Marker::file("package.json"), Marker::file("yarn.lock")],
            ProjectType::Node {
                manager: NodePm::Yarn,
            },
        );
    }

    #[test]
    fn node_bun() {
        case(
            &[Marker::file("package.json"), Marker::file("bun.lockb")],
            ProjectType::Node {
                manager: NodePm::Bun,
            },
        );
    }

    #[test]
    fn node_npm_with_lock() {
        case(
            &[
                Marker::file("package.json"),
                Marker::file("package-lock.json"),
            ],
            ProjectType::Node {
                manager: NodePm::Npm,
            },
        );
    }

    #[test]
    fn node_npm_alone() {
        case(
            &[Marker::file("package.json")],
            ProjectType::Node {
                manager: NodePm::Npm,
            },
        );
    }

    #[test]
    fn rust_cargo_toml() {
        case(&[Marker::file("Cargo.toml")], ProjectType::Rust);
    }

    #[test]
    fn go_mod() {
        case(&[Marker::file("go.mod")], ProjectType::Go);
    }

    #[test]
    fn ruby_gemfile_only() {
        case(&[Marker::file("Gemfile")], ProjectType::Ruby { rails: false });
    }

    #[test]
    fn ruby_rails() {
        case(
            &[
                Marker::file("Gemfile"),
                Marker::file("config/application.rb"),
            ],
            ProjectType::Ruby { rails: true },
        );
    }

    #[test]
    fn python_pyproject_generic() {
        case(
            &[Marker::file_with(
                "pyproject.toml",
                "[project]\nname = \"x\"\n",
            )],
            ProjectType::Python {
                framework: Some(PyFw::Generic),
            },
        );
    }

    #[test]
    fn python_pyproject_fastapi() {
        case(
            &[Marker::file_with(
                "pyproject.toml",
                "[project]\nname = \"x\"\ndependencies = [\"fastapi\"]\n",
            )],
            ProjectType::Python {
                framework: Some(PyFw::FastAPI),
            },
        );
    }

    #[test]
    fn python_django() {
        case(
            &[
                Marker::file("manage.py"),
                Marker::file_with("pyproject.toml", "[project]\nname = \"x\"\n"),
            ],
            ProjectType::Python {
                framework: Some(PyFw::Django),
            },
        );
    }

    #[test]
    fn empty_dir_is_unknown() {
        case(&[], ProjectType::Unknown);
    }

    // A fastlane Gemfile at an Android project root must not shadow the Android
    // anchor (regression: ha-android was misclassified as Ruby).
    #[test]
    fn android_with_fastlane_gemfile() {
        case(
            &[
                Marker::file("Gemfile"),
                Marker::file("app/build.gradle.kts"),
            ],
            ProjectType::Android,
        );
    }

    // Same precedence rule for an iOS project carrying a fastlane Gemfile.
    #[test]
    fn ios_with_fastlane_gemfile() {
        case(
            &[Marker::file("Gemfile"), Marker::dir("Foo.xcodeproj/")],
            ProjectType::Ios,
        );
    }

    #[test]
    fn android_id_literal_in_app_gradle() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[Marker::file_with(
                "app/build.gradle.kts",
                "android {\n    defaultConfig {\n        applicationId = \"com.example.app\"\n    }\n}\n",
            )],
        );
        assert_eq!(
            detect_android_id(tmp.path()),
            Some("com.example.app".to_string())
        );
    }

    // applicationId set from a Kotlin constant in a convention plugin (the
    // ha-android shape).
    #[test]
    fn android_id_via_convention_plugin_const() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file_with(
                    "app/build.gradle.kts",
                    "plugins { id(\"homeassistant.android.application\") }\n",
                ),
                Marker::file_with(
                    "build-logic/convention/src/main/kotlin/AndroidApplicationConventionPlugin.kt",
                    "private const val APPLICATION_ID = \"io.homeassistant.companion.android\"\n\
                     class P { fun a() { namespace = APPLICATION_ID\n applicationId = APPLICATION_ID } }\n",
                ),
            ],
        );
        assert_eq!(
            detect_android_id(tmp.path()),
            Some("io.homeassistant.companion.android".to_string())
        );
    }

    // Only an AGP namespace literal is present (no applicationId at all).
    #[test]
    fn android_id_via_namespace_literal() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[Marker::file_with(
                "app/build.gradle.kts",
                "android {\n    namespace = \"com.example.ns\"\n}\n",
            )],
        );
        assert_eq!(
            detect_android_id(tmp.path()),
            Some("com.example.ns".to_string())
        );
    }

    fn roots(projects: &[(AppInfo, PathBuf)], base: &Path) -> Vec<String> {
        projects
            .iter()
            .map(|(_, p)| {
                p.strip_prefix(base)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    /// Pre-fix `discover_recursive`: classified the scan root and returned
    /// immediately without walking children — the source of the `$HOME` bug.
    fn legacy_discover_recursive(dir: &Path, depth: usize, out: &mut Vec<(AppInfo, PathBuf)>) {
        let pt = classify_dir(dir);
        if pt != ProjectType::Unknown {
            out.push((build_app_info(dir, &pt), dir.to_path_buf()));
            return;
        }
        if depth >= MAX_DISCOVERY_DEPTH {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || is_discovery_skip_dir(name) {
                continue;
            }
            legacy_discover_recursive(&path, depth + 1, out);
        }
    }

    fn legacy_discover_projects(start_dir: &Path) -> Vec<(AppInfo, PathBuf)> {
        let mut found: Vec<(AppInfo, PathBuf)> = Vec::new();
        legacy_discover_recursive(start_dir, 0, &mut found);
        found.sort_by(|a, b| a.1.cmp(&b.1));
        found
    }

    // Projects nested deeper than one level (the `~/projects/<group>/<repo>`
    // shape) must all be discovered, not just direct children.
    #[test]
    fn discover_finds_deeply_nested_projects() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file("group-a/repo1/Cargo.toml"),
                Marker::file("group-a/repo2/go.mod"),
                Marker::file("group-b/nested/repo3/pubspec.yaml"),
            ],
        );
        let got = AppDetector::new().discover_projects(tmp.path());
        assert_eq!(
            roots(&got, tmp.path()),
            vec![
                "group-a/repo1".to_string(),
                "group-a/repo2".to_string(),
                "group-b/nested/repo3".to_string(),
            ]
        );
    }

    // Discovery stops at a project root: a Flutter app's internal android/ios
    // sub-projects must not surface as separate entries.
    #[test]
    fn discover_does_not_descend_into_found_project() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file("app/pubspec.yaml"),
                Marker::file("app/android/app/build.gradle.kts"),
            ],
        );
        let got = AppDetector::new().discover_projects(tmp.path());
        assert_eq!(roots(&got, tmp.path()), vec!["app".to_string()]);
        assert_eq!(got[0].0.project_type, ProjectType::Flutter);
    }

    /// Exact reproduction of the reported bug: `mdev purge` from `$HOME` with a
    /// stray `package.json` returned only `→ /Users/dan` and never reached
    /// `~/projects/...`. The fixed scanner must list nested repos and must not
    /// report the container root when children exist.
    #[test]
    fn regression_home_package_json_finds_nested_repos_not_container() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file("package.json"),
                Marker::file("projects/open-source/cli-apps-helper/Cargo.toml"),
                Marker::file("projects/other-app/go.mod"),
            ],
        );
        let got = AppDetector::new().discover_projects(tmp.path());
        let paths = roots(&got, tmp.path());
        assert_eq!(
            paths,
            vec![
                "projects/open-source/cli-apps-helper".to_string(),
                "projects/other-app".to_string(),
            ],
            "must discover nested repos under a container scan root"
        );
        assert!(
            !paths.contains(&"".to_string()),
            "container scan root must not be the only (or any) result when children exist"
        );
        assert_ne!(
            got.len(),
            1,
            "must not collapse to a single bogus project at the scan root"
        );
    }

    /// Documents the pre-fix failure mode: stopping at the first scan-root match.
    /// Kept so CI proves we understand the regression and won't reintroduce it.
    #[test]
    fn regression_legacy_scan_root_stop_would_miss_nested_repos() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file("package.json"),
                Marker::file("projects/open-source/cli-apps-helper/Cargo.toml"),
                Marker::file("projects/other-app/go.mod"),
            ],
        );
        let legacy = legacy_discover_projects(tmp.path());
        let legacy_paths = roots(&legacy, tmp.path());
        assert_eq!(
            legacy_paths,
            vec!["".to_string()],
            "legacy behavior incorrectly reports only the container root"
        );

        let fixed = AppDetector::new().discover_projects(tmp.path());
        let fixed_paths = roots(&fixed, tmp.path());
        assert_ne!(
            fixed_paths, legacy_paths,
            "fixed discovery must differ from the legacy bug"
        );
        assert_eq!(fixed_paths.len(), 2);
    }

    // Container scan roots (e.g. $HOME) with stray marker files must not
    // block discovery of real projects nested underneath.
    #[test]
    fn discover_container_scan_root_defers_to_children_gemfile() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file("Gemfile"),
                Marker::file("projects/open-source/cli-apps-helper/Cargo.toml"),
                Marker::file("projects/other-app/go.mod"),
            ],
        );
        let got = AppDetector::new().discover_projects(tmp.path());
        assert_eq!(
            roots(&got, tmp.path()),
            vec![
                "projects/open-source/cli-apps-helper".to_string(),
                "projects/other-app".to_string(),
            ]
        );
    }

    #[test]
    fn discover_container_scan_root_defers_to_children_package_json() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file("package.json"),
                Marker::file("projects/foo/Cargo.toml"),
            ],
        );
        let got = AppDetector::new().discover_projects(tmp.path());
        assert_eq!(roots(&got, tmp.path()), vec!["projects/foo".to_string()]);
    }

    // A standalone project at the scan root (no child projects) is still found.
    #[test]
    fn discover_scan_root_kept_when_no_children() {
        let tmp = TempDir::new().unwrap();
        seed(&tmp, &[Marker::file("Gemfile")]);
        let got = AppDetector::new().discover_projects(tmp.path());
        assert_eq!(roots(&got, tmp.path()), vec!["".to_string()]);
        assert_eq!(got[0].0.project_type, ProjectType::Ruby { rails: false });
    }

    // Strong project at scan root does not descend into its own internals.
    #[test]
    fn discover_strong_scan_root_does_not_descend() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file("pubspec.yaml"),
                Marker::file("android/app/build.gradle.kts"),
            ],
        );
        let got = AppDetector::new().discover_projects(tmp.path());
        assert_eq!(roots(&got, tmp.path()), vec!["".to_string()]);
        assert_eq!(got[0].0.project_type, ProjectType::Flutter);
    }

    #[test]
    fn discover_skips_macos_home_containers() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file("package.json"),
                Marker::file("Library/Zed/pkg/package.json"),
                Marker::file("projects/real/Cargo.toml"),
            ],
        );
        let got = AppDetector::new().discover_projects(tmp.path());
        assert_eq!(roots(&got, tmp.path()), vec!["projects/real".to_string()]);
    }

    // Dependency/build/VCS dirs are never descended into during discovery.
    #[test]
    fn discover_skips_dependency_dirs() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file("node_modules/pkg/Cargo.toml"),
                Marker::file(".git/sub/go.mod"),
                Marker::file("real/Cargo.toml"),
            ],
        );
        let got = AppDetector::new().discover_projects(tmp.path());
        assert_eq!(roots(&got, tmp.path()), vec!["real".to_string()]);
    }

    // `build` output dirs must be skipped so stale compiled caches never win.
    #[test]
    fn android_id_skips_build_output_dirs() {
        let tmp = TempDir::new().unwrap();
        seed(
            &tmp,
            &[
                Marker::file_with(
                    "app/build.gradle.kts",
                    "android {\n    namespace = \"com.example.real\"\n}\n",
                ),
                Marker::file_with(
                    "app/build/generated/Stale.kt",
                    "val APPLICATION_ID = \"com.example.stale\"\n",
                ),
            ],
        );
        assert_eq!(
            detect_android_id(tmp.path()),
            Some("com.example.real".to_string())
        );
    }
}
