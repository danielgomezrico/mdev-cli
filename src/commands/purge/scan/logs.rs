//! Application log folders that only grow.

use std::path::PathBuf;

use super::super::candidate::{display_path, Candidate, Category, Size, Target, Tier};
use super::{existing, ScanContext, Scanner};

pub struct LogsScanner;

impl Scanner for LogsScanner {
    fn category(&self) -> Category {
        Category::Logs
    }

    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
        let home = &ctx.home;
        let mut out = Vec::new();

        let lib_logs = home.join("Library").join("Logs");
        if let Ok(entries) = std::fs::read_dir(&lib_logs) {
            let group = display_path(&lib_logs, home);
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().map(|t| !t.is_symlink()).unwrap_or(false))
                .map(|e| e.path())
                .collect();
            paths.sort();
            for p in paths {
                let label = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let target = if p.is_dir() {
                    Target::dir(p)
                } else {
                    Target::file(p)
                };
                out.push(Candidate::new(
                    Category::Logs,
                    group.clone(),
                    label,
                    target,
                    Size::Unknown,
                    Tier::Local,
                ));
            }
        }

        for p in existing(vec![home.join(".npm").join("_logs")]) {
            out.push(Candidate::new(
                Category::Logs,
                "npm",
                display_path(&p, home),
                Target::dir(p),
                Size::Unknown,
                Tier::Local,
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, FakeRunner};
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn lists_each_log_folder_and_npm_logs() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join("Library/Logs/CoreSimulator")).unwrap();
        fs::create_dir_all(home.join("Library/Logs/Homebrew")).unwrap();
        fs::write(home.join("Library/Logs/app.log"), b"x").unwrap();
        fs::create_dir_all(home.join(".npm/_logs")).unwrap();
        let runner = FakeRunner::new(&[]);
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], home, vec![], &runner);

        let found = LogsScanner.scan(&c);

        let labels: Vec<&str> = found.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["CoreSimulator", "Homebrew", "app.log", "~/.npm/_logs"]
        );
        assert!(found.iter().all(|c| c.tier == Tier::Local));
    }
}
