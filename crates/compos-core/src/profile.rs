//! Host profiles (ARCHITECTURE.md §3). A profile selects paths and, later,
//! transports and enforcement depth. It never changes the document model.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProfile {
    DevMac,
    DevLinux,
}

impl HostProfile {
    pub fn resolve() -> Self {
        if cfg!(target_os = "macos") {
            HostProfile::DevMac
        } else {
            HostProfile::DevLinux
        }
    }

    /// Default vault root for this profile. `None` when the environment
    /// gives no home directory to anchor it.
    pub fn default_vault_root(&self) -> Option<PathBuf> {
        match self {
            HostProfile::DevMac => {
                let home = std::env::var_os("HOME")?;
                Some(
                    PathBuf::from(home)
                        .join("Library")
                        .join("Application Support")
                        .join("compos")
                        .join("vault"),
                )
            }
            HostProfile::DevLinux => {
                if let Some(xdg) = std::env::var_os("XDG_DATA_HOME")
                    && !xdg.is_empty()
                {
                    return Some(PathBuf::from(xdg).join("compos").join("vault"));
                }
                let home = std::env::var_os("HOME")?;
                Some(
                    PathBuf::from(home)
                        .join(".local")
                        .join("share")
                        .join("compos")
                        .join("vault"),
                )
            }
        }
    }
}
