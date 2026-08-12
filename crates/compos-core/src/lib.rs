//! compos-core: the canonical document authority.
//!
//! Pure, synchronous library. The vault, content-addressed object store,
//! append-only revision journal, and the six-step save transaction live here.
//! Constitutional rule 1 (single writer) is enforced in this crate by
//! construction: `VaultWriter` is the only mutation path, and it can only be
//! obtained from a `Vault` holding the exclusive vault lock.

pub mod codec;
pub mod command;
pub mod derived;
pub mod error;
pub mod external;
pub mod fsutil;
pub mod ids;
pub mod journal;
pub mod lease;
pub mod objects;
pub mod profile;
pub mod proposal;
mod reconcile;
pub mod vault;
pub mod writer;

pub use codec::{Codec, CodecRegistry, Fidelity, MarkdownIdentity, logical_digest};
pub use command::{
    CommandHandler, CommandRegistry, CommandSpec, Effect, NetworkPolicy, ResourceClass,
};
pub use derived::{DERIVED_SCHEMA_VERSION, DerivedIndex, SearchHit};
pub use error::VaultError;
pub use external::ExternalScan;
pub use ids::{DocId, ObjectHash, ProposalId, RevisionId};
pub use journal::{DocHead, DocIndex, JournalRecord, RevisionOrigin};
pub use lease::{Lease, LeaseId};
pub use profile::HostProfile;
pub use proposal::{
    AcceptOutcome, CreateProposal, Hunk, PROPOSAL_RECORD_VERSION, Proposal, ProposalState,
};
pub use vault::Vault;
pub use writer::{DocRef, SaveOutcome, SaveRequest, VaultWriter};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The newest vault format this build can open (ARCHITECTURE.md §5.2).
pub const VAULT_FORMAT: u32 = 1;

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_nonempty() {
        assert!(!super::VERSION.is_empty());
    }
}
