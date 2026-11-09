use colored::Colorize;
use dialoguer::{Confirm, Input, Password};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

const TICKS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
    pub fn set_position(&self, pos: u64) {
        self.0.set_position(pos);
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
