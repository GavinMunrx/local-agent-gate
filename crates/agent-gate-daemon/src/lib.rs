pub mod audit_store;
pub mod pairing;
pub mod pending;
pub mod reaper;
pub mod server;

pub use audit_store::AuditStore;
pub use pending::PendingStore;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// How long a request waits for an approval surface before expiring.
///
/// This must stay below the timeout any adapter gives its hook process, so the
/// daemon's own expiry response arrives before the adapter is killed. See
/// `docs/status.md`.
pub const DEFAULT_REQUEST_TIMEOUT_SECONDS: i64 = 120;

/// How often expired requests are swept out of the pending queue.
pub const DEFAULT_REAP_INTERVAL_SECONDS: u64 = 5;

/// Default port for the optional network listener.
pub const DEFAULT_LAN_PORT: u16 = 8787;

pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    pub token_path: PathBuf,
    pub request_timeout_seconds: i64,
    pub reap_interval_seconds: u64,
    /// When set, also listen on this address for phones and watches. Off by
    /// default: exposing an approval API to the network is opt-in.
    pub lan_addr: Option<SocketAddr>,
}

impl DaemonConfig {
    pub fn new(socket_path: PathBuf, db_path: PathBuf, token_path: PathBuf) -> Self {
        DaemonConfig {
            socket_path,
            db_path,
            token_path,
            request_timeout_seconds: DEFAULT_REQUEST_TIMEOUT_SECONDS,
            reap_interval_seconds: DEFAULT_REAP_INTERVAL_SECONDS,
            lan_addr: None,
        }
    }
}

pub async fn run(config: DaemonConfig) -> anyhow::Result<()> {
    let audit = AuditStore::open(&config.db_path)?;
    let pending = PendingStore::new();
    let token = pairing::load_or_create(&config.token_path)?;
    let (changes, _) = tokio::sync::broadcast::channel(64);
    let state = Arc::new(server::AppState {
        audit,
        pending,
        request_timeout_seconds: config.request_timeout_seconds,
        token,
        changes,
    });

    reaper::spawn(
        Arc::clone(&state),
        Duration::from_secs(config.reap_interval_seconds),
    );

    if let Some(addr) = config.lan_addr {
        let network = server::build_network_router(Arc::clone(&state));
        tokio::spawn(async move {
            if let Err(err) = server::serve_tcp(addr, network).await {
                eprintln!("network listener stopped: {err:#}");
            }
        });
    }

    let app = server::build_router(Arc::clone(&state));
    server::serve_unix(&config.socket_path, app).await
}
