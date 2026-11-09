//! Checkbox tree over a `SelectionTree`. Pure `handle` + `render` so both
//! are testable without a real terminal; `run` owns the event loop.

use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::commands::purge::candidate::Tier;
use crate::commands::purge::text_ui::{mark, size_text};
use crate::commands::purge::tree::{Check, NodeId, NodeKind, SelectionTree};
use crate::fs_size::human_size;

use super::keys::{self, Key};
use super::SelectOutcome;

const PAGE: usize = 10;

pub struct App<'a> {
    pub tree: &'a mut SelectionTree,
    pub state: TreeState<NodeId>,
    pub filter: String,
    pub filter_mode: bool,
    pub title: String,
    width: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Confirm,
    Abort,
}

impl<'a> App<'a> {
    pub fn new(tree: &'a mut SelectionTree, title: &str) -> Self {
        let mut state = TreeState::default();
        for root in tree.roots() {
            state.open(vec![*root]);
        }
        if let Some(first) = tree.roots().first() {
            state.select(vec![*first]);
        }
        Self {
            tree,
            state,
            filter: String::new(),
            filter_mode: false,
            title: title.to_string(),
            width: 100,
        }
    }

    fn selected_node(&self) -> Option<NodeId> {
        self.state.selected().last().copied()
    }

    /// Visible items honouring the filter; branches containing matches are
    /// opened so the matches show.
    fn items(&mut self) -> Vec<TreeItem<'static, NodeId>> {
        let roots: Vec<NodeId> = self.tree.roots().to_vec();
        let mut out = Vec::new();
        for root in roots {
            if let Some(item) = self.item_for(root, 0) {
                out.push(item);
            }
        }
        out
    }

    fn item_for(&mut self, id: NodeId, depth: usize) -> Option<TreeItem<'static, NodeId>> {
        if !self.tree.matches(id, &self.filter) {
            return None;
        }
        let children: Vec<NodeId> = self.tree.node(id).children.clone();
        let text = self.row(id, depth);
        if children.is_empty() {
            return Some(TreeItem::new_leaf(id, text));
        }
        let kids: Vec<TreeItem<'static, NodeId>> = children
            .into_iter()
            .filter_map(|c| self.item_for(c, depth + 1))
            .collect();
        if !self.filter.is_empty() {
            self.state.open(self.tree.path_of(id));
        }
        TreeItem::new(id, text, kids).ok()
    }

    fn row(&self, id: NodeId, depth: usize) -> Line<'static> {
        let node = self.tree.node(id);
        let check = self.tree.check(id);
        let mut label = node.label.clone();
        let mut tier_hint = "";
        if let NodeKind::Item(item) = node.kind {
            let cand = self.tree.candidate(item);
            if let Some(cmd) = cand.target.command_line() {
                label = cmd;
            }
            if cand.tier == Tier::Global {
                tier_hint = " ◆";
            }
        }
        let size = size_text(self.tree, id);
        // borders (2) + indent per depth (2) + open/closed symbol (2)
        let used = 2 + depth * 2 + 2 + 4 + label.chars().count() + tier_hint.len() + size.len();
        let pad = (self.width as usize).saturating_sub(used).max(1);
        let label_style = match check {
            Check::Checked => Style::new(),
            Check::Partial => Style::new(),
            Check::Unchecked => Style::new().dim(),
        };
        let mark_style = match check {
            Check::Checked => Style::new().fg(Color::Green),
            Check::Partial => Style::new().fg(Color::Yellow),
            Check::Unchecked => Style::new().dim(),
        };
        let mut spans = vec![
            Span::styled(mark(check).to_string(), mark_style),
            Span::raw(" "),
            Span::styled(label, label_style),
        ];
        if !tier_hint.is_empty() {
            spans.push(Span::styled(
                tier_hint.to_string(),
                Style::new().fg(Color::Yellow),
            ));
        }
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(size, Style::new().fg(Color::Cyan)));
        Line::from(spans)
    }
}

/// Apply one key. Pure with respect to the terminal.
pub fn handle(app: &mut App, key: Key) -> Flow {
    if app.filter_mode {
        match key {
            Key::Esc => {
                app.filter.clear();
                app.filter_mode = false;
            }
            Key::Enter => app.filter_mode = false,
            Key::Backspace => {
                app.filter.pop();
            }
            Key::Char(c) => app.filter.push(c),
            Key::Up => {
                app.state.key_up();
            }
            Key::Down => {
                app.state.key_down();
            }
            Key::Quit => return Flow::Abort,
            _ => {}
        }
        return Flow::Continue;
    }
    match key {
        Key::Up => {
            app.state.key_up();
        }
        Key::Down => {
            app.state.key_down();
        }
        Key::Left => {
            app.state.key_left();
        }
        Key::Right => {
            app.state.key_right();
        }
        Key::PageUp => {
            for _ in 0..PAGE {
                app.state.key_up();
            }
        }
        Key::PageDown => {
            for _ in 0..PAGE {
                app.state.key_down();
            }
        }
        Key::Home => {
            app.state.select_first();
        }
        Key::End => {
            app.state.select_last();
        }
        Key::Space => {
            if let Some(id) = app.selected_node() {
                app.tree.toggle(id);
            }
        }
        Key::SelectAll => app.tree.set_all(true),
        Key::SelectNone => app.tree.set_all(false),
        Key::Filter => app.filter_mode = true,
        Key::Enter => return Flow::Confirm,
        Key::Esc | Key::Quit => return Flow::Abort,
        Key::Backspace => {
            app.filter.clear();
        }
        Key::Char(_) | Key::Other => {}
    }
    Flow::Continue
}

pub fn render(app: &mut App, frame: &mut Frame) {
    let area = frame.area();
    app.width = area.width;
    let [tree_area, footer_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(area);

    let totals = app.tree.totals();
    let approx = if totals.has_unknown { "~" } else { "" };
    let title = Line::from(vec![
        Span::styled(format!(" {} ", app.title), Style::new().bold().cyan()),
        Span::raw(format!(
            "— {} of {} items · {}{} of {}{} ",
            totals.selected,
            totals.total,
            approx,
            human_size(totals.selected_bytes),
            approx,
            human_size(totals.total_bytes)
        )),
    ]);

    let items = app.items();
    let block = Block::bordered().title(title);
    match Tree::new(&items) {
        Ok(tree) => {
            let tree = tree
                .block(block)
                .node_closed_symbol("▸ ")
                .node_open_symbol("▾ ")
                .node_no_children_symbol("  ")
                .highlight_style(
                    Style::new()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                );
            frame.render_stateful_widget(tree, tree_area, &mut app.state);
        }
        Err(_) => frame.render_widget(block, tree_area),
    }

    render_footer(app, frame, footer_area);
}

fn render_footer(app: &App, frame: &mut Frame, area: Rect) {
    let filter_line = if app.filter_mode {
        Line::from(vec![
            Span::styled(" filter: ", Style::new().yellow()),
            Span::raw(app.filter.clone()),
            Span::styled("▏", Style::new().yellow()),
            Span::styled("  (Enter keep · Esc clear)", Style::new().dim()),
        ])
    } else if !app.filter.is_empty() {
        Line::from(vec![
            Span::styled(" filter: ", Style::new().yellow()),
            Span::raw(app.filter.clone()),
            Span::styled("  (Backspace clears)", Style::new().dim()),
        ])
    } else {
        Line::from(Span::styled(
            " ◆ = expensive to recreate (unchecked by default)",
            Style::new().dim(),
        ))
    };
    let keys_line = Line::from(vec![
        key("↑↓", "move"),
        key("␣", "toggle"),
        key("←→", "collapse/expand"),
        key("a", "all"),
        key("n", "none"),
        key("/", "filter"),
        key("⏎", "delete selected"),
        key("q", "quit"),
    ]);
    frame.render_widget(Paragraph::new(vec![filter_line, keys_line]), area);
}

fn key(k: &str, what: &str) -> Span<'static> {
    Span::raw(format!(" {} {} ", k.bold(), what.dim()))
}

pub fn run(tree: &mut SelectionTree, title: &str) -> std::io::Result<SelectOutcome> {
    let mut terminal = ratatui::init();
    let mut app = App::new(tree, title);
    let outcome = loop {
        terminal.draw(|frame| render(&mut app, frame))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        if let Event::Key(ev) = event::read()? {
            if ev.kind != KeyEventKind::Press {
                continue;
            }
            let key = keys::from_event(&ev, app.filter_mode);
            match handle(&mut app, key) {
                Flow::Continue => {}
                Flow::Confirm => break SelectOutcome::Confirm,
                Flow::Abort => break SelectOutcome::Abort,
            }
        }
    };
    ratatui::restore();
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::purge::candidate::{Candidate, Category, Size, Target};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn sample() -> SelectionTree {
        SelectionTree::build(vec![
            Candidate::new(
                Category::Projects,
                "~/app",
                "node_modules",
                Target::dir(PathBuf::from("/app/node_modules")),
                Size::Bytes(2048),
                Tier::Local,
            ),
            Candidate::new(
                Category::Projects,
                "~/app",
                "dist",
                Target::dir(PathBuf::from("/app/dist")),
                Size::Bytes(1024),
                Tier::Local,
            ),
            Candidate::new(
                Category::PackageCaches,
                "npm",
                "~/.npm",
                Target::dir(PathBuf::from("/home/.npm")),
                Size::Bytes(4096),
                Tier::Global,
            ),
        ])
    }

    /// Render (so the tree state knows the visible rows) then apply the key,
    /// exactly like the event loop does.
    fn press(app: &mut App, key: Key) -> Flow {
        screen(app, 80, 20);
        handle(app, key)
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| render(app, f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut lines = Vec::new();
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            lines.push(line);
        }
        lines.join("\n")
    }

    #[test]
    fn space_toggles_selected_category_and_enter_confirms() {
        let mut tree = sample();
        let mut app = App::new(&mut tree, "t");
        let projects = app.tree.roots()[0];
        assert_eq!(handle(&mut app, Key::Space), Flow::Continue);
        assert_eq!(app.tree.check(projects), Check::Unchecked);
        handle(&mut app, Key::Space);
        assert_eq!(app.tree.check(projects), Check::Checked);
        assert_eq!(handle(&mut app, Key::Enter), Flow::Confirm);
        assert_eq!(handle(&mut app, Key::Quit), Flow::Abort);
    }

    #[test]
    fn navigation_reaches_items_and_all_none_work() {
        let mut tree = sample();
        let mut app = App::new(&mut tree, "t");
        press(&mut app, Key::Down); // group ~/app
        press(&mut app, Key::Right); // open group
        press(&mut app, Key::Down); // first item (node_modules)
        let sel = app.selected_node().unwrap();
        assert_eq!(app.tree.node(sel).label, "node_modules");
        press(&mut app, Key::Space);
        assert_eq!(app.tree.check(app.tree.roots()[0]), Check::Partial);
        press(&mut app, Key::SelectAll);
        assert_eq!(app.tree.totals().selected, 3);
        press(&mut app, Key::SelectNone);
        assert_eq!(app.tree.totals().selected, 0);
    }

    #[test]
    fn filter_mode_collects_text_and_esc_clears() {
        let mut tree = sample();
        let mut app = App::new(&mut tree, "t");
        handle(&mut app, Key::Filter);
        assert!(app.filter_mode);
        handle(&mut app, Key::Char('n'));
        handle(&mut app, Key::Char('p'));
        assert_eq!(app.filter, "np");
        handle(&mut app, Key::Enter);
        assert!(!app.filter_mode);
        assert_eq!(app.filter, "np");
        let items = app.items();
        assert_eq!(items.len(), 1, "only Package caches matches 'np'");
        handle(&mut app, Key::Filter);
        handle(&mut app, Key::Esc);
        assert!(app.filter.is_empty());
    }

    #[test]
    fn render_shows_marks_sizes_and_totals() {
        let mut tree = sample();
        let mut app = App::new(&mut tree, "mdev purge");
        let text = screen(&mut app, 80, 12);
        assert!(text.contains("mdev purge"), "{text}");
        assert!(text.contains("2 of 3 items"), "{text}");
        assert!(text.contains("[x] Projects"), "{text}");
        assert!(text.contains("3.0 KB"), "{text}");
        assert!(text.contains("[ ] Package caches"), "{text}");
        assert!(text.contains("toggle"), "{text}");
        assert!(text.contains("◆"), "{text}");
        press(&mut app, Key::Down);
        press(&mut app, Key::Right);
        press(&mut app, Key::Down);
        press(&mut app, Key::Space);
        let text = screen(&mut app, 80, 12);
        assert!(text.contains("[-] Projects"), "{text}");
        assert!(text.contains("[ ] node_modules"), "{text}");
    }
}
