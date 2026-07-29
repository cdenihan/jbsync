//! Where the shared settings live, and how they travel between machines.
//!
//! The engine never talks to Git. It asks a [`Backend`] for three views of the
//! store — what this machine has, what everyone else has, and the last state
//! both agreed on — merges them itself, and hands the result back. That is the
//! smallest contract that supports a real three-way merge, and every candidate
//! backend can provide it:
//!
//! | Backend        | working copy   | remote            | base                  |
//! |----------------|----------------|-------------------|-----------------------|
//! | Git (shipping) | the work tree  | `origin/<branch>` | `git merge-base`      |
//! | Turso / libSQL | local replica  | `SELECT` after pull | last-synced snapshot |
//! | Custom HTTP    | local cache    | `GET /changes`    | last-synced snapshot   |
//! | Convex         | local cache    | `query()`         | last-synced snapshot   |
//!
//! Backends that cannot name a common ancestor keep a copy of the last state
//! they reconciled; that snapshot *is* the base. Reactivity stays out of this
//! trait deliberately — only Convex offers real push, so it belongs in a
//! separate opt-in capability rather than forcing every backend to fake one.

pub mod git;

use std::{collections::BTreeMap, path::Path};

use crate::error::Result;

/// Store contents keyed by path relative to the store root.
pub type Tree = BTreeMap<String, Vec<u8>>;

/// Remote state that the local copy has not accounted for yet.
#[derive(Debug, Clone)]
pub struct Incoming {
    /// Opaque marker for the remote position, meaningful only to the backend.
    pub cursor: String,
    /// Full remote state.
    pub remote: Tree,
    /// The last state local and remote agreed on, and therefore the base of
    /// every three-way merge.
    pub base: Tree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Published {
    /// The working copy already matched the published state.
    Unchanged,
    Committed {
        cursor: String,
        files: usize,
    },
}

pub trait Backend {
    /// Short description for status output, e.g. `git (origin: git@…)`.
    fn describe(&self) -> String;

    /// The directory holding the working copy of the store.
    fn workdir(&self) -> &Path;

    /// Prepares the store so the engine can read and write it.
    fn initialize(&self) -> Result<()>;

    /// Remote state the working copy has not merged yet, or `None` when there
    /// is no remote configured or nothing new to take.
    fn incoming(&self) -> Result<Option<Incoming>>;

    /// Records the working copy as the new published state.
    fn publish(&self, message: &str) -> Result<Published>;

    /// Declares the working copy reconciled with `cursor`, after the engine
    /// merged the incoming state into it.
    fn reconcile(&self, cursor: &str, message: &str) -> Result<()>;
}
