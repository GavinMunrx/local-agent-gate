use crate::paths;
use std::net::SocketAddr;

pub async fn run(lan: bool, port: u16) -> anyhow::Result<()> {
    let mut config = agent_gate_daemon::DaemonConfig::new(
        paths::socket_path(),
        paths::db_path(),
        paths::token_path(),
    );
    if lan {
        // 0.0.0.0 so phones on the same network can reach it. The pairing
        // token is what actually gates access.
        config.lan_addr = Some(SocketAddr::from(([0, 0, 0, 0], port)));
    }
    agent_gate_daemon::run(config).await
}
