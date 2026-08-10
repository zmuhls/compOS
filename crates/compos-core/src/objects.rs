//! Content-addressed immutable object store: `objects/sha256/<aa>/<hash>`.
//! Objects are write-once; `put` is idempotent by construction.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::VaultError;
use crate::fsutil;
use crate::ids::ObjectHash;

#[derive(Debug)]
pub struct ObjectStore {
    root: PathBuf,
    tmp: PathBuf,
}

impl ObjectStore {
    pub(crate) fn new(vault_root: &Path) -> Self {
        Self {
            root: vault_root.join("objects").join("sha256"),
            tmp: vault_root.join("tmp"),
        }
    }

    pub fn path_of(&self, hash: &ObjectHash) -> PathBuf {
        let hex = hash.hex();
        self.root.join(&hex[..2]).join(hex)
    }

    pub fn contains(&self, hash: &ObjectHash) -> bool {
        self.path_of(hash).is_file()
    }

    /// Store bytes, returning their identity. Dedupes on existing objects.
    /// Durability: temp write + fsync in `tmp/`, rename into the shard,
    /// fsync the shard directory (save transaction step 2, §5.3).
    pub fn put(&self, bytes: &[u8]) -> Result<ObjectHash, VaultError> {
        let hash = ObjectHash::of(bytes);
        let dest = self.path_of(&hash);
        if dest.is_file() {
            return Ok(hash);
        }
        let tmp = self
            .tmp
            .join(format!("obj-{}", uuid::Uuid::now_v7().simple()));
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);

        // Immutability hint before the object becomes visible.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o444));
        }

        let shard = dest.parent().expect("object path always has a shard dir");
        fs::create_dir_all(shard)?;
        fs::rename(&tmp, &dest)?;
        fsutil::fsync_dir(shard)?;
        Ok(hash)
    }

    pub fn read(&self, hash: &ObjectHash) -> Result<Vec<u8>, VaultError> {
        fs::read(self.path_of(hash)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                VaultError::ObjectMissing {
                    hash: hash.as_str().to_owned(),
                }
            } else {
                VaultError::Io(e)
            }
        })
    }

    /// Re-hash stored bytes and compare against the name.
    pub fn verify(&self, hash: &ObjectHash) -> Result<bool, VaultError> {
        let bytes = self.read(hash)?;
        Ok(&ObjectHash::of(&bytes) == hash)
    }
}
