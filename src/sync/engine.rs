//! Orchestration: what actually happens when you run `jbsync sync`.
//!
//! A sync is two reconciliations, both using the same three-way merge:
//!
//! 1. **Store against the other machines.** Whatever the backend reports as
//!    incoming is merged into the local copy of the store, and the result is
//!    recorded as reconciled.
//! 2. **Each IDE against the store.** Every IDE contributes its settings and
//!    receives everyone else's. Doing the IDEs one after another means two IDEs
//!    that changed different settings both get their way, and two IDEs that
//!    changed the *same* setting produce exactly one reported conflict.
//!
//! The two directions are deliberately asymmetric. What goes *into* the store
//! is pruned down to genuine user choices. What comes *out* is applied to the
//! IDE's real file as individual settings, so everything pruning removed — and
//! everything the IDE keeps that jbsync does not manage — stays untouched.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::{
    merge::{self, ConflictPolicy, FileMerge},
    report::{FileReport, IdeReport, SyncReport},
};
use crate::{
    backend::{Backend, Published, git::GitBackend},
    config::{self, LocalConfig, MachineConfig, SyncConfig},
    error::{JbsyncError, Result},
    ide::{self, Ide},
    paths::Paths,
    plugins,
    progress::Progress,
    settings::{defaults, manifest, prune, roamable},
    xml::{dom, project},
};

/// Settings live under this prefix inside the store, keeping them clearly
/// separate from jbsync's own files (`sync.toml`, `machines/`, `plugins.json`).
const SHARED: &str = "shared";

/// Timestamped backup runs kept on disk. Every sync that overwrites anything
/// creates one, so without a cap they accumulate for the life of the install.
/// Ten is enough to walk back a bad run and small enough to stay unnoticeable.
const BACKUP_RUNS_KEPT: usize = 10;

/// How many times reconciliation may repeat before it is considered stuck.
/// Two passes is the normal case and the third only confirms convergence.
const MAX_PASSES: usize = 4;

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub dry_run: bool,
    pub policy: ConflictPolicy,
    pub message: String,
    /// Restrict the run to IDEs matching these selectors.
    pub only: Vec<String>,
    /// Skip writing merged settings back into the IDEs.
    pub collect_only: bool,
    /// Actually install missing plugins from Marketplace, rather than only
    /// reporting them. On by default; `--no-install-plugins` turns it off.
    pub install_plugins: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            policy: ConflictPolicy::default(),
            message: "Sync JetBrains settings".to_string(),
            only: Vec::new(),
            collect_only: false,
            install_plugins: true,
        }
    }
}

/// What reconciling plugins produced: lines to show, and whether any of it
/// actually landed in the store.
#[derive(Debug, Default)]
struct PluginOutcome {
    lines: Vec<String>,
    manifest_changed: bool,
}

pub struct Engine {
    pub paths: Paths,
    pub local: LocalConfig,
    pub sync_config: SyncConfig,
    pub machine: String,
    pub machine_config: MachineConfig,
    pub ides: Vec<Ide>,
    backend: Box<dyn Backend>,
}

impl Engine {
    pub fn open(config_dir: Option<PathBuf>) -> Result<Self> {
        let paths = Paths::discover(config_dir)?;
        let local = LocalConfig::load(&paths.local_config_path())?;
        let workdir = paths.data_dir(&local.repo);

        if local.repo.backend != "git" {
            return Err(JbsyncError::configuration(format!(
                "unknown backend {:?}. Only \"git\" is implemented; see src/backend.rs \
                 for the contract a new backend must satisfy.",
                local.repo.backend
            )));
        }
        let backend = Box::new(GitBackend::new(
            workdir.clone(),
            local.repo.remote.clone(),
            local.repo.branch.clone(),
        ));
        backend.initialize()?;

        let mut sync_config = SyncConfig::load(&workdir.join("sync.toml"))?;
        let machine = config::machine_id(local.machine.id.as_deref());
        let machine_config =
            MachineConfig::load(&workdir.join(format!("machines/{machine}.toml")))?;
        // This machine's exclusions are folded in here so that discovery has a
        // single list to consult, matching how `prune_rules` merges the two
        // layers of omit rules.
        sync_config
            .jetbrains
            .exclude
            .extend(machine_config.jetbrains.exclude.clone());

        let root = local.jetbrains.root.as_deref().map_or_else(
            crate::platform::default_jetbrains_root,
            |value| {
                if value == "auto" {
                    crate::platform::default_jetbrains_root()
                } else {
                    PathBuf::from(value)
                }
            },
        );
        let install_roots: Vec<PathBuf> = if local.jetbrains.install_roots.is_empty() {
            crate::platform::default_install_roots()
        } else {
            local
                .jetbrains
                .install_roots
                .iter()
                .map(PathBuf::from)
                .collect()
        };
        let metadata = ide::discover_product_metadata(&install_roots);
        let ides = ide::discover_ides(&root, &sync_config.jetbrains.ides, &metadata)?;

        Ok(Self {
            paths,
            local,
            sync_config,
            machine,
            machine_config,
            ides,
            backend,
        })
    }

    pub fn store_root(&self) -> &Path {
        self.backend.workdir()
    }

    fn shared_path(&self, relative: &str) -> PathBuf {
        self.store_root().join(SHARED).join(relative)
    }

    fn base_path(&self, ide: &Ide, relative: &str) -> PathBuf {
        self.paths
            .base_dir()
            .join(directory_name(ide))
            .join(relative)
    }

    fn selected_ides(&self, only: &[String]) -> Vec<&Ide> {
        if only.is_empty() {
            return self.ides.iter().collect();
        }
        self.ides
            .iter()
            .filter(|candidate| {
                only.iter()
                    .any(|selector| ide::ide_matches_selector(candidate, selector))
            })
            .collect()
    }

    /// The rules that decide what is a user choice, from both config layers.
    fn prune_rules(&self) -> Vec<crate::config::XmlOmitRule> {
        let mut rules = self.sync_config.xml.omit.clone();
        rules.extend(self.machine_config.xml.omit.clone());
        rules
    }

    /// Reduces an IDE's file to what belongs in the store: canonical form, with
    /// everything that is not a user choice removed.
    fn store_view(
        &self,
        relative: &str,
        raw: &[u8],
        defaults: Option<&defaults::ProductDefaults>,
    ) -> (Option<Vec<u8>>, Vec<prune::Removal>) {
        let Ok(text) = std::str::from_utf8(raw) else {
            return (Some(raw.to_vec()), Vec::new());
        };
        if roamable::is_tombstone(text) {
            return (None, Vec::new());
        }
        let Ok(mut document) = dom::parse(text) else {
            return (Some(raw.to_vec()), Vec::new());
        };
        let outcome = prune::prune_document(
            relative,
            &mut document,
            &self.prune_rules(),
            self.sync_config.xml.use_defaults,
            defaults.filter(|_| self.sync_config.xml.use_defaults),
        );

        if outcome.is_empty {
            return (None, outcome.removed);
        }
        (
            Some(dom::serialize(&document).into_bytes()),
            outcome.removed,
        )
    }

    pub fn sync(&mut self, options: &SyncOptions) -> Result<SyncReport> {
        self.sync_reporting(options, &mut Progress::silent())
    }

    /// As [`Engine::sync`], but showing what it is working on. A sync spends
    /// most of its time waiting on `git` and on walking IDE directories, and
    /// saying nothing for that long is indistinguishable from a hang.
    pub fn sync_reporting(
        &mut self,
        options: &SyncOptions,
        progress: &mut Progress,
    ) -> Result<SyncReport> {
        // Held for the whole run. A sync reads the IDEs, rewrites the store and
        // writes back; two overlapping runs could interleave those steps and
        // publish a half-merged result.
        let Some(_lock) = self.paths.try_lock()? else {
            return Err(JbsyncError::other(
                "another jbsync run is in progress; wait for it to finish",
            ));
        };

        let mut report = SyncReport {
            machine: self.machine.clone(),
            backend: self.backend.describe(),
            dry_run: options.dry_run,
            ..SyncReport::default()
        };

        let mut staging = Staging::new(options.dry_run);
        progress.step("checking for changes from other machines...");
        report.from_remote = self.take_incoming(options, &mut staging)?;

        // IDEs are reconciled in sequence, so an IDE handled early cannot know
        // about settings a later one is about to contribute. Repeating until
        // nothing moves lets those reach every IDE in this run rather than the
        // next one. Each pass records what it agreed on, so a pass never
        // re-reports the previous pass's work; two passes is the normal case
        // and the third only confirms convergence.
        for pass in 0..MAX_PASSES {
            let outcome = self.reconcile_ides(options, &mut staging, progress, pass)?;
            let progressed = outcome.iter().any(|ide| !ide.is_empty());
            absorb(&mut report.ides, outcome);
            if !progressed {
                break;
            }
            // A debug_assert here meant release builds stopped after the last
            // pass and said nothing, so a genuinely unsettled sync looked
            // complete. Nothing written is wrong -- every pass is a valid merge
            // -- but the run is unfinished, and only the report can say so.
            if pass + 1 == MAX_PASSES {
                report.warnings.push(format!(
                    "reconciliation had not settled after {MAX_PASSES} passes;                      some settings may need another `jbsync sync`.                      Please report this at https://github.com/cdenihan/jbsync/issues"
                ));
            }
        }
        progress.step("checking plugins...");
        let plugins = self.reconcile_plugins(options)?;
        report.plugins = plugins.lines;
        report.manifest_changed = plugins.manifest_changed;

        if !options.dry_run {
            // Best-effort: failing to tidy old backups must never fail a sync
            // that already succeeded.
            let _ = prune_backups(&self.paths.backups_dir(), BACKUP_RUNS_KEPT);
        }

        if !options.dry_run && report.changes_the_store() {
            progress.step("committing...");
            let published = self.backend.publish(&options.message)?;
            report.published = match published {
                Published::Committed { files, cursor } => Some(format!(
                    "{files} file(s) at {}{}",
                    short(&cursor),
                    // Without a remote the commit never leaves this machine,
                    // and saying "published" would imply otherwise.
                    if self.local.repo.remote.is_some() {
                        ""
                    } else {
                        " (local store only - `jbsync repo set <url>` to share)"
                    }
                )),
                Published::Unchanged => None,
            };
        }
        progress.clear();
        Ok(report)
    }

    /// Records installed plugins in the store and reports which ones are
    /// missing elsewhere, installing them unless `--no-install-plugins` says
    /// otherwise. Installing launches the IDE binary and downloads from
    /// Marketplace, so a sync can be slow the first time a plugin appears.
    fn reconcile_plugins(&self, options: &SyncOptions) -> Result<PluginOutcome> {
        if !self.sync_config.plugins.enabled {
            return Ok(PluginOutcome::default());
        }
        let selected = self.selected_ides(&options.only);
        let manifest_path = self.store_root().join("plugins.json");
        let stored = plugins::Manifest::load(&manifest_path)?;
        let observed = plugins::collect(&selected, &self.sync_config);
        // Kept to answer "did this run change the manifest", which decides
        // whether there is anything to commit. The merge below consumes the
        // original.
        let before = stored.plugins.clone();

        // Union: a plugin another machine published stays in the manifest even
        // though it is not installed here.
        let mut merged: std::collections::BTreeMap<String, plugins::Plugin> = stored
            .plugins
            .into_iter()
            .map(|plugin| (plugin.id.clone(), plugin))
            .collect();
        let mut lines = Vec::new();
        for plugin in observed.plugins {
            let entry = merged.entry(plugin.id.clone());
            match entry {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    lines.push(format!("+ {} ({})", plugin.id, plugin.version));
                    slot.insert(plugin);
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    for product in &plugin.source_products {
                        if !slot.get().source_products.contains(product) {
                            slot.get_mut().source_products.push(product.clone());
                        }
                    }
                    slot.get_mut().version = plugin.version;
                }
            }
        }
        let manifest = plugins::Manifest {
            version: 1,
            plugins: merged.into_values().collect(),
        };
        // Rewriting an identical manifest would churn the file's mtime on
        // every run for nothing.
        let manifest_changed = manifest.plugins != before;
        if !options.dry_run && manifest_changed {
            manifest.save(&manifest_path)?;
        }

        for action in plugins::plan_installs(&selected, &manifest, &self.sync_config) {
            if action.install {
                lines.push(format!("{}: install {}", action.ide, action.plugin));
                if !options.dry_run && options.install_plugins {
                    plugins::install(
                        self.ides
                            .iter()
                            .find(|ide| directory_name(ide) == action.ide)
                            .ok_or_else(|| JbsyncError::other("IDE disappeared mid-sync"))?,
                        &action.plugin,
                        &self.sync_config.plugins,
                    )?;
                }
            } else {
                lines.push(format!(
                    "{}: skip {} ({})",
                    action.ide, action.plugin, action.reason
                ));
            }
        }
        Ok(PluginOutcome {
            lines,
            manifest_changed,
        })
    }

    /// Turns off JetBrains' bundled Backup and Sync.
    ///
    /// Both tools write the same files, so leaving both enabled means they
    /// overwrite each other's work in a loop.
    pub fn disable_builtin_sync(&self, dry_run: bool) -> Result<Vec<String>> {
        const SWITCHES: [(&str, &str, &str); 2] = [
            ("settingsSync.xml", "SettingsSyncSettings", "syncEnabled"),
            (
                "settingsSyncLocal.xml",
                "SettingsSyncLocalSettings",
                "crossIdeSyncEnabled",
            ),
        ];
        let mut actions = Vec::new();
        for ide in &self.ides {
            for (file, component_name, option_name) in SWITCHES {
                let path = ide.path.join("options").join(file);
                let mut root = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| dom::parse(&text).ok())
                    .unwrap_or_else(|| dom::Element::new("application"));

                let address =
                    format!("component[name={component_name}]/option[name={option_name}]/@value");
                if project::project(&root).get(&address).map(String::as_str) == Some("false") {
                    continue;
                }
                actions.push(format!(
                    "{}: disable JetBrains Backup and Sync ({option_name})",
                    directory_name(ide)
                ));
                if dry_run {
                    continue;
                }
                // Build the donor so `set_leaf` has ancestors to copy shells from.
                let mut donor = dom::Element::new("application");
                let mut component = dom::Element::new("component");
                component
                    .attributes
                    .insert("name".to_string(), component_name.to_string());
                let mut option = dom::Element::new("option");
                option
                    .attributes
                    .insert("name".to_string(), option_name.to_string());
                option
                    .attributes
                    .insert("value".to_string(), "false".to_string());
                component.children.push(option);
                donor.children.push(component);

                project::set_leaf(&mut root, &donor, &address, "false");
                write_or_remove(&path, Some(dom::serialize(&root).as_bytes()))?;
            }
        }
        Ok(actions)
    }

    /// Merges whatever other machines have published into the local store copy.
    fn take_incoming(
        &self,
        options: &SyncOptions,
        staging: &mut Staging,
    ) -> Result<Vec<FileReport>> {
        let Some(incoming) = self.backend.incoming()? else {
            return Ok(Vec::new());
        };
        let mut reports = Vec::new();
        let mut paths: BTreeSet<String> = incoming.base.keys().cloned().collect();
        paths.extend(incoming.remote.keys().cloned());
        paths.extend(staging.files_under(self.store_root()));

        for relative in paths {
            if crate::backend::git::is_internal_path(&relative) {
                continue;
            }
            let absolute = self.store_root().join(&relative);
            let local = staging.read(&absolute);
            let base = incoming.base.get(&relative).cloned();
            let remote = incoming.remote.get(&relative).cloned();

            // Only files under `shared/` are IDE settings; the rest is jbsync's
            // own configuration and merges as opaque text.
            let merged = if relative.starts_with(&format!("{SHARED}/")) {
                merge::merge_file(
                    base.as_deref(),
                    local.as_deref(),
                    remote.as_deref(),
                    options.policy,
                )
            } else {
                merge::merge_opaque(
                    base.as_deref(),
                    local.as_deref(),
                    remote.as_deref(),
                    options.policy,
                )
            };
            if merged.is_noop() {
                continue;
            }
            staging.write(&absolute, merged.content.as_deref())?;
            reports.push(FileReport {
                path: display_path(&relative),
                incoming: merged.incoming,
                outgoing: merged.outgoing,
                conflicts: merged.conflicts,
                pruned: Vec::new(),
            });
        }

        if !options.dry_run {
            self.backend.reconcile(&incoming.cursor, &options.message)?;
        }
        Ok(reports)
    }

    /// Reconciles each IDE with the store, in turn.
    fn reconcile_ides(
        &self,
        options: &SyncOptions,
        staging: &mut Staging,
        progress: &mut Progress,
        pass: usize,
    ) -> Result<Vec<IdeReport>> {
        let mut reports = Vec::new();
        let stamp = timestamp();

        // Learn the allowlist from every launched IDE, not just the selected
        // ones, so `--ide WebStorm` still benefits from IntelliJ's knowledge,
        // and fold in what other machines have already taught the store.
        let all: Vec<&Ide> = self
            .ides
            .iter()
            .filter(|ide| ide.has_been_launched())
            .collect();
        let remembered = self.remembered_manifest(staging)?;
        let manifest = roamable::learned_manifest(&all, &remembered);
        self.remember_manifest(&all, staging)?;

        // Capture factory defaults before reconciling, so a product installed
        // but not yet opened teaches this very run what its defaults are.
        for ide in &self.ides {
            if !ide.has_been_launched() {
                self.capture_defaults(ide, &manifest, staging)?;
            }
        }

        let selected = self.selected_ides(&options.only);
        let total = selected.len();
        for (index, ide) in selected.into_iter().enumerate() {
            progress.step(&format!(
                "pass {} - {} ({}/{total})...",
                pass + 1,
                directory_name(ide),
                index + 1
            ));
            // An IDE that has never been started has only factory defaults to
            // offer, and the first-run import wizard may discard anything
            // written into it. Report it rather than silently skipping.
            if !ide.has_been_launched() {
                reports.push(IdeReport {
                    directory: directory_name(ide),
                    display_name: ide
                        .metadata
                        .as_ref()
                        .map(|metadata| metadata.name.clone())
                        .unwrap_or_default(),
                    files: Vec::new(),
                    skipped: Some(
                        "never launched - recorded its factory defaults; start it once for it to \
                         take part"
                            .to_string(),
                    ),
                });
                continue;
            }
            let discovered = roamable::discover(ide, &self.sync_config, &manifest)?;
            let mut relatives: BTreeSet<String> = discovered.keys().cloned().collect();
            relatives.extend(staging.files_under(&self.store_root().join(SHARED)));

            let product_defaults = self.defaults_for(ide, staging)?;
            let target = FileTarget {
                ide,
                discovered: &discovered,
                options,
                stamp: &stamp,
                defaults: product_defaults.as_ref(),
            };
            let mut files = Vec::new();
            for relative in relatives {
                let report = self.reconcile_file(&target, &relative, staging)?;
                if let Some(report) = report {
                    files.push(report);
                }
            }
            reports.push(IdeReport {
                directory: directory_name(ide),
                display_name: ide
                    .metadata
                    .as_ref()
                    .map(|metadata| metadata.name.clone())
                    .unwrap_or_default(),
                files,
                skipped: None,
            });
        }
        Ok(reports)
    }

    /// Records a never-launched IDE's factory defaults into the store.
    ///
    /// This is the only moment the defaults are observable: once the IDE runs,
    /// its files mix defaults with whatever the user then changed.
    fn capture_defaults(
        &self,
        ide: &Ide,
        manifest: &[String],
        staging: &mut Staging,
    ) -> Result<()> {
        let path = self
            .store_root()
            .join(defaults::relative_path(&ide.product));
        let mut recorded = match staging.read(&path) {
            Some(raw) => defaults::ProductDefaults::parse(
                &String::from_utf8(raw)
                    .map_err(|_| JbsyncError::configuration("defaults file is not valid UTF-8"))?,
            )?,
            None => defaults::ProductDefaults::new(&ide.product, build_of(ide)),
        };

        let mut changed = false;
        for (relative, source) in roamable::discover(ide, &self.sync_config, manifest)? {
            let Ok(raw) = std::fs::read(&source) else {
                continue;
            };
            let Ok(text) = String::from_utf8(raw) else {
                continue;
            };
            let Ok(document) = dom::parse(&text) else {
                continue;
            };
            changed |=
                recorded.record(&relative, project::project(&document).into_iter().collect());
        }
        if changed {
            recorded.build = build_of(ide).to_string();
            staging.write(&path, Some(recorded.encode()?.as_bytes()))?;
        }
        Ok(())
    }

    /// The factory defaults recorded for this IDE's product, if any.
    fn defaults_for(
        &self,
        ide: &Ide,
        staging: &Staging,
    ) -> Result<Option<defaults::ProductDefaults>> {
        let path = self
            .store_root()
            .join(defaults::relative_path(&ide.product));
        let Some(raw) = staging.read(&path) else {
            return Ok(None);
        };
        let text = String::from_utf8(raw)
            .map_err(|_| JbsyncError::configuration("defaults file is not valid UTF-8"))?;
        Ok(Some(defaults::ProductDefaults::parse(&text)?))
    }

    /// The allowlist other machines have already contributed to the store.
    fn remembered_manifest(&self, staging: &Staging) -> Result<Vec<String>> {
        let path = self.store_root().join(manifest::FILE_NAME);
        let Some(raw) = staging.read(&path) else {
            return Ok(Vec::new());
        };
        let text = String::from_utf8(raw)
            .map_err(|_| JbsyncError::configuration("manifest.toml is not valid UTF-8"))?;
        Ok(toml::from_str::<manifest::StoredManifest>(&text)
            .map_err(|error| JbsyncError::configuration(format!("manifest.toml: {error}")))?
            .roamable)
    }

    /// Publishes what this machine's IDEs can prove about roamability, so a
    /// machine that never ran Backup and Sync inherits it.
    fn remember_manifest(&self, ides: &[&Ide], staging: &mut Staging) -> Result<()> {
        let path = self.store_root().join(manifest::FILE_NAME);
        let mut stored = match staging.read(&path) {
            Some(raw) => {
                let text = String::from_utf8(raw)
                    .map_err(|_| JbsyncError::configuration("manifest.toml is not valid UTF-8"))?;
                toml::from_str(&text).map_err(|error| {
                    JbsyncError::configuration(format!("manifest.toml: {error}"))
                })?
            }
            None => manifest::StoredManifest::default(),
        };
        if stored.absorb(&roamable::observed_manifest(ides)) {
            staging.write(&path, Some(stored.encode()?.as_bytes()))?;
        }
        Ok(())
    }

    fn reconcile_file(
        &self,
        target: &FileTarget<'_>,
        relative: &str,
        staging: &mut Staging,
    ) -> Result<Option<FileReport>> {
        let FileTarget {
            ide,
            discovered,
            options,
            stamp,
            defaults,
        } = *target;
        let ide_file = discovered.get(relative).cloned().unwrap_or_else(|| {
            ide.path.join(roamable::target_relative_path(
                relative,
                ide,
                &self.sync_config,
            ))
        });
        // Through staging, not the filesystem: a dry run buffers the writes it
        // would make to this IDE, and later passes must see them. Reading the
        // disk directly would leave the IDE looking frozen while the store
        // moved on, so a second pass would conclude the IDE had deleted files.
        let raw = staging.read(&ide_file);
        let (local_view, removed) = raw.as_ref().map_or((None, Vec::new()), |bytes| {
            self.store_view(relative, bytes, defaults)
        });

        // The file is present but holds nothing worth sharing — every setting in
        // it was pruned as a default. That is "no opinion", not "deleted", and
        // conflating the two makes two IDEs fight: one keeps contributing the
        // file and the other keeps withdrawing it. Contribute nothing and leave
        // both sides alone.
        if raw.is_some() && local_view.is_none() {
            return Ok(None);
        }

        let store_file = self.shared_path(relative);
        let store = staging.read(&store_file);
        let base_file = self.base_path(ide, relative);
        let base = staging.read(&base_file);

        // No special case for first contact. With no base, any setting the IDE
        // does not define is taken from the store, so a newly installed IDE
        // still adopts everything; only a setting both sides define differently
        // is a conflict, and there the default of keeping the local value means
        // a sync never silently discards what an IDE already had.
        let merged = merge::merge_file(
            base.as_deref(),
            local_view.as_deref(),
            store.as_deref(),
            options.policy,
        );

        // Record what this IDE and the store agreed on even when nothing
        // changed. Without that snapshot the next divergence has no common
        // ancestor, so a setting only one side later edits looks like a
        // conflict rather than a plain update.
        if base.as_deref() != merged.content.as_deref() {
            staging.write(&base_file, merged.content.as_deref())?;
        }

        if merged.is_noop() && removed.is_empty() {
            return Ok(None);
        }

        staging.write(&store_file, merged.content.as_deref())?;
        if !options.collect_only {
            self.write_back(
                &WriteBack {
                    ide,
                    relative,
                    ide_file: &ide_file,
                    raw: raw.as_deref(),
                    merged: &merged,
                    stamp,
                },
                staging,
            )?;
        }

        Ok(Some(FileReport {
            path: relative.to_string(),
            incoming: merged.incoming,
            outgoing: merged.outgoing,
            conflicts: merged.conflicts,
            pruned: removed,
        }))
    }

    /// Applies the incoming settings to the IDE's own file.
    ///
    /// For XML this edits individual leaves rather than overwriting, so
    /// everything pruning removed from the store view — registry keys the IDE
    /// owns, tutorial progress, migration flags — survives untouched.
    fn write_back(&self, target: &WriteBack<'_>, staging: &mut Staging) -> Result<()> {
        let WriteBack {
            ide,
            relative,
            ide_file,
            raw,
            merged,
            stamp,
        } = *target;
        if merged.incoming.is_empty() {
            return Ok(());
        }
        let Some(content) = &merged.content else {
            // The setting file went away everywhere; leave the IDE's copy alone
            // rather than deleting a file we may not fully own.
            return Ok(());
        };

        let updated = match (
            raw.and_then(|bytes| std::str::from_utf8(bytes).ok()),
            std::str::from_utf8(content).ok(),
        ) {
            (Some(raw_text), Some(merged_text)) => {
                match (dom::parse(raw_text), dom::parse(merged_text)) {
                    (Ok(mut target), Ok(donor)) => {
                        for change in &merged.incoming {
                            if change.path.is_empty() {
                                continue;
                            }
                            match &change.to {
                                Some(value) => {
                                    project::set_leaf(&mut target, &donor, &change.path, value);
                                }
                                None => project::remove_leaf(&mut target, &change.path),
                            }
                        }
                        dom::serialize(&target).into_bytes()
                    }
                    _ => content.clone(),
                }
            }
            // No local file yet, or not XML: take the merged content as-is.
            _ => content.clone(),
        };

        if raw == Some(updated.as_slice()) {
            return Ok(());
        }
        if self.sync_config.jetbrains.backups && raw.is_some() {
            let backup = self
                .paths
                .backups_dir()
                .join(stamp)
                .join(directory_name(ide))
                .join(relative);
            staging.write(&backup, raw)?;
        }
        staging.write(ide_file, Some(&updated))
    }
}

/// Everything `reconcile_file` needs that does not change from file to file.
struct FileTarget<'a> {
    ide: &'a Ide,
    discovered: &'a std::collections::BTreeMap<String, PathBuf>,
    options: &'a SyncOptions,
    stamp: &'a str,
    defaults: Option<&'a defaults::ProductDefaults>,
}

/// The build this IDE reports, or empty when its installation is unknown.
fn build_of(ide: &Ide) -> &str {
    ide.metadata
        .as_ref()
        .map_or("", |metadata| metadata.build_number.as_str())
}

/// Folds a later reconciliation pass into the running report, so each IDE and
/// file is listed once with everything that happened to it.
fn absorb(accumulated: &mut Vec<IdeReport>, pass: Vec<IdeReport>) {
    for incoming in pass {
        let Some(existing) = accumulated
            .iter_mut()
            .find(|candidate| candidate.directory == incoming.directory)
        else {
            accumulated.push(incoming);
            continue;
        };
        for file in incoming.files {
            match existing
                .files
                .iter_mut()
                .find(|candidate| candidate.path == file.path)
            {
                Some(target) => {
                    target.incoming.extend(file.incoming);
                    target.outgoing.extend(file.outgoing);
                    target.conflicts.extend(file.conflicts);
                    // Every pass prunes the same settings, so keep the first
                    // pass's list rather than repeating it.
                    if target.pruned.is_empty() {
                        target.pruned = file.pruned;
                    }
                }
                None => existing.files.push(file),
            }
        }
        existing
            .files
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
}

fn directory_name(ide: &Ide) -> String {
    ide.path.file_name().map_or_else(
        || ide.product.clone(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Store-relative paths shown without the `shared/` prefix, which is noise.
fn display_path(relative: &str) -> String {
    relative
        .strip_prefix(&format!("{SHARED}/"))
        .unwrap_or(relative)
        .to_string()
}

fn short(cursor: &str) -> String {
    cursor.chars().take(8).collect()
}

/// One file's worth of context for writing merged settings back into an IDE.
struct WriteBack<'a> {
    ide: &'a Ide,
    relative: &'a str,
    ide_file: &'a Path,
    raw: Option<&'a [u8]>,
    merged: &'a FileMerge,
    stamp: &'a str,
}

/// Buffers writes so a dry run can still be truthful.
///
/// A sync reconciles the IDEs one after another, so the second IDE must see
/// what the first contributed. During a dry run nothing reaches disk, and
/// without this buffer every IDE would report the store as empty and claim to
/// be adding files that an earlier IDE had already supplied.
#[derive(Default)]
struct Staging {
    dry_run: bool,
    pending: std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>,
}

impl Staging {
    fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            pending: std::collections::BTreeMap::new(),
        }
    }

    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        if let Some(buffered) = self.pending.get(path) {
            return buffered.clone();
        }
        std::fs::read(path).ok()
    }

    fn write(&mut self, path: &Path, content: Option<&[u8]>) -> Result<()> {
        if self.dry_run {
            self.pending
                .insert(path.to_path_buf(), content.map(<[u8]>::to_vec));
            return Ok(());
        }
        write_or_remove(path, content)
    }

    /// Paths beneath `root` that exist once buffered writes are taken into
    /// account.
    fn files_under(&self, root: &Path) -> Vec<String> {
        let mut found: BTreeSet<String> = walk(root, root).into_iter().collect();
        for (path, content) in &self.pending {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let relative = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if content.is_some() {
                found.insert(relative);
            } else {
                found.remove(&relative);
            }
        }
        found.into_iter().collect()
    }
}

/// Keeps the most recent `keep` timestamped backup runs and removes the rest.
/// Directory names are the sort key: they are `YYYYMMDD-HHMMSS`, so
/// lexicographic order is chronological order.
fn prune_backups(root: &Path, keep: usize) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(());
    };
    let mut runs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    if runs.len() <= keep {
        return Ok(());
    }
    runs.sort();
    for stale in &runs[..runs.len() - keep] {
        std::fs::remove_dir_all(stale)?;
    }
    Ok(())
}

fn write_or_remove(path: &Path, content: Option<&[u8]>) -> Result<()> {
    if let Some(bytes) = content {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write then rename so a crash cannot leave a truncated settings
        // file behind.
        let temporary = path.with_extension("jbsync-tmp");
        std::fs::write(&temporary, bytes)?;
        // Renaming replaces the inode, so the destination would otherwise come
        // back with whatever the umask allows. JetBrains deliberately keeps
        // several roamable files at 0600 -- vcs.xml, github.xml, gitlab.xml,
        // ide.general.xml -- and quietly republishing those as world-readable
        // undoes a protection the platform chose on the user's behalf.
        if let Ok(existing) = std::fs::metadata(path)
            && let Err(error) = std::fs::set_permissions(&temporary, existing.permissions())
        {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        std::fs::rename(&temporary, path)?;
        Ok(())
    } else {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn walk(root: &Path, strip: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(strip) {
                found.push(
                    relative
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        }
    }
    found
}

/// `YYYYMMDD-HHMMSS` in UTC, so backup directories sort chronologically
/// without pulling in a date library.
fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);

    // Howard Hinnant's civil_from_days.
    #[allow(clippy::cast_possible_wrap)]
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year + i64::from(month <= 2);

    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_sortable_and_well_formed() {
        let stamp = timestamp();
        assert_eq!(stamp.len(), 15, "YYYYMMDD-HHMMSS");
        assert_eq!(stamp.as_bytes()[8], b'-');
        assert!(stamp.starts_with("20"));
        assert!(
            stamp[..8].chars().all(|c| c.is_ascii_digit()),
            "date part is numeric"
        );
    }

    #[test]
    fn display_path_hides_the_store_prefix() {
        assert_eq!(display_path("shared/options/laf.xml"), "options/laf.xml");
        assert_eq!(display_path("sync.toml"), "sync.toml");
    }

    #[test]
    fn old_backup_runs_are_pruned_newest_first() {
        let directory = tempfile::tempdir().unwrap();
        for stamp in [
            "20260101-000000",
            "20260102-000000",
            "20260103-000000",
            "20260104-000000",
        ] {
            std::fs::create_dir_all(directory.path().join(stamp).join("IDE")).unwrap();
        }
        prune_backups(directory.path(), 2).unwrap();

        let mut left: Vec<String> = std::fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["20260103-000000", "20260104-000000"]);
    }

    #[test]
    fn pruning_backups_is_a_no_op_when_under_the_cap() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("20260101-000000")).unwrap();
        prune_backups(directory.path(), 10).unwrap();
        assert!(directory.path().join("20260101-000000").exists());
        // A missing directory is normal on a first run, not an error.
        prune_backups(&directory.path().join("absent"), 10).unwrap();
    }

    /// JetBrains keeps several roamable files at 0600. Rewriting one through a
    /// temporary file must not hand it back with the umask's permissions.
    #[cfg(unix)]
    #[test]
    fn rewriting_a_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vcs.xml");
        write_or_remove(&path, Some(b"<a />")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_or_remove(&path, Some(b"<b />")).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a private file must stay private");
    }

    #[test]
    fn write_or_remove_replaces_and_deletes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/file.xml");
        write_or_remove(&path, Some(b"<a />")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"<a />");
        write_or_remove(&path, Some(b"<b />")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"<b />");
        write_or_remove(&path, None).unwrap();
        assert!(!path.exists());
    }
}
