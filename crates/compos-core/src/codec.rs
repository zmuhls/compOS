//! Codec registry (ARCHITECTURE.md §8): export, ingest, and publish
//! symmetry. A codec registration includes *both* directions by trait
//! shape — a one-way codec is unrepresentable, which is how "CompOS rejects
//! one-way codecs" is enforced at compile time rather than review time.
//!
//! The round-trip law every codec must satisfy over its fixture corpus:
//!
//! ```text
//! logical_digest(resource) = logical_digest(import(export(resource)))
//! object hashes before export = object hashes after import
//! ```
//!
//! Phase 1 ships exactly one codec — Markdown identity — and the harness in
//! `tests/roundtrip.rs` walks the fixture corpus generically so Phase-4
//! codecs inherit the gate by dropping fixtures into place.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::VaultError;
use crate::ids::ObjectHash;

/// How much survives a round trip. Only `Lossless` codecs may claim byte
/// identity; others must still round-trip their canonical projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fidelity {
    Lossless,
    Structural,
    Archival,
}

/// Both directions of one format. `import` maps external bytes into
/// canonical form; `export` maps canonical form back out.
pub trait Codec: Send + Sync {
    fn id(&self) -> &'static str;
    fn fidelity(&self) -> Fidelity;
    fn import(&self, external: &[u8]) -> Result<Vec<u8>, VaultError>;
    fn export(&self, canonical: &[u8]) -> Result<Vec<u8>, VaultError>;
}

/// The content-identity function over a resource's canonical form, used by
/// the round-trip law (§8, glossary).
pub fn logical_digest(canonical: &[u8]) -> ObjectHash {
    ObjectHash::of(canonical)
}

/// Markdown is CompOS's native canonical form, so its codec is the identity
/// function in both directions — the first fixture the harness ever runs,
/// and the baseline every other codec is measured against.
pub struct MarkdownIdentity;

impl Codec for MarkdownIdentity {
    fn id(&self) -> &'static str {
        "markdown"
    }

    fn fidelity(&self) -> Fidelity {
        Fidelity::Lossless
    }

    fn import(&self, external: &[u8]) -> Result<Vec<u8>, VaultError> {
        Ok(external.to_vec())
    }

    fn export(&self, canonical: &[u8]) -> Result<Vec<u8>, VaultError> {
        Ok(canonical.to_vec())
    }
}

#[derive(Default)]
pub struct CodecRegistry {
    codecs: BTreeMap<&'static str, Box<dyn Codec>>,
}

impl CodecRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Box::new(MarkdownIdentity))
            .expect("built-in codec registration is infallible");
        r
    }

    pub fn register(&mut self, codec: Box<dyn Codec>) -> Result<(), VaultError> {
        let id = codec.id();
        if self.codecs.contains_key(id) {
            return Err(VaultError::ValidationFailed {
                reason: format!("codec '{id}' already registered"),
            });
        }
        self.codecs.insert(id, codec);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&dyn Codec> {
        self.codecs.get(id).map(|c| c.as_ref())
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.codecs.keys().copied().collect()
    }
}
