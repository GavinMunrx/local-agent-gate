use crate::audit_store::AuditStore;
use crate::pending::PendingStore;
use crate::pairing;
use agent_gate_policy::{
    classify, ActionInfo, AgentInfo, ApprovalRequest, AuditEvent, Decision, PolicyConfig,
    PolicyDecision, ProjectInfo,
};
use axum::extract::Path;
use axum::http::{header, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::{extract::State, routing::get, routing::post, Json, Router};
use std::convert::Infallible;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use chrono::{Duration, Utc};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tower::Service;

pub struct AppState {
    pub audit: AuditStore,
    pub pending: PendingStore,
    pub request_timeout_seconds: i64,
    /// Bearer token required on network listeners. The Unix socket is exempt:
    /// reaching it already implies local access as this user.
    pub token: String,
    /// Fires whenever the pending queue changes, so approval surfaces can be
    /// pushed to rather than poll.
    pub changes: broadcast::Sender<()>,
}

impl AppState {
    pub fn notify_change(&self) {
        // An error just means nobody is listening yet.
        let _ = self.changes.send(());
    }
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecideRequest {
    pub decision: String,
}

#[derive(Debug, Serialize)]
pub struct DecideResponse {
    pub ok: bool,
}

async fn health() -> &'static str {
    "ok"
}

async fn list_pending(State(state): State<Arc<AppState>>) -> Json<Vec<ApprovalRequest>> {
    Json(state.pending.list())
}

async fn decide_pending(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<DecideRequest>,
) -> Json<DecideResponse> {
    let decision = match payload.decision.as_str() {
        "allow" | "allow_once" => Decision::AllowOnce,
        _ => Decision::DenyOnce,
    };
    let ok = state.pending.decide(&id, decision);
    if ok {
        state.notify_change();
    }
    Json(DecideResponse { ok })
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
        expires_at: now + Duration::seconds(state.request_timeout_seconds),
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
            let rx = state.pending.insert(request.clone());
            state.notify_change();
            let wait = tokio::time::timeout(
                std::time::Duration::from_secs(state.request_timeout_seconds.max(0) as u64),
                rx,
            )
            .await;
            match wait {
                Ok(Ok(decision)) => {
                    let reason = match decision {
                        Decision::AllowOnce => "Approved by an approval surface".to_string(),
                        _ => "Denied by an approval surface".to_string(),
                    };
                    (decision, reason)
                }
                // Expiry is the reaper's job alone. Cleaning up here too would
                // race it into a duplicate audit event, and would not run at all
                // in the common case: a disconnected client's handler is dropped
                // mid-await and never reaches this arm.
                _ => (Decision::Expired, crate::reaper::EXPIRY_REASON.to_string()),
            }
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
    if decision != Decision::Expired {
        if let Err(err) = state.audit.insert(&event) {
            eprintln!("failed to persist audit event: {err:#}");
        }
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

/// Streams the pending queue to an approval surface. Emits immediately on
/// connect so a client starts with the current state rather than waiting for
/// the next change, then on every change after that.
async fn events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.changes.subscribe();
    let initial = tokio_stream::once(());
    let changes = BroadcastStream::new(receiver).filter_map(|r| r.ok());
    let stream = initial.chain(changes).map(move |()| {
        let payload = serde_json::to_string(&state.pending.list()).unwrap_or_else(|_| "[]".into());
        Ok(Event::default().event("pending").data(payload))
    });
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

/// Rejects network requests that do not present the pairing token.
async fn require_token(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !pairing::matches(&state.token, presented) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "pairing token required" })),
        )
            .into_response();
    }
    next.run(request).await
}

fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/approve", post(approve))
        .route("/pending", get(list_pending))
        .route("/pending/:id/decide", post(decide_pending))
        .route("/events", get(events))
        .with_state(state)
}

/// Router for the Unix socket: no authentication, since filesystem
/// permissions already gate it.
pub fn build_router(state: Arc<AppState>) -> Router {
    routes(Arc::clone(&state)).with_state(state)
}

/// Router for a network listener: identical, but every request must carry the
/// pairing token.
pub fn build_network_router(state: Arc<AppState>) -> Router {
    routes(Arc::clone(&state))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_token,
        ))
        .with_state(state)
}

/// Serves the network router on a TCP listener, for phones and watches on the
/// local network or a user-owned tunnel.
pub async fn serve_tcp(addr: std::net::SocketAddr, app: Router) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Local Agent Gate daemon listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
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
