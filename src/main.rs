mod app_detector;
mod commands;
mod device_manager;
mod fs_size;
mod logger;
mod models;
mod parallel;
mod runner;
mod tui;

use clap::{CommandFactory, Parser, Subcommand};
use commands::{
    clear::ClearArgs, doctor, emulator::EmulatorArgs, kill::KillArgs, purge::PurgeArgs,
    reboot::RebootArgs, simulator::SimulatorArgs, uninstall::UninstallArgs,
};
use runner::ProcessRunner;

const VERSION: &str = "1.0.0";

#[derive(Parser)]
#[command(
    name = "mdev",
    about = "mdev — caches and lifecycle for Flutter, Android, iOS, Node, Rust, Go, Ruby/Rails, and Python projects.\nRun from within your project directory."
)]
struct Cli {
    /// Print the mdev version.
    #[arg(long)]
    version: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Uninstall the app from connected devices.
    ///
    /// Starts an Android emulator when none is attached.
    #[command(visible_alias = "u")]
    Uninstall(UninstallArgs),
    /// Clear app data and restart on connected devices.
    ///
    /// Starts an Android emulator when none is attached.
    #[command(visible_alias = "c")]
    Clear(ClearArgs),
    /// Kill the running process for the current project (app on devices, or dev server)
    #[command(visible_alias = "x")]
    Kill(KillArgs),
    /// Restart the running process for the current project (relaunch the app, or restart the dev server)
    #[command(visible_alias = "r")]
    Reboot(RebootArgs),
    /// Find and delete build artifacts, caches, temp files, logs and stale worktrees
    #[command(visible_alias = "p")]
    Purge(PurgeArgs),
    /// Manage Android AVD emulators (e.g. config tweaks)
    #[command(visible_alias = "e")]
    Emulator(EmulatorArgs),
    /// Turn an iOS simulator or Android emulator on (`sim i` / `sim a`) or off (`--off`)
    #[command(visible_alias = "sim")]
    Simulator(SimulatorArgs),
    /// Check development environment
    #[command(visible_alias = "d")]
    Doctor,
}

fn main() {
    let cli = Cli::parse();

    if cli.version {
        println!("mdev version {}", VERSION);
        std::process::exit(0);
    }

    let runner = ProcessRunner::new();

    let exit_code = match cli.command {
        None => {
            // Print help and exit
            Cli::command().print_help().unwrap();
            println!();
            0
        }
        Some(Commands::Uninstall(ref args)) => commands::uninstall::run(args, &runner),
        Some(Commands::Clear(ref args)) => commands::clear::run(args, &runner),
        Some(Commands::Kill(ref args)) => commands::kill::run(args, &runner),
        Some(Commands::Reboot(ref args)) => commands::reboot::run(args, &runner),
        Some(Commands::Purge(ref args)) => commands::purge::run(args, &runner),
        Some(Commands::Emulator(ref args)) => commands::emulator::run(args, &runner),
        Some(Commands::Simulator(ref args)) => commands::simulator::run(args, &runner),
        Some(Commands::Doctor) => doctor::run(&runner),
    };

    std::process::exit(exit_code);
}
