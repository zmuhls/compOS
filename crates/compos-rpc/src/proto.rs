//! JSON-RPC 2.0 wire types and the typed application errors of the composd
//! API contract (ARCHITECTURE.md §6). `VaultError` variants map 1:1 onto
//! wire errors — that symmetry is maintained deliberately on both sides.

use compos_core::VaultError;
use serde::Deserialize;
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: u32 = 1;

// JSON-RPC 2.0 standard errors.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;

// Application errors (§6 typed errors plus their natural companions).
pub const STALE_BASE: i64 = 1001;
pub const LEASE_HELD: i64 = 1002;
pub const CAPABILITY_DENIED: i64 = 1003;
pub const VALIDATION_FAILED: i64 = 1004;
pub const VAULT_BUSY: i64 = 1005;
pub const FORMAT_UNSUPPORTED: i64 = 1006;
pub const DOC_NOT_FOUND: i64 = 1007;
pub const COMMAND_UNKNOWN: i64 = 1008;
pub const JOURNAL_CORRUPT: i64 = 1009;
pub const OBJECT_MISSING: i64 = 1010;
pub const JOB_UNKNOWN: i64 = 1011;
pub const INTERNAL: i64 = 1099;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Absent id = notification; the server sends no response.
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone)]
pub struct WireError {
    pub code: i64,
    pub name: &'static str,
    pub message: String,
}

impl WireError {
    pub fn new(code: i64, name: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            name,
            message: message.into(),
        }
    }

    pub fn capability_denied(message: impl Into<String>) -> Self {
        Self::new(CAPABILITY_DENIED, "CAPABILITY_DENIED", message)
    }

    pub fn validation_failed(message: impl Into<String>) -> Self {
        Self::new(VALIDATION_FAILED, "VALIDATION_FAILED", message)
    }
}

/// The 1:1 map from library errors to wire errors.
pub fn map_vault_error(e: &VaultError) -> WireError {
    use VaultError as E;
    let (code, name) = match e {
        E::StaleBase { .. } => (STALE_BASE, "STALE_BASE"),
        E::LeaseHeld => (LEASE_HELD, "LEASE_HELD"),
        E::CapabilityDenied { .. } => (CAPABILITY_DENIED, "CAPABILITY_DENIED"),
        E::ValidationFailed { .. } | E::InvalidPath { .. } => {
            (VALIDATION_FAILED, "VALIDATION_FAILED")
        }
        E::VaultBusy => (VAULT_BUSY, "VAULT_BUSY"),
        E::FormatUnsupported { .. } => (FORMAT_UNSUPPORTED, "FORMAT_UNSUPPORTED"),
        E::DocNotFound(_) => (DOC_NOT_FOUND, "DOC_NOT_FOUND"),
        E::CommandUnknown(_) => (COMMAND_UNKNOWN, "COMMAND_UNKNOWN"),
        E::JournalCorrupt { .. } => (JOURNAL_CORRUPT, "JOURNAL_CORRUPT"),
        E::ObjectMissing { .. } => (OBJECT_MISSING, "OBJECT_MISSING"),
        E::AlreadyAVault(_)
        | E::NotAVault(_)
        | E::ReadOnly
        | E::Derived(_)
        | E::Io(_)
        | E::Json(_) => (INTERNAL, "INTERNAL"),
    };
    WireError::new(code, name, e.to_string())
}

pub fn response_ok(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

pub fn response_err(id: &Value, err: &WireError) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": err.code,
            "message": err.message,
            "data": {"type": err.name},
        },
    })
    .to_string()
}

pub fn notification(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}
