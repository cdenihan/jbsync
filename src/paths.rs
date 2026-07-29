//! Where jbsync keeps its own local state, as distinct from the sync-data
//! store the backend replicates. Mirrors XFER's `~/.xfer`-style layout via
//! `rust_cli_release::SecureDir`.

use std::path::{Path, PathBuf};

use rust_cli_release::{LockedJsonStore, SecureDir};
use serde::{Deserialize, Serialize};

use crate::{config::RepoConfig, error::Result};

pub struct Paths {
    app_dir: SecureDir,
}

impl Paths {
    pub fn discover(override_root: Option<PathBuf>) -> Result<Self> {
        Ok(Self {
            app_dir: SecureDir::discover("jbsync", override_root)?,
        })
    }

    pub fn app_root(&self) -> &Path {
        self.app_dir.root()
    }

    pub fn local_config_path(&self) -> PathBuf {
        self.app_dir.path("config.toml")
    }

    /// The local working copy of the sync-data store (`shared/`, `machines/`,
    /// `sync.toml`, `plugins.json`), defaulting under the app directory unless
    /// `repo.path` overrides it.
    pub fn data_dir(&self, repo: &RepoConfig) -> PathBuf {
        repo.path
            .clone()
            .unwrap_or_else(|| self.app_dir.root().join("data"))
    }

    pub fn state_store(&self) -> LockedJsonStore<StateFile> {
        LockedJsonStore::new(self.app_dir.clone(), "state.json")
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StateFile {
    #[serde(default = "state_version")]
    pub version: u32,
    #[serde(default)]
    pub machine: String,
    /// IDE config directory path -> relative file path -> SHA-256 digest, as
    /// of the last successful sync.
    #[serde(default)]
    pub files: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

fn state_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_defaults_under_app_root() {
        let directory = tempfile::tempdir().unwrap();
        let paths = Paths::discover(Some(directory.path().to_path_buf())).unwrap();
        let repo = RepoConfig::default();
        assert_eq!(paths.data_dir(&repo), directory.path().join("data"));
    }

    #[test]
    fn data_dir_honors_explicit_override() {
        let directory = tempfile::tempdir().unwrap();
        let paths = Paths::discover(Some(directory.path().to_path_buf())).unwrap();
        let repo = RepoConfig {
            path: Some(PathBuf::from("/custom/data")),
            ..RepoConfig::default()
        };
        assert_eq!(paths.data_dir(&repo), PathBuf::from("/custom/data"));
    }
}
