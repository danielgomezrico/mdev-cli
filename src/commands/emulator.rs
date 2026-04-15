use clap::{Args, Subcommand};
use colored::Colorize;
use std::path::{Path, PathBuf};

use crate::logger::Logger;
use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct EmulatorArgs {
    #[command(subcommand)]
    pub command: EmulatorCommands,
}

#[derive(Subcommand, Debug)]
pub enum EmulatorCommands {
    /// Apply config.ini tweaks to every local Android AVD.
    ///
    /// Default tweak: showAVDManager=no (takes effect on next emulator boot).
    /// Use --set key=value to add or override tweaks. Keys are case-sensitive
    /// and emulator-version-dependent; verify against Android emulator docs
    /// before trusting non-default keys.
    Config(EmulatorConfigArgs),
    /// List known AVD config.ini tweaks with human-readable descriptions.
    ///
    /// Useful to discover what you can pass to `mdev emulator config --set`.
    /// Values here are recommended defaults; tweak per your setup.
    List,
}

#[derive(Args, Debug)]
pub struct EmulatorConfigArgs {
    /// Preview without writing.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Also edit AVDs whose emulator appears to be running.
    #[arg(long)]
    pub force: bool,
    /// Only apply to this AVD (repeatable, by name).
    #[arg(long = "avd")]
    pub avd: Vec<String>,
    /// Extra key=value tweak (repeatable; overrides defaults for same key).
    #[arg(long = "set")]
    pub set: Vec<String>,
    /// Copy config.ini to config.ini.bak before editing.
    #[arg(long)]
    pub backup: bool,
    /// Verbose output.
    #[arg(short = 'v', long)]
    pub verbose: bool,
}

const DEFAULT_TWEAKS: &[(&str, &str)] = &[("showAVDManager", "no")];

/// Catalog of known-good AVD config.ini keys for `mdev emulator list`.
/// (key, recommended value, human description).
/// Keys are case-sensitive and emulator-version-dependent; verify against
/// Android emulator docs before trusting non-default keys.
const KNOWN_TWEAKS: &[(&str, &str, &str)] = &[
    (
        "showAVDManager",
        "no",
        "Hide the AVD Manager / extended controls panel on next boot.",
    ),
    (
        "hw.keyboard",
        "yes",
        "Enable hardware keyboard passthrough (type from host).",
    ),
    (
        "hw.mainKeys",
        "no",
        "Hide hardware navigation buttons (use on-screen system nav).",
    ),
    (
        "hw.gpu.enabled",
        "yes",
        "Enable GPU acceleration for the emulator.",
    ),
    (
        "hw.gpu.mode",
        "host",
        "GPU rendering mode: host | swiftshader_indirect | off.",
    ),
    (
        "hw.audioInput",
        "no",
        "Disable microphone input from host (quieter, safer).",
    ),
    (
        "hw.audioOutput",
        "no",
        "Disable audio output from the emulator.",
    ),
    (
        "hw.camera.back",
        "none",
        "Back camera: none | emulated | webcam0.",
    ),
    (
        "hw.camera.front",
        "none",
        "Front camera: none | emulated | webcam0.",
    ),
    (
        "hw.ramSize",
        "2048",
        "Guest RAM in MB. Raise for heavier apps; lower to save host RAM.",
    ),
    (
        "vm.heapSize",
        "256",
        "Per-app VM heap size in MB.",
    ),
    (
        "disk.dataPartition.size",
        "6G",
        "Userdata partition size. Accepts suffixes like M or G.",
    ),
    (
        "hw.lcd.density",
        "420",
        "Display density (dpi). Must match the skin's resolution.",
    ),
    (
        "skin.dynamic",
        "yes",
        "Allow the skin to resize with the window.",
    ),
    (
        "runtime.network.speed",
        "full",
        "Simulated network speed: full | gsm | edge | umts | lte …",
    ),
    (
        "runtime.network.latency",
        "none",
        "Simulated network latency: none | gsm | edge | umts …",
    ),
    (
        "fastboot.forceColdBoot",
        "no",
        "Force a cold boot on every launch (slower, clean state).",
    ),
];

struct Avd {
    name: String,
    dir: PathBuf,
    config_ini: PathBuf,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum Eol {
    Lf,
    Crlf,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum UpsertOutcome {
    Added,
    Replaced,
    Unchanged,
}

struct UpsertReport {
    per_key: Vec<(String, UpsertOutcome)>,
    #[allow(dead_code)]
    eol: Eol,
}

enum ApplyOutcome {
    Updated(UpsertReport),
    Unchanged,
    Skipped(#[allow(dead_code)] String),
    Failed(#[allow(dead_code)] String),
}

#[derive(Default, Debug)]
struct Counters {
    updated: u32,
    unchanged: u32,
    skipped: u32,
    failed: u32,
}

pub fn run(args: &EmulatorArgs, _runner: &dyn Runner) -> i32 {
    match &args.command {
        EmulatorCommands::Config(cfg) => run_config(cfg),
        EmulatorCommands::List => run_list(),
    }
}

fn run_list() -> i32 {
    let logger = Logger::new();
    logger.info(&format!("{}", "mdev emulator list".cyan().bold()));
    logger.info(&"──────────────────────────────".dimmed().to_string());
    logger.info("Known AVD config.ini tweaks (apply with `mdev emulator config --set key=value`):");
    logger.info("");

    let key_width = KNOWN_TWEAKS.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);
    let val_width = KNOWN_TWEAKS.iter().map(|(_, v, _)| v.len()).max().unwrap_or(0);

    for (key, val, desc) in KNOWN_TWEAKS {
        let is_default = DEFAULT_TWEAKS.iter().any(|(dk, _)| dk == key);
        let marker = if is_default { "★".yellow().to_string() } else { " ".to_string() };
        let key_padded = format!("{:<kw$}", key, kw = key_width);
        let val_padded = format!("{:<vw$}", val, vw = val_width);
        logger.info(&format!(
            "  {} {}  {}  {}",
            marker,
            key_padded.green(),
            val_padded.cyan(),
            desc.dimmed(),
        ));
    }
    logger.info("");
    logger.info(&format!(
        "{} applied by default. Values shown are recommended — override with `--set`.",
        "★".yellow()
    ));
    0
}

fn run_config(cfg: &EmulatorConfigArgs) -> i32 {
    let logger = Logger::new();
    logger.info(&format!("{}", "mdev emulator config".cyan().bold()));
    logger.info(&"──────────────────────────────".dimmed().to_string());

    let root = match resolve_avd_root() {
        Some(r) => r,
        None => {
            logger.warn("No Android AVD root found (set ANDROID_AVD_HOME or install Android SDK)");
            return 0;
        }
    };
    logger.info(&format!("AVD root: {}", root.display()));

    let mut avds = discover_avds(&root, &logger, cfg.verbose);
    if avds.is_empty() {
        logger.warn(&format!("No AVDs found at {}", root.display()));
        return 0;
    }
    logger.info(&format!("Found {} AVD(s).", avds.len()));

    let user_tweaks = match parse_tweaks(&cfg.set) {
        Ok(t) => t,
        Err(e) => {
            logger.err(&e);
            return 1;
        }
    };
    let tweaks = merge_tweaks(DEFAULT_TWEAKS, user_tweaks);

    if !cfg.avd.is_empty() {
        let present: std::collections::HashSet<&str> =
            avds.iter().map(|a| a.name.as_str()).collect();
        let missing: Vec<&str> = cfg
            .avd
            .iter()
            .filter(|n| !present.contains(n.as_str()))
            .map(|s| s.as_str())
            .collect();
        if !missing.is_empty() {
            logger.err(&format!("AVD(s) not found: {}", missing.join(", ")));
            return 1;
        }
        let wanted: std::collections::HashSet<&str> = cfg.avd.iter().map(|s| s.as_str()).collect();
        avds.retain(|a| wanted.contains(a.name.as_str()));
    }

    let total = avds.len();
    let mut counters = Counters::default();

    for avd in &avds {
        logger.info(&format!("  → {}", avd.name));
        if cfg.verbose {
            logger.detail(&format!("    dir: {}", avd.dir.display()));
        }
        match apply(avd, &tweaks, cfg, &logger) {
            ApplyOutcome::Updated(report) => {
                let any_change = report
                    .per_key
                    .iter()
                    .any(|(_, o)| *o != UpsertOutcome::Unchanged);
                if any_change {
                    counters.updated += 1;
                } else {
                    counters.unchanged += 1;
                }
            }
            ApplyOutcome::Unchanged => counters.unchanged += 1,
            ApplyOutcome::Skipped(_) => counters.skipped += 1,
            ApplyOutcome::Failed(_) => counters.failed += 1,
        }
    }

    let ok = counters.updated + counters.unchanged;
    logger.info(&format!("{}/{} AVD(s) configured.", ok, total));
    if counters.skipped > 0 {
        logger.warn(&format!("{} skipped", counters.skipped));
    }
    if counters.failed > 0 {
        logger.err(&format!("{} failed", counters.failed));
    }

    if counters.failed > 0 {
        1
    } else {
        0
    }
}

fn resolve_avd_root() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("ANDROID_AVD_HOME") {
        if !v.is_empty() {
            let p = PathBuf::from(v);
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    if let Ok(v) = std::env::var("ANDROID_USER_HOME") {
        if !v.is_empty() {
            let p = PathBuf::from(v).join("avd");
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".android").join("avd");
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

fn discover_avds(root: &Path, logger: &Logger, verbose: bool) -> Vec<Avd> {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<Avd> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !filename.ends_with(".ini") {
            continue;
        }
        let stem = filename.trim_end_matches(".ini").to_string();
        if stem.starts_with('.') || stem == "hardware-qemu" {
            continue;
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                if verbose {
                    logger.detail(&format!("    skip {}: {}", filename, e));
                }
                continue;
            }
        };
        let candidate = match parse_registry_path(&contents) {
            Some(p) => PathBuf::from(p),
            None => root.join(format!("{}.avd", stem)),
        };
        if !candidate.is_dir() {
            if verbose {
                logger.detail(&format!(
                    "    skip {}: avd dir missing ({})",
                    stem,
                    candidate.display()
                ));
            }
            continue;
        }
        let config_ini = candidate.join("config.ini");
        if !config_ini.is_file() {
            if verbose {
                logger.detail(&format!("    skip {}: config.ini missing", stem));
            }
            continue;
        }
        out.push(Avd {
            name: stem,
            dir: candidate,
            config_ini,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn parse_registry_path(contents: &str) -> Option<String> {
    for line in contents.lines() {
        let trimmed = line.trim_start();
        // Must match "path" exactly (not "path.rel").
        if let Some(rest) = trimmed.strip_prefix("path") {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix('=') {
                let val = val.trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn parse_tweaks(set: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for raw in set {
        let idx = match raw.find('=') {
            Some(i) => i,
            None => return Err(format!("invalid --set value: {}, expected key=value", raw)),
        };
        let k = raw[..idx].trim().to_string();
        let v = raw[idx + 1..].to_string();
        if k.is_empty() {
            return Err(format!("invalid --set value: {}, expected key=value", raw));
        }
        if let Some(pos) = out.iter().position(|(ek, _)| ek == &k) {
            out[pos] = (k, v);
        } else {
            out.push((k, v));
        }
    }
    Ok(out)
}

fn merge_tweaks(
    defaults: &[(&str, &str)],
    overrides: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = defaults
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    for (k, v) in overrides {
        if let Some(pos) = out.iter().position(|(ek, _)| ek == &k) {
            out[pos] = (k, v);
        } else {
            out.push((k, v));
        }
    }
    out
}

fn upsert_ini(content: &str, tweaks: &[(String, String)]) -> (String, UpsertReport) {
    let eol = if content.contains("\r\n") {
        Eol::Crlf
    } else {
        Eol::Lf
    };
    let eol_str = match eol {
        Eol::Crlf => "\r\n",
        Eol::Lf => "\n",
    };

    // Split on '\n', strip trailing '\r' if CRLF.
    let mut lines: Vec<String> = if content.is_empty() {
        Vec::new()
    } else {
        content
            .split('\n')
            .map(|l| {
                if eol == Eol::Crlf {
                    l.strip_suffix('\r').unwrap_or(l).to_string()
                } else {
                    l.to_string()
                }
            })
            .collect()
    };

    // Drop trailing empty lines (they represent trailing newlines or blank tail).
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }

    let mut per_key: Vec<(String, UpsertOutcome)> = Vec::new();

    for (k, v) in tweaks {
        let re = regex::Regex::new(&format!(r"^\s*{}\s*=", regex::escape(k))).unwrap();
        let mut found_idx: Option<usize> = None;
        for (i, line) in lines.iter().enumerate() {
            if re.is_match(line) {
                found_idx = Some(i);
                break;
            }
        }
        let new_line = format!("{}={}", k, v);
        match found_idx {
            Some(i) => {
                let existing_val = match lines[i].split_once('=') {
                    Some((_, rhs)) => rhs.trim().to_string(),
                    None => String::new(),
                };
                if existing_val == *v {
                    per_key.push((k.clone(), UpsertOutcome::Unchanged));
                } else {
                    lines[i] = new_line;
                    per_key.push((k.clone(), UpsertOutcome::Replaced));
                }
            }
            None => {
                lines.push(new_line);
                per_key.push((k.clone(), UpsertOutcome::Added));
            }
        }
    }

    let mut joined = lines.join(eol_str);
    if !joined.is_empty() {
        joined.push_str(eol_str);
    } else {
        // empty file edge: tweaks must have pushed at least one; if tweaks empty, stay empty.
        // If tweaks were non-empty, lines wouldn't be empty here.
    }

    (joined, UpsertReport { per_key, eol })
}

fn is_emulator_running(avd_dir: &Path) -> bool {
    avd_dir.join("hardware-qemu.ini.lock").exists() || avd_dir.join("multiinstance.lock").exists()
}

fn apply(
    avd: &Avd,
    tweaks: &[(String, String)],
    cfg: &EmulatorConfigArgs,
    logger: &Logger,
) -> ApplyOutcome {
    if is_emulator_running(&avd.dir) && !cfg.force {
        logger.warn(&format!(
            "    {} emulator running — skipped (use --force)",
            "!".yellow()
        ));
        return ApplyOutcome::Skipped("emulator running".to_string());
    }

    let content = match std::fs::read_to_string(&avd.config_ini) {
        Ok(c) => c,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                logger.err(&format!("    {} {}", "✗".red(), e));
                return ApplyOutcome::Failed(e.to_string());
            }
            logger.warn(&format!(
                "    {} config.ini unreadable: {}",
                "!".yellow(),
                e
            ));
            return ApplyOutcome::Skipped(e.to_string());
        }
    };

    let (new_content, report) = upsert_ini(&content, tweaks);

    if cfg.dry_run {
        for (k, outcome) in &report.per_key {
            let val = tweaks
                .iter()
                .find(|(ek, _)| ek == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            let glyph = "~".cyan();
            let label = match outcome {
                UpsertOutcome::Added => "would add",
                UpsertOutcome::Replaced => "would replace",
                UpsertOutcome::Unchanged => "unchanged",
            };
            logger.info(&format!("    {} {} {}={}", glyph, label, k, val));
        }
        return ApplyOutcome::Updated(report);
    }

    let all_unchanged = report
        .per_key
        .iter()
        .all(|(_, o)| *o == UpsertOutcome::Unchanged);
    if all_unchanged && content == new_content {
        for (k, _) in &report.per_key {
            let val = tweaks
                .iter()
                .find(|(ek, _)| ek == k)
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            logger.info(&format!("    {} {}={} (already set)", "~".cyan(), k, val));
        }
        return ApplyOutcome::Unchanged;
    }

    if cfg.backup {
        let bak = avd.dir.join("config.ini.bak");
        if let Err(e) = std::fs::copy(&avd.config_ini, &bak) {
            logger.warn(&format!("    {} backup failed: {}", "!".yellow(), e));
        }
    }

    if let Err(e) = std::fs::write(&avd.config_ini, &new_content) {
        logger.err(&format!("    {} write failed: {}", "✗".red(), e));
        return ApplyOutcome::Failed(e.to_string());
    }

    for (k, outcome) in &report.per_key {
        let val = tweaks
            .iter()
            .find(|(ek, _)| ek == k)
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        match outcome {
            UpsertOutcome::Added | UpsertOutcome::Replaced => {
                logger.info(&format!("    {} set {}={}", "✓".green(), k, val));
            }
            UpsertOutcome::Unchanged => {
                logger.info(&format!("    {} {}={} (already set)", "~".cyan(), k, val));
            }
        }
    }

    ApplyOutcome::Updated(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    fn tw(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (s(k), s(v))).collect()
    }

    #[test]
    fn upsert_missing_key_appends() {
        let input = "foo=1\nbar=2";
        let (out, rep) = upsert_ini(input, &tw(&[("baz", "3")]));
        assert_eq!(out, "foo=1\nbar=2\nbaz=3\n");
        assert_eq!(rep.per_key, vec![(s("baz"), UpsertOutcome::Added)]);
    }

    #[test]
    fn upsert_existing_differs_replaces() {
        let input = "foo=1\nbar=2\n";
        let (out, rep) = upsert_ini(input, &tw(&[("bar", "9")]));
        assert_eq!(out, "foo=1\nbar=9\n");
        assert_eq!(rep.per_key, vec![(s("bar"), UpsertOutcome::Replaced)]);
    }

    #[test]
    fn upsert_existing_identical_unchanged() {
        let input = "foo=1\nbar=2\n";
        let (out, rep) = upsert_ini(input, &tw(&[("bar", "2")]));
        assert_eq!(out, input);
        assert_eq!(rep.per_key, vec![(s("bar"), UpsertOutcome::Unchanged)]);
    }

    #[test]
    fn upsert_preserves_crlf() {
        let input = "foo=1\r\nbar=2\r\n";
        let (out, rep) = upsert_ini(input, &tw(&[("baz", "3")]));
        assert_eq!(out, "foo=1\r\nbar=2\r\nbaz=3\r\n");
        assert_eq!(rep.eol, Eol::Crlf);
    }

    #[test]
    fn upsert_adds_single_trailing_eol_when_missing() {
        let input = "foo=1";
        let (out, _) = upsert_ini(input, &tw(&[("foo", "1")]));
        assert_eq!(out, "foo=1\n");
    }

    #[test]
    fn upsert_collapses_multiple_trailing_eols_to_one() {
        let input = "foo=1\n\n\n";
        let (out, _) = upsert_ini(input, &tw(&[("foo", "1")]));
        assert_eq!(out, "foo=1\n");
    }

    #[test]
    fn upsert_multiple_tweaks_single_pass() {
        let input = "foo=1\n";
        let (out, rep) = upsert_ini(input, &tw(&[("foo", "2"), ("bar", "3")]));
        assert_eq!(out, "foo=2\nbar=3\n");
        assert_eq!(
            rep.per_key,
            vec![
                (s("foo"), UpsertOutcome::Replaced),
                (s("bar"), UpsertOutcome::Added),
            ]
        );
    }

    #[test]
    fn upsert_empty_file_appends_with_eol() {
        let input = "";
        let (out, rep) = upsert_ini(input, &tw(&[("foo", "1")]));
        assert_eq!(out, "foo=1\n");
        assert_eq!(rep.per_key, vec![(s("foo"), UpsertOutcome::Added)]);
    }

    #[test]
    fn parse_tweaks_rejects_missing_equals() {
        let err = parse_tweaks(&[s("invalidtoken")]).unwrap_err();
        assert!(err.contains("invalidtoken"));
    }

    #[test]
    fn parse_tweaks_later_duplicate_wins() {
        let out = parse_tweaks(&[s("k=a"), s("k=b")]).unwrap();
        assert_eq!(out, vec![(s("k"), s("b"))]);
    }

    #[test]
    fn parse_registry_path_ignores_path_rel_and_merge() {
        let contents = "path.rel=avd/Pixel_9.avd\npath=/abs/Pixel_9.avd\ntarget=android-37\n";
        assert_eq!(parse_registry_path(contents), Some(s("/abs/Pixel_9.avd")));
        // ordering: path.rel appears first but must be ignored
        let only_rel = "path.rel=avd/Pixel_9.avd\n";
        assert_eq!(parse_registry_path(only_rel), None);
        // merge_tweaks: override replaces default value
        let merged = merge_tweaks(
            &[("showAVDManager", "no")],
            vec![(s("showAVDManager"), s("yes"))],
        );
        assert_eq!(merged, vec![(s("showAVDManager"), s("yes"))]);
    }

    #[test]
    fn upsert_escapes_regex_metachars_in_key() {
        // Key contains '.' which is a regex metachar; must not match unrelated lines.
        let input = "hwXkeyboard=no\nhw.keyboard=no\n";
        let (out, rep) = upsert_ini(input, &tw(&[("hw.keyboard", "yes")]));
        assert_eq!(out, "hwXkeyboard=no\nhw.keyboard=yes\n");
        assert_eq!(
            rep.per_key,
            vec![(s("hw.keyboard"), UpsertOutcome::Replaced)]
        );
    }
}
