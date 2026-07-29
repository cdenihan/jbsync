//! Where jbsync keeps its own local state, as distinct from the sync-data
//! store the backend replicates. Mirrors XFER's `~/.xfer`-style layout via
//! `rust_cli_toolkit::SecureDir`.

use std::path::{Path, PathBuf};

use rust_cli_toolkit::{LockGuard, SecureDir};

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

    /// Per-IDE snapshots of the last state that IDE and the store agreed on.
    /// This is the `base` of every three-way merge, so it has to hold real
    /// content rather than a digest.
    pub fn base_dir(&self) -> PathBuf {
        self.app_dir.root().join("base")
    }

    /// Timestamped copies of IDE files, taken before they are overwritten.
    pub fn backups_dir(&self) -> PathBuf {
        self.app_dir.root().join("backups")
    }

    /// Takes the lock that serializes whole syncs.
    ///
    /// A sync reads the IDEs, rewrites the store, and writes back — two runs
    /// overlapping (a shell and an editor hook, say) could interleave those
    /// steps and publish a half-merged result. Reports `None` rather than
    /// waiting, so the caller can say a run is already in progress.
    pub fn try_lock(&self) -> Result<Option<LockGuard>> {
        Ok(self.app_dir.try_lock_exclusive("sync.lock")?)
    }
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
