use crate::{client, gitinfo, paths};
use serde_json::json;

pub async fn run(argv: Vec<String>) -> anyhow::Result<i32> {
    if argv.is_empty() {
        anyhow::bail!("no command given; usage: agent-gate run -- <command> [args...]");
    }

    let cwd = std::env::current_dir()?;
    let git = gitinfo::discover(&cwd);
    let command = shell_words::join(&argv);

    let payload = json!({
        "agent": {
            "id": "generic-shell",
            "name": "Generic Shell Wrapper",
        },
        "projectPath": git.project_path.to_string_lossy(),
        "gitRemote": git.remote,
        "gitBranch": git.branch,
        "command": command,
        "argv": argv,
        "workingDirectory": cwd.to_string_lossy(),
    });

    let response = client::post_json(&paths::socket_path(), "/approve", &payload).await?;
    let decision = response
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or("deny_once")
        .to_string();
    let reason = response
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let risk_level = response
        .get("riskLevel")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let allowed = matches!(decision.as_str(), "allow_once" | "allow_similar" | "auto_allowed");

    if !allowed {
        eprintln!("Local Agent Gate: blocked `{command}` (risk: {risk_level}) — {reason}");
        return Ok(1);
    }

    if decision == "auto_allowed" {
        eprintln!("Local Agent Gate: auto-allowed `{command}` (risk: {risk_level})");
    }

    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .status()?;
    Ok(status.code().unwrap_or(1))
}
