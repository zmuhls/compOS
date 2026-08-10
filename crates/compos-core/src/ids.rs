use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable document identity, independent of path (ARCHITECTURE.md §5.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocId(String);

/// Identity of one revision in a document's linear chain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RevisionId(String);

/// Portable content identity: `sha256:<64 lowercase hex>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectHash(String);

macro_rules! prefixed_id {
    ($ty:ident, $prefix:literal) => {
        impl $ty {
            pub fn generate() -> Self {
                Self(format!(
                    concat!($prefix, "{}"),
                    uuid::Uuid::now_v7().simple()
                ))
            }

            pub fn from_string(raw: String) -> Self {
                Self(raw)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

prefixed_id!(DocId, "d_");
prefixed_id!(RevisionId, "r_");

impl ObjectHash {
    const PREFIX: &'static str = "sha256:";

    /// Hash content bytes into their portable object identity.
    pub fn of(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self(format!("{}{}", Self::PREFIX, hex::encode(digest)))
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let hex_part = raw.strip_prefix(Self::PREFIX)?;
        if hex_part.len() == 64 && hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
            Some(Self(raw.to_ascii_lowercase()))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 64-char hex portion, used for on-disk sharding.
    pub fn hex(&self) -> &str {
        &self.0[Self::PREFIX.len()..]
    }
}

impl fmt::Display for ObjectHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_hash_round_trip() {
        let h = ObjectHash::of(b"hello");
        assert!(h.as_str().starts_with("sha256:"));
        assert_eq!(h.hex().len(), 64);
        assert_eq!(ObjectHash::parse(h.as_str()), Some(h));
    }

    #[test]
    fn object_hash_rejects_garbage() {
        assert!(ObjectHash::parse("sha256:zz").is_none());
        assert!(ObjectHash::parse("md5:abcd").is_none());
    }

    #[test]
    fn ids_are_prefixed_and_unique() {
        let a = DocId::generate();
        let b = DocId::generate();
        assert!(a.as_str().starts_with("d_"));
        assert_ne!(a, b);
        assert!(RevisionId::generate().as_str().starts_with("r_"));
    }
}
