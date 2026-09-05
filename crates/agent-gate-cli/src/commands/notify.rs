use crate::paths;
use agent_gate_daemon::notify::NotifyConfig;
use anyhow::{Context, Result};
use std::io::Read;

/// Writes a starting config, with a topic nobody can guess.
///
/// The topic is the only thing standing between a stranger and your
/// notifications, so it is generated rather than chosen: a memorable topic on
/// a public server is a topic someone else can subscribe to.
pub fn setup(server: Option<String>) -> Result<()> {
    let path = paths::notify_config_path();
    if path.exists() {
        println!("{} already exists; edit it directly.", path.display());
        return Ok(());
    }

    let topic = format!("agent-gate-{}", random_hex(12)?);
    let server = server.unwrap_or_else(|| "https://ntfy.sh".to_string());
    let address = crate::commands::pair::local_addresses()
        .into_iter()
        .next()
        .unwrap_or_else(|| "YOUR-MAC-ADDRESS".to_string());
    let callback = format!("http://{address}:8787");

    let contents = format!(
        "# Push notifications for Local Agent Gate.
#
# Delete this file to turn notifications off again.
#
# The relay only ever learns that an approval is waiting, its risk tier, the
# agent and the project name. Decisions and the approval page go straight to
# this Mac at callback_base, never through the relay.

server: {server}

# Anyone who knows this can read your notifications. Treat it as a secret.
topic: {topic}

# How your phone reaches this daemon. A LAN address works at home; use a
# Tailscale name to answer from anywhere.
callback_base: {callback}

# view   - one button that opens the approval page (nothing secret on the wire)
# decide - adds Allow/Deny buttons, each a single-use capability for this one
#          request. Convenient, but those buttons pass through the relay.
actions: view

# Whether the command text may leave this machine. Off by default.
include_command: false

# Don't buzz below this tier: low, medium, high.
min_risk: medium
"
    );
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, contents)?;

    println!("Wrote {}", path.display());
    println!();
    println!("Next:");
    println!("  1. Install ntfy from the App Store (free), and subscribe to:");
    println!("       {topic}");
    println!("  2. Check callback_base is how your phone reaches this Mac.");
    println!("  3. Restart the daemon, then: agent-gate notify test");
    println!();
    println!("Your Watch mirrors the phone's notifications, so nothing extra is");
    println!("needed for the wrist.");
    Ok(())
}

pub fn status() -> Result<()> {
    let path = paths::notify_config_path();
    match NotifyConfig::load(&path)? {
        None => {
            println!("Notifications are off.");
            println!("No config at {}.", path.display());
            println!();
            println!("Turn them on with: agent-gate notify setup");
        }
        Some(config) => {
            println!("Notifications are on.");
            println!("  server:        {}", config.server);
            println!("  topic:         {}", config.topic);
            println!("  callback:      {}", config.callback_base);
            println!("  actions:       {:?}", config.actions);
            println!("  min risk:      {}", config.min_risk);
            println!(
                "  command text:  {}",
                if config.include_command { "included" } else { "withheld" }
            );
            println!();
            println!("The daemon reads this at startup; restart it after editing.");
        }
    }
    Ok(())
}

/// Publishes a test notification, so the wiring can be proven before an agent
/// depends on it.
pub async fn test() -> Result<()> {
    let path = paths::notify_config_path();
    let config = NotifyConfig::load(&path)?
        .context("notifications are off; run `agent-gate notify setup` first")?;

    let body = serde_json::json!({
        "topic": config.topic,
        "title": "Local Agent Gate",
        "message": "Test notification. If this reached your wrist, the wiring works.",
        "priority": 4,
        "tags": ["white_check_mark"],
        "actions": [{ "action": "view", "label": "Open", "url": format!("{}/", config.callback_base.trim_end_matches('/')) }],
    });

    let response = reqwest::Client::new()
        .post(config.server.trim_end_matches('/'))
        .json(&body)
        .send()
        .await
        .with_context(|| format!("posting to {}", config.server))?;

    if response.status().is_success() {
        println!("Sent to {}.", config.topic_url());
        println!("If nothing arrives, check you are subscribed to `{}`.", config.topic);
    } else {
        println!("Rejected: HTTP {}", response.status());
        println!("{}", response.text().await.unwrap_or_default());
    }
    Ok(())
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")
        .context("opening /dev/urandom")?
        .read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}
