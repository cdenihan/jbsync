//! Deciding *which* files sync, and *which* settings inside them are real user
//! choices rather than the IDE's own bookkeeping.

pub mod prune;
pub mod roamable;

pub use prune::{PruneOutcome, prune_document};
pub use roamable::{DELETED_TOMBSTONE, discover, is_tombstone};
