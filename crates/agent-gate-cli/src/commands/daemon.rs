use crate::paths;

pub async fn run() -> anyhow::Result<()> {
    let config = agent_gate_daemon::DaemonConfig::new(paths::socket_path(), paths::db_path());
    agent_gate_daemon::run(config).await
}
