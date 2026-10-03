#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
compile_error!("jbsync supports only Apple Silicon Macs (aarch64-apple-darwin)");

pub mod backend;
pub mod cli;
pub mod config;
pub mod error;
pub mod ide;
pub mod paths;
pub mod platform;
pub mod plugins;
pub mod progress;
pub mod settings;
pub mod style;
pub mod sync;
pub mod update;
pub mod version;
pub mod xml;

pub use error::{JbsyncError, Result};
pub use version::VERSION;
