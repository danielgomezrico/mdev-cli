mod app_detector;
mod commands;
mod device_manager;
mod logger;
mod models;
mod runner;

use clap::{CommandFactory, Parser, Subcommand};
use commands::{
    clear::ClearArgs, doctor, emulator::EmulatorArgs, keystore::KeystoreArgs, purge::PurgeArgs,
    uninstall::UninstallArgs,
};
use runner::ProcessRunner;

const VERSION: &str = "1.0.0";

#[derive(Parser)]
#[command(
    name = "mdev",
    about = "Flutter/Android/iOS developer CLI helper.\nRun from within your project directory."
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
    /// Uninstall the app from connected devices
    Uninstall(UninstallArgs),
    /// Clear app data and restart on connected devices
    Clear(ClearArgs),
    /// Purge build artifacts and caches
    Purge(PurgeArgs),
    /// Generate an Android signing keystore
    Keystore(KeystoreArgs),
    /// Manage Android AVD emulators (e.g. config tweaks)
    Emulator(EmulatorArgs),
    /// Check development environment
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
        Some(Commands::Purge(ref args)) => commands::purge::run(args, &runner),
        Some(Commands::Keystore(ref args)) => commands::keystore::run(args, &runner),
        Some(Commands::Emulator(ref args)) => commands::emulator::run(args, &runner),
        Some(Commands::Doctor) => doctor::run(&runner),
    };

    std::process::exit(exit_code);
}
