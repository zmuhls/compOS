//! Document leases. In the slice these are process-local; the semantics
//! defined here become the `LEASE_HELD` contract of the composd API
//! (ARCHITECTURE.md §6) once compos-rpc exists.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::VaultError;
use crate::ids::DocId;

// DECISION(user): define what a lease means before RPC exists (5–10 lines
// of judgment, recorded here or as amended code):
//   1. Is a lease required for save(), or is base-revision matching
//      sufficient while the vault is single-writer? (Scaffold default:
//      leases are optional; base matching is the gate.)
//   2. TTL value and renewal rule? (Scaffold default: 30s, no renewal.)
//   3. Does expiry auto-release, or does taking over a held document
//      require an explicit steal command? (Scaffold default: auto-release
//      on expiry.)
// These semantics become observable API behavior the moment two shells or
// an agent session can address the same vault.

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LeaseId(String);

impl LeaseId {
    pub fn generate() -> Self {
        Self(format!("l_{}", uuid::Uuid::now_v7().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub const DEFAULT_LEASE_TTL_MS: u64 = 30_000;

#[derive(Debug, Clone)]
pub struct Lease {
    pub id: LeaseId,
    pub doc: DocId,
    pub acquired_ms: u64,
    pub ttl_ms: u64,
}

impl Lease {
    fn live_at(&self, now_ms: u64) -> bool {
        now_ms < self.acquired_ms.saturating_add(self.ttl_ms)
    }
}

#[derive(Debug, Default)]
pub(crate) struct LeaseTable {
    leases: HashMap<DocId, Lease>,
}

impl LeaseTable {
    pub fn acquire(&mut self, doc: DocId, now_ms: u64) -> Result<Lease, VaultError> {
        if let Some(existing) = self.leases.get(&doc)
            && existing.live_at(now_ms)
        {
            return Err(VaultError::LeaseHeld);
        }
        let lease = Lease {
            id: LeaseId::generate(),
            doc: doc.clone(),
            acquired_ms: now_ms,
            ttl_ms: DEFAULT_LEASE_TTL_MS,
        };
        self.leases.insert(doc, lease.clone());
        Ok(lease)
    }

    /// Scaffold policy: a save presenting no lease succeeds unless a live
    /// lease held by someone else exists; presenting the matching lease id
    /// always succeeds.
    pub fn check(
        &self,
        doc: &DocId,
        presented: Option<&LeaseId>,
        now_ms: u64,
    ) -> Result<(), VaultError> {
        match self.leases.get(doc) {
            Some(lease) if lease.live_at(now_ms) => match presented {
                Some(id) if *id == lease.id => Ok(()),
                _ => Err(VaultError::LeaseHeld),
            },
            _ => Ok(()),
        }
    }

    pub fn release(&mut self, doc: &DocId, id: &LeaseId) {
        if let Some(lease) = self.leases.get(doc)
            && lease.id == *id
        {
            self.leases.remove(doc);
        }
    }
}
