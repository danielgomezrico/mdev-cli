//! End-of-run summary: per category counts and bytes, total, failures.

use std::collections::BTreeMap;
use std::time::Duration;

use colored::Colorize;

use crate::fs_size::human_size;

use super::candidate::Category;
use super::executor::Outcome;
use super::tree::SelectionTree;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CategoryStats {
    pub attempted: usize,
    pub succeeded: usize,
    pub bytes: u64,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub per_category: BTreeMap<Category, CategoryStats>,
    pub total_bytes: u64,
    pub failures: Vec<(String, String)>,
}

pub fn summarize(tree: &SelectionTree, outcomes: &[Outcome]) -> Summary {
    let mut s = Summary::default();
    for o in outcomes {
        let cand = tree.candidate(o.item);
        let stats = s.per_category.entry(cand.category).or_default();
        stats.attempted += 1;
        match &o.error {
            None => {
                stats.succeeded += 1;
                stats.bytes = stats.bytes.saturating_add(o.bytes_freed);
                s.total_bytes = s.total_bytes.saturating_add(o.bytes_freed);
            }
            Some(e) => {
                let what = cand
                    .target
                    .path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| cand.label.clone());
                s.failures.push((what, e.clone()));
            }
        }
    }
    s
}

pub fn render(summary: &Summary, elapsed: Duration, dry_run: bool) -> String {
    let verb = if dry_run { "would free" } else { "freed" };
    let mut out = String::new();
    out.push_str(&format!(
        "\n{} ({:.1}s)\n",
        "Purge summary".cyan().bold(),
        elapsed.as_secs_f64()
    ));
    for (cat, st) in &summary.per_category {
        let failed = st.attempted - st.succeeded;
        let tail = if failed > 0 {
            format!("   {}", format!("{failed} failed").red())
        } else {
            String::new()
        };
        out.push_str(&format!(
            "  {:<16} {:>3}/{:<3} {:>10}{}\n",
            cat.label(),
            st.succeeded,
            st.attempted,
            human_size(st.bytes),
            tail
        ));
    }
    out.push_str(&format!(
        "  {} {}\n",
        format!("Total {verb}:").bold(),
        human_size(summary.total_bytes).green().bold()
    ));
    if !summary.failures.is_empty() {
        out.push_str(&format!("\n{}\n", "Failures".red().bold()));
        for (what, err) in &summary.failures {
            out.push_str(&format!("  {} {}: {}\n", "✗".red(), what, err));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::purge::candidate::{Candidate, ItemId, Size, Target, Tier};
    use std::path::PathBuf;

    #[test]
    fn summarize_groups_by_category_and_collects_failures() {
        let tree = SelectionTree::build(vec![
            Candidate::new(
                Category::Projects,
                "g",
                "a",
                Target::dir(PathBuf::from("/a")),
                Size::Bytes(10),
                Tier::Local,
            ),
            Candidate::new(
                Category::Projects,
                "g",
                "b",
                Target::dir(PathBuf::from("/b")),
                Size::Bytes(10),
                Tier::Local,
            ),
            Candidate::new(
                Category::Temp,
                "g",
                "c",
                Target::dir(PathBuf::from("/c")),
                Size::Bytes(5),
                Tier::Local,
            ),
        ]);
        let outcomes = vec![
            Outcome {
                item: ItemId(0),
                bytes_freed: 10,
                error: None,
            },
            Outcome {
                item: ItemId(1),
                bytes_freed: 0,
                error: Some("busy".into()),
            },
            Outcome {
                item: ItemId(2),
                bytes_freed: 5,
                error: None,
            },
        ];
        let s = summarize(&tree, &outcomes);
        assert_eq!(s.total_bytes, 15);
        assert_eq!(s.per_category[&Category::Projects].attempted, 2);
        assert_eq!(s.per_category[&Category::Projects].succeeded, 1);
        assert_eq!(s.failures, vec![("/b".to_string(), "busy".to_string())]);
        let text = render(&s, Duration::from_millis(1500), false);
        assert!(text.contains("Projects"));
        assert!(text.contains("1 failed"));
        assert!(text.contains("/b: busy"));
        assert!(text.contains("15 B"));
    }
}
