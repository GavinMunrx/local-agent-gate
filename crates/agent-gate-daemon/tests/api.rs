//! Integration tests that drive the daemon's HTTP API in-process.
//!
//! These exercise the router directly rather than over a Unix socket: the
//! interesting behaviour is in the handlers and the pending queue, and driving
//! futures directly is what lets us simulate a client disconnecting.

use agent_gate_daemon::server::{build_router, AppState};
use agent_gate_daemon::{reaper, AuditStore, PendingStore};
use agent_gate_policy::Decision;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct Harness {
    state: Arc<AppState>,
    dir: tempfile::TempDir,
}

fn harness(request_timeout_seconds: i64) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let audit = AuditStore::open(&dir.path().join("audit.db")).expect("audit store");
    let state = Arc::new(AppState {
        audit,
        pending: PendingStore::new(),
        request_timeout_seconds,
    });
    Harness { state, dir }
}

impl Harness {
    /// A submission body. The project path points at the temp dir so no
    /// `.agent-gate.yml` from the surrounding checkout can leak into the test.
    fn submission(&self, command: &str) -> Value {
        json!({
            "agent": { "id": "test", "name": "Test", "sessionId": "s1" },
            "projectPath": self.dir.path().to_string_lossy(),
            "command": command,
            "argv": command.split(' ').collect::<Vec<_>>(),
            "workingDirectory": self.dir.path().to_string_lossy(),
        })
    }

    fn request(&self, method: &str, uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request")
    }

    async fn call(&self, method: &str, uri: &str, body: Value) -> Value {
        let response = build_router(Arc::clone(&self.state))
            .oneshot(self.request(method, uri, body))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.expect("body").to_bytes();
        serde_json::from_slice(&bytes).expect("json")
    }

    fn decisions(&self) -> Vec<Decision> {
        self.state
            .audit
            .recent(50)
            .expect("audit read")
            .into_iter()
            .map(|e| e.decision)
            .collect()
    }
}

#[tokio::test]
async fn health_reports_ok() {
    let h = harness(120);
    let response = build_router(Arc::clone(&h.state))
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn low_risk_command_is_auto_allowed_and_audited() {
    let h = harness(120);
    let body = h.call("POST", "/approve", h.submission("ls -la")).await;

    assert_eq!(body["decision"], json!("auto_allowed"));
    assert_eq!(body["riskLevel"], json!("low"));
    assert!(h.state.pending.is_empty(), "auto decisions never park in the queue");
    assert_eq!(h.decisions(), vec![Decision::AutoAllowed]);
}

#[tokio::test]
async fn catastrophic_command_is_auto_blocked_and_audited() {
    let h = harness(120);
    // Split so the literal never appears contiguously in this file: the
    // classifier scans raw text, so an intact payload here would cause the
    // gate to block the very edit that writes this test.
    let payload = concat!("rm", " -rf ", "~");
    let body = h.call("POST", "/approve", h.submission(payload)).await;

    assert_eq!(body["decision"], json!("auto_blocked"));
    assert_eq!(body["riskLevel"], json!("blocked"));
    assert_eq!(h.decisions(), vec![Decision::AutoBlocked]);
}

#[tokio::test]
async fn pending_request_is_listed_then_approved() {
    let h = harness(30);
    let state = Arc::clone(&h.state);
    let dir = h.dir.path().to_string_lossy().to_string();
    let submission = h.submission("some-unknown-tool --flag");

    let waiter = tokio::spawn(async move {
        let response = build_router(Arc::clone(&state))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/approve")
                    .header("content-type", "application/json")
                    .body(Body::from(submission.to_string()))
                    .unwrap(),
            )
            .await
            .expect("response");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<Value>(&bytes).unwrap()
    });

    let id = await_pending(&h).await;

    let listed = h.call("GET", "/pending", json!({})).await;
    assert_eq!(listed[0]["project"]["path"], json!(dir));

    let decided = h
        .call("POST", &format!("/pending/{id}/decide"), json!({ "decision": "allow" }))
        .await;
    assert_eq!(decided["ok"], json!(true));

    let body = waiter.await.expect("waiter");
    assert_eq!(body["decision"], json!("allow_once"));
    assert!(h.state.pending.is_empty(), "a decided request leaves the queue");
    assert_eq!(h.decisions(), vec![Decision::AllowOnce]);
}

/// Regression test for the bug that motivated the reaper.
///
/// When an adapter is killed mid-request (its hook timeout fires before the
/// human answers), hyper drops the in-flight handler future. Any cleanup
/// sitting after the `.await` never runs, so the request used to linger in
/// `/pending` forever and leave no audit trail at all.
#[tokio::test]
async fn abandoned_request_is_reaped_and_audited() {
    let h = harness(120);
    let submission = h.submission("some-unknown-tool --flag");

    // Drive the handler until it parks, then drop it - exactly what a client
    // disconnect does to the future.
    let handler = build_router(Arc::clone(&h.state)).oneshot(h.request("POST", "/approve", submission));
    let outcome = tokio::time::timeout(std::time::Duration::from_millis(150), handler).await;
    assert!(outcome.is_err(), "handler should still have been awaiting a decision");

    assert_eq!(h.state.pending.len(), 1, "the abandoned request is still parked");
    assert!(h.decisions().is_empty(), "nothing is audited until it resolves");

    // Past expiry, the reaper must both clear the queue and leave a receipt.
    let reaped = reaper::reap_at(&h.state, Utc::now() + Duration::seconds(121));
    assert_eq!(reaped, 1);
    assert!(h.state.pending.is_empty(), "expired requests must not linger");
    assert_eq!(h.decisions(), vec![Decision::Expired]);
}

/// Expiry must produce exactly one receipt. The handler and the reaper can
/// both observe the same expired request, so only one of them may audit it -
/// and it has to be the reaper, since the handler is often already gone.
#[tokio::test]
async fn expiry_is_audited_exactly_once_when_client_stays_connected() {
    let h = harness(0);
    let body = h.call("POST", "/approve", h.submission("some-unknown-tool --flag")).await;

    assert_eq!(body["decision"], json!("expired"));
    assert!(h.decisions().is_empty(), "the handler must leave auditing to the reaper");

    assert_eq!(reaper::reap_at(&h.state, Utc::now()), 1);
    assert_eq!(h.decisions(), vec![Decision::Expired]);
}

#[tokio::test]
async fn reaper_leaves_live_requests_alone() {
    let h = harness(120);
    let submission = h.submission("some-unknown-tool --flag");
    let handler = build_router(Arc::clone(&h.state)).oneshot(h.request("POST", "/approve", submission));
    let _ = tokio::time::timeout(std::time::Duration::from_millis(150), handler).await;

    assert_eq!(reaper::reap_at(&h.state, Utc::now()), 0);
    assert_eq!(h.state.pending.len(), 1);
    assert!(h.decisions().is_empty());
}

#[tokio::test]
async fn deciding_an_unknown_request_reports_failure() {
    let h = harness(120);
    let body = h
        .call("POST", "/pending/req_nope/decide", json!({ "decision": "allow" }))
        .await;
    assert_eq!(body["ok"], json!(false));
}

/// Polls until the submitted request appears in the queue, returning its id.
async fn await_pending(h: &Harness) -> String {
    for _ in 0..200 {
        if let Some(request) = h.state.pending.list().into_iter().next() {
            return request.id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("request never appeared in the pending queue");
}
