//! OS-specific discovery of the `JetBrains` config root and installation search roots.

use std::path::PathBuf;

pub fn default_jetbrains_root() -> PathBuf {
    let home = dirs_home();
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support/JetBrains");
    }
    if cfg!(target_os = "windows") {
        let appdata = std::env::var_os("APPDATA").map_or_else(|| home.clone(), PathBuf::from);
        return appdata.join("JetBrains");
    }
    let config_home =
        std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    config_home.join("JetBrains")
}

pub fn default_install_roots() -> Vec<PathBuf> {
    let home = dirs_home();
    if cfg!(target_os = "macos") {
        return vec![
            PathBuf::from("/Applications"),
            home.join("Applications"),
            home.join("Library/Application Support/JetBrains/Toolbox/apps"),
        ];
    }
    if cfg!(target_os = "windows") {
        let mut roots = vec![
            std::env::var_os("PROGRAMFILES")
                .map_or_else(|| PathBuf::from("C:/Program Files"), PathBuf::from)
                .join("JetBrains"),
            std::env::var_os("LOCALAPPDATA")
                .map_or_else(|| home.join("AppData/Local"), PathBuf::from)
                .join("JetBrains/Toolbox/apps"),
        ];
        if let Some(program_files_x86) = std::env::var_os("PROGRAMFILES(X86)") {
            roots.push(PathBuf::from(program_files_x86).join("JetBrains"));
        }
        return roots;
    }
    vec![
        PathBuf::from("/opt"),
        home.join(".local/share/JetBrains/Toolbox/apps"),
        home.join(".local/share/JetBrains"),
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
