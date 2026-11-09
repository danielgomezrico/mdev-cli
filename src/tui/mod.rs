//! Full-screen terminal UI. The only module that touches `ratatui` and its
//! re-exported `crossterm`; the rest of the crate sees a two-function API.

use std::io::IsTerminal;

use crate::commands::purge::tree::SelectionTree;

mod keys;
mod tree_select;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectOutcome {
    Confirm,
    Abort,
}

/// Let the user toggle nodes of `tree` in place. Returns how they left.
pub fn select_tree(tree: &mut SelectionTree, title: &str) -> std::io::Result<SelectOutcome> {
    tree_select::run(tree, title)
}

pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

pub fn terminal_width() -> Option<usize> {
    ratatui::crossterm::terminal::size()
        .ok()
        .map(|(w, _)| w as usize)
}
