//! Non-interactive rendering of the selection tree (dry-run, non-TTY).

use crate::fs_size::human_size;

use super::tree::{Check, NodeId, SelectionTree};

const SIZE_COL: usize = 10;

pub fn mark(check: Check) -> &'static str {
    match check {
        Check::Checked => "[x]",
        Check::Unchecked => "[ ]",
        Check::Partial => "[-]",
    }
}

/// Size text for a node: `1.2 GB`, `~1.2 GB` when partly unknown, `?` when
/// nothing is known.
pub fn size_text(tree: &SelectionTree, id: NodeId) -> String {
    let (bytes, unknown) = tree.size(id);
    match (bytes, unknown) {
        (0, true) => "?".to_string(),
        (b, true) => format!("~{}", human_size(b)),
        (b, false) => human_size(b),
    }
}

/// Full tree, every node expanded, one line per node.
pub fn render(tree: &SelectionTree, width: usize) -> String {
    let mut out = String::new();
    for root in tree.roots() {
        render_node(tree, *root, 0, width, &mut out);
    }
    out
}

fn render_node(tree: &SelectionTree, id: NodeId, depth: usize, width: usize, out: &mut String) {
    let node = tree.node(id);
    let indent = "  ".repeat(depth);
    let mut label = node.label.clone();
    if let super::tree::NodeKind::Item(item) = node.kind {
        if let Some(cmd) = tree.candidate(item).target.command_line() {
            label = cmd;
        }
    }
    let left = format!("{}{} {}", indent, mark(tree.check(id)), label);
    let size = size_text(tree, id);
    let pad = width.saturating_sub(left.chars().count() + SIZE_COL).max(1);
    out.push_str(&format!("{}{}{:>SIZE_COL$}\n", left, " ".repeat(pad), size));
    for child in &node.children {
        render_node(tree, *child, depth + 1, width, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::purge::candidate::{Candidate, Category, Size, Target, Tier};
    use std::path::PathBuf;

    #[test]
    fn renders_marks_labels_and_sizes() {
        let tree = SelectionTree::build(vec![
            Candidate::new(
                Category::Projects,
                "~/app",
                "node_modules",
                Target::dir(PathBuf::from("/app/node_modules")),
                Size::Bytes(2048),
                Tier::Local,
            ),
            Candidate::new(
                Category::Docker,
                "docker",
                "unused images",
                Target::action("docker", &["image", "prune", "-af"], None, vec![]),
                Size::Unknown,
                Tier::Global,
            ),
        ]);
        let text = render(&tree, 60);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 6);
        assert!(lines[0].starts_with("[x] Projects"));
        assert!(lines[0].ends_with("2.0 KB"));
        assert!(lines[2].starts_with("    [x] node_modules"));
        assert!(lines[3].starts_with("[ ] Docker"));
        assert!(lines[3].ends_with("?"));
        assert!(lines[5].contains("$ docker image prune -af"));
    }

    #[test]
    fn size_text_marks_partial_unknown() {
        let tree = SelectionTree::build(vec![
            Candidate::new(
                Category::Xcode,
                "g",
                "a",
                Target::dir(PathBuf::from("/a")),
                Size::Bytes(1024),
                Tier::Local,
            ),
            Candidate::new(
                Category::Xcode,
                "g",
                "b",
                Target::action("x", &[], None, vec![]),
                Size::Unknown,
                Tier::Local,
            ),
        ]);
        assert_eq!(size_text(&tree, tree.roots()[0]), "~1.0 KB");
    }
}
