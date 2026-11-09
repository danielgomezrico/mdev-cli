use colored::Colorize;
use dialoguer::{Confirm, Input, Password};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::time::Duration;

/// Spinner frames; the last entry is drawn once the indicator finishes.
const TICKS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✓"];

pub struct Logger;

impl Logger {
    pub fn new() -> Self {
        Self
    }

    pub fn info(&self, msg: &str) {
        println!("{}", msg);
    }

    pub fn err(&self, msg: &str) {
        eprintln!("{}", msg.red());
    }

    pub fn warn(&self, msg: &str) {
        println!("{}", msg.yellow());
    }

    pub fn success(&self, msg: &str) {
        println!("{}", msg.green());
    }

    pub fn detail(&self, msg: &str) {
        println!("{}", msg.dimmed());
    }

    pub fn prompt(&self, msg: &str) -> String {
        Input::new()
            .with_prompt(msg)
            .interact_text()
            .unwrap_or_default()
    }

    pub fn prompt_password(&self, msg: &str) -> String {
        Password::new()
            .with_prompt(msg)
            .interact()
            .unwrap_or_default()
    }

    pub fn confirm(&self, msg: &str, default_value: bool) -> bool {
        Confirm::new()
            .with_prompt(msg)
            .default(default_value)
            .interact()
            .unwrap_or(default_value)
    }

    /// Indeterminate spinner with a message. Caller finishes it.
    pub fn progress(&self, msg: &str) -> Progress {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_strings(TICKS),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        Progress(pb)
    }

    /// Determinate `pos/len` bar with a message. Caller finishes it.
    pub fn progress_bar(&self, total: u64, msg: &str) -> Progress {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} [{bar:24.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("█▓░")
                .tick_strings(TICKS),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        Progress(pb)
    }

    /// Several live indicators stacked on screen. Caller clears it.
    pub fn progress_group(&self) -> ProgressGroup {
        ProgressGroup(MultiProgress::new())
    }
}

/// Stack of indicators, one line each, redrawn together.
pub struct ProgressGroup(MultiProgress);

impl ProgressGroup {
    /// Spinner line `⠋ prefix   message`; message truncates to the terminal.
    pub fn spinner(&self, prefix: &str, msg: &str) -> Progress {
        let pb = self.0.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {prefix:<15.bold} {wide_msg}")
                .unwrap()
                .tick_strings(TICKS),
        );
        pb.set_prefix(prefix.to_string());
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        Progress(pb)
    }

    /// Determinate bar line below the spinners.
    pub fn bar(&self, total: u64, msg: &str) -> Progress {
        let pb = self.0.add(ProgressBar::new(total));
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.cyan} [{bar:24.cyan/blue}] {pos}/{len} {wide_msg}",
            )
            .unwrap()
            .progress_chars("█▓░")
            .tick_strings(TICKS),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        Progress(pb)
    }

    /// Remove every line from the screen.
    pub fn clear(&self) {
        let _ = self.0.clear();
    }
}

impl Default for Logger {
    fn default() -> Self {
        Self::new()
    }
}

/// Live progress indicator (spinner or bar). Wraps the terminal library so
/// callers never touch it directly. Safe to share across threads.
#[derive(Clone)]
pub struct Progress(ProgressBar);

impl Progress {
    pub fn set_message(&self, msg: String) {
        self.0.set_message(msg);
    }

    #[allow(dead_code)]
    pub fn set_prefix(&self, prefix: String) {
        self.0.set_prefix(prefix);
    }

    pub fn set_position(&self, pos: u64) {
        self.0.set_position(pos);
    }

    pub fn set_length(&self, len: u64) {
        self.0.set_length(len);
    }

    #[allow(dead_code)]
    pub fn inc(&self, delta: u64) {
        self.0.inc(delta);
    }

    /// Run `f` with the indicator hidden so printed lines are not eaten.
    pub fn suspend<R>(&self, f: impl FnOnce() -> R) -> R {
        self.0.suspend(f)
    }

    pub fn finish_and_clear(&self) {
        self.0.finish_and_clear();
    }

    pub fn finish_with_message(&self, msg: String) {
        self.0.finish_with_message(msg);
    }
}
