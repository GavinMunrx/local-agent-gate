use crate::paths;
use agent_gate_daemon::AuditStore;

pub fn run(limit: i64) -> anyhow::Result<()> {
    let db_path = paths::db_path();
    if !db_path.exists() {
        println!("No audit events yet (no daemon has run).");
        return Ok(());
    }

    let store = AuditStore::open(&db_path)?;
    let events = store.recent(limit)?;

    if events.is_empty() {
        println!("No audit events yet.");
        return Ok(());
    }

    println!(
        "{:<20} {:<8} {:<14} {:<40} REASON",
        "TIME", "RISK", "DECISION", "COMMAND"
    );
    for event in events {
        let command = if event.command.len() > 38 {
            format!("{}...", &event.command[..35])
        } else {
            event.command.clone()
        };
        println!(
            "{:<20} {:<8} {:<14} {:<40} {}",
            event.timestamp.format("%Y-%m-%d %H:%M:%S"),
            event.risk_level.to_string(),
            event.decision.to_string(),
            command,
            event.reason,
        );
    }

    Ok(())
}
