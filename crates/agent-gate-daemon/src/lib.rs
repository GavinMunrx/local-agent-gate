pub mod audit_store;
pub mod pending;
pub mod server;

pub use audit_store::AuditStore;
pub use pending::PendingStore;

use std::path::PathBuf;
use std::sync::Arc;

pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
}

pub async fn run(config: DaemonConfig) -> anyhow::Result<()> {
    let audit = AuditStore::open(&config.db_path)?;
    let pending = PendingStore::new();
    let state = Arc::new(server::AppState { audit, pending });
    let app = server::build_router(state);
    server::serve_unix(&config.socket_path, app).await
}
