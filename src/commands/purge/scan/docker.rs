//! Docker reclaimable space, measured with `docker system df` and freed
//! through prune commands.

use super::super::candidate::{Candidate, Category, Freed, Size, Target, Tier};
use super::{ScanContext, Scanner};

pub struct DockerScanner;

impl Scanner for DockerScanner {
    fn category(&self) -> Category {
        Category::Docker
    }

    fn scan(&self, ctx: &ScanContext) -> Vec<Candidate> {
        if ctx.runner.which("docker").is_none() {
            return Vec::new();
        }
        ctx.note("docker system df");
        let df = ctx
            .runner
            .run("docker", &["system", "df", "--format", "{{json .}}"], None);
        if !df.is_success() {
            return Vec::new();
        }
        let usage = parse_system_df(&df.stdout);
        // Stopped containers may hold state (e.g. a dev DB): never by default.
        [
            (
                usage.build_cache,
                "build cache",
                &["builder", "prune", "-f"][..],
                Tier::Local,
            ),
            (
                usage.containers,
                "stopped containers",
                &["container", "prune", "-f"],
                Tier::Global,
            ),
            (
                usage.images,
                "all unused images",
                &["image", "prune", "-af"],
                Tier::Global,
            ),
        ]
        .into_iter()
        .filter(|(bytes, ..)| *bytes > 0)
        .map(|(bytes, label, args, tier)| {
            Candidate::new(
                Category::Docker,
                "docker",
                label,
                Target::action("docker", args, None, Freed::Reported),
                Size::Bytes(bytes),
                tier,
            )
        })
        .collect()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DockerUsage {
    pub images: u64,
    pub containers: u64,
    pub build_cache: u64,
}

/// One JSON object per line, e.g. `{"Type":"Images","Reclaimable":"1.2GB (40%)",...}`.
pub fn parse_system_df(out: &str) -> DockerUsage {
    let mut u = DockerUsage::default();
    for line in out.lines() {
        let v: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind = v.get("Type").and_then(|t| t.as_str()).unwrap_or("");
        let reclaimable = v
            .get("Reclaimable")
            .and_then(|r| r.as_str())
            .map(parse_docker_size)
            .unwrap_or(0);
        match kind {
            "Images" => u.images = reclaimable,
            "Containers" => u.containers = reclaimable,
            "Build Cache" => u.build_cache = reclaimable,
            _ => {}
        }
    }
    u
}

/// Bytes in the `Total reclaimed space: 1.2GB` line printed by `docker … prune`.
pub fn reclaimed(out: &str) -> Option<u64> {
    out.lines()
        .find_map(|l| l.trim().strip_prefix("Total reclaimed space:"))
        .map(parse_docker_size)
}

/// `"1.2GB (40%)"` / `"512MiB"` → bytes. Decimal (`kB`, `MB`…) and binary
/// (`KiB`, `MiB`…) units.
pub fn parse_docker_size(s: &str) -> u64 {
    let head = s.split_whitespace().next().unwrap_or("");
    let num_end = head
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(head.len());
    let num: f64 = head[..num_end].parse().unwrap_or(0.0);
    let unit = head[num_end..].to_ascii_lowercase();
    let (prefix, base) = match unit.strip_suffix("ib") {
        Some(p) => (p, 1024f64),
        None => (unit.strip_suffix('b').unwrap_or(&unit), 1000f64),
    };
    let exp = match prefix {
        "k" => 1,
        "m" => 2,
        "g" => 3,
        "t" => 4,
        _ => 0,
    };
    (num * base.powi(exp)) as u64
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, FakeRunner};
    use super::*;
    use crate::runner::RunResult;
    use std::path::PathBuf;
    use tempfile::TempDir;

    const DF: &str = r#"{"Type":"Images","TotalCount":"5","Active":"2","Size":"3GB","Reclaimable":"1.5GB (50%)"}
{"Type":"Containers","TotalCount":"3","Active":"1","Size":"100MB","Reclaimable":"80MB (80%)"}
{"Type":"Local Volumes","TotalCount":"2","Active":"2","Size":"1GB","Reclaimable":"0B (0%)"}
{"Type":"Build Cache","TotalCount":"10","Active":"0","Size":"500MB","Reclaimable":"500MB"}"#;

    #[test]
    fn parses_sizes_and_df_lines() {
        assert_eq!(parse_docker_size("1.5GB (50%)"), 1_500_000_000);
        assert_eq!(parse_docker_size("80MB (80%)"), 80_000_000);
        assert_eq!(parse_docker_size("0B (0%)"), 0);
        assert_eq!(parse_docker_size("12kB"), 12_000);
        assert_eq!(parse_docker_size("2KiB"), 2048);
        assert_eq!(parse_docker_size("1.5MiB"), 1_572_864);
        assert_eq!(parse_docker_size("2GiB"), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_docker_size("1TiB"), 1 << 40);
        assert_eq!(parse_docker_size("17B"), 17);
        let u = parse_system_df(DF);
        assert_eq!(
            u,
            DockerUsage {
                images: 1_500_000_000,
                containers: 80_000_000,
                build_cache: 500_000_000
            }
        );
    }

    #[test]
    fn parses_total_reclaimed_space() {
        let out = "ID\tRECLAIMABLE\tSIZE\nabc\ttrue\t1MB\nTotal reclaimed space: 1.5GB";
        assert_eq!(reclaimed(out), Some(1_500_000_000));
        assert_eq!(reclaimed("Total:\t2.1GiB"), None);
        assert_eq!(reclaimed("Total reclaimed space: 0B"), Some(0));
        assert_eq!(reclaimed(""), None);
    }

    #[test]
    fn only_build_cache_is_local_containers_and_images_global() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(&["docker"]).respond("docker", RunResult::success(DF.into()));
        let roots: Vec<PathBuf> = vec![];
        let c = ctx(&roots, &[], tmp.path(), vec![], &runner);
        let found = DockerScanner.scan(&c);
        let got: Vec<(String, Tier, Size)> = found
            .iter()
            .map(|c| (c.target.command_line().unwrap(), c.tier, c.size))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "$ docker builder prune -f".into(),
                    Tier::Local,
                    Size::Bytes(500_000_000)
                ),
                (
                    "$ docker container prune -f".into(),
                    Tier::Global,
                    Size::Skipped
                ),
                (
                    "$ docker image prune -af".into(),
                    Tier::Global,
                    Size::Skipped
                ),
            ]
        );
    }

    #[test]
    fn no_docker_or_dead_daemon_means_nothing() {
        let tmp = TempDir::new().unwrap();
        let roots: Vec<PathBuf> = vec![];
        let none = FakeRunner::new(&[]);
        assert!(DockerScanner
            .scan(&ctx(&roots, &[], tmp.path(), vec![], &none))
            .is_empty());
        let dead = FakeRunner::new(&["docker"])
            .respond("docker", RunResult::failure(1, "Cannot connect".into()));
        assert!(DockerScanner
            .scan(&ctx(&roots, &[], tmp.path(), vec![], &dead))
            .is_empty());
    }
}
