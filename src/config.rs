//! Two-tier configuration: a local, unsynced `config.toml` that only knows how
//! to reach the sync-data store and where `JetBrains` lives on this machine, and
//! a `sync.toml` that lives *inside* the sync-data store and therefore
//! replicates to every machine automatically.

use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

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
    /// Branch the git backend publishes to.
    pub branch: String,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            backend: "git".to_string(),
            remote: None,
            path: None,
            branch: "main".to_string(),
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
    pub plugins: PluginsConfig,
    pub xml: XmlConfig,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            version: 1,
            jetbrains: JetbrainsConfig::default(),
            plugins: PluginsConfig::default(),
            xml: XmlConfig::default(),
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

/// What happened when a rule was written, so the caller can say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleWrite {
    Added,
    /// The identical rule was already there, so the file was left alone.
    AlreadyPresent,
}

/// Appends a `[[plugins.rule]]` block to `sync.toml`.
///
/// Appends rather than re-serializing the whole config: `SyncConfig` would
/// round-trip every default into the file and drop the comments a person wrote
/// there. A new array-of-tables at the end is valid TOML whatever precedes it,
/// and last-match-wins means a later rule is also the one that takes effect.
pub fn append_plugin_rule(path: &std::path::Path, rule: &PluginRule) -> Result<RuleWrite> {
    let existing: SyncConfig = read_toml(path)?;
    if existing.plugins.rule.iter().any(|current| {
        current.id == rule.id && current.ide == rule.ide && current.action == rule.action
    }) {
        return Ok(RuleWrite::AlreadyPresent);
    }

    let mut contents = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    if !contents.is_empty() {
        contents.push('\n');
    }
    let _ = write!(
        contents,
        "[[plugins.rule]]\nid = \"{}\"\nide = \"{}\"\naction = \"{}\"\n",
        toml_escape(&rule.id),
        toml_escape(&rule.ide),
        toml_escape(&rule.action)
    );

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    Ok(RuleWrite::Added)
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

/// Escapes the two characters that can end a TOML basic string early. Globs
/// bring `*`, `?` and brackets, none of which need escaping.
fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
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

// ---------------------------------------------------------------------------
// Per-machine override: `machines/<id>.toml` inside the sync-data store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct MachineConfig {
    pub jetbrains: MachineJetbrainsConfig,
    pub xml: MachineXmlConfig,
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
    fn appending_a_rule_keeps_the_comments_around_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sync.toml");
        std::fs::write(&path, "# hand written\n[jetbrains]\nbackups = false\n").unwrap();

        let rule = PluginRule {
            id: "com.falsepattern.zigbrains".to_string(),
            ide: "CLion*".to_string(),
            action: "only".to_string(),
        };
        assert_eq!(append_plugin_rule(&path, &rule).unwrap(), RuleWrite::Added);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# hand written"), "{text}");
        let loaded = SyncConfig::load(&path).unwrap();
        assert!(!loaded.jetbrains.backups, "existing settings must survive");
        assert_eq!(loaded.plugins.rule.len(), 1);
        assert_eq!(loaded.plugins.rule[0].action, "only");
        assert_eq!(loaded.plugins.rule[0].ide, "CLion*");
    }

    #[test]
    fn appending_the_same_rule_twice_changes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sync.toml");
        let rule = PluginRule {
            id: "com.example.tool".to_string(),
            ide: "*".to_string(),
            action: "deny".to_string(),
        };
        append_plugin_rule(&path, &rule).unwrap();
        let once = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            append_plugin_rule(&path, &rule).unwrap(),
            RuleWrite::AlreadyPresent
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), once);
    }

    #[test]
    fn a_rule_written_into_a_missing_file_still_parses() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("sync.toml");
        let rule = PluginRule {
            id: "com.example.tool".to_string(),
            ide: "RustRover*".to_string(),
            action: "only".to_string(),
        };
        append_plugin_rule(&path, &rule).unwrap();
        let loaded = SyncConfig::load(&path).unwrap();
        assert_eq!(loaded.plugins.rule.len(), 1);
        assert_eq!(loaded.plugins.rule[0].id, "com.example.tool");
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
