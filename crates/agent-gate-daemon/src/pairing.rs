//! Pairing token for network access.
//!
//! The Unix socket needs no authentication: reaching it already means local
//! filesystem access as this user. A TCP listener is a different matter - it
//! exposes an API that can approve dangerous commands to anything that can
//! route to this machine - so every network request must carry a bearer token.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

/// Length of the raw token before hex encoding.
const TOKEN_BYTES: usize = 32;

/// Loads the pairing token, generating one on first use.
///
/// The file is created 0600. A token that leaks is equivalent to handing over
/// approval authority, so it is never logged in full.
pub fn load_or_create(path: &Path) -> Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    let token = generate()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &token)
        .with_context(|| format!("writing pairing token to {}", path.display()))?;
    restrict_permissions(path)?;
    Ok(token)
}

fn generate() -> Result<String> {
    let mut buf = [0u8; TOKEN_BYTES];
    let mut urandom =
        std::fs::File::open("/dev/urandom").context("opening /dev/urandom for token generation")?;
    urandom.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

/// Compares tokens without leaking their contents through timing.
pub fn matches(expected: &str, presented: &str) -> bool {
    if expected.len() != presented.len() {
        return false;
    }
    expected
        .bytes()
        .zip(presented.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// A short prefix safe to show in logs and pairing UI.
pub fn fingerprint(token: &str) -> String {
    format!("{}...", &token[..token.len().min(8)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_long_and_unique() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert_eq!(a.len(), TOKEN_BYTES * 2);
        assert_ne!(a, b, "tokens must not repeat");
    }

    #[test]
    fn token_is_persisted_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        let first = load_or_create(&path).unwrap();
        let second = load_or_create(&path).unwrap();
        assert_eq!(first, second, "a restart must not invalidate paired devices");
    }

    #[test]
    fn token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        load_or_create(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "token must not be group/world readable");
    }

    #[test]
    fn comparison_rejects_wrong_and_truncated_tokens() {
        assert!(matches("abc123", "abc123"));
        assert!(!matches("abc123", "abc124"));
        assert!(!matches("abc123", "abc"));
        assert!(!matches("abc123", ""));
    }
}
