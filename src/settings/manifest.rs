//! The roamable-file allowlist, remembered in the store so it survives the
//! machine that learned it.
//!
//! JetBrains only writes a `settingsSync/` tree once its bundled Backup and
//! Sync has run. That makes the tree excellent evidence and a terrible
//! dependency: a machine set up from scratch has none, and some products ship
//! without one even on a machine where their siblings have it.
//!
//! So the union is recorded in `manifest.toml` at the root of the store. It
//! only ever grows, and it replicates like everything else there, which means a
//! machine that has never run Backup and Sync still gets the full allowlist —
//! learned once, by any machine, forever.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::{JbsyncError, Result};

/// Filename at the root of the store.
pub const FILE_NAME: &str = "manifest.toml";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct StoredManifest {
    pub version: u32,
    /// Store-relative glob patterns, sorted. Union of every `settingsSync`
    /// tree any machine has ever observed.
    pub roamable: Vec<String>,
}

impl Default for StoredManifest {
    fn default() -> Self {
        Self {
            version: 1,
            roamable: Vec::new(),
        }
    }
}

impl StoredManifest {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)?;
        toml::from_str(&contents)
            .map_err(|error| JbsyncError::configuration(format!("{}: {error}", path.display())))
    }

    pub fn encode(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|error| JbsyncError::configuration(error.to_string()))
    }

    /// Folds newly observed entries in, returning `true` when that added
    /// something. Entries are never removed: an IDE being uninstalled, or
    /// Backup and Sync being switched off, is not evidence that a file stopped
    /// being roamable.
    pub fn absorb(&mut self, observed: &[String]) -> bool {
        let before = self.roamable.len();
        let mut merged: BTreeSet<String> = self.roamable.drain(..).collect();
        merged.extend(observed.iter().cloned());
        self.roamable = merged.into_iter().collect();
        self.roamable.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absorbing_is_a_growing_union() {
        let mut manifest = StoredManifest::default();
        assert!(manifest.absorb(&["options/editor.xml".to_string()]));
        assert!(manifest.absorb(&["options/laf.xml".to_string()]));
        // Already known, so nothing changes and no commit is provoked.
        assert!(!manifest.absorb(&["options/editor.xml".to_string()]));
        assert_eq!(manifest.roamable, ["options/editor.xml", "options/laf.xml"]);
    }

    #[test]
    fn an_ide_disappearing_does_not_shrink_the_record() {
        let mut manifest = StoredManifest::default();
        manifest.absorb(&["options/editor.xml".to_string()]);
        // A machine with no settingsSync tree at all observes nothing.
        assert!(!manifest.absorb(&[]));
        assert_eq!(manifest.roamable, ["options/editor.xml"]);
    }

    #[test]
    fn round_trips_through_toml() {
        let mut manifest = StoredManifest::default();
        manifest.absorb(&["options/editor.xml".to_string()]);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        std::fs::write(&path, manifest.encode().unwrap()).unwrap();
        let loaded = StoredManifest::load(&path).unwrap();
        assert_eq!(loaded.roamable, manifest.roamable);
        assert_eq!(loaded.version, 1);
    }
}
