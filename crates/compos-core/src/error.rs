use std::path::PathBuf;

use thiserror::Error;

use crate::ids::RevisionId;

/// Typed vault errors. Variant names mirror the wire errors of the composd
/// API contract (ARCHITECTURE.md §6) so the future RPC layer maps 1:1.
#[derive(Debug, Error)]
pub enum VaultError {
    #[error("stale base revision: head is {expected:?}, request based on {got:?}")]
    StaleBase {
        expected: Option<RevisionId>,
        got: Option<RevisionId>,
    },
    #[error("document lease held by another session")]
    LeaseHeld,
    #[error("vault is busy: another process holds the vault lock")]
    VaultBusy,
    #[error("vault format {found} is not supported (this build supports up to {supported})")]
    FormatUnsupported { found: u32, supported: u32 },
    #[error("capability denied: {reason}")]
    CapabilityDenied { reason: String },
    #[error("validation failed: {reason}")]
    ValidationFailed { reason: String },
    #[error("journal corrupt in {segment} at line {line}: {reason}")]
    JournalCorrupt {
        segment: String,
        line: u64,
        reason: String,
    },
    #[error("content object missing: {hash}")]
    ObjectMissing { hash: String },
    #[error("not a vault (no compos.json): {0}")]
    NotAVault(PathBuf),
    #[error("already a vault: {0}")]
    AlreadyAVault(PathBuf),
    #[error("document not found: {0}")]
    DocNotFound(String),
    #[error("proposal not found: {0}")]
    ProposalNotFound(String),
    #[error("unknown command: {0}")]
    CommandUnknown(String),
    #[error("invalid vault path {path:?}: {reason}")]
    InvalidPath { path: String, reason: String },
    #[error("vault opened read-only")]
    ReadOnly,
    #[error("derived index: {0}")]
    Derived(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}
