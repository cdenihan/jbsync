//! Which files in an IDE config directory are worth syncing.
//!
//! The allowlist is *learned* rather than hard-coded.
//!
//! Any IDE that has run JetBrains' bundled Backup and Sync leaves a
//! `settingsSync/` working tree, and that tree is the platform's own answer to
//! "what roams" — it is produced by the same `RoamingType` annotations the
//! platform uses internally. Pooling those trees across every installed IDE
//! gives an authoritative list for free, including for IDEs that never enabled
//! Backup and Sync.
//!
//! This matters more than it sounds. Guessing `options/*.xml` sweeps up files
//! JetBrains deliberately does not roam: `other.xml` is per-machine UI state,
//! and the `llm.*.xml` files hold opaque JSON blobs that cannot be merged
//! meaningfully. A hand-maintained list would drift; a learned one does not.
//!
//! Only when no IDE has such a tree does `BUILTIN_MANIFEST` step in. Either
//! way the result is filtered by the exclusion list, which removes caches,
//! telemetry, machine-local state, and credentials.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use globset::{Glob, GlobSetBuilder};

use crate::{
    config::SyncConfig,
    error::{JbsyncError, Result},
    ide::Ide,
};

/// JetBrains records a removed setting by writing this literal into the
/// `settingsSync` tree instead of deleting the file.
pub const DELETED_TOMBSTONE: &str = "DELETED";

/// Fallback allowlist, used only when no installed IDE has a `settingsSync`
/// tree to learn from.
///
/// This mirrors what the platform actually roams, save for the one documented
/// exception below. It is deliberately *not* `options/*.xml`: JetBrains
/// excludes plenty of files in that directory — `other.xml` holds per-machine
/// UI state, `llm.*.xml` holds opaque JSON blobs — and syncing them produces
/// noise and unresolvable conflicts.
///
/// Every entry here has been checked against real `settingsSync` trees: a file
/// that exists in an IDE's `options/` but never appears in that IDE's
/// `settingsSync/` is one the platform declines to roam, and does not belong
/// here. `find.xml` (search history), `advancedSettings.xml`,
/// `console-font.xml`, `terminal-font.xml` and `textmate.xml` all failed that
/// check across four products and were removed.
///
/// `project.default.xml` is the one deliberate exception, and the reasoning is
/// worth stating because it breaks the rule above. The platform declines to
/// roam it, but not because it holds nothing worth roaming: it holds *Settings
/// for New Projects*, the template every project you create starts from, and
/// those are choices in exactly the sense the rest of this list is about. What
/// it also holds is dialog geometry and an opaque JSON blob, which is reason
/// enough for the platform to skip the file wholesale. jbsync does not have to
/// make that trade, because it prunes per component rather than per file — see
/// the `project.default.xml` rules in `settings::prune`.
const BUILTIN_MANIFEST: &[&str] = &[
    "codestyles/**",
    "colors/**",
    "fileTemplates/**",
    "filetypes/**",
    "inspection/**",
    "keymaps/**",
    "quicklists/**",
    "templates/**",
    "options/colors.scheme.xml",
    "options/csvSettings.xml",
    "options/customization.xml",
    "options/databaseDrivers.xml",
    "options/databaseSettings.xml",
    "options/dataViewsSettings.xml",
    "options/debugger.xml",
    "options/diff.xml",
    "options/editor-font.xml",
    "options/editor.xml",
    "options/file.template.settings.xml",
    "options/filetypes.xml",
    "options/github.xml",
    "options/gitlab.xml",
    "options/grazie_global.xml",
    "options/ide-features-trainer.xml",
    "options/ide.general.xml",
    "options/IntelliLang.xml",
    "options/laf.xml",
    "options/project.default.xml",
    "options/sshConfigs.xml",
    "options/terminal.xml",
    "options/ui.lnf.xml",
    "options/vcs.xml",
];

/// Never sync these. Caches and indices, machine-local layout, credentials,
/// telemetry, and one-shot promotional/onboarding state.
const EXCLUDES: &[&str] = &[
    "**/.DS_Store",
    ".DS_Store",
    "*.db",
    "*.db-*",
    "app-internal-state.db*",
    "bundled_plugins.txt",
    "disabled_plugins.txt",
    "early-access-registry.txt",
    "event-log-metadata/**",
    "extensions/**",
    "grazie/**",
    "idea.key",
    "log/**",
    "plugins/**",
    "splash-subscription-mode.txt",
    "ssl/**",
    "system/**",
    "tasks/**",
    "updatedBrokenPlugins.db",
    "workspace/**",
    // The bundled sync's own state. Syncing it would make the two tools fight.
    "settingsSync/**",
    "options/settingsSync.xml",
    "options/settingsSyncLocal.xml",
    // Machine-local: window geometry, recent paths, per-host trust.
    "options/ide.general.local.xml",
    "options/path.macros.xml",
    "options/proxy.settings.xml",
    "options/recentProjects.xml",
    "options/sshRecentConnections*.xml",
    "options/trusted-paths.xml",
    "options/updates.xml",
    "options/window.layouts.xml",
    "options/window.state.xml",
    // Telemetry and usage counters.
    "options/actionSummary.xml",
    "options/EventLogAllowedList.xml",
    "options/features.usage.statistics.xml",
    "options/usage.statistics.xml",
    // Licensing and quota, which is per-account and not portable.
    "options/AIAssistantQuotaManager2.xml",
    "options/trace_license_storage.xml",
    // One-shot onboarding and promotion flags.
    "options/AIChatContextPopupPromotionState.xml",
    "options/AIOnboardingPromoWindowAdvisor.xml",
    "options/DefaultAgentRollout.xml",
    "options/embeddings-activation.xml",
    "options/InstallJunieHubActionManager.xml",
    "options/ml.chat.completion.survey.xml",
    "options/llm.cloud.completion.xml",
];

fn glob_set(patterns: &[String]) -> Result<globset::GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|error| {
            JbsyncError::configuration(format!("invalid pattern {pattern:?}: {error}"))
        })?);
    }
    builder
        .build()
        .map_err(|error| JbsyncError::configuration(error.to_string()))
}

/// The name this IDE uses for its VM options file (`idea.vmoptions`,
/// `pycharm.vmoptions`, ...), so it can be stored under one shared name.
pub fn vm_options_name(ide: &Ide, config: &SyncConfig) -> String {
    if let Some(name) = config.jetbrains.vmoptions_names.get(&ide.product) {
        return name.clone();
    }
    if let Some(metadata) = &ide.metadata
        && !metadata.vm_options_file.is_empty()
    {
        return metadata.vm_options_file.clone();
    }
    "idea.vmoptions".to_string()
}

/// Maps a product-specific path onto the name used inside the store, so every
/// IDE shares one `idea.vmoptions`.
pub fn canonical_relative_path(relative: &str, ide: &Ide, config: &SyncConfig) -> String {
    if relative == vm_options_name(ide, config) {
        "idea.vmoptions".to_string()
    } else {
        relative.to_string()
    }
}

/// The reverse of [`canonical_relative_path`]: where a stored file belongs
/// inside a particular IDE.
pub fn target_relative_path(relative: &str, ide: &Ide, config: &SyncConfig) -> String {
    if relative == "idea.vmoptions" {
        vm_options_name(ide, config)
    } else {
        relative.to_string()
    }
}

pub fn is_tombstone(contents: &str) -> bool {
    contents.trim() == DELETED_TOMBSTONE
}

/// Relative paths the IDE's own bundled sync considers roamable.
fn settings_sync_manifest(ide: &Ide) -> Vec<String> {
    let root = ide.path.join("settingsSync");
    let mut found = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                // `.git` is the bundled sync's own history, not settings.
                if path.file_name().is_some_and(|name| name == ".git") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            let relative = to_slash(relative);
            // Anything dot-prefixed here belongs to the bundled sync's own
            // bookkeeping (`.metainfo/`, `.gitignore`) or to the operating
            // system (`.DS_Store`) — never to a setting. Filtering by the
            // convention rather than by name keeps the published manifest clean
            // as JetBrains adds more of them.
            if relative.split('/').any(|part| part.starts_with('.')) {
                continue;
            }
            // A tombstone records a deletion; the live file, if any, is stale.
            if std::fs::read_to_string(&path).is_ok_and(|contents| is_tombstone(&contents)) {
                continue;
            }
            found.push(relative);
        }
    }
    found
}

fn to_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The allowlist to apply to every IDE on this machine.
///
/// Any IDE that has run JetBrains' bundled Backup and Sync carries an
/// authoritative list of what the platform roams, produced by the same
/// `RoamingType` annotations the IDE uses internally. Pooling those lists means
/// an IDE that never enabled Backup and Sync — or a freshly installed one —
/// still syncs exactly the right files, learned from its siblings rather than
/// from a list this project would have to maintain by hand.
/// `remembered` is the union every machine has contributed so far, replicated
/// through the store. Including it means a machine where Backup and Sync was
/// never enabled — a fresh laptop, or a product like WebStorm that ships
/// without the tree its siblings have — inherits everything the fleet has
/// learned, instead of dropping to the built-in list.
pub fn learned_manifest(ides: &[&Ide], remembered: &[String]) -> Vec<String> {
    observed_manifest(ides)
        .into_iter()
        .chain(remembered.iter().cloned())
        // The built-in list is a floor, not a last resort. Unioning it always
        // means one IDE with a thin `settingsSync` tree cannot narrow what the
        // rest of them sync.
        .chain(BUILTIN_MANIFEST.iter().map(|value| (*value).to_string()))
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Only what the installed IDEs can prove right now, with nothing remembered or
/// built-in folded in. This is what gets published back to the store, so the
/// record stays evidence the platform itself produced.
pub fn observed_manifest(ides: &[&Ide]) -> Vec<String> {
    let mut pooled: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for ide in ides {
        pooled.extend(settings_sync_manifest(ide));
    }
    pooled.into_iter().collect()
}

/// Every file this IDE should contribute, keyed by its path inside the store.
///
/// `manifest` comes from [`learned_manifest`]; passing an empty slice falls
/// back to the built-in list.
pub fn discover(
    ide: &Ide,
    config: &SyncConfig,
    manifest: &[String],
) -> Result<BTreeMap<String, PathBuf>> {
    let excludes = {
        let mut patterns: Vec<String> = if config.jetbrains.use_default_excludes {
            EXCLUDES.iter().map(|value| (*value).to_string()).collect()
        } else {
            Vec::new()
        };
        patterns.extend(config.jetbrains.exclude.clone());
        glob_set(&patterns)?
    };
    let explicit = glob_set(&config.jetbrains.explicit_include)?;

    let mut patterns: Vec<String> = if manifest.is_empty() {
        BUILTIN_MANIFEST
            .iter()
            .map(|value| (*value).to_string())
            .collect()
    } else {
        manifest.to_vec()
    };
    patterns.extend(config.jetbrains.include.clone().unwrap_or_default());
    let manifest = glob_set(&patterns)?;

    let mut candidates: Vec<String> = walk_relative(&ide.path);
    candidates.sort_unstable();
    candidates.dedup();

    let mut selected = BTreeMap::new();
    for relative in candidates {
        let absolute = ide.path.join(&relative);
        if !absolute.is_file() {
            continue;
        }
        let explicitly_included = explicit.is_match(&relative);
        if !explicitly_included && (excludes.is_match(&relative) || !manifest.is_match(&relative)) {
            continue;
        }
        selected.insert(canonical_relative_path(&relative, ide, config), absolute);
    }
    Ok(selected)
}

/// Top-level files plus the directories the manifest can match, without
/// descending into caches. Keeps discovery cheap on large config directories.
fn walk_relative(root: &Path) -> Vec<String> {
    const SKIP: [&str; 8] = [
        "plugins",
        "workspace",
        "system",
        "log",
        "event-log-metadata",
        "settingsSync",
        "tasks",
        "extensions",
    ];
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if !SKIP.contains(&name.as_ref()) {
                    stack.push(path);
                }
            } else if let Ok(relative) = path.strip_prefix(root) {
                found.push(to_slash(relative));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ide::{Ide, ProductMetadata};

    fn ide_at(path: &Path, product: &str, vm_options: &str) -> Ide {
        Ide {
            product: product.to_string(),
            path: path.to_path_buf(),
            pattern_index: 0,
            metadata: Some(ProductMetadata {
                vm_options_file: vm_options.to_string(),
                ..ProductMetadata::default()
            }),
        }
    }

    #[test]
    fn detects_tombstones() {
        assert!(is_tombstone("DELETED"));
        assert!(is_tombstone("DELETED\n"));
        assert!(!is_tombstone("<application />"));
    }

    #[test]
    fn vm_options_are_stored_under_one_shared_name() {
        let directory = tempfile::tempdir().unwrap();
        let ide = ide_at(directory.path(), "PyCharm", "pycharm.vmoptions");
        let config = SyncConfig::default();
        assert_eq!(
            canonical_relative_path("pycharm.vmoptions", &ide, &config),
            "idea.vmoptions"
        );
        assert_eq!(
            target_relative_path("idea.vmoptions", &ide, &config),
            "pycharm.vmoptions"
        );
    }

    #[test]
    fn discovery_applies_manifest_and_excludes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("options")).unwrap();
        std::fs::create_dir_all(root.join("plugins/some-plugin")).unwrap();
        std::fs::write(root.join("options/editor.xml"), "<application />").unwrap();
        std::fs::write(root.join("options/window.state.xml"), "<application />").unwrap();
        std::fs::write(root.join("plugins/some-plugin/a.jar"), "binary").unwrap();
        std::fs::write(root.join("idea.key"), "secret").unwrap();

        let ide = ide_at(root, "IntelliJIdea", "idea.vmoptions");
        let found = discover(&ide, &SyncConfig::default(), &[]).unwrap();

        assert!(found.contains_key("options/editor.xml"));
        assert!(
            !found.contains_key("options/window.state.xml"),
            "machine-local"
        );
        assert!(!found.keys().any(|key| key.starts_with("plugins/")));
        assert!(!found.contains_key("idea.key"), "credentials never sync");
    }

    /// The platform does not roam this file, so no `settingsSync` tree will
    /// ever contribute it and the built-in floor is the only thing that can.
    /// Losing this entry silently stops *Settings for New Projects* syncing.
    #[test]
    fn settings_for_new_projects_are_in_the_builtin_floor() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("options")).unwrap();
        std::fs::write(root.join("options/project.default.xml"), "<application />").unwrap();

        let ide = ide_at(root, "WebStorm", "webstorm.vmoptions");
        // Not just present in the list: still selected once an IDE with a
        // narrower learned tree has had its say.
        let manifest = learned_manifest(&[&ide], &["options/laf.xml".to_string()]);
        assert!(
            discover(&ide, &SyncConfig::default(), &manifest)
                .unwrap()
                .contains_key("options/project.default.xml")
        );
    }

    #[test]
    fn explicit_include_overrides_the_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("idea.vmoptions"), "-Xmx4g").unwrap();

        let ide = ide_at(root, "IntelliJIdea", "idea.vmoptions");
        let mut config = SyncConfig::default();
        assert!(
            !discover(&ide, &config, &[])
                .unwrap()
                .contains_key("idea.vmoptions"),
            "not in the built-in manifest"
        );

        config.jetbrains.explicit_include = vec!["*.vmoptions".to_string()];
        assert!(
            discover(&ide, &config, &[])
                .unwrap()
                .contains_key("idea.vmoptions")
        );
    }

    #[test]
    fn the_manifest_is_pooled_across_ides() {
        let directory = tempfile::tempdir().unwrap();
        let with_tree = directory.path().join("IntelliJIdea2026.2");
        let without_tree = directory.path().join("WebStorm2026.2");
        std::fs::create_dir_all(with_tree.join("settingsSync/options")).unwrap();
        std::fs::create_dir_all(without_tree.join("options")).unwrap();
        std::fs::write(
            with_tree.join("settingsSync/options/laf.xml"),
            "<application />",
        )
        .unwrap();

        let first = ide_at(&with_tree, "IntelliJIdea", "idea.vmoptions");
        let second = ide_at(&without_tree, "WebStorm", "webstorm.vmoptions");
        let manifest = learned_manifest(&[&first, &second], &[]);
        assert!(manifest.contains(&"options/laf.xml".to_string()));
        assert_eq!(
            observed_manifest(&[&first, &second]),
            vec!["options/laf.xml".to_string()],
            "only what an IDE proved is published back to the store"
        );

        // The IDE with no tree of its own still gets the pooled allowlist, and
        // nothing outside it — `other.xml` is not roamed by JetBrains.
        std::fs::write(without_tree.join("options/laf.xml"), "<application />").unwrap();
        std::fs::write(without_tree.join("options/other.xml"), "<application />").unwrap();
        let found = discover(&second, &SyncConfig::default(), &manifest).unwrap();
        assert!(found.contains_key("options/laf.xml"));
        assert!(!found.contains_key("options/other.xml"));
    }

    #[test]
    fn without_any_learned_tree_the_builtin_list_applies() {
        let directory = tempfile::tempdir().unwrap();
        let ide = ide_at(directory.path(), "WebStorm", "webstorm.vmoptions");
        let manifest = learned_manifest(&[&ide], &[]);
        assert!(manifest.contains(&"options/editor.xml".to_string()));
        assert!(
            !manifest.iter().any(|entry| entry == "options/other.xml"),
            "per-machine UI state must never be in the fallback list"
        );
    }

    /// The point of remembering: a machine with no `settingsSync` tree of its
    /// own still syncs what another machine proved roams.
    #[test]
    fn a_remembered_entry_widens_the_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let ide = ide_at(directory.path(), "WebStorm", "webstorm.vmoptions");
        let remembered = vec!["options/unusual.xml".to_string()];
        let manifest = learned_manifest(&[&ide], &remembered);
        assert!(manifest.contains(&"options/unusual.xml".to_string()));
        // Remembering must not narrow the floor the built-in list provides.
        assert!(manifest.contains(&"options/editor.xml".to_string()));
        assert!(
            observed_manifest(&[&ide]).is_empty(),
            "this machine observed nothing, so it publishes nothing"
        );
    }

    #[test]
    fn settings_sync_tombstones_are_not_adopted() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("settingsSync/options")).unwrap();
        std::fs::write(root.join("settingsSync/options/terminal.xml"), "DELETED").unwrap();
        std::fs::write(root.join("settingsSync/options/laf.xml"), "<application />").unwrap();

        let ide = ide_at(root, "IntelliJIdea", "idea.vmoptions");
        let manifest = settings_sync_manifest(&ide);
        assert!(manifest.contains(&"options/laf.xml".to_string()));
        assert!(!manifest.contains(&"options/terminal.xml".to_string()));
    }
}
