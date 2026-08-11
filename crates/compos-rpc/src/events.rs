//! Event fan-out (§6): topics with per-topic monotonic sequence numbers so a
//! reconnecting client can detect gaps and resync. Publishing is sync and
//! cheap — callable from the watcher thread as well as async handlers.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

/// The topics the contract defines today. `events.subscribe` rejects
/// anything else, so typos fail loudly instead of subscribing to silence.
pub const TOPICS: &[&str] = &[
    "revision.committed",
    "doc.external_change",
    "proposal.updated",
    "proposal.stale",
    "index.progress",
    "job.progress",
];

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub topic: String,
    pub seq: u64,
    pub payload: Value,
}

#[derive(Debug)]
pub struct EventBus {
    seqs: Mutex<HashMap<String, u64>>,
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            seqs: Mutex::new(HashMap::new()),
            tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Current sequence number for a topic (0 = nothing published yet).
    pub fn seq(&self, topic: &str) -> u64 {
        *self.seqs.lock().unwrap().get(topic).unwrap_or(&0)
    }

    pub fn publish(&self, topic: &str, payload: Value) {
        let seq = {
            let mut seqs = self.seqs.lock().unwrap();
            let s = seqs.entry(topic.to_owned()).or_insert(0);
            *s += 1;
            *s
        };
        // No receivers is fine; events are fire-and-forget.
        let _ = self.tx.send(Event {
            topic: topic.to_owned(),
            seq,
            payload,
        });
    }
}
