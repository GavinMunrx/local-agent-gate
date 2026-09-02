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
    /// Interactively decide on pending approvals from the terminal.
    Approve,
    /// Show recent audit events.
    Audit {
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Agent adapter hooks (invoked by the agent itself, not by a human).
    Hook {
        #[command(subcommand)]
        agent: HookAgent,
    },
}

#[derive(Subcommand)]
enum HookAgent {
    /// Claude Code `PreToolUse` hook: reads hook JSON on stdin, writes a
    /// `hookSpecificOutput` decision to stdout.
    ClaudeCode,
    /// Codex CLI `PreToolUse` hook, same contract as Claude Code's except
    /// that Codex has no "defer" decision.
    Codex,
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
        Command::Approve => {
            commands::approve::run().await?;
        }
        Command::Audit { limit } => {
            commands::audit::run(limit)?;
        }
        Command::Hook { agent } => {
            let code = match agent {
                HookAgent::ClaudeCode => {
                    commands::hook::run(commands::hook::Adapter::ClaudeCode).await?
                }
                HookAgent::Codex => commands::hook::run(commands::hook::Adapter::Codex).await?,
            };
            std::process::exit(code);
        }
    }

    Ok(())
}
