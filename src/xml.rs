//! JetBrains settings XML: an order-preserving DOM, a deterministic
//! serializer, and a flat path projection used for diffing and merging.
//!
//! Storage stays XML so it is lossless by construction. The projection exists
//! only so `jbsync` can talk about, diff, and merge *individual settings*
//! rather than whole files.

pub mod dom;
pub mod project;

pub use dom::{Element, ParseError, parse, serialize};
pub use project::{Projection, project, sugar};
