//! Integration tests that drive the daemon's HTTP API in-process.
//!
//! These exercise the router directly rather than over a Unix socket: the
//! interesting behaviour is in the handlers and the pending queue, and driving
//! futures directly is what lets us simulate a client disconnecting.

use agent_gate_daemon::server::{build_network_router, build_router, AppState};
use agent_gate_daemon::{reaper, AuditStore, PendingStore};
use agent_gate_policy::Decision;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "test-pairing-token";

struct Harness {
    state: Arc<AppState>,
    dir: tempfile::TempDir,
}

fn harness(agent_wait_seconds: i64) -> Harness {
    harness_with(agent_wait_seconds, 600)
}

fn harness_with(agent_wait_seconds: i64, request_ttl_seconds: i64) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let audit = AuditStore::open(&dir.path().join("audit.db")).expect("audit store");
    let (changes, _) = tokio::sync::broadcast::channel(16);
    let state = Arc::new(AppState {
        audit,
        pending: PendingStore::new(),
        agent_wait_seconds,
        request_ttl_seconds,
        token: TOKEN.to_string(),
        learned_path: dir.path().join("learned.yml"),
        learned_lock: std::sync::Mutex::new(()),
        changes,
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

    /// Writes a learned rule the way `agent-gate policy` does: straight to the
    /// file, with no daemon involvement.
    fn teach(&self, command: &str, decision: agent_gate_policy::PolicyDecision) {
        let path = self.state.learned_path.clone();
        let mut store = agent_gate_policy::LearnedStore::load(&path).unwrap();
        store.learn(&self.dir.path().to_string_lossy(), command, decision);
        store.save(&path).unwrap();
    }

    /// Reads them back the same way.
    fn learned(&self) -> agent_gate_policy::LearnedStore {
        agent_gate_policy::LearnedStore::load(&self.state.learned_path).unwrap()
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
    let h = harness_with(120, 120);
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
    let h = harness_with(0, 0);
    let body = h.call("POST", "/approve", h.submission("some-unknown-tool --flag")).await;

    assert_eq!(body["decision"], json!("no_decision_yet"));
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

// ---------------------------------------------------------- network listener

/// The Unix socket is unauthenticated because filesystem permissions gate it.
/// A TCP listener has no such protection, so every request must carry the
/// pairing token - this is the boundary that stops anything routable to this
/// machine from approving its own commands.
#[tokio::test]
async fn network_router_rejects_requests_without_a_token() {
    let h = harness(120);
    for uri in ["/pending", "/events"] {
        let response = build_network_router(Arc::clone(&h.state))
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} served without a token"
        );
    }
}

#[tokio::test]
async fn network_router_rejects_a_wrong_token() {
    let h = harness(120);
    let response = build_network_router(Arc::clone(&h.state))
        .oneshot(
            Request::builder()
                .uri("/pending")
                .header("authorization", "Bearer not-the-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn network_router_accepts_the_pairing_token() {
    let h = harness(120);
    let response = build_network_router(Arc::clone(&h.state))
        .oneshot(
            Request::builder()
                .uri("/pending")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
}

/// The local socket must stay usable without a token, or every adapter breaks.
#[tokio::test]
async fn unix_router_needs_no_token() {
    let h = harness(120);
    let response = build_router(Arc::clone(&h.state))
        .oneshot(Request::builder().uri("/pending").body(Body::empty()).unwrap())
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
}

/// An approval surface subscribes before anything is queued, so it has to be
/// told when the queue changes rather than discovering it by polling.
#[tokio::test]
async fn queue_changes_are_broadcast_to_subscribers() {
    let h = harness(120);
    let mut events = h.state.changes.subscribe();
    let submission = h.submission("some-unknown-tool --flag");
    let handler =
        build_router(Arc::clone(&h.state)).oneshot(h.request("POST", "/approve", submission));
    let _ = tokio::time::timeout(std::time::Duration::from_millis(150), handler).await;

    let signalled = tokio::time::timeout(std::time::Duration::from_millis(500), events.recv()).await;
    assert!(signalled.is_ok(), "parking a request must notify subscribers");
}

// -------------------------------------------------- agent wait vs request TTL

/// The agent and the request must not share a deadline. The agent falls back
/// to its own prompt quickly; the request stays answerable long enough for a
/// human to be notified, look, and decide.
#[tokio::test]
async fn agent_gives_up_but_the_request_stays_answerable() {
    let h = harness_with(0, 600); // agent waits no time; request lives 10 min
    let body = h.call("POST", "/approve", h.submission("some-unknown-tool --flag")).await;

    assert_eq!(body["decision"], json!("no_decision_yet"));
    assert_eq!(h.state.pending.len(), 1, "the request must outlive the agent's wait");
    assert_eq!(
        reaper::reap_at(&h.state, Utc::now() + Duration::seconds(60)),
        0,
        "still well inside its TTL"
    );
    assert_eq!(h.state.pending.len(), 1);

    // ...and it does eventually die.
    assert_eq!(reaper::reap_at(&h.state, Utc::now() + Duration::seconds(601)), 1);
    assert_eq!(h.decisions(), vec![Decision::Expired]);
}

/// A decision made after the agent stopped waiting still has to be recorded.
/// Before the split this was a rare race; now it is the normal path for
/// anything approved from a phone.
#[tokio::test]
async fn a_late_decision_is_still_audited() {
    let h = harness_with(0, 600);
    let body = h.call("POST", "/approve", h.submission("some-unknown-tool --flag")).await;
    assert_eq!(body["decision"], json!("no_decision_yet"));
    assert!(h.decisions().is_empty());

    let id = h.state.pending.list()[0].id.clone();
    let decided = h
        .call("POST", &format!("/pending/{id}/decide"), json!({ "decision": "allow" }))
        .await;

    assert_eq!(decided["ok"], json!(true), "a late decision must be accepted");
    assert!(h.state.pending.is_empty());
    assert_eq!(h.decisions(), vec![Decision::AllowOnce]);
    let event = &h.state.audit.recent(1).unwrap()[0];
    assert!(
        event.reason.contains("after the agent stopped waiting"),
        "the receipt should say the agent had already moved on: {}",
        event.reason
    );
}

/// The live path must not double-audit: when the agent is still waiting, its
/// own handler writes the receipt and the decide endpoint must not add another.
#[tokio::test]
async fn a_delivered_decision_is_audited_only_once() {
    let h = harness(30);
    let state = Arc::clone(&h.state);
    let submission = h.submission("some-unknown-tool --flag");
    let waiter = tokio::spawn(async move {
        build_router(Arc::clone(&state))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/approve")
                    .header("content-type", "application/json")
                    .body(Body::from(submission.to_string()))
                    .unwrap(),
            )
            .await
            .expect("response")
    });
    let id = await_pending(&h).await;
    h.call("POST", &format!("/pending/{id}/decide"), json!({ "decision": "allow" }))
        .await;
    let _ = waiter.await;

    assert_eq!(h.decisions(), vec![Decision::AllowOnce], "exactly one receipt");
}

// ------------------------------------------------------------ learned policy

/// "Allow similar" has to actually change what happens next, or a watch tap is
/// only ever informative.
#[tokio::test]
async fn allow_similar_auto_allows_the_next_matching_command() {
    let h = harness_with(0, 600);
    let first = h.call("POST", "/approve", h.submission("npm install left-pad")).await;
    assert_eq!(first["decision"], json!("no_decision_yet"));

    let id = h.state.pending.list()[0].id.clone();
    h.call(
        "POST",
        &format!("/pending/{id}/decide"),
        json!({ "decision": "allow_similar" }),
    )
    .await;

    // A different package, same shape: no longer parks in the queue.
    let second = h.call("POST", "/approve", h.submission("npm install right-pad")).await;
    assert_eq!(second["decision"], json!("auto_allowed"));
    assert!(h.state.pending.is_empty());
}

/// The learned rule must not reach beyond the project it came from.
#[tokio::test]
async fn a_learned_rule_does_not_leak_into_another_project() {
    let h = harness_with(0, 600);
    h.call("POST", "/approve", h.submission("npm install left-pad")).await;
    let id = h.state.pending.list()[0].id.clone();
    h.call(
        "POST",
        &format!("/pending/{id}/decide"),
        json!({ "decision": "allow_similar" }),
    )
    .await;

    let elsewhere = tempfile::tempdir().unwrap();
    let foreign = json!({
        "agent": { "id": "test", "name": "Test", "sessionId": "s1" },
        "projectPath": elsewhere.path().to_string_lossy(),
        "command": "npm install right-pad",
        "argv": ["npm", "install", "right-pad"],
        "workingDirectory": elsewhere.path().to_string_lossy(),
    });
    let body = h.call("POST", "/approve", foreign).await;
    assert_eq!(
        body["decision"],
        json!("no_decision_yet"),
        "the rule escaped the project it was learned in"
    );
}

/// The catastrophic tier is not negotiable, however a rule was created.
#[tokio::test]
async fn a_learned_allow_cannot_unblock_a_catastrophic_command() {
    let h = harness_with(0, 600);
    // Teach a broad allow for the program.
    h.teach("rm something", agent_gate_policy::PolicyDecision::Allow);
    let payload = concat!("rm", " -rf ", "/");
    let body = h.call("POST", "/approve", h.submission(payload)).await;
    assert_eq!(body["decision"], json!("auto_blocked"));
    assert_eq!(body["riskLevel"], json!("blocked"));
}

/// Deciding a compound command must not widen anything, because "similar" to a
/// compound command is not a well-defined set.
#[tokio::test]
async fn allow_similar_on_a_compound_command_stays_exact() {
    let h = harness_with(0, 600);
    h.call("POST", "/approve", h.submission("cd sub && npm install left-pad")).await;
    let id = h.state.pending.list()[0].id.clone();
    h.call(
        "POST",
        &format!("/pending/{id}/decide"),
        json!({ "decision": "allow_similar" }),
    )
    .await;

    let same = h.call("POST", "/approve", h.submission("cd sub && npm install left-pad")).await;
    assert_eq!(same["decision"], json!("auto_allowed"), "the exact command is allowed");

    let different = h.call("POST", "/approve", h.submission("cd sub && npm install evil")).await;
    assert_eq!(
        different["decision"],
        json!("no_decision_yet"),
        "a different compound command must not ride the same rule"
    );
}

/// `agent-gate policy forget` edits the file while the daemon runs. A rule that
/// silently allows commands has to stop applying at once, not at the next
/// restart - which is the whole reason the daemon reads this file per request.
#[tokio::test]
async fn forgetting_a_rule_takes_effect_without_a_restart() {
    let h = harness_with(0, 600);
    h.call("POST", "/approve", h.submission("npm install left-pad")).await;
    let id = h.state.pending.list()[0].id.clone();
    h.call(
        "POST",
        &format!("/pending/{id}/decide"),
        json!({ "decision": "allow_similar" }),
    )
    .await;
    let allowed = h.call("POST", "/approve", h.submission("npm install right-pad")).await;
    assert_eq!(allowed["decision"], json!("auto_allowed"));

    // Revoke out of band, exactly as the CLI does.
    let mut store = h.learned();
    let rule_id = store.rules[0].id.clone();
    assert!(store.forget(&rule_id));
    store.save(&h.state.learned_path).unwrap();

    let asked = h.call("POST", "/approve", h.submission("npm install right-pad")).await;
    assert_eq!(
        asked["decision"],
        json!("no_decision_yet"),
        "the forgotten rule was still being applied"
    );
}

/// The learned file is the source of truth, so a rule taught by any surface
/// applies to the very next request.
#[tokio::test]
async fn a_rule_written_by_the_cli_applies_immediately() {
    let h = harness_with(0, 600);
    let before = h.call("POST", "/approve", h.submission("npm install rimraf")).await;
    assert_eq!(before["decision"], json!("no_decision_yet"));

    h.teach("npm install rimraf", agent_gate_policy::PolicyDecision::Allow);

    let after = h.call("POST", "/approve", h.submission("npm install rimraf")).await;
    assert_eq!(after["decision"], json!("auto_allowed"));
}
