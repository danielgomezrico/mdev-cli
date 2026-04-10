use colored::Colorize;
use dialoguer::{Confirm, Input, Password};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

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

    /// Returns a spinner progress bar. Caller is responsible for finishing it.
    pub fn progress(&self, msg: &str) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        pb
    }
}

impl Default for Logger {
    fn default() -> Self {
        Self::new()
    }
}
