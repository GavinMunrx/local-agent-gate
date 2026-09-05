use crate::{client, paths};
use serde_json::json;
use std::io::{self, Write};

pub async fn run() -> anyhow::Result<()> {
    let socket_path = paths::socket_path();
    let response = client::get_json(&socket_path, "/pending").await?;
    let pending = response
        .as_array()
        .cloned()
        .unwrap_or_default();

    if pending.is_empty() {
        println!("No pending approvals.");
        return Ok(());
    }

    for request in pending {
        let id = request.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let agent_name = request
            .pointer("/agent/name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown agent");
        let project_name = request
            .pointer("/project/name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown project");
        let command = request
            .pointer("/action/command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let working_directory = request
            .pointer("/action/workingDirectory")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let risk_level = request
            .pointer("/risk/level")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let reasons: Vec<&str> = request
            .pointer("/risk/reasons")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let expires_at = request.get("expiresAt").and_then(|v| v.as_str()).unwrap_or("");

        println!("\n=== Local Agent Gate: Approval Requested ===");
        println!("Agent:        {agent_name}");
        println!("Project:      {project_name}");
        println!("Directory:    {working_directory}");
        println!("Command:      {command}");
        println!("Risk:         {risk_level}");
        for reason in reasons {
            println!("  Reason:     {reason}");
        }
        println!("Expires at:   {expires_at}");
        // Show the scope a "similar" answer would grant *before* asking for
        // it. A learned rule is the one answer here that changes what runs
        // later without asking again, so it must not be a blind tap. The
        // daemon derives the wording so every surface says the same thing.
        let scope = request
            .get("similarScope")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        println!("Similar:      {scope}");
        println!();
        println!("  [y] allow once          [s] allow similar from now on");
        println!("  [n] deny (default)      [b] block similar from now on");
        print!("Choice [y/n/s/b]: ");
        io::stdout().flush().ok();

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        // Anything unrecognised denies: the fail-closed default has to survive
        // a stray keystroke, and only "once" answers are reachable by accident.
        let decision = match input.trim().to_lowercase().as_str() {
            "y" | "yes" => "allow_once",
            "s" | "similar" => "allow_similar",
            "b" | "block" => "block_similar",
            _ => "deny_once",
        };

        let decide_path = format!("/pending/{id}/decide");
        let result = client::post_json(&socket_path, &decide_path, &json!({ "decision": decision })).await?;
        let ok = result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if ok {
            println!("Recorded: {decision}");
            if decision == "allow_similar" || decision == "block_similar" {
                println!("Learned:  {scope}");
                println!("          Review with `agent-gate policy list`.");
            }
        } else {
            println!("This request already expired or was decided elsewhere.");
        }
    }

    Ok(())
}
