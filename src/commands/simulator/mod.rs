pub mod android;
pub mod ios;

use clap::{Args, Subcommand};

use crate::runner::Runner;

#[derive(Args, Debug)]
pub struct SimulatorArgs {
    #[command(subcommand)]
    pub command: SimulatorCommands,
}

#[derive(Subcommand, Debug)]
pub enum SimulatorCommands {
    /// Boot an iOS simulator and show it in Simulator.app (--off shuts it down).
    ///
    /// Reuses an already-booted simulator; otherwise picks the newest iOS
    /// runtime that has a device matching --device.
    ///
    /// On: mdev sim i [--device "iPhone 17 Pro"]
    ///
    /// Off: mdev sim i --off [--device "iPhone 17 Pro"]
    #[command(visible_alias = "i")]
    Ios(ios::IosArgs),
    /// Start an Android emulator and wait until it finishes booting (--off stops it).
    ///
    /// Reuses the AVD if it is already running, and reports the emulator's own
    /// log when it fails to start.
    ///
    /// On: mdev sim a [--avd Pixel_7_API_34]
    ///
    /// Off: mdev sim a --off [--avd Pixel_7_API_34] — every running emulator when --avd is omitted
    #[command(visible_alias = "a")]
    Android(android::AndroidArgs),
}

pub fn run(args: &SimulatorArgs, runner: &dyn Runner) -> i32 {
    match &args.command {
        SimulatorCommands::Ios(ios_args) => ios::run(ios_args, runner),
        SimulatorCommands::Android(android_args) => android::run(android_args, runner),
    }
}
