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
        print!("Allow this command? [y/N]: ");
        io::stdout().flush().ok();

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let decision = match input.trim().to_lowercase().as_str() {
            "y" | "yes" => "allow_once",
            _ => "deny_once",
        };

        let decide_path = format!("/pending/{id}/decide");
        let result = client::post_json(&socket_path, &decide_path, &json!({ "decision": decision })).await?;
        let ok = result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if ok {
            println!("Recorded: {decision}");
        } else {
            println!("This request already expired or was decided elsewhere.");
        }
    }

    Ok(())
}
