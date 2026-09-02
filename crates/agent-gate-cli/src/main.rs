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
    Daemon {
        /// Also listen on the network so phones and watches can approve.
        #[arg(long)]
        lan: bool,
        #[arg(long, default_value_t = agent_gate_daemon::DEFAULT_LAN_PORT)]
        port: u16,
    },
    /// Show what an approval surface needs to connect over the network.
    Pair {
        #[arg(long, default_value_t = agent_gate_daemon::DEFAULT_LAN_PORT)]
        port: u16,
        /// Print the token in full rather than a fingerprint.
        #[arg(long)]
        show_token: bool,
    },
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
    /// Install, remove, or inspect the agent hook wiring.
    Adapters {
        #[command(subcommand)]
        action: AdapterAction,
    },
    /// Agent adapter hooks (invoked by the agent itself, not by a human).
    Hook {
        #[command(subcommand)]
        agent: HookAgent,
    },
}

#[derive(Subcommand)]
enum AdapterAction {
    /// Show which agents are wired up, and where.
    List,
    /// Wire an agent's PreToolUse hook to this gate.
    Install {
        #[arg(value_enum)]
        agent: AgentName,
        /// Install into the user's global agent config rather than this project.
        #[arg(long)]
        global: bool,
        /// Hook timeout in seconds. Must exceed the daemon's request expiry.
        #[arg(long, default_value_t = 130)]
        timeout: u64,
    },
    /// Remove this gate's hook from an agent's config.
    Uninstall {
        #[arg(value_enum)]
        agent: AgentName,
        #[arg(long)]
        global: bool,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum AgentName {
    ClaudeCode,
    Codex,
    Cursor,
    GeminiCli,
    Antigravity,
}

impl AgentName {
    fn adapter(self) -> commands::hook::Adapter {
        use commands::hook::Adapter;
        match self {
            AgentName::ClaudeCode => Adapter::ClaudeCode,
            AgentName::Codex => Adapter::Codex,
            AgentName::Cursor => Adapter::Cursor,
            AgentName::GeminiCli => Adapter::GeminiCli,
            AgentName::Antigravity => Adapter::Antigravity,
        }
    }
}

#[derive(Subcommand)]
enum HookAgent {
    /// Claude Code `PreToolUse` hook: reads hook JSON on stdin, writes a
    /// `hookSpecificOutput` decision to stdout.
    ClaudeCode,
    /// Codex CLI `PreToolUse` hook, same contract as Claude Code's except
    /// that Codex has no "defer" decision.
    Codex,
    /// Cursor `beforeShellExecution` hook.
    Cursor,
    /// Gemini CLI `BeforeTool` hook for `run_shell_command`.
    GeminiCli,
    /// Antigravity `PreToolUse` hook for `run_command`.
    Antigravity,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Daemon { lan, port } => {
            commands::daemon::run(lan, port).await?;
        }
        Command::Pair { port, show_token } => {
            commands::pair::run(port, show_token)?;
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
        Command::Adapters { action } => {
            let project = std::env::current_dir()?;
            match action {
                AdapterAction::List => commands::adapters::list(&project).await?,
                AdapterAction::Install {
                    agent,
                    global,
                    timeout,
                } => {
                    let scope = if global {
                        commands::adapters::Scope::Global
                    } else {
                        commands::adapters::Scope::Project
                    };
                    let target =
                        commands::adapters::Target::resolve(agent.adapter(), scope, &project)?;
                    commands::adapters::install(&target, timeout)?;
                }
                AdapterAction::Uninstall { agent, global } => {
                    let scope = if global {
                        commands::adapters::Scope::Global
                    } else {
                        commands::adapters::Scope::Project
                    };
                    let target =
                        commands::adapters::Target::resolve(agent.adapter(), scope, &project)?;
                    commands::adapters::uninstall(&target)?;
                }
            }
        }
        Command::Hook { agent } => {
            let code = match agent {
                HookAgent::ClaudeCode => {
                    commands::hook::run(commands::hook::Adapter::ClaudeCode).await?
                }
                HookAgent::Codex => commands::hook::run(commands::hook::Adapter::Codex).await?,
                HookAgent::Cursor => commands::hook::run(commands::hook::Adapter::Cursor).await?,
                HookAgent::GeminiCli => {
                    commands::hook::run(commands::hook::Adapter::GeminiCli).await?
                }
                HookAgent::Antigravity => {
                    commands::hook::run(commands::hook::Adapter::Antigravity).await?
                }
            };
            std::process::exit(code);
        }
    }

    Ok(())
}
