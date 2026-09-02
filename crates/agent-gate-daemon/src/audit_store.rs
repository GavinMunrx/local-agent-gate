use agent_gate_policy::{AuditEvent, Decision, RiskLevel};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct AuditStore {
    conn: Mutex<Connection>,
}

impl AuditStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS audit_events (
                id TEXT PRIMARY KEY,
                request_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                project_path TEXT NOT NULL,
                command TEXT NOT NULL,
                risk_level TEXT NOT NULL,
                decision TEXT NOT NULL,
                reason TEXT NOT NULL,
                duration_ms INTEGER NOT NULL
            )",
            (),
        )?;
        Ok(AuditStore {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert(&self, event: &AuditEvent) -> anyhow::Result<()> {
        let conn = self.conn.lock().expect("audit db lock poisoned");
        conn.execute(
            "INSERT INTO audit_events
                (id, request_id, timestamp, agent_id, project_path, command, risk_level, decision, reason, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                event.id,
                event.request_id,
                event.timestamp.to_rfc3339(),
                event.agent_id,
                event.project_path,
                event.command,
                event.risk_level.to_string(),
                event.decision.to_string(),
                event.reason,
                event.duration_ms,
            ],
        )?;
        Ok(())
    }

    pub fn recent(&self, limit: i64) -> anyhow::Result<Vec<AuditEvent>> {
        let conn = self.conn.lock().expect("audit db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, request_id, timestamp, agent_id, project_path, command, risk_level, decision, reason, duration_ms
             FROM audit_events ORDER BY timestamp DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |row| {
            let timestamp_str: String = row.get(2)?;
            let risk_level_str: String = row.get(6)?;
            let decision_str: String = row.get(7)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                timestamp_str,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                risk_level_str,
                decision_str,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })?;

        let mut events = Vec::new();
        for row in rows {
            let (id, request_id, timestamp, agent_id, project_path, command, risk_level, decision, reason, duration_ms) = row?;
            events.push(AuditEvent {
                id,
                request_id,
                timestamp: chrono::DateTime::parse_from_rfc3339(&timestamp)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                agent_id,
                project_path,
                command,
                risk_level: parse_risk_level(&risk_level),
                decision: parse_decision(&decision),
                reason,
                duration_ms,
            });
        }
        Ok(events)
    }
}

fn parse_risk_level(s: &str) -> RiskLevel {
    match s {
        "low" => RiskLevel::Low,
        "medium" => RiskLevel::Medium,
        "high" => RiskLevel::High,
        _ => RiskLevel::Blocked,
    }
}

fn parse_decision(s: &str) -> Decision {
    match s {
        "allow_once" => Decision::AllowOnce,
        "deny_once" => Decision::DenyOnce,
        "allow_similar" => Decision::AllowSimilar,
        "block_similar" => Decision::BlockSimilar,
        "auto_allowed" => Decision::AutoAllowed,
        "auto_blocked" => Decision::AutoBlocked,
        "no_decision_yet" => Decision::NoDecisionYet,
        _ => Decision::Expired,
    }
}
