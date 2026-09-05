use crate::paths;
use anyhow::Result;

/// Prints what an approval surface needs to connect over the network.
///
/// The token is approval authority in a string, so it is only ever shown on
/// the machine that owns it, and never written to the audit log.
pub fn run(port: u16, show_token: bool) -> Result<()> {
    let token = agent_gate_daemon::pairing::load_or_create(&paths::token_path())?;
    let addresses = local_addresses();

    println!("Pairing details for Local Agent Gate");
    println!();
    if addresses.is_empty() {
        println!("  no non-loopback address found; is this machine on a network?");
    } else {
        for address in &addresses {
            println!("  http://{address}:{port}");
        }
    }
    println!();
    if show_token {
        println!("  token: {token}");
        println!();
        // A phone cannot set a header when it opens a link, so the pairing URL
        // carries the token in the query. The page stores it and strips it from
        // the address bar, but the link itself is as sensitive as the token.
        println!("  Open on the phone:");
        for address in &addresses {
            println!("    http://{address}:{port}/?token={token}");
        }
        if !addresses.is_empty() {
            println!();
        }
        println!("  Or open http://<address>:{port} and paste the token there.");
        println!("  Other clients send it as: Authorization: Bearer <token>");
    } else {
        println!(
            "  token: {} (re-run with --show-token to reveal)",
            agent_gate_daemon::pairing::fingerprint(&token)
        );
    }
    println!();
    println!("  Anyone with this token can approve commands. Treat it like a password,");
    println!("  and prefer a user-owned tunnel (Tailscale, WireGuard) over an open LAN.");
    println!();
    println!("  The daemon only listens on the network when started with --lan.");
    Ok(())
}

/// Best-effort local IPv4 addresses, read from `ifconfig` to avoid pulling in
/// a networking crate for one diagnostic.
pub fn local_addresses() -> Vec<String> {
    let output = match std::process::Command::new("ifconfig").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("inet ")?;
            let addr = rest.split_whitespace().next()?;
            if addr.starts_with("127.") || addr.contains(':') {
                return None;
            }
            Some(addr.to_string())
        })
        .collect()
}
