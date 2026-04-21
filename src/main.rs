mod app_detector;
mod commands;
mod device_manager;
mod logger;
mod models;
mod runner;

use clap::{CommandFactory, Parser, Subcommand};
use commands::{
    clear::ClearArgs, completions::CompletionsArgs, doctor, emulator::EmulatorArgs,
    keystore::KeystoreArgs, purge::PurgeArgs, uninstall::UninstallArgs,
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
    #[command(visible_alias = "u")]
    Uninstall(UninstallArgs),
    /// Clear app data and restart on connected devices
    #[command(visible_alias = "c")]
    Clear(ClearArgs),
    /// Purge build artifacts and caches
    #[command(visible_alias = "p")]
    Purge(PurgeArgs),
    /// Generate an Android signing keystore
    #[command(visible_alias = "k")]
    Keystore(KeystoreArgs),
    /// Manage Android AVD emulators (e.g. config tweaks)
    #[command(visible_alias = "e")]
    Emulator(EmulatorArgs),
    /// Check development environment
    #[command(visible_alias = "d")]
    Doctor,
    /// Generate shell completion script (bash, zsh, fish, powershell, elvish)
    #[command(visible_alias = "s")]
    Completions(CompletionsArgs),
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
        Some(Commands::Completions(ref args)) => commands::completions::run::<Cli>(args),
    };

    std::process::exit(exit_code);
}
