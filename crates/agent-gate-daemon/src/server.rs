use crate::approval::prompt_terminal;
use crate::audit_store::AuditStore;
use agent_gate_policy::{
    classify, ActionInfo, AgentInfo, ApprovalRequest, AuditEvent, Decision, PolicyConfig,
    PolicyDecision, ProjectInfo,
};
use axum::{extract::State, routing::get, routing::post, Json, Router};
use chrono::{Duration, Utc};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tower::Service;

const REQUEST_TIMEOUT_SECONDS: i64 = 300;

pub struct AppState {
    pub audit: AuditStore,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitRequest {
    pub agent: AgentInfo,
    pub project_path: String,
    pub git_remote: Option<String>,
    pub git_branch: Option<String>,
    pub command: String,
    pub argv: Vec<String>,
    pub working_directory: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitResponse {
    pub request_id: String,
    pub decision: String,
    pub reason: String,
    pub risk_level: String,
    pub risk_reasons: Vec<String>,
}

async fn health() -> &'static str {
    "ok"
}

async fn approve(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SubmitRequest>,
) -> Json<SubmitResponse> {
    let now = Utc::now();
    let project_name = PathBuf::from(&payload.project_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| payload.project_path.clone());

    let risk = classify(&payload.command, payload.git_branch.as_deref());
    let policy_config = PolicyConfig::load_from_dir(&PathBuf::from(&payload.project_path))
        .unwrap_or_default();
    let policy_outcome = policy_config.evaluate(&payload.command, payload.git_branch.as_deref(), risk.level);

    let request = ApprovalRequest {
        id: agent_gate_policy::new_id("req"),
        created_at: now,
        expires_at: now + Duration::seconds(REQUEST_TIMEOUT_SECONDS),
        agent: payload.agent.clone(),
        project: ProjectInfo {
            path: payload.project_path.clone(),
            name: project_name,
            git_remote: payload.git_remote.clone(),
            git_branch: payload.git_branch.clone(),
        },
        action: ActionInfo {
            kind: "shell_command".to_string(),
            command: payload.command.clone(),
            argv: payload.argv.clone(),
            working_directory: payload.working_directory.clone(),
        },
        risk: risk.clone(),
        policy: policy_outcome.clone(),
    };

    let (decision, reason) = match policy_outcome.decision {
        PolicyDecision::Deny => (
            Decision::AutoBlocked,
            format!("Denied by policy ({})", describe_match(&policy_outcome.matched_rule_ids)),
        ),
        PolicyDecision::Allow => (
            Decision::AutoAllowed,
            format!("Allowed by policy ({})", describe_match(&policy_outcome.matched_rule_ids)),
        ),
        PolicyDecision::Ask => {
            let decision = prompt_terminal(request.clone()).await;
            let reason = match decision {
                Decision::AllowOnce => "Approved from terminal".to_string(),
                _ => "Denied from terminal".to_string(),
            };
            (decision, reason)
        }
    };

    let decided_at = Utc::now();
    let event = AuditEvent {
        id: agent_gate_policy::new_id("evt"),
        request_id: request.id.clone(),
        timestamp: decided_at,
        agent_id: request.agent.id.clone(),
        project_path: request.project.path.clone(),
        command: request.action.command.clone(),
        risk_level: request.risk.level,
        decision,
        reason: reason.clone(),
        duration_ms: (decided_at - now).num_milliseconds(),
    };
    if let Err(err) = state.audit.insert(&event) {
        eprintln!("failed to persist audit event: {err:#}");
    }

    Json(SubmitResponse {
        request_id: request.id,
        decision: decision.to_string(),
        reason,
        risk_level: risk.level.to_string(),
        risk_reasons: risk.reasons,
    })
}

fn describe_match(rule_ids: &[String]) -> String {
    if rule_ids.is_empty() {
        "risk-based default".to_string()
    } else {
        rule_ids.join(", ")
    }
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/approve", post(approve))
        .with_state(state)
}

pub async fn serve_unix(socket_path: &std::path::Path, app: Router) -> anyhow::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(socket_path)?;
    println!("Local Agent Gate daemon listening on {}", socket_path.display());

    loop {
        let (stream, _addr) = listener.accept().await?;
        let tower_service = app.clone();
        tokio::spawn(async move {
            let socket = TokioIo::new(stream);
            let hyper_service = hyper::service::service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                let mut svc = tower_service.clone();
                async move { svc.call(request).await }
            });
            if let Err(err) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(socket, hyper_service)
                .await
            {
                eprintln!("connection error: {err:#}");
            }
        });
    }
}
