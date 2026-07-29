use std::io;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, JbsyncError>;

#[derive(Debug, Error)]
pub enum JbsyncError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("configuration error: {0}")]
    Configuration(String),

    #[error("no {role} matches {selector:?}. Available IDEs: {available}")]
    NoMatchingIde {
        role: &'static str,
        selector: String,
        available: String,
    },

    #[error("XML error in {path}: {source}")]
    Xml {
        path: String,
        #[source]
        source: xmltree::ParseError,
    },

    #[error("git error: {0}")]
    Git(String),

    #[error("{0}")]
    Other(String),
}

impl JbsyncError {
    pub fn configuration(message: impl Into<String>) -> Self {
        Self::Configuration(message.into())
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }
}

impl From<rust_cli_release::Error> for JbsyncError {
    fn from(error: rust_cli_release::Error) -> Self {
        Self::Other(error.to_string())
    }
}
