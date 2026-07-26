use agent_gate_policy::{ApprovalRequest, Decision};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

pub struct PendingStore {
    entries: Mutex<HashMap<String, Entry>>,
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

    /// Removes the entry and sends the decision. Returns `false` if no
    /// matching pending entry was found (already decided or expired).
    pub fn decide(&self, id: &str, decision: Decision) -> bool {
        let entry = self.entries.lock().expect("pending store lock poisoned").remove(id);
        match entry {
            Some(entry) => entry.responder.send(decision).is_ok(),
            None => false,
        }
    }
}

impl Default for PendingStore {
    fn default() -> Self {
        Self::new()
    }
}
