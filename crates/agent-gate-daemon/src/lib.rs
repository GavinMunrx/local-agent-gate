pub mod approval;
pub mod audit_store;
pub mod server;

pub use audit_store::AuditStore;

use std::path::PathBuf;
use std::sync::Arc;

pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
}

pub async fn run(config: DaemonConfig) -> anyhow::Result<()> {
    let audit = AuditStore::open(&config.db_path)?;
    let state = Arc::new(server::AppState { audit });
    let app = server::build_router(state);
    server::serve_unix(&config.socket_path, app).await
}
