//! OS-specific discovery of the `JetBrains` config root and installation search roots.

use std::path::PathBuf;

pub fn default_jetbrains_root() -> PathBuf {
    let home = dirs_home();
    home.join("Library/Application Support/JetBrains")
}

pub fn default_install_roots() -> Vec<PathBuf> {
    let home = dirs_home();
    vec![
        PathBuf::from("/Applications"),
        home.join("Applications"),
        home.join("Library/Application Support/JetBrains/Toolbox/apps"),
    ]
}

fn dirs_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jetbrains_root_is_non_empty() {
        assert!(default_jetbrains_root().components().count() > 0);
    }

    #[test]
    fn install_roots_are_non_empty() {
        assert!(!default_install_roots().is_empty());
    }
}
