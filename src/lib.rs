pub mod backend;
pub mod cli;
pub mod config;
pub mod error;
pub mod ide;
pub mod paths;
pub mod platform;
pub mod plugins;
pub mod settings;
pub mod sync;
pub mod update;
pub mod version;
pub mod xml;

pub use error::{JbsyncError, Result};
pub use version::VERSION;
