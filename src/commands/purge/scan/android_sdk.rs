//! Android SDK / emulator leftovers outside projects.

use std::path::PathBuf;

use super::super::candidate::{display_path, Candidate, Category, Size, Target, Tier};
use super::{child_dirs, existing, ScanContext, Scanner};

pub struct AndroidSdkScanner;

impl Scanner for AndroidSdkScanner {
    fn category(&self) -> Category {
        Category::Android
    }

    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
        let home = &ctx.home;
        let android = home.join(".android");
        let mut out = Vec::new();

        let gradle_home = std::env::var("GRADLE_USER_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".gradle"));
        for p in existing(vec![
            android.join("cache"),
            android.join("build-cache"),
            gradle_home.join("daemon"),
        ]) {
            out.push(Candidate::new(
                Category::Android,
                "SDK caches",
                display_path(&p, home),
                Target::dir(p),
                Size::Unknown,
                Tier::Local,
            ));
        }

        let locks = avd_lock_files(&android.join("avd"));
        if !locks.is_empty() && !emulator_running(ctx) {
            for p in locks {
                out.push(Candidate::new(
                    Category::Android,
                    "Stale AVD lock files",
                    display_path(&p, home),
                    Target::file(p),
                    Size::Unknown,
                    Tier::Local,
                ));
            }
        }
        out
    }
}

/// `*.lock` files inside every `<avd>/*.avd` directory.
pub fn avd_lock_files(avd_dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for avd in child_dirs(avd_dir) {
        if avd.extension().and_then(|e| e.to_str()) != Some("avd") {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&avd) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("lock") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn emulator_running(ctx: &ScanContext) -> bool {
    if ctx.runner.which("pgrep").is_none() {
        return true; // cannot tell — assume busy, keep the locks
    }
    ctx.runner
        .run("pgrep", &["-f", "qemu-system"], None)
        .is_success()
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, FakeRunner};
    use super::*;
    use crate::runner::RunResult;
    use std::fs;
    use tempfile::TempDir;

    fn fixture(home: &std::path::Path) {
        fs::create_dir_all(home.join(".android/cache")).unwrap();
        fs::create_dir_all(home.join(".android/avd/Pixel.avd")).unwrap();
        fs::write(
            home.join(".android/avd/Pixel.avd/hardware-qemu.ini.lock"),
            b"",
        )
        .unwrap();
        fs::write(home.join(".android/avd/Pixel.avd/config.ini"), b"").unwrap();
        fs::create_dir_all(home.join(".gradle/daemon")).unwrap();
    }

    #[test]
    fn lists_caches_and_locks_when_no_emulator_runs() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let runner =
            FakeRunner::new(&["pgrep"]).respond("pgrep", RunResult::failure(1, String::new()));
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], tmp.path(), vec![], &runner);
        let found = AndroidSdkScanner.scan(&c);
        let labels: Vec<&str> = found.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "~/.android/cache",
                "~/.gradle/daemon",
                "~/.android/avd/Pixel.avd/hardware-qemu.ini.lock"
            ]
        );
    }

    #[test]
    fn keeps_locks_while_emulator_runs() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let runner = FakeRunner::new(&["pgrep"]).respond("pgrep", RunResult::success("123".into()));
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], tmp.path(), vec![], &runner);
        let found = AndroidSdkScanner.scan(&c);
        assert!(found.iter().all(|c| !c.label.ends_with(".lock")));
    }
}
