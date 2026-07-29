//! Discovery of installed `JetBrains` IDE config directories and their
//! `product-info.json` installation metadata.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use globset::Glob;
use serde::Deserialize;
use walkdir::WalkDir;

use crate::error::{JbsyncError, Result};

#[derive(Debug, Clone)]
pub struct Ide {
    pub product: String,
    pub path: PathBuf,
    pub pattern_index: usize,
    pub metadata: Option<ProductMetadata>,
}

/// The files the IntelliJ platform itself accepts as proof that a directory is
/// a real config directory rather than one an installer just created.
///
/// Mirrors `ConfigImportHelper#OPTIONS` (copied as `InitialConfigImportState.OPTIONS`
/// in intellij-community). An installer such as Toolbox lays down an `options/`
/// full of factory defaults before the IDE has ever run; none of these three
/// appear until the IDE actually starts.
const LAUNCHED_MARKERS: [&str; 3] = [
    "options/other.xml",
    "options/ide.general.xml",
    "options/options.xml",
];

impl Ide {
    /// Whether this IDE has ever been started.
    ///
    /// A never-launched IDE must not take part in a sync. Its `options/` holds
    /// the product's factory defaults, which would otherwise be harvested into
    /// the store and pushed onto machines where the user really did choose
    /// something; and anything written into it races the first-run import
    /// wizard, which may discard or overwrite the lot.
    #[must_use]
    pub fn has_been_launched(&self) -> bool {
        LAUNCHED_MARKERS
            .iter()
            .any(|marker| self.path.join(marker).is_file())
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProductMetadata {
    pub data_directory_name: String,
    pub name: String,
    pub product_code: String,
    pub build_number: String,
    pub version: String,
    pub bundled_plugins: Vec<String>,
    pub modules: Vec<String>,
    pub launcher: String,
    pub vm_options_file: String,
    pub installation_root: PathBuf,
    pub product_info: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
struct RawProductInfo {
    name: Option<String>,
    #[serde(rename = "productCode")]
    product_code: Option<String>,
    #[serde(rename = "dataDirectoryName")]
    data_directory_name: Option<String>,
    #[serde(rename = "buildNumber")]
    build_number: Option<String>,
    version: Option<String>,
    #[serde(default, rename = "bundledPlugins")]
    bundled_plugins: Vec<String>,
    #[serde(default)]
    modules: Vec<String>,
    #[serde(default)]
    launch: Vec<RawLaunch>,
}

#[derive(Debug, Default, Deserialize)]
struct RawLaunch {
    os: Option<String>,
    #[serde(rename = "launcherPath")]
    launcher_path: Option<String>,
    #[serde(rename = "vmOptionsFilePath")]
    vm_options_file_path: Option<String>,
}

fn current_os_tag() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Linux"
    }
}

fn installation_root(info_path: &Path) -> PathBuf {
    for parent in info_path.ancestors().skip(1) {
        if parent.join("plugins").is_dir() {
            return parent.to_path_buf();
        }
    }
    info_path
        .parent()
        .map_or_else(|| info_path.to_path_buf(), Path::to_path_buf)
}

pub fn resolve_product_metadata(path: &Path) -> Option<ProductMetadata> {
    let contents = std::fs::read_to_string(path).ok()?;
    let info: RawProductInfo = serde_json::from_str(&contents).ok()?;
    let data_directory_name = info.data_directory_name?;
    let root = installation_root(path);
    let os_tag = current_os_tag();
    let selected = info
        .launch
        .iter()
        .find(|entry| entry.os.as_deref().is_none_or(|os| os == os_tag))
        .or_else(|| info.launch.first());
    let launcher = selected
        .and_then(|entry| entry.launcher_path.as_deref())
        .map(|launcher| {
            path.parent()
                .unwrap_or(path)
                .join(launcher)
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    let vm_options_file = selected
        .and_then(|entry| entry.vm_options_file_path.as_deref())
        .and_then(|value| Path::new(value).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(ProductMetadata {
        name: info
            .name
            .clone()
            .unwrap_or_else(|| data_directory_name.clone()),
        product_code: info.product_code.unwrap_or_default(),
        build_number: info.build_number.unwrap_or_default(),
        version: info.version.unwrap_or_default(),
        bundled_plugins: info.bundled_plugins,
        modules: info.modules,
        launcher,
        vm_options_file,
        installation_root: root,
        product_info: path.to_path_buf(),
        data_directory_name,
    })
}

pub fn discover_product_metadata(install_roots: &[PathBuf]) -> HashMap<String, ProductMetadata> {
    let mut found: HashMap<String, ProductMetadata> = HashMap::new();
    for root in install_roots {
        if !root.exists() {
            continue;
        }
        let candidates: Vec<PathBuf> = if root
            .file_name()
            .is_some_and(|name| name == "product-info.json")
        {
            vec![root.clone()]
        } else {
            WalkDir::new(root)
                .into_iter()
                .filter_map(std::result::Result::ok)
                .filter(|entry| entry.file_name() == "product-info.json")
                .map(|entry| entry.path().to_path_buf())
                .collect()
        };
        for path in candidates {
            let Some(metadata) = resolve_product_metadata(&path) else {
                continue;
            };
            let name = metadata.data_directory_name.clone();
            let better = found.get(&name).is_none_or(|current| {
                digit_runs(&metadata.build_number) > digit_runs(&current.build_number)
            });
            if better {
                found.insert(name, metadata);
            }
        }
    }
    found
}

/// Digit runs in `text` as a tuple-comparable key, e.g. "2024.3.1" -> [2024, 3, 1].
/// Falls back to `[0]` so IDEs/builds without digits still sort deterministically.
pub fn digit_runs(text: &str) -> Vec<u64> {
    let mut result = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            result.push(current.parse().unwrap_or(0));
            current.clear();
        }
    }
    if !current.is_empty() {
        result.push(current.parse().unwrap_or(0));
    }
    if result.is_empty() {
        result.push(0);
    }
    result
}

fn leading_letters(name: &str) -> String {
    name.chars().take_while(char::is_ascii_alphabetic).collect()
}

pub fn product_name(dirname: &str) -> String {
    let letters = leading_letters(dirname);
    if letters.is_empty() {
        dirname.to_string()
    } else {
        letters
    }
}

pub fn discover_ides<S: std::hash::BuildHasher>(
    root: &Path,
    patterns: &[String],
    product_metadata: &HashMap<String, ProductMetadata, S>,
) -> Result<Vec<Ide>> {
    let mut found: Vec<Ide> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let entries: Vec<PathBuf> = match std::fs::read_dir(root) {
        Ok(read_dir) => read_dir
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect(),
        Err(_) => Vec::new(),
    };
    for (index, pattern) in patterns.iter().enumerate() {
        let glob = Glob::new(pattern)
            .map_err(|error| {
                JbsyncError::configuration(format!("invalid IDE pattern {pattern:?}: {error}"))
            })?
            .compile_matcher();
        let mut candidates: Vec<&PathBuf> = entries
            .iter()
            .filter(|path| path.file_name().is_some_and(|name| glob.is_match(name)))
            .collect();
        candidates.sort_by_key(|path| std::cmp::Reverse(digit_runs(&path_file_name(path))));
        for path in candidates {
            if seen.insert(path.clone()) {
                let name = path_file_name(path);
                found.push(Ide {
                    product: product_name(&name),
                    path: path.clone(),
                    pattern_index: index,
                    metadata: product_metadata.get(&name).cloned(),
                });
            }
        }
    }
    found.sort_by_key(|ide| {
        (
            ide.pattern_index,
            std::cmp::Reverse(digit_runs(&path_file_name(&ide.path))),
        )
    });
    Ok(found)
}

fn path_file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn ide_matches_selector(ide: &Ide, selector: &str) -> bool {
    let expanded = shellexpand_home(selector);
    let candidate = Path::new(&expanded);
    if candidate.is_absolute() {
        return candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.to_path_buf())
            == ide.path.canonicalize().unwrap_or_else(|_| ide.path.clone());
    }
    let Ok(glob) = Glob::new(selector) else {
        return false;
    };
    let glob = glob.compile_matcher();
    let mut names = vec![path_file_name(&ide.path), ide.product.clone()];
    if let Some(metadata) = &ide.metadata {
        names.push(metadata.name.clone());
        names.push(metadata.product_code.clone());
        names.push(metadata.data_directory_name.clone());
    }
    names
        .iter()
        .any(|name| !name.is_empty() && glob.is_match(name))
}

fn shellexpand_home(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    value.to_string()
}

pub fn select_ide<'a>(ides: &'a [Ide], selector: &str, role: &'static str) -> Result<&'a Ide> {
    ides.iter()
        .find(|ide| ide_matches_selector(ide, selector))
        .ok_or_else(|| JbsyncError::NoMatchingIde {
            role,
            selector: selector.to_string(),
            available: ides
                .iter()
                .map(|ide| path_file_name(&ide.path))
                .collect::<Vec<_>>()
                .join(", "),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_runs_extracts_tuple() {
        assert_eq!(digit_runs("2024.3.1"), vec![2024, 3, 1]);
        assert_eq!(digit_runs("no-digits"), vec![0]);
    }

    #[test]
    fn product_name_strips_trailing_digits() {
        assert_eq!(product_name("IntelliJIdea2024.3"), "IntelliJIdea");
        assert_eq!(product_name("PyCharm2023.1"), "PyCharm");
    }

    #[test]
    fn digit_runs_orders_newer_versions_first() {
        let mut versions = vec![
            "IntelliJIdea2023.1",
            "IntelliJIdea2024.3",
            "IntelliJIdea2024.1",
        ];
        versions.sort_by_key(|name| std::cmp::Reverse(digit_runs(name)));
        assert_eq!(
            versions,
            vec![
                "IntelliJIdea2024.3",
                "IntelliJIdea2024.1",
                "IntelliJIdea2023.1"
            ]
        );
    }
}
