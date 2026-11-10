//! Pure selection model: Category → Group → Item with tri-state checks.
//! No terminal code here so it is fully unit-testable.

use std::collections::BTreeMap;

use super::candidate::{Candidate, Category, ItemId, Size, Tier};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Category(Category),
    Group,
    Item(ItemId),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub label: String,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Checked,
    Unchecked,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    pub selected: usize,
    pub total: usize,
    pub selected_bytes: u64,
    pub total_bytes: u64,
    pub has_unknown: bool,
}

#[derive(Debug, Clone)]
pub struct SelectionTree {
    nodes: Vec<Node>,
    roots: Vec<NodeId>,
    candidates: Vec<Candidate>,
    checked: Vec<bool>,
}

fn size_key(size: Size) -> (u8, std::cmp::Reverse<u64>) {
    match size {
        Size::Bytes(b) => (0, std::cmp::Reverse(b)),
        Size::Unknown | Size::Skipped => (1, std::cmp::Reverse(0)),
    }
}

impl SelectionTree {
    /// Categories in `Category::ALL` order; groups and items by size desc,
    /// unknown sizes last. Items start at their candidate's default.
    pub fn build(candidates: Vec<Candidate>) -> Self {
        let checked: Vec<bool> = candidates.iter().map(Candidate::default_selected).collect();
        let mut tree = Self {
            nodes: Vec::new(),
            roots: Vec::new(),
            candidates,
            checked,
        };

        for cat in Category::ALL {
            let mut groups: BTreeMap<String, Vec<ItemId>> = BTreeMap::new();
            for (i, c) in tree.candidates.iter().enumerate() {
                if c.category == cat {
                    groups.entry(c.group.clone()).or_default().push(ItemId(i));
                }
            }
            if groups.is_empty() {
                continue;
            }
            let cat_id = tree.push(None, cat.label().to_string(), NodeKind::Category(cat));
            tree.roots.push(cat_id);

            let mut group_list: Vec<(String, Vec<ItemId>)> = groups.into_iter().collect();
            group_list.sort_by_key(|(_, items)| {
                let known: Vec<u64> = items
                    .iter()
                    .filter_map(|i| match tree.candidates[i.0].size {
                        Size::Bytes(b) => Some(b),
                        Size::Unknown | Size::Skipped => None,
                    })
                    .collect();
                if known.is_empty() {
                    size_key(Size::Unknown)
                } else {
                    size_key(Size::Bytes(known.iter().sum()))
                }
            });

            for (group, mut items) in group_list {
                let gid = tree.push(Some(cat_id), group, NodeKind::Group);
                items.sort_by_key(|i| size_key(tree.candidates[i.0].size));
                for item in items {
                    let label = tree.candidates[item.0].label.clone();
                    tree.push(Some(gid), label, NodeKind::Item(item));
                }
            }
        }
        tree
    }

    fn push(&mut self, parent: Option<NodeId>, label: String, kind: NodeKind) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent,
            children: Vec::new(),
            label,
            kind,
        });
        if let Some(p) = parent {
            self.nodes[p.0].children.push(id);
        }
        id
    }

    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn candidate(&self, id: ItemId) -> &Candidate {
        &self.candidates[id.0]
    }

    /// Ancestors (root first) followed by `id` — the identifier path a tree
    /// widget needs.
    pub fn path_of(&self, id: NodeId) -> Vec<NodeId> {
        let mut path = vec![id];
        let mut cur = id;
        while let Some(p) = self.nodes[cur.0].parent {
            path.push(p);
            cur = p;
        }
        path.reverse();
        path
    }

    fn leaves(&self, id: NodeId, out: &mut Vec<ItemId>) {
        match self.nodes[id.0].kind {
            NodeKind::Item(item) => out.push(item),
            _ => {
                for c in &self.nodes[id.0].children {
                    self.leaves(*c, out);
                }
            }
        }
    }

    pub fn check(&self, id: NodeId) -> Check {
        let mut leaves = Vec::new();
        self.leaves(id, &mut leaves);
        if leaves.is_empty() {
            return Check::Unchecked;
        }
        let on = leaves.iter().filter(|i| self.checked[i.0]).count();
        if on == 0 {
            Check::Unchecked
        } else if on == leaves.len() {
            Check::Checked
        } else {
            Check::Partial
        }
    }

    /// Leaf: flip. Branch: check everything below unless already fully
    /// checked, in which case uncheck everything below.
    pub fn toggle(&mut self, id: NodeId) {
        let target = self.check(id) != Check::Checked;
        let mut leaves = Vec::new();
        self.leaves(id, &mut leaves);
        for leaf in leaves {
            self.checked[leaf.0] = target;
        }
    }

    pub fn set_all(&mut self, on: bool) {
        self.checked.iter_mut().for_each(|c| *c = on);
    }

    #[cfg(test)]
    pub fn select_defaults(&mut self) {
        for (i, c) in self.candidates.iter().enumerate() {
            self.checked[i] = c.default_selected();
        }
    }

    pub fn select_tier(&mut self, tier: Tier, on: bool) {
        for (i, c) in self.candidates.iter().enumerate() {
            if c.tier == tier {
                self.checked[i] = on;
            }
        }
    }

    /// Sum of known sizes below `id`, and whether any size was unknown.
    pub fn size(&self, id: NodeId) -> (u64, bool) {
        let mut leaves = Vec::new();
        self.leaves(id, &mut leaves);
        let mut total = 0u64;
        let mut unknown = false;
        for i in leaves {
            match self.candidates[i.0].size {
                Size::Bytes(b) => total = total.saturating_add(b),
                Size::Unknown | Size::Skipped => unknown = true,
            }
        }
        (total, unknown)
    }

    pub fn totals(&self) -> Totals {
        let mut t = Totals::default();
        for (i, c) in self.candidates.iter().enumerate() {
            t.total += 1;
            match c.size {
                Size::Bytes(b) => t.total_bytes = t.total_bytes.saturating_add(b),
                Size::Unknown | Size::Skipped => t.has_unknown = true,
            }
            if self.checked[i] {
                t.selected += 1;
                t.selected_bytes = t.selected_bytes.saturating_add(c.size.bytes());
            }
        }
        t
    }

    pub fn selected_items(&self) -> Vec<ItemId> {
        (0..self.candidates.len())
            .filter(|i| self.checked[*i])
            .map(ItemId)
            .collect()
    }

    /// `true` when the node's label, its candidate path, or any descendant
    /// matches `filter` (case-insensitive substring). Empty filter matches all.
    pub fn matches(&self, id: NodeId, filter: &str) -> bool {
        if filter.is_empty() {
            return true;
        }
        let needle = filter.to_lowercase();
        let node = &self.nodes[id.0];
        if node.label.to_lowercase().contains(&needle) {
            return true;
        }
        if let NodeKind::Item(item) = node.kind {
            if let Some(p) = self.candidates[item.0].target.path() {
                if p.to_string_lossy().to_lowercase().contains(&needle) {
                    return true;
                }
            }
        }
        node.children.iter().any(|c| self.matches(*c, filter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::purge::candidate::Target;
    use std::path::PathBuf;

    fn cand(cat: Category, group: &str, label: &str, size: Size, tier: Tier) -> Candidate {
        Candidate::new(
            cat,
            group,
            label,
            Target::dir(PathBuf::from(format!("/{}/{}", group, label))),
            size,
            tier,
        )
    }

    fn sample() -> SelectionTree {
        SelectionTree::build(vec![
            cand(Category::Temp, "tmp", "small", Size::Bytes(10), Tier::Local),
            cand(
                Category::Projects,
                "app",
                "node_modules",
                Size::Bytes(100),
                Tier::Local,
            ),
            cand(
                Category::Projects,
                "app",
                "dist",
                Size::Bytes(300),
                Tier::Local,
            ),
            cand(
                Category::Projects,
                "lib",
                "target",
                Size::Bytes(1000),
                Tier::Local,
            ),
            cand(
                Category::PackageCaches,
                "npm",
                "~/.npm",
                Size::Unknown,
                Tier::Global,
            ),
        ])
    }

    fn child_labels(t: &SelectionTree, id: NodeId) -> Vec<String> {
        t.node(id)
            .children
            .iter()
            .map(|c| t.node(*c).label.clone())
            .collect()
    }

    #[test]
    fn build_orders_categories_groups_and_items() {
        let t = sample();
        let roots: Vec<String> = t.roots().iter().map(|r| t.node(*r).label.clone()).collect();
        assert_eq!(roots, vec!["Projects", "Temp dirs", "Package caches"]);
        let projects = t.roots()[0];
        assert_eq!(child_labels(&t, projects), vec!["lib", "app"]);
        let app = t.node(projects).children[1];
        assert_eq!(child_labels(&t, app), vec!["dist", "node_modules"]);
    }

    #[test]
    fn defaults_follow_tier() {
        let t = sample();
        assert_eq!(t.check(t.roots()[0]), Check::Checked);
        assert_eq!(t.check(t.roots()[2]), Check::Unchecked);
        assert_eq!(t.totals().selected, 4);
    }

    #[test]
    fn toggle_leaf_makes_parents_partial() {
        let mut t = sample();
        let projects = t.roots()[0];
        let app = t.node(projects).children[1];
        let dist = t.node(app).children[0];
        t.toggle(dist);
        assert_eq!(t.check(dist), Check::Unchecked);
        assert_eq!(t.check(app), Check::Partial);
        assert_eq!(t.check(projects), Check::Partial);
    }

    #[test]
    fn toggle_partial_branch_checks_everything_then_unchecks() {
        let mut t = sample();
        let projects = t.roots()[0];
        let app = t.node(projects).children[1];
        let dist = t.node(app).children[0];
        t.toggle(dist);
        t.toggle(projects);
        assert_eq!(t.check(projects), Check::Checked);
        t.toggle(projects);
        assert_eq!(t.check(projects), Check::Unchecked);
        assert_eq!(
            t.check(t.roots()[1]),
            Check::Checked,
            "other category untouched"
        );
    }

    #[test]
    fn set_all_and_defaults_and_tier() {
        let mut t = sample();
        t.set_all(true);
        assert_eq!(t.totals().selected, 5);
        t.set_all(false);
        assert_eq!(t.totals().selected, 0);
        t.select_defaults();
        assert_eq!(t.totals().selected, 4);
        t.select_tier(Tier::Global, true);
        assert_eq!(t.totals().selected, 5);
    }

    #[test]
    fn totals_track_unknown_sizes() {
        let mut t = sample();
        let tot = t.totals();
        assert_eq!(tot.total_bytes, 1410);
        assert_eq!(tot.selected_bytes, 1410);
        assert!(tot.has_unknown);
        t.set_all(true);
        assert_eq!(t.totals().selected_bytes, 1410, "unknown adds 0 bytes");
        let (bytes, unknown) = t.size(t.roots()[2]);
        assert_eq!((bytes, unknown), (0, true));
    }

    #[test]
    fn filter_matches_descendants_and_paths() {
        let t = sample();
        let projects = t.roots()[0];
        assert!(t.matches(projects, "DIST"));
        assert!(!t.matches(projects, "npm"));
        assert!(t.matches(t.roots()[2], "/npm/"));
        assert!(t.matches(projects, ""));
    }

    #[test]
    fn path_of_walks_to_root() {
        let t = sample();
        let projects = t.roots()[0];
        let app = t.node(projects).children[1];
        let dist = t.node(app).children[0];
        assert_eq!(t.path_of(dist), vec![projects, app, dist]);
    }

    #[test]
    fn selected_items_are_candidate_ids() {
        let mut t = sample();
        t.set_all(false);
        let temp = t.roots()[1];
        t.toggle(temp);
        assert_eq!(t.selected_items(), vec![ItemId(0)]);
    }
}
