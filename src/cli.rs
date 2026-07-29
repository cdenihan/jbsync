use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "jbsync",
    version = crate::VERSION,
    about = "Local-first settings and plugin sync for JetBrains IDEs",
    long_about = "jbsync synchronizes JetBrains IDE settings and plugins across machines. It reads JetBrains' own roamable-settings allowlist, resolves plugin compatibility from installation metadata, and keeps a local-first sync-data store that a Git remote (or another backend) can replicate."
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Replace the running executable with the requested release.
    Update {
        #[arg(long, default_value = "latest")]
        version: String,
    },
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Update { version } => {
            let summary = crate::update::update_current(&version, false)?;
            println!("{}", serde_json::to_string(&summary)?);
        }
    }
    Ok(())
}
