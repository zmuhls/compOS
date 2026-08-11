//! Document leases: optional, advisory coordination layered over the real
//! correctness gate (base-revision matching). Process-local tier-3 state —
//! lost on restart by design.
//!
//! RATIFIED(user, 2026-08-11) — these semantics are the `LEASE_HELD`
//! contract of the composd API (ARCHITECTURE.md §6):
//!   1. Leases are optional for save(); base matching is the gate. A save
//!      presenting no lease succeeds unless another session holds a live
//!      lease; presenting the matching lease id always succeeds.
//!   2. TTL is 60 s, sliding: any successful save by the holder — or an
//!      explicit renew — extends the lease by a full TTL.
//!   3. Expiry auto-releases: past its TTL a lease no longer binds and the
//!      next acquire wins. There is no steal verb; expiry is the release.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::VaultError;
use crate::ids::DocId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LeaseId(String);

impl LeaseId {
    pub fn generate() -> Self {
        Self(format!("l_{}", uuid::Uuid::now_v7().simple()))
    }

    pub fn from_string(raw: String) -> Self {
        Self(raw)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub const DEFAULT_LEASE_TTL_MS: u64 = 60_000;

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
    /// Acquire a lease. An expired lease auto-releases here: the next
    /// acquire simply wins (ratified rule 3).
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

    /// The save-path gate (ratified rules 1 and 2): no live lease → pass;
    /// the holder presenting its id → pass *and slide the TTL*; anyone
    /// else while a live lease exists → `LeaseHeld`.
    pub fn check_and_renew(
        &mut self,
        doc: &DocId,
        presented: Option<&LeaseId>,
        now_ms: u64,
    ) -> Result<(), VaultError> {
        match self.leases.get_mut(doc) {
            Some(lease) if lease.live_at(now_ms) => match presented {
                Some(id) if *id == lease.id => {
                    lease.acquired_ms = now_ms;
                    Ok(())
                }
                _ => Err(VaultError::LeaseHeld),
            },
            _ => Ok(()),
        }
    }

    /// Explicit renewal (ratified rule 2). Renewing a lease that has
    /// already expired or was never held fails — re-acquire instead.
    pub fn renew(&mut self, doc: &DocId, id: &LeaseId, now_ms: u64) -> Result<(), VaultError> {
        match self.leases.get_mut(doc) {
            Some(lease) if lease.live_at(now_ms) && lease.id == *id => {
                lease.acquired_ms = now_ms;
                Ok(())
            }
            Some(lease) if lease.live_at(now_ms) => Err(VaultError::LeaseHeld),
            _ => Err(VaultError::ValidationFailed {
                reason: "lease expired or unknown; acquire a new one".to_owned(),
            }),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> DocId {
        DocId::generate()
    }

    #[test]
    fn no_lease_no_gate() {
        let mut t = LeaseTable::default();
        assert!(t.check_and_renew(&doc(), None, 0).is_ok());
    }

    #[test]
    fn other_sessions_blocked_while_live() {
        let mut t = LeaseTable::default();
        let d = doc();
        let lease = t.acquire(d.clone(), 1_000).unwrap();
        assert!(matches!(
            t.check_and_renew(&d, None, 1_000 + DEFAULT_LEASE_TTL_MS - 1),
            Err(VaultError::LeaseHeld)
        ));
        let stranger = LeaseId::generate();
        assert!(matches!(
            t.check_and_renew(&d, Some(&stranger), 2_000),
            Err(VaultError::LeaseHeld)
        ));
        assert!(t.check_and_renew(&d, Some(&lease.id), 2_000).is_ok());
    }

    #[test]
    fn holder_save_slides_the_ttl() {
        let mut t = LeaseTable::default();
        let d = doc();
        let lease = t.acquire(d.clone(), 0).unwrap();
        // Holder saves at t=50s: lease now runs to t=110s.
        t.check_and_renew(&d, Some(&lease.id), 50_000).unwrap();
        // Without the slide this would have expired at t=60s.
        assert!(matches!(
            t.check_and_renew(&d, None, 100_000),
            Err(VaultError::LeaseHeld)
        ));
        // Past the slid deadline it no longer binds.
        assert!(t.check_and_renew(&d, None, 110_001).is_ok());
    }

    #[test]
    fn explicit_renew_extends_and_expired_renew_fails() {
        let mut t = LeaseTable::default();
        let d = doc();
        let lease = t.acquire(d.clone(), 0).unwrap();
        t.renew(&d, &lease.id, 30_000).unwrap(); // runs to 90s
        assert!(matches!(
            t.check_and_renew(&d, None, 89_999),
            Err(VaultError::LeaseHeld)
        ));
        // Renewing after expiry is a validation error, not a takeover path.
        assert!(matches!(
            t.renew(&d, &lease.id, 90_001),
            Err(VaultError::ValidationFailed { .. })
        ));
    }

    #[test]
    fn expiry_auto_releases_next_acquire_wins() {
        let mut t = LeaseTable::default();
        let d = doc();
        let first = t.acquire(d.clone(), 0).unwrap();
        assert!(matches!(
            t.acquire(d.clone(), 1_000),
            Err(VaultError::LeaseHeld)
        ));
        let second = t.acquire(d.clone(), DEFAULT_LEASE_TTL_MS + 1).unwrap();
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn release_frees_immediately() {
        let mut t = LeaseTable::default();
        let d = doc();
        let lease = t.acquire(d.clone(), 0).unwrap();
        t.release(&d, &lease.id);
        assert!(t.check_and_renew(&d, None, 1).is_ok());
    }
}
