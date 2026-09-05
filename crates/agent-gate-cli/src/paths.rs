use std::path::PathBuf;

fn app_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("local-agent-gate")
}

pub fn socket_path() -> PathBuf {
    app_dir().join("agent-gate.sock")
}

pub fn db_path() -> PathBuf {
    app_dir().join("audit.db")
}

pub fn token_path() -> PathBuf {
    app_dir().join("pairing-token")
}

pub fn learned_policy_path() -> PathBuf {
    app_dir().join("learned-policy.yml")
}

pub fn notify_config_path() -> PathBuf {
    app_dir().join("notify.yml")
}
