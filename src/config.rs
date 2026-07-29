//! Two-tier configuration: a local, unsynced `config.toml` that only knows how
//! to reach the sync-data store and where `JetBrains` lives on this machine, and
//! a `sync.toml` that lives *inside* the sync-data store and therefore
//! replicates to every machine automatically.

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

fn read_toml<T: Default + for<'de> Deserialize<'de>>(path: &std::path::Path) -> Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let contents = std::fs::read_to_string(path)?;
    toml::from_str(&contents).map_err(|error| {
        crate::error::JbsyncError::configuration(format!("{}: {error}", path.display()))
    })
}

fn write_toml<T: Serialize>(path: &std::path::Path, value: &T) -> Result<()> {
    let encoded = toml::to_string_pretty(value)
        .map_err(|error| crate::error::JbsyncError::configuration(error.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, encoded)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Local, machine-specific config: ~/.jbsync/config.toml
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct LocalConfig {
    pub repo: RepoConfig,
    pub jetbrains: LocalJetbrainsConfig,
    pub machine: MachineIdentity,
}

impl LocalConfig {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        read_toml(path)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        write_toml(path, self)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RepoConfig {
    /// Only "git" is implemented today; other values are reserved for future
    /// backends (Convex, Turso, a custom Railway service, ...).
    pub backend: String,
    /// A remote the local bare repo pushes to/pulls from. Left unset, the
    /// sync-data store stays purely local under this machine's data directory.
    pub remote: Option<String>,
    /// Overrides where the local working copy of the sync-data store lives.
    pub path: Option<PathBuf>,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            backend: "git".to_string(),
            remote: None,
            path: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct LocalJetbrainsConfig {
    /// "auto" (or unset) detects the OS-conventional `JetBrains` config root.
    pub root: Option<String>,
    pub install_roots: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct MachineIdentity {
    pub id: Option<String>,
}

// ---------------------------------------------------------------------------
// Synced policy config: `sync.toml` at the root of the sync-data store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SyncConfig {
    pub version: u32,
    pub jetbrains: JetbrainsConfig,
    pub bootstrap: BootstrapConfig,
    pub plugins: PluginsConfig,
    pub xml: XmlConfig,
    pub text: TextConfig,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            version: 1,
            jetbrains: JetbrainsConfig::default(),
            bootstrap: BootstrapConfig::default(),
            plugins: PluginsConfig::default(),
            xml: XmlConfig::default(),
            text: TextConfig::default(),
        }
    }
}

impl SyncConfig {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        read_toml(path)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        write_toml(path, self)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct JetbrainsConfig {
    pub ides: Vec<String>,
    pub backups: bool,
    pub use_default_excludes: bool,
    pub explicit_include: Vec<String>,
    pub exclude: Vec<String>,
    /// `None` keeps the engine default of `["**", "*"]`.
    pub include: Option<Vec<String>>,
    pub vmoptions_names: BTreeMap<String, String>,
}

impl Default for JetbrainsConfig {
    fn default() -> Self {
        Self {
            ides: vec!["*20??.*".to_string()],
            backups: true,
            use_default_excludes: true,
            explicit_include: Vec::new(),
            exclude: Vec::new(),
            include: None,
            vmoptions_names: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct BootstrapConfig {
    pub source: Option<String>,
    pub plugin_sources: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PluginsConfig {
    pub enabled: bool,
    pub launchers: BTreeMap<String, String>,
    pub rule: Vec<PluginRule>,
    pub capability: Vec<PluginCapability>,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            launchers: BTreeMap::new(),
            rule: Vec::new(),
            capability: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PluginRule {
    pub id: String,
    #[serde(default = "wildcard")]
    pub ide: String,
    pub action: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PluginCapability {
    #[serde(default = "wildcard")]
    pub ide: String,
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

fn wildcard() -> String {
    "*".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct XmlConfig {
    pub use_defaults: bool,
    pub omit: Vec<XmlOmitRule>,
}

impl Default for XmlConfig {
    fn default() -> Self {
        Self {
            use_defaults: true,
            omit: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct XmlOmitRule {
    pub file: String,
    pub component: Option<String>,
    #[serde(default = "default_element")]
    pub element: String,
    pub option: Option<String>,
    pub attribute: Option<String>,
    #[serde(default)]
    pub equals: String,
}

fn default_element() -> String {
    "option".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TextConfig {
    pub use_defaults: bool,
    pub omit: Vec<TextOmitRule>,
}

impl Default for TextConfig {
    fn default() -> Self {
        Self {
            use_defaults: true,
            omit: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TextOmitRule {
    pub file: String,
    pub prefix: Option<String>,
    pub contains: Option<String>,
    pub regex: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-machine override: `machines/<id>.toml` inside the sync-data store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct MachineConfig {
    pub jetbrains: MachineJetbrainsConfig,
    pub xml: MachineXmlConfig,
    pub text: MachineTextConfig,
}

impl MachineConfig {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        read_toml(path)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct MachineJetbrainsConfig {
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct MachineXmlConfig {
    pub omit: Vec<XmlOmitRule>,
    pub set: Vec<XmlSetRule>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct XmlSetRule {
    pub file: String,
    pub component: Option<String>,
    #[serde(default = "default_element")]
    pub element: String,
    pub option: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct MachineTextConfig {
    pub omit: Vec<TextOmitRule>,
}

/// Sanitizes a raw machine identifier to `[A-Za-z0-9._-]`, matching the
/// filename-safety constraint on `machines/<id>.toml`.
pub fn sanitize_machine_id(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "machine".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn machine_id(explicit: Option<&str>) -> String {
    let raw = explicit
        .map(str::to_string)
        .or_else(|| std::env::var("JBSYNC_MACHINE").ok())
        .or_else(|| hostname().map(|name| name.split('.').next().unwrap_or(&name).to_string()))
        .unwrap_or_else(|| "machine".to_string());
    sanitize_machine_id(&raw)
}

fn hostname() -> Option<String> {
    #[cfg(unix)]
    {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().to_string())
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_unsafe_characters() {
        assert_eq!(sanitize_machine_id("My Laptop!"), "My-Laptop");
        assert_eq!(sanitize_machine_id(""), "machine");
        assert_eq!(sanitize_machine_id("---"), "machine");
    }

    #[test]
    fn default_sync_config_matches_engine_defaults() {
        let config = SyncConfig::default();
        assert_eq!(config.jetbrains.ides, vec!["*20??.*"]);
        assert!(config.jetbrains.backups);
        assert!(config.jetbrains.use_default_excludes);
        assert!(config.plugins.enabled);
    }

    #[test]
    fn round_trips_through_toml() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sync.toml");
        let mut config = SyncConfig::default();
        config.jetbrains.exclude.push("options/foo.xml".to_string());
        config.save(&path).unwrap();
        let loaded = SyncConfig::load(&path).unwrap();
        assert_eq!(loaded.jetbrains.exclude, vec!["options/foo.xml"]);
    }
}
