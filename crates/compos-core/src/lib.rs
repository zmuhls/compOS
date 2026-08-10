//! compos-core: the canonical document authority.
//!
//! Pure, synchronous library. The vault, content-addressed object store,
//! append-only revision journal, and the six-step save transaction live here.
//! Constitutional rule 1 (single writer) is enforced in this crate by
//! construction: `VaultWriter` is the only mutation path, and it can only be
//! obtained from a `Vault` holding the exclusive vault lock.

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
