use crate::paths;
use agent_gate_policy::{LearnedStore, PolicyDecision};
use anyhow::Result;

/// Shows rules learned from "allow similar" / "deny similar" decisions.
///
/// A learned rule quietly changes what runs without asking, so being able to
/// see and revoke the whole set matters as much as being able to create one.
pub fn list() -> Result<()> {
    let store = LearnedStore::load(&paths::learned_policy_path())?;
    if store.rules.is_empty() {
        println!("No learned rules.");
        println!();
        println!("They are created by deciding a pending request with");
        println!("`allow_similar` or `block_similar` from an approval surface.");
        return Ok(());
    }

    println!("{} learned rule(s):", store.rules.len());
    println!();
    for rule in &store.rules {
        let verb = match rule.decision {
            PolicyDecision::Allow => "ALLOW",
            PolicyDecision::Deny => "DENY ",
            PolicyDecision::Ask => "ASK  ",
        };
        println!("  {verb} {}", rule.matcher.describe());
        println!("        in {}", rule.project_path);
        println!(
            "        learned {} from `{}`",
            rule.created_at.format("%Y-%m-%d %H:%M"),
            rule.learned_from
        );
        println!("        id {}", rule.id);
        println!();
    }
    println!("Remove one with: agent-gate policy forget <id>");
    Ok(())
}

pub fn forget(id: &str) -> Result<()> {
    let path = paths::learned_policy_path();
    let mut store = LearnedStore::load(&path)?;
    if store.forget(id) {
        store.save(&path)?;
        println!("Forgot {id}.");
        println!("Effective immediately - the daemon re-reads learned rules per request.");
    } else {
        println!("No learned rule with id {id}.");
    }
    Ok(())
}

pub fn forget_all() -> Result<()> {
    let path = paths::learned_policy_path();
    let store = LearnedStore::load(&path)?;
    let count = store.rules.len();
    LearnedStore::default().save(&path)?;
    println!("Forgot {count} learned rule(s).");
    println!("Effective immediately - the daemon re-reads learned rules per request.");
    Ok(())
}
