use crate::{client, gitinfo, paths};
use serde::Deserialize;
use serde_json::json;
use std::io::Read;

#[derive(Deserialize)]
struct HookInput {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
}

/// Implements the Claude Code `PreToolUse` hook contract: reads the hook
/// payload from stdin, and for `Bash` tool calls forwards the command to the
/// Local Agent Gate daemon. Prints `hookSpecificOutput.permissionDecision`
/// JSON to stdout and always exits 0 (the hook itself never errors; an
/// unreachable daemon "defers" to Claude Code's normal permission flow
/// instead of blocking everything).
pub async fn claude_code() -> anyhow::Result<i32> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let parsed: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(0),
    };

    if parsed.tool_name != "Bash" {
        return Ok(0);
    }

    let command = parsed
        .tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if command.is_empty() {
        return Ok(0);
    }

    let cwd = parsed
        .cwd
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let git = gitinfo::discover(&cwd);
    let argv = shell_words::split(&command).unwrap_or_else(|_| vec![command.clone()]);

    let payload = json!({
        "agent": {
            "id": "claude-code",
            "name": "Claude Code",
            "sessionId": parsed.session_id,
        },
        "projectPath": git.project_path.to_string_lossy(),
        "gitRemote": git.remote,
        "gitBranch": git.branch,
        "command": command,
        "argv": argv,
        "workingDirectory": cwd.to_string_lossy(),
    });

    let (permission_decision, reason) =
        match client::post_json(&paths::socket_path(), "/approve", &payload).await {
            Ok(response) => {
                let decision = response
                    .get("decision")
                    .and_then(|v| v.as_str())
                    .unwrap_or("deny_once");
                let risk = response
                    .get("riskLevel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let daemon_reason = response.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                let allowed = matches!(decision, "allow_once" | "allow_similar" | "auto_allowed");
                let permission = if allowed { "allow" } else { "deny" };
                (permission, format!("Local Agent Gate ({risk} risk): {daemon_reason}"))
            }
            Err(_) => (
                "defer",
                "Local Agent Gate daemon not running; falling back to normal Claude Code permissions"
                    .to_string(),
            ),
        };

    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": permission_decision,
            "permissionDecisionReason": reason,
        }
    });
    println!("{output}");
    Ok(0)
}
