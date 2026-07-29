//! What a product's settings look like before anyone has touched them.
//!
//! A JetBrains installer creates the config directory and fills `options/` with
//! the product's factory defaults *before* the IDE has ever run. That directory
//! is the cleanest possible answer to "did the user choose this, or did it come
//! with the product?" — and it exists only in the window between installing an
//! IDE and opening it for the first time.
//!
//! So jbsync captures it when it sees it. A never-launched IDE takes no part in
//! the sync, but its files are projected into `defaults/<Product>.toml` in the
//! store, where they replicate to every machine. Afterwards, any setting whose
//! value still equals the recorded default is treated as not-a-choice and kept
//! out of the shared store.
//!
//! This is the same judgement the platform already makes for itself — when a
//! component's state equals its default-constructed state, nothing is written
//! at all. Capturing the defaults extends that to the components which write
//! themselves out regardless.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{JbsyncError, Result};

/// Where a product's defaults live inside the store.
#[must_use]
pub fn relative_path(product: &str) -> String {
    format!("defaults/{}.toml", sanitize(product))
}

/// Products come from directory names, so keep them to something filename-safe.
fn sanitize(product: &str) -> String {
    let cleaned: String = product
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "product".to_string()
    } else {
        trimmed
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ProductDefaults {
    pub version: u32,
    pub product: String,
    /// Build the capture came from, for diagnosis. Not used for matching: a
    /// value that was this product's default at any point is not evidence of a
    /// choice, and refusing to match across builds would make the capture
    /// useless the first time the IDE updated.
    pub build: String,
    /// Store-relative file path -> setting address -> default value.
    pub files: BTreeMap<String, BTreeMap<String, String>>,
}

impl ProductDefaults {
    pub fn new(product: &str, build: &str) -> Self {
        Self {
            version: 1,
            product: product.to_string(),
            build: build.to_string(),
            files: BTreeMap::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text)
            .map_err(|error| JbsyncError::configuration(format!("defaults: {error}")))
    }

    pub fn encode(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|error| JbsyncError::configuration(error.to_string()))
    }

    /// Records one file's projected defaults, replacing any earlier capture of
    /// that file. Returns whether anything changed.
    pub fn record(&mut self, relative: &str, values: BTreeMap<String, String>) -> bool {
        if values.is_empty() {
            return false;
        }
        if self.files.get(relative) == Some(&values) {
            return false;
        }
        self.files.insert(relative.to_string(), values);
        true
    }

    /// The default value of one setting, if this product has a recorded one.
    #[must_use]
    pub fn value(&self, relative: &str, address: &str) -> Option<&str> {
        self.files.get(relative)?.get(address).map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn recording_is_idempotent() {
        let mut defaults = ProductDefaults::new("WebStorm", "262.1");
        assert!(defaults.record("options/editor.xml", values(&[("a", "1")])));
        assert!(!defaults.record("options/editor.xml", values(&[("a", "1")])));
        assert!(defaults.record("options/editor.xml", values(&[("a", "2")])));
    }

    #[test]
    fn an_empty_capture_records_nothing() {
        let mut defaults = ProductDefaults::new("WebStorm", "262.1");
        assert!(!defaults.record("options/editor.xml", BTreeMap::new()));
        assert!(defaults.is_empty());
    }

    #[test]
    fn looks_up_only_its_own_file() {
        let mut defaults = ProductDefaults::new("WebStorm", "262.1");
        defaults.record("options/editor.xml", values(&[("tabs", "4")]));
        assert_eq!(defaults.value("options/editor.xml", "tabs"), Some("4"));
        assert_eq!(defaults.value("options/laf.xml", "tabs"), None);
        assert_eq!(defaults.value("options/editor.xml", "wrap"), None);
    }

    #[test]
    fn round_trips_through_toml() {
        let mut defaults = ProductDefaults::new("WebStorm", "262.1");
        defaults.record("options/editor.xml", values(&[("tabs", "4")]));
        let loaded = ProductDefaults::parse(&defaults.encode().unwrap()).unwrap();
        assert_eq!(loaded.value("options/editor.xml", "tabs"), Some("4"));
        assert_eq!(loaded.build, "262.1");
    }

    #[test]
    fn product_names_stay_filename_safe() {
        assert_eq!(relative_path("IntelliJIdea"), "defaults/IntelliJIdea.toml");
        // Product names come from directory names, so a hostile one must not be
        // able to escape `defaults/`. Separators become dashes, leaving a single
        // path component whatever it started as.
        for hostile in ["../etc", "a/b", "..", "", "/"] {
            let path = relative_path(hostile);
            let tail = path.strip_prefix("defaults/").expect("stays in defaults/");
            assert!(!tail.contains('/'), "{hostile:?} produced {path:?}");
            assert!(
                !std::path::Path::new(tail)
                    .components()
                    .any(|part| part == std::path::Component::ParentDir),
                "{hostile:?} produced {path:?}"
            );
        }
    }
}
