use agent_gate_policy::{ApprovalRequest, Decision};
use std::io::{self, Write};

pub async fn prompt_terminal(request: ApprovalRequest) -> Decision {
    tokio::task::spawn_blocking(move || prompt_terminal_blocking(&request))
        .await
        .unwrap_or(Decision::DenyOnce)
}

fn prompt_terminal_blocking(request: &ApprovalRequest) -> Decision {
    println!("\n=== Local Agent Gate: Approval Requested ===");
    println!("Agent:        {}", request.agent.name);
    println!("Project:      {}", request.project.name);
    println!("Directory:    {}", request.action.working_directory);
    println!("Command:      {}", request.action.command);
    println!("Risk:         {}", request.risk.level);
    for reason in &request.risk.reasons {
        println!("  Reason:     {reason}");
    }
    if !request.policy.matched_rule_ids.is_empty() {
        println!("Matched rule: {}", request.policy.matched_rule_ids.join(", "));
    }
    println!("Expires at:   {}", request.expires_at.to_rfc3339());
    print!("Allow this command? [y/N]: ");
    let _ = io::stdout().flush();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return Decision::DenyOnce;
    }

    match input.trim().to_lowercase().as_str() {
        "y" | "yes" => Decision::AllowOnce,
        _ => Decision::DenyOnce,
    }
}
