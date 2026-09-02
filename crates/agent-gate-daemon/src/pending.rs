use agent_gate_policy::{ApprovalRequest, Decision};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

pub struct PendingStore {
    entries: Mutex<HashMap<String, Entry>>,
}

/// The outcome of deciding a pending request.
pub struct Decided {
    pub request: ApprovalRequest,
    /// Whether a still-waiting agent received the decision.
    pub delivered: bool,
}

struct Entry {
    request: ApprovalRequest,
    responder: oneshot::Sender<Decision>,
}

impl PendingStore {
    pub fn new() -> Self {
        PendingStore {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, request: ApprovalRequest) -> oneshot::Receiver<Decision> {
        let (tx, rx) = oneshot::channel();
        let id = request.id.clone();
        self.entries.lock().expect("pending store lock poisoned").insert(
            id,
            Entry {
                request,
                responder: tx,
            },
        );
        rx
    }

    pub fn remove(&self, id: &str) {
        self.entries.lock().expect("pending store lock poisoned").remove(id);
    }

    pub fn list(&self) -> Vec<ApprovalRequest> {
        self.entries
            .lock()
            .expect("pending store lock poisoned")
            .values()
            .map(|e| e.request.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().expect("pending store lock poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Removes and returns every entry that expired at or before `now`.
    ///
    /// The submitting client may be long gone — its request handler is dropped
    /// the moment it disconnects, taking any cleanup that sits after an `await`
    /// with it. So expiry cannot be left to the handler; something outside the
    /// connection lifetime has to sweep. Dropping an entry also drops its
    /// responder, which wakes any handler still waiting on it.
    pub fn reap_expired(&self, now: DateTime<Utc>) -> Vec<ApprovalRequest> {
        let mut entries = self.entries.lock().expect("pending store lock poisoned");
        let expired: Vec<String> = entries
            .values()
            .filter(|e| e.request.expires_at <= now)
            .map(|e| e.request.id.clone())
            .collect();
        expired
            .iter()
            .filter_map(|id| entries.remove(id).map(|e| e.request))
            .collect()
    }

    /// Removes the entry and sends the decision.
    ///
    /// Returns the request that was decided, or `None` if no matching entry
    /// existed. `delivered` says whether a waiting agent actually received the
    /// decision: once the agent has stopped waiting the request stays
    /// answerable, so a human decision still has to be recorded even though
    /// nobody is listening for it.
    pub fn decide(&self, id: &str, decision: Decision) -> Option<Decided> {
        let entry = self
            .entries
            .lock()
            .expect("pending store lock poisoned")
            .remove(id)?;
        let request = entry.request;
        let delivered = entry.responder.send(decision).is_ok();
        Some(Decided { request, delivered })
    }
}

impl Default for PendingStore {
    fn default() -> Self {
        Self::new()
    }
}
