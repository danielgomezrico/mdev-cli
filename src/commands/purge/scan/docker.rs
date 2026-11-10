//! Docker reclaimable space, measured with `docker system df` and freed
//! through prune commands.

use super::super::candidate::{Candidate, Category, Size, Target, Tier};
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
                Target::action("docker", args, None, vec![]),
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

/// `"1.2GB (40%)"` → bytes. Docker prints decimal units.
pub fn parse_docker_size(s: &str) -> u64 {
    let head = s.split_whitespace().next().unwrap_or("");
    let num_end = head
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(head.len());
    let num: f64 = head[..num_end].parse().unwrap_or(0.0);
    let unit = head[num_end..].trim().to_ascii_lowercase();
    let mult: f64 = match unit.as_str() {
        "b" | "" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        _ => 1.0,
    };
    (num * mult) as u64
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
                    Size::Bytes(80_000_000)
                ),
                (
                    "$ docker image prune -af".into(),
                    Tier::Global,
                    Size::Bytes(1_500_000_000)
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
