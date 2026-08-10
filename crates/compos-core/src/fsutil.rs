//! Durability primitives shared by the object store, journal, and save
//! transaction. Every acknowledged write in CompOS flows through the
//! append → flush → fsync discipline these helpers encode.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Fsync a directory so a preceding rename within it is durable.
///
/// Some filesystems refuse fsync on directory handles (EINVAL/ENOTSUP); the
/// rename that preceded this call is still atomic, so those cases degrade to
/// success rather than failing the transaction.
pub fn fsync_dir(dir: &Path) -> io::Result<()> {
    let f = File::open(dir)?;
    match f.sync_all() {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOTSUP)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Write `bytes` to `target` atomically: temp file in the same directory,
/// fsync, rename over the target, fsync the directory. A reader never
/// observes a partial file; a crash leaves either the old or the new content
/// (plus at worst a stray `.compos-tmp-*` for reconciliation to sweep).
pub fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent dir"))?;
    let tmp = dir.join(format!(".compos-tmp-{}", uuid::Uuid::now_v7().simple()));
    let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    if let Err(e) = fs::rename(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fsync_dir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.md");
        write_atomic(&target, b"one").unwrap();
        write_atomic(&target, b"two").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"two");
        // no stray temp files
        let strays: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".compos-tmp-"))
            .collect();
        assert!(strays.is_empty());
    }
}
