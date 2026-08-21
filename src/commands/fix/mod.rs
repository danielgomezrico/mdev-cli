pub mod android;

use clap::{Args, Subcommand};

use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct FixArgs {
    #[command(subcommand)]
    pub command: FixCommands,
}

#[derive(Subcommand, Debug)]
pub enum FixCommands {
    /// Repair a running Android emulator that lost its network (Wi-Fi shows "!", nothing loads).
    ///
    /// Diagnoses the emulator first, then applies the smallest fix that works:
    /// leaves airplane mode, re-enables Wi-Fi, toggles Wi-Fi to force
    /// re-validation, and — when the emulator's own DNS forwarder is dead
    /// because the host resolvers changed — cold-boots it with working
    /// `-dns-server` addresses.
    ///
    /// mdev fix a [--avd Pixel_7_API_34] [--dns 8.8.8.8,1.1.1.1] [--no-restart] [--yes]
    #[command(visible_alias = "a")]
    Android(android::AndroidArgs),
}

pub fn run(args: &FixArgs, runner: &dyn Runner) -> i32 {
    match &args.command {
        FixCommands::Android(android_args) => android::run(android_args, runner),
    }
}
