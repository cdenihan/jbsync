use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

use crate::{
    config::LocalConfig,
    error::Result,
    paths::Paths,
    plugins,
    sync::{ConflictPolicy, Engine, SyncOptions, render},
};

#[derive(Debug, Parser)]
#[command(
    name = "jbsync",
    version = crate::VERSION,
    about = "Settings and plugin sync for JetBrains IDEs",
    long_about = "jbsync keeps JetBrains IDE settings in step across IDEs and machines.\n\n\
                  It stores only the settings you actually changed, merges them one setting \
                  at a time so two IDEs editing the same file do not collide, and keeps the \
                  sync repository itself out of your way."
)]
pub struct Cli {
    /// Override the jbsync data directory (default: ~/.jbsync).
    #[arg(long, global = true, env = "JBSYNC_CONFIG_DIR")]
    config_dir: Option<PathBuf>,

    /// Also list settings dropped for not being user choices.
    #[arg(long, short, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Prefer {
    /// This machine's value wins a conflict.
    Local,
    /// The incoming value wins a conflict.
    Remote,
    /// Report conflicts and change nothing.
    Neither,
}

impl From<Prefer> for ConflictPolicy {
    fn from(value: Prefer) -> Self {
        match value {
            Prefer::Local => Self::PreferLocal,
            Prefer::Remote => Self::PreferRemote,
            Prefer::Neither => Self::Fail,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Set up the local configuration and the sync store.
    Init {
        /// Git remote to publish to. Without one the store stays on this machine.
        #[arg(long)]
        remote: Option<String>,
        /// Name for this machine in reports and per-machine overrides.
        #[arg(long)]
        machine: Option<String>,
    },
    /// Show what a sync would do, without changing anything.
    Status,
    /// Reconcile every IDE with the store, and the store with other machines.
    Sync {
        /// Report the plan without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Who wins when both sides changed the same setting.
        #[arg(long, value_enum, default_value = "local")]
        prefer: Prefer,
        /// Limit to IDEs matching these selectors (name, product, or glob).
        #[arg(long = "ide")]
        ides: Vec<String>,
        /// Gather settings into the store without writing back to the IDEs.
        #[arg(long)]
        collect_only: bool,
        /// Install missing plugins from Marketplace via the IDE launcher.
        #[arg(long)]
        install_plugins: bool,
        /// Commit message for the git backend.
        #[arg(long, short, default_value = "Sync JetBrains settings")]
        message: String,
    },
    /// List the JetBrains IDEs jbsync can see.
    Ides,
    /// Inspect or change where the store is kept.
    Repo {
        #[command(subcommand)]
        action: RepoAction,
    },
    /// Show the recorded plugins and which IDEs are missing them.
    Plugins,
    /// Turn off JetBrains' bundled Backup and Sync so the two do not fight.
    DisableBuiltinSync {
        #[arg(long)]
        dry_run: bool,
    },
    /// Replace the running executable with the requested release.
    Update {
        /// Release to install, e.g. 2026.07.29.2. Defaults to the newest.
        #[arg(long, default_value = "latest")]
        version: String,
        /// Print the result as JSON instead of a sentence.
        #[arg(long)]
        json: bool,
    },
    /// Print a shell completion script.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Subcommand)]
enum RepoAction {
    /// Show the store location and remote.
    Show,
    /// Point the store at a git remote.
    Set {
        /// Remote URL, e.g. git@github.com:you/jetbrains-settings.git
        url: String,
    },
    /// Stop publishing to a remote and keep the store local.
    Unset,
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { remote, machine } => init(cli.config_dir, remote, machine)?,
        Command::Status => {
            let mut engine = Engine::open(cli.config_dir)?;
            let options = SyncOptions {
                dry_run: true,
                ..SyncOptions::default()
            };
            let report = engine.sync(&options)?;
            print!("{}", render(&report, cli.verbose));
        }
        Command::Sync {
            dry_run,
            prefer,
            ides,
            collect_only,
            install_plugins,
            message,
        } => {
            let mut engine = Engine::open(cli.config_dir)?;
            let options = SyncOptions {
                dry_run,
                policy: prefer.into(),
                message,
                only: ides,
                collect_only,
                install_plugins,
            };
            let report = engine.sync(&options)?;
            print!("{}", render(&report, cli.verbose));
        }
        Command::Ides => {
            let engine = Engine::open(cli.config_dir)?;
            if engine.ides.is_empty() {
                println!("No JetBrains IDEs found.");
            }
            for ide in &engine.ides {
                let name = ide
                    .path
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let build = ide.metadata.as_ref().map_or_else(String::new, |metadata| {
                    format!("build {}", metadata.build_number)
                });
                println!("{name:<24}{:<16}{build}", ide.product);
            }
        }
        Command::Repo { action } => repo(cli.config_dir, action)?,
        Command::Plugins => {
            let engine = Engine::open(cli.config_dir)?;
            let manifest = plugins::Manifest::load(&engine.store_root().join("plugins.json"))?;
            if manifest.plugins.is_empty() {
                println!("No plugins recorded yet. Run `jbsync sync` first.");
            }
            for plugin in &manifest.plugins {
                println!(
                    "{:<44}{:<12}{}",
                    plugin.id,
                    plugin.version,
                    plugin.source_products.join(", ")
                );
            }
            let selected: Vec<&crate::ide::Ide> = engine.ides.iter().collect();
            for action in plugins::plan_installs(&selected, &manifest, &engine.sync_config) {
                let verb = if action.install { "missing" } else { "skip" };
                println!(
                    "  {}: {verb} {} ({})",
                    action.ide, action.plugin, action.reason
                );
            }
        }
        Command::DisableBuiltinSync { dry_run } => {
            let engine = Engine::open(cli.config_dir)?;
            let actions = engine.disable_builtin_sync(dry_run)?;
            if actions.is_empty() {
                println!("JetBrains Backup and Sync is already off everywhere.");
            }
            for action in actions {
                println!("{action}");
            }
        }
        Command::Update { version, json } => {
            if !json {
                if version == "latest" {
                    eprintln!("Checking for the latest jbsync release...");
                } else {
                    eprintln!("Installing jbsync {version}...");
                }
            }
            let summary = crate::update::update_current(&version, json)?;
            if json {
                println!("{}", serde_json::to_string(&summary)?);
            } else {
                println!("{}", summary.describe("jbsync"));
            }
        }
        Command::Completions { shell } => {
            let mut command = Cli::command();
            let name = command.get_name().to_string();
            clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
        }
    }
    Ok(())
}

fn init(
    config_dir: Option<PathBuf>,
    remote: Option<String>,
    machine: Option<String>,
) -> Result<()> {
    let paths = Paths::discover(config_dir.clone())?;
    let path = paths.local_config_path();
    let mut local = LocalConfig::load(&path)?;
    if remote.is_some() {
        local.repo.remote = remote;
    }
    if machine.is_some() {
        local.machine.id = machine;
    }
    local.save(&path)?;

    // Opening the engine creates and prepares the store.
    let engine = Engine::open(config_dir)?;
    println!("config  {}", path.display());
    println!("store   {}", engine.store_root().display());
    println!("machine {}", engine.machine);
    println!("found   {} IDE(s)", engine.ides.len());
    println!("\nNext: jbsync status");
    Ok(())
}

fn repo(config_dir: Option<PathBuf>, action: RepoAction) -> Result<()> {
    let paths = Paths::discover(config_dir.clone())?;
    let path = paths.local_config_path();
    let mut local = LocalConfig::load(&path)?;

    match action {
        RepoAction::Show => {
            println!("store   {}", paths.data_dir(&local.repo).display());
            println!("backend {}", local.repo.backend);
            println!(
                "remote  {}",
                local.repo.remote.as_deref().unwrap_or("(local only)")
            );
            println!("branch  {}", local.repo.branch);
        }
        RepoAction::Set { url } => {
            local.repo.remote = Some(url.clone());
            local.save(&path)?;
            Engine::open(config_dir)?;
            println!("Store now publishes to {url}");
        }
        RepoAction::Unset => {
            local.repo.remote = None;
            local.save(&path)?;
            Engine::open(config_dir)?;
            println!("Store is now local only.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn prefer_maps_onto_the_merge_policy() {
        assert_eq!(
            ConflictPolicy::from(Prefer::Local),
            ConflictPolicy::PreferLocal
        );
        assert_eq!(
            ConflictPolicy::from(Prefer::Remote),
            ConflictPolicy::PreferRemote
        );
        assert_eq!(ConflictPolicy::from(Prefer::Neither), ConflictPolicy::Fail);
    }

    #[test]
    fn sync_defaults_to_writing_and_preferring_local() {
        let cli = Cli::parse_from(["jbsync", "sync"]);
        let Command::Sync {
            dry_run,
            prefer,
            collect_only,
            install_plugins,
            ..
        } = cli.command
        else {
            panic!("expected sync");
        };
        assert!(!dry_run);
        assert!(!collect_only);
        assert!(
            !install_plugins,
            "installing plugins launches the IDE, so it must be opt-in"
        );
        assert_eq!(ConflictPolicy::from(prefer), ConflictPolicy::PreferLocal);
    }

    #[test]
    fn ide_selectors_accumulate() {
        let cli = Cli::parse_from(["jbsync", "sync", "--ide", "PyCharm*", "--ide", "CLion*"]);
        let Command::Sync { ides, .. } = cli.command else {
            panic!("expected sync");
        };
        assert_eq!(ides, vec!["PyCharm*", "CLion*"]);
    }
}
