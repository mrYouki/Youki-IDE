mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "youki", version, about = "Youki Shell plugin build tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the plugin project in the current directory (or --path)
    Build(commands::build::BuildArgs),

    /// Manage the Android SDK/NDK components used for building
    Sdk {
        #[command(subcommand)]
        action: commands::sdk::SdkAction,
    },

    /// Check that all required tools are installed and reachable
    Doctor(commands::doctor::DoctorArgs),

    /// Scaffold a new plugin project
    New(commands::new::NewArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Build(args) => commands::build::run(args),
        Commands::Sdk { action } => commands::sdk::run(action),
        Commands::Doctor(args) => commands::doctor::run(args),
        Commands::New(args) => commands::new::run(args),
    }
}
