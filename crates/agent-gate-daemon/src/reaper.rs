use crate::server::AppState;
use agent_gate_policy::{AuditEvent, Decision};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

pub const EXPIRY_REASON: &str = "No approval surface responded before expiry";

/// Sweeps expired pending requests once, recording an audit event for each.
///
/// Returns the number of requests reaped.
pub fn reap_once(state: &AppState) -> usize {
    reap_at(state, Utc::now())
}

/// [`reap_once`] with an explicit clock, so tests need not wait out an expiry.
pub fn reap_at(state: &AppState, now: DateTime<Utc>) -> usize {
    let expired = state.pending.reap_expired(now);
    for request in &expired {
        let event = AuditEvent {
            id: agent_gate_policy::new_id("evt"),
            request_id: request.id.clone(),
            timestamp: now,
            agent_id: request.agent.id.clone(),
            project_path: request.project.path.clone(),
            command: request.action.command.clone(),
            risk_level: request.risk.level,
            decision: Decision::Expired,
            reason: EXPIRY_REASON.to_string(),
            duration_ms: (now - request.created_at).num_milliseconds(),
        };
        if let Err(err) = state.audit.insert(&event) {
            eprintln!("failed to persist expiry audit event: {err:#}");
        }
    }
    expired.len()
}

/// Runs [`reap_once`] on an interval for the lifetime of the daemon.
pub fn spawn(state: Arc<AppState>, interval: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            reap_once(&state);
        }
    })
}
