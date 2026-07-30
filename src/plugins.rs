//! Plugin sync by manifest, not by copying files.
//!
//! Plugin directories carry compiled code and sometimes native libraries, so
//! copying them between machines — or between a macOS and a Windows install —
//! is unsound. Instead jbsync records *which* third-party plugins are present,
//! along with the compatibility metadata from each descriptor, and other
//! machines install them from Marketplace through the IDE's own launcher. This
//! is how JetBrains' bundled Backup and Sync handles plugins too.
//!
//! Bundled plugins are deliberately excluded: they ship with the product, and
//! trying to install them elsewhere either fails or is a no-op.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::{
    config::{PluginsConfig, SyncConfig},
    error::{JbsyncError, Result},
    ide::{Ide, digit_runs},
    xml::dom,
};

/// What a plugin's `META-INF/plugin.xml` says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Plugin {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub since_build: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub until_build: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incompatible_with: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provided_modules: Vec<String>,
    /// Uses the modern `<content>` descriptor form.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub modular: bool,
    /// Products this plugin was actually observed installed in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_products: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(default = "manifest_version")]
    pub version: u32,
    #[serde(default)]
    pub plugins: Vec<Plugin>,
}

fn manifest_version() -> u32 {
    1
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)?;
        serde_json::from_str(&contents)
            .map_err(|error| JbsyncError::configuration(format!("{}: {error}", path.display())))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut encoded = serde_json::to_string_pretty(self)
            .map_err(|error| JbsyncError::other(error.to_string()))?;
        encoded.push('\n');
        std::fs::write(path, encoded)?;
        Ok(())
    }

    pub fn ids(&self) -> BTreeSet<String> {
        self.plugins
            .iter()
            .map(|plugin| plugin.id.clone())
            .collect()
    }
}

/// Reads `META-INF/plugin.xml`, whether it sits loose in the plugin directory
/// or inside one of its jars.
fn descriptor_xml(directory: &Path) -> Option<String> {
    let direct = directory.join("META-INF/plugin.xml");
    if let Ok(contents) = std::fs::read_to_string(&direct) {
        return Some(contents);
    }
    let mut jars: Vec<std::path::PathBuf> = Vec::new();
    for candidate in [directory.join("lib"), directory.to_path_buf()] {
        let Ok(entries) = std::fs::read_dir(&candidate) else {
            continue;
        };
        jars.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "jar")),
        );
    }
    jars.sort();
    for jar in jars {
        let Ok(file) = std::fs::File::open(&jar) else {
            continue;
        };
        let Ok(mut archive) = zip::ZipArchive::new(file) else {
            continue;
        };
        let Ok(mut entry) = archive.by_name("META-INF/plugin.xml") else {
            continue;
        };
        let mut contents = String::new();
        if std::io::Read::read_to_string(&mut entry, &mut contents).is_ok() {
            return Some(contents);
        }
    }
    None
}

fn child_text(root: &dom::Element, name: &str) -> String {
    root.children
        .iter()
        .find(|child| child.name == name)
        .and_then(|child| child.text.clone())
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn parse_descriptor(contents: &str, fallback_id: &str) -> Option<Plugin> {
    let root = dom::parse(contents).ok()?;

    let mut required = BTreeSet::new();
    let mut incompatible = BTreeSet::new();
    let mut provided = BTreeSet::new();
    let mut modular = false;

    for child in &root.children {
        match child.name.as_str() {
            "depends" => {
                let value = child.text.clone().unwrap_or_default();
                // Optional dependencies do not gate installation.
                if !value.is_empty()
                    && child.attributes.get("optional").map(String::as_str) != Some("true")
                {
                    required.insert(value);
                }
            }
            // Modern form: <dependencies><module name=.. loading=required/>.
            "dependencies" => {
                for dependency in &child.children {
                    let Some(value) = dependency
                        .attributes
                        .get("name")
                        .or_else(|| dependency.attributes.get("id"))
                    else {
                        continue;
                    };
                    let loading = dependency
                        .attributes
                        .get("loading")
                        .map_or("optional", String::as_str);
                    if loading == "required" {
                        required.insert(value.clone());
                    }
                }
            }
            "incompatible-with" => {
                if let Some(value) = &child.text {
                    incompatible.insert(value.clone());
                }
            }
            "module" => {
                if let Some(value) = child.attributes.get("value") {
                    provided.insert(value.clone());
                }
            }
            "content" => {
                modular = true;
                for module in &child.children {
                    if let Some(value) = module.attributes.get("name") {
                        provided.insert(value.clone());
                    }
                }
            }
            _ => {}
        }
    }

    let idea_version = root
        .children
        .iter()
        .find(|child| child.name == "idea-version");
    let attribute = |name: &str| {
        idea_version
            .and_then(|node| node.attributes.get(name))
            .cloned()
            .unwrap_or_default()
    };

    let id = {
        let candidate = child_text(&root, "id");
        let candidate = if candidate.is_empty() {
            child_text(&root, "name")
        } else {
            candidate
        };
        if candidate.is_empty() {
            fallback_id.to_string()
        } else {
            candidate
        }
    };
    let name = {
        let candidate = child_text(&root, "name");
        if candidate.is_empty() {
            id.clone()
        } else {
            candidate
        }
    };

    Some(Plugin {
        id,
        name,
        version: child_text(&root, "version"),
        since_build: attribute("since-build"),
        until_build: attribute("until-build"),
        required_dependencies: required.into_iter().collect(),
        incompatible_with: incompatible.into_iter().collect(),
        provided_modules: provided.into_iter().collect(),
        modular,
        source_products: Vec::new(),
    })
}

/// Plugin IDs shipped with the product, which must never be installed again.
pub fn bundled_ids(ide: &Ide) -> BTreeSet<String> {
    let mut found: BTreeSet<String> = ide
        .metadata
        .as_ref()
        .map(|metadata| metadata.bundled_plugins.iter().cloned().collect())
        .unwrap_or_default();
    if let Ok(contents) = std::fs::read_to_string(ide.path.join("bundled_plugins.txt")) {
        for line in contents.lines().filter(|line| !line.trim().is_empty()) {
            found.insert(line.split('|').next().unwrap_or(line).trim().to_string());
        }
    }
    found
}

/// Bundled ids after `plugins.capability` removals, which is what decides
/// whether an install is needed.
///
/// Deliberately narrower than [`capabilities`]: that set answers "can this IDE
/// satisfy a dependency named X", and so also contains platform modules,
/// modules other plugins provide, and configured additions. None of those mean
/// the plugin itself is present, so none of them may suppress an install.
fn bundled_here(ide: &Ide, config: &PluginsConfig) -> BTreeSet<String> {
    let mut found = bundled_ids(ide);
    for rule in &config.capability {
        if targets_ide(&rule.ide, ide) {
            for removed in &rule.remove {
                found.remove(removed);
            }
        }
    }
    found
}

/// Third-party plugins installed into this IDE.
pub fn installed(ide: &Ide, include_bundled: bool) -> BTreeMap<String, Plugin> {
    let bundled = bundled_ids(ide);
    let mut found = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(ide.path.join("plugins")) else {
        return found;
    };
    let mut directories: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !path.is_symlink())
        .collect();
    directories.sort();

    for directory in directories {
        let fallback = directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(contents) = descriptor_xml(&directory) else {
            continue;
        };
        let Some(mut plugin) = parse_descriptor(&contents, &fallback) else {
            continue;
        };
        if !include_bundled && bundled.contains(&plugin.id) {
            continue;
        }
        plugin.source_products = vec![ide.product.clone()];
        found.insert(plugin.id.clone(), plugin);
    }
    found
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    globset::Glob::new(pattern).is_ok_and(|glob| glob.compile_matcher().is_match(value))
}

fn targets_ide(pattern: &str, ide: &Ide) -> bool {
    let directory = ide
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    glob_matches(pattern, &directory) || glob_matches(pattern, &ide.product)
}

/// Everything this IDE can satisfy a dependency with: platform modules,
/// bundled plugins, and whatever is already installed.
pub fn capabilities(
    ide: &Ide,
    installed_plugins: &BTreeMap<String, Plugin>,
    config: &PluginsConfig,
) -> BTreeSet<String> {
    let mut found: BTreeSet<String> = ide
        .metadata
        .as_ref()
        .map(|metadata| metadata.modules.iter().cloned().collect())
        .unwrap_or_default();
    found.extend(bundled_ids(ide));
    for (id, plugin) in installed_plugins {
        found.insert(id.clone());
        found.extend(plugin.provided_modules.iter().cloned());
    }
    for rule in &config.capability {
        if targets_ide(&rule.ide, ide) {
            found.extend(rule.add.iter().cloned());
            for removed in &rule.remove {
                found.remove(removed);
            }
        }
    }
    found
}

/// Compares an IDE build against a plugin's declared range.
fn build_is_compatible(target: &str, since: &str, until: &str) -> std::result::Result<(), String> {
    let target_value = digit_runs(target);
    if !since.is_empty() && target_value < digit_runs(since) {
        return Err(format!("needs build {since}+"));
    }
    if until.is_empty() {
        return Ok(());
    }
    if let Some(prefix) = until.strip_suffix(".*") {
        let prefix = digit_runs(prefix);
        if target_value.len() < prefix.len() || target_value[..prefix.len()] != prefix[..] {
            return Err(format!("ends at build {until}"));
        }
    } else if target_value > digit_runs(until) {
        return Err(format!("ends at build {until}"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub compatible: bool,
    pub reason: String,
}

/// Whether `plugin` can be installed into `ide`, and why.
pub fn compatibility(
    plugin: &Plugin,
    ide: &Ide,
    capabilities: &BTreeSet<String>,
    managed: &BTreeSet<String>,
    config: &PluginsConfig,
) -> Verdict {
    // An explicit rule always wins, so a user can override our reasoning.
    let mut manual: Option<Verdict> = None;
    for rule in &config.rule {
        if !glob_matches(&rule.id, &plugin.id) {
            continue;
        }
        let hits_ide = targets_ide(&rule.ide, ide);
        match rule.action.to_ascii_lowercase().as_str() {
            // `only` is the one action that says something about the IDEs it
            // does *not* name: confining a plugin to one product otherwise
            // takes a blanket deny plus a narrower allow, and getting that
            // pair in the wrong order silently does nothing.
            "only" => {
                manual = Some(if hits_ide {
                    Verdict {
                        compatible: true,
                        reason: format!("only rule for {}", rule.ide),
                    }
                } else {
                    Verdict {
                        compatible: false,
                        reason: format!("only for {}", rule.ide),
                    }
                });
            }
            action @ ("allow" | "deny") if hits_ide => {
                manual = Some(Verdict {
                    compatible: action == "allow",
                    reason: format!("manual {action} rule"),
                });
            }
            _ => {}
        }
    }

    let verdict = evaluate(plugin, ide, capabilities, managed);
    manual.unwrap_or(verdict)
}

fn evaluate(
    plugin: &Plugin,
    ide: &Ide,
    capabilities: &BTreeSet<String>,
    managed: &BTreeSet<String>,
) -> Verdict {
    let target_build = ide
        .metadata
        .as_ref()
        .map(|metadata| metadata.build_number.clone())
        .unwrap_or_default();

    if let Err(reason) =
        build_is_compatible(&target_build, &plugin.since_build, &plugin.until_build)
    {
        return Verdict {
            compatible: false,
            reason,
        };
    }

    let blocked: Vec<&String> = plugin
        .incompatible_with
        .iter()
        .filter(|module| capabilities.contains(*module))
        .collect();
    if !blocked.is_empty() {
        return Verdict {
            compatible: false,
            reason: format!(
                "declares incompatibility with {}",
                blocked
                    .iter()
                    .map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }

    let missing: Vec<&String> = plugin
        .required_dependencies
        .iter()
        .filter(|module| !capabilities.contains(*module) && !managed.contains(*module))
        .collect();
    if !missing.is_empty() {
        return Verdict {
            compatible: false,
            reason: format!(
                "missing required {}",
                missing
                    .iter()
                    .map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }

    if !plugin.required_dependencies.is_empty() || plugin.modular {
        return Verdict {
            compatible: true,
            reason: "required modules are available".to_string(),
        };
    }
    // A descriptor that declares no product dependency at all tells us nothing,
    // so only trust it where the plugin was actually seen working.
    if plugin.source_products.contains(&ide.product) {
        Verdict {
            compatible: true,
            reason: "already present in this product".to_string(),
        }
    } else {
        Verdict {
            compatible: false,
            reason: "descriptor declares no product dependency".to_string(),
        }
    }
}

/// A plugin that is installed but cannot load, because something it declares as
/// required is not present in that IDE.
#[derive(Debug, Clone)]
pub struct BrokenPlugin {
    pub ide: String,
    pub plugin: String,
    /// Dependencies the IDE cannot satisfy.
    pub missing: Vec<String>,
    /// The subset of `missing` that looks like a Marketplace plugin rather than
    /// a platform module, and so can be fixed by installing it.
    pub installable: Vec<String>,
}

/// Checks the plugins each IDE *already has*, rather than the ones jbsync would
/// add.
///
/// `plan_installs` deliberately skips anything already installed, so a plugin
/// put there by hand — or copied in by Toolbox when it set the IDE up — that
/// the IDE cannot satisfy stays invisible until the IDE itself complains at
/// startup. This is the check that catches it first.
pub fn diagnose(ides: &[&Ide], config: &SyncConfig) -> Vec<BrokenPlugin> {
    let mut broken = Vec::new();
    for ide in ides {
        let present = installed(ide, true);
        let capable = capabilities(ide, &present, &config.plugins);
        // Only third-party plugins are worth reporting: a bundled plugin with
        // an unmet dependency is the product's own business.
        for (id, plugin) in installed(ide, false) {
            let missing: Vec<String> = plugin
                .required_dependencies
                .iter()
                .filter(|needed| !capable.contains(*needed))
                .cloned()
                .collect();
            if missing.is_empty() {
                continue;
            }
            broken.push(BrokenPlugin {
                ide: ide
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                plugin: id,
                installable: missing
                    .iter()
                    .filter(|needed| is_marketplace_plugin(needed))
                    .cloned()
                    .collect(),
                missing,
            });
        }
    }
    broken
}

/// Distinguishes a dependency that could be installed from a platform module
/// that could not.
///
/// Platform capabilities are namespaced `com.intellij.modules.*`; anything else
/// declared as required is a plugin, and a missing one is fixable.
fn is_marketplace_plugin(dependency: &str) -> bool {
    !dependency.starts_with("com.intellij.modules.")
}

/// Builds the manifest from what is installed across every IDE.
pub fn collect(ides: &[&Ide], config: &SyncConfig) -> Manifest {
    let mut merged: BTreeMap<String, Plugin> = BTreeMap::new();
    for ide in ides {
        for (id, plugin) in installed(ide, false) {
            merged
                .entry(id)
                .and_modify(|existing| {
                    for product in &plugin.source_products {
                        if !existing.source_products.contains(product) {
                            existing.source_products.push(product.clone());
                        }
                    }
                })
                .or_insert(plugin);
        }
    }
    let _ = config;
    Manifest {
        version: manifest_version(),
        plugins: merged.into_values().collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallAction {
    pub ide: String,
    pub plugin: String,
    pub install: bool,
    pub reason: String,
}

/// Works out which manifest plugins are missing from each IDE.
pub fn plan_installs(
    ides: &[&Ide],
    manifest: &Manifest,
    config: &SyncConfig,
) -> Vec<InstallAction> {
    if !config.plugins.enabled {
        return Vec::new();
    }
    let managed = manifest.ids();
    let mut actions = Vec::new();
    for ide in ides {
        let present = installed(ide, true);
        let capable = capabilities(ide, &present, &config.plugins);
        let bundled = bundled_here(ide, &config.plugins);
        let directory = ide
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for plugin in &manifest.plugins {
            // `present` only covers the config `plugins/` directory, so a
            // bundled plugin looks missing there. Installing over one is a
            // no-op the launcher rejects with "already installed" on every
            // run.
            if present.contains_key(&plugin.id) || bundled.contains(&plugin.id) {
                continue;
            }
            let verdict = compatibility(plugin, ide, &capable, &managed, &config.plugins);
            actions.push(InstallAction {
                ide: directory.clone(),
                plugin: plugin.id.clone(),
                install: verdict.compatible,
                reason: verdict.reason,
            });
        }
    }
    actions
}

/// Installs a plugin through the IDE's own launcher, which handles Marketplace
/// download, signature verification, and dependency resolution.
pub fn install(ide: &Ide, plugin_id: &str, config: &PluginsConfig) -> Result<()> {
    let launcher = config
        .launchers
        .get(&ide.product)
        .cloned()
        .or_else(|| {
            ide.metadata
                .as_ref()
                .map(|metadata| metadata.launcher.clone())
                .filter(|value| !value.is_empty())
        })
        .ok_or_else(|| {
            JbsyncError::configuration(format!(
                "no launcher known for {}; set plugins.launchers.{} in sync.toml",
                ide.product, ide.product
            ))
        })?;

    let status = std::process::Command::new(&launcher)
        .args(["installPlugins", plugin_id])
        .status()
        .map_err(|error| JbsyncError::other(format!("{launcher}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(JbsyncError::other(format!(
            "{launcher} installPlugins {plugin_id} exited with {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ide::ProductMetadata;

    const MODERN: &str = r#"<idea-plugin>
      <id>com.example.tool</id>
      <name>Example Tool</name>
      <version>1.2.3</version>
      <idea-version since-build="243.0" until-build="252.*" />
      <depends>com.intellij.modules.python</depends>
      <depends optional="true">com.intellij.modules.json</depends>
      <incompatible-with>com.example.rival</incompatible-with>
      <content><module name="com.example.tool.extra" /></content>
    </idea-plugin>"#;

    fn ide_with(build: &str, product: &str, modules: &[&str]) -> Ide {
        Ide {
            product: product.to_string(),
            path: std::path::PathBuf::from(format!("/tmp/{product}2026.2")),
            pattern_index: 0,
            metadata: Some(ProductMetadata {
                build_number: build.to_string(),
                modules: modules.iter().map(|value| (*value).to_string()).collect(),
                ..ProductMetadata::default()
            }),
        }
    }

    #[test]
    fn parses_a_modern_descriptor() {
        let plugin = parse_descriptor(MODERN, "fallback").unwrap();
        assert_eq!(plugin.id, "com.example.tool");
        assert_eq!(plugin.version, "1.2.3");
        assert_eq!(plugin.since_build, "243.0");
        assert_eq!(
            plugin.required_dependencies,
            vec!["com.intellij.modules.python"],
            "optional dependencies must not gate installation"
        );
        assert_eq!(plugin.incompatible_with, vec!["com.example.rival"]);
        assert_eq!(plugin.provided_modules, vec!["com.example.tool.extra"]);
        assert!(plugin.modular);
    }

    #[test]
    fn falls_back_to_the_directory_name_without_an_id() {
        let plugin = parse_descriptor("<idea-plugin></idea-plugin>", "some-dir").unwrap();
        assert_eq!(plugin.id, "some-dir");
    }

    #[test]
    fn build_ranges_are_respected_including_wildcards() {
        assert!(build_is_compatible("252.1", "243.0", "252.*").is_ok());
        assert!(build_is_compatible("242.0", "243.0", "").is_err());
        assert!(build_is_compatible("253.1", "243.0", "252.*").is_err());
        assert!(build_is_compatible("252.9999", "", "252.*").is_ok());
    }

    #[test]
    fn a_python_plugin_is_refused_by_a_c_ide() {
        let plugin = parse_descriptor(MODERN, "x").unwrap();
        let clion = ide_with("252.1", "CLion", &["com.intellij.modules.clion"]);
        let verdict = compatibility(
            &plugin,
            &clion,
            &capabilities(&clion, &BTreeMap::new(), &PluginsConfig::default()),
            &BTreeSet::new(),
            &PluginsConfig::default(),
        );
        assert!(!verdict.compatible);
        assert!(verdict.reason.contains("missing required"));
    }

    #[test]
    fn the_same_plugin_is_accepted_by_pycharm() {
        let plugin = parse_descriptor(MODERN, "x").unwrap();
        let pycharm = ide_with("252.1", "PyCharm", &["com.intellij.modules.python"]);
        let verdict = compatibility(
            &plugin,
            &pycharm,
            &capabilities(&pycharm, &BTreeMap::new(), &PluginsConfig::default()),
            &BTreeSet::new(),
            &PluginsConfig::default(),
        );
        assert!(verdict.compatible, "{}", verdict.reason);
    }

    #[test]
    fn a_manual_rule_overrides_the_computed_verdict() {
        let plugin = parse_descriptor(MODERN, "x").unwrap();
        let clion = ide_with("252.1", "CLion", &[]);
        let config = PluginsConfig {
            rule: vec![crate::config::PluginRule {
                id: "com.example.*".to_string(),
                ide: "CLion*".to_string(),
                action: "allow".to_string(),
            }],
            ..PluginsConfig::default()
        };
        let verdict = compatibility(
            &plugin,
            &clion,
            &capabilities(&clion, &BTreeMap::new(), &config),
            &BTreeSet::new(),
            &config,
        );
        assert!(verdict.compatible);
        assert_eq!(verdict.reason, "manual allow rule");
    }

    #[test]
    fn an_only_rule_confines_a_plugin_to_one_product() {
        let plugin = parse_descriptor(MODERN, "x").unwrap();
        let config = PluginsConfig {
            rule: vec![crate::config::PluginRule {
                id: "com.example.tool".to_string(),
                ide: "CLion*".to_string(),
                action: "only".to_string(),
            }],
            ..PluginsConfig::default()
        };

        let clion = ide_with("252.1", "CLion", &["com.intellij.modules.python"]);
        let named = compatibility(
            &plugin,
            &clion,
            &capabilities(&clion, &BTreeMap::new(), &config),
            &BTreeSet::new(),
            &config,
        );
        assert!(named.compatible, "{}", named.reason);

        // The point of `only`: it must also speak for the IDEs it never names.
        let pycharm = ide_with("252.1", "PyCharm", &["com.intellij.modules.python"]);
        let other = compatibility(
            &plugin,
            &pycharm,
            &capabilities(&pycharm, &BTreeMap::new(), &config),
            &BTreeSet::new(),
            &config,
        );
        assert!(!other.compatible);
        assert_eq!(other.reason, "only for CLion*");
    }

    #[test]
    fn an_only_rule_beats_the_compatibility_check() {
        // A CLion-only plugin that CLion would otherwise refuse: the rule is an
        // override, exactly as `allow` is.
        let plugin = parse_descriptor(MODERN, "x").unwrap();
        let clion = ide_with("252.1", "CLion", &["com.intellij.modules.clion"]);
        let config = PluginsConfig {
            rule: vec![crate::config::PluginRule {
                id: "com.example.*".to_string(),
                ide: "CLion*".to_string(),
                action: "only".to_string(),
            }],
            ..PluginsConfig::default()
        };
        let verdict = compatibility(
            &plugin,
            &clion,
            &capabilities(&clion, &BTreeMap::new(), &config),
            &BTreeSet::new(),
            &config,
        );
        assert!(verdict.compatible, "{}", verdict.reason);
    }

    #[test]
    fn an_only_rule_ignores_plugins_it_does_not_name() {
        let plugin = parse_descriptor(MODERN, "x").unwrap();
        let pycharm = ide_with("252.1", "PyCharm", &["com.intellij.modules.python"]);
        let config = PluginsConfig {
            rule: vec![crate::config::PluginRule {
                id: "com.other.plugin".to_string(),
                ide: "CLion*".to_string(),
                action: "only".to_string(),
            }],
            ..PluginsConfig::default()
        };
        let verdict = compatibility(
            &plugin,
            &pycharm,
            &capabilities(&pycharm, &BTreeMap::new(), &config),
            &BTreeSet::new(),
            &config,
        );
        assert!(
            verdict.compatible,
            "a rule about another plugin must not deny this one: {}",
            verdict.reason
        );
    }

    #[test]
    fn an_incompatible_declaration_blocks_installation() {
        let plugin = parse_descriptor(MODERN, "x").unwrap();
        let ide = ide_with(
            "252.1",
            "PyCharm",
            &["com.intellij.modules.python", "com.example.rival"],
        );
        let verdict = compatibility(
            &plugin,
            &ide,
            &capabilities(&ide, &BTreeMap::new(), &PluginsConfig::default()),
            &BTreeSet::new(),
            &PluginsConfig::default(),
        );
        assert!(!verdict.compatible);
        assert!(verdict.reason.contains("incompatibility"));
    }

    #[test]
    fn a_bundled_plugin_is_never_planned_for_installation() {
        // A bundled plugin lives in the app bundle, not the config `plugins/`
        // directory, so it looks missing to `installed`. Planning it anyway
        // makes every sync launch the IDE to be told "already installed".
        let directory = tempfile::tempdir().unwrap();
        let mut pycharm = ide_with("252.1", "PyCharm", &["com.intellij.modules.python"]);
        pycharm.path = directory.path().join("PyCharm2026.2");
        pycharm.metadata.as_mut().unwrap().bundled_plugins = vec!["com.example.tool".to_string()];

        let manifest = Manifest {
            version: 1,
            plugins: vec![parse_descriptor(MODERN, "x").unwrap()],
        };
        let actions = plan_installs(&[&pycharm], &manifest, &SyncConfig::default());
        assert!(
            actions.is_empty(),
            "bundled plugin should need no install: {actions:?}"
        );
    }

    #[test]
    fn a_capability_alone_does_not_suppress_an_install() {
        // `capabilities` answers "can a dependency named X be satisfied", so it
        // holds platform modules, modules other plugins provide, and configured
        // additions. None of those mean the plugin itself is there, and using
        // that set to skip would drop the install without even reporting it.
        let directory = tempfile::tempdir().unwrap();
        let mut pycharm = ide_with("252.1", "PyCharm", &["com.intellij.modules.python"]);
        pycharm.path = directory.path().join("PyCharm2026.2");

        let config = SyncConfig {
            plugins: PluginsConfig {
                capability: vec![crate::config::PluginCapability {
                    ide: "*".to_string(),
                    add: vec!["com.example.tool".to_string()],
                    remove: Vec::new(),
                }],
                ..PluginsConfig::default()
            },
            ..SyncConfig::default()
        };
        let manifest = Manifest {
            version: 1,
            plugins: vec![parse_descriptor(MODERN, "x").unwrap()],
        };
        let actions = plan_installs(&[&pycharm], &manifest, &config);
        assert_eq!(actions.len(), 1, "{actions:?}");
        assert!(actions[0].install, "{}", actions[0].reason);
    }

    #[test]
    fn removing_a_bundled_capability_forces_the_install_back() {
        let directory = tempfile::tempdir().unwrap();
        let mut pycharm = ide_with("252.1", "PyCharm", &["com.intellij.modules.python"]);
        pycharm.path = directory.path().join("PyCharm2026.2");
        pycharm.metadata.as_mut().unwrap().bundled_plugins = vec!["com.example.tool".to_string()];

        let config = SyncConfig {
            plugins: PluginsConfig {
                capability: vec![crate::config::PluginCapability {
                    ide: "PyCharm*".to_string(),
                    add: Vec::new(),
                    remove: vec!["com.example.tool".to_string()],
                }],
                ..PluginsConfig::default()
            },
            ..SyncConfig::default()
        };
        let manifest = Manifest {
            version: 1,
            plugins: vec![parse_descriptor(MODERN, "x").unwrap()],
        };
        let actions = plan_installs(&[&pycharm], &manifest, &config);
        assert_eq!(actions.len(), 1, "an explicit removal overrides bundling");
        assert_eq!(actions[0].plugin, "com.example.tool");
    }

    #[test]
    fn a_genuinely_absent_plugin_is_still_planned() {
        let directory = tempfile::tempdir().unwrap();
        let mut pycharm = ide_with("252.1", "PyCharm", &["com.intellij.modules.python"]);
        pycharm.path = directory.path().join("PyCharm2026.2");

        let manifest = Manifest {
            version: 1,
            plugins: vec![parse_descriptor(MODERN, "x").unwrap()],
        };
        let actions = plan_installs(&[&pycharm], &manifest, &SyncConfig::default());
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].plugin, "com.example.tool");
        assert!(actions[0].install, "{}", actions[0].reason);
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("plugins.json");
        let manifest = Manifest {
            version: 1,
            plugins: vec![parse_descriptor(MODERN, "x").unwrap()],
        };
        manifest.save(&path).unwrap();
        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded.plugins, manifest.plugins);
        assert_eq!(loaded.ids().len(), 1);
    }

    #[test]
    fn a_missing_manifest_is_simply_empty() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = Manifest::load(&directory.path().join("absent.json")).unwrap();
        assert!(manifest.plugins.is_empty());
    }
}
