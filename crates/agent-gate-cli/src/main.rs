mod client;
mod commands;
mod gitinfo;
mod paths;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent-gate", about = "Local command firewall for AI coding agents")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the local approval daemon in the foreground.
    Daemon,
    /// Run a command through the approval gate.
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Show recent audit events.
    Audit {
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Daemon => {
            commands::daemon::run().await?;
        }
        Command::Run { command } => {
            let code = commands::run::run(command).await?;
            std::process::exit(code);
        }
        Command::Audit { limit } => {
            commands::audit::run(limit)?;
        }
    }

    Ok(())
}
