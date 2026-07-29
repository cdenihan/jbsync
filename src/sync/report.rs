//! Saying what a sync did, in terms of settings rather than files.

use std::fmt::Write as _;

use super::merge::{Change, Conflict, Side};
use crate::settings::prune::Removal;

/// Everything that happened to one file, from one IDE's point of view.
#[derive(Debug, Clone, Default)]
pub struct FileReport {
    pub path: String,
    /// Settings this file is receiving.
    pub incoming: Vec<Change>,
    /// Settings this file is contributing to the store.
    pub outgoing: Vec<Change>,
    pub conflicts: Vec<Conflict>,
    /// Settings dropped for not being user choices.
    pub pruned: Vec<Removal>,
}

impl FileReport {
    /// True when nothing about this file changed.
    ///
    /// Pruned settings are deliberately excluded: they describe what was left
    /// out of the store, which is the same every run. Counting them as change
    /// would make a settled sync look like it still had work to do.
    pub fn is_empty(&self) -> bool {
        self.incoming.is_empty() && self.outgoing.is_empty() && self.conflicts.is_empty()
    }

    /// True when there is anything at all to show, including prunes.
    pub fn has_detail(&self) -> bool {
        !self.is_empty() || !self.pruned.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct IdeReport {
    /// Config directory name, e.g. `IntelliJIdea2026.2`.
    pub directory: String,
    /// Display name from `product-info.json`, when available.
    pub display_name: String,
    pub files: Vec<FileReport>,
    /// Set when the IDE took no part in this run, holding the reason to show.
    pub skipped: Option<String>,
}

impl IdeReport {
    pub fn is_empty(&self) -> bool {
        self.files.iter().all(FileReport::is_empty)
    }

    pub fn has_detail(&self) -> bool {
        self.files.iter().any(FileReport::has_detail)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub machine: String,
    pub backend: String,
    pub dry_run: bool,
    /// Changes pulled from other machines into the store.
    pub from_remote: Vec<FileReport>,
    /// Per-IDE reconciliation on this machine.
    pub ides: Vec<IdeReport>,
    pub published: Option<String>,
    pub plugins: Vec<String>,
}

impl SyncReport {
    pub fn conflicts(&self) -> usize {
        let in_files =
            |files: &[FileReport]| -> usize { files.iter().map(|file| file.conflicts.len()).sum() };
        in_files(&self.from_remote)
            + self
                .ides
                .iter()
                .map(|ide| in_files(&ide.files))
                .sum::<usize>()
    }

    pub fn is_empty(&self) -> bool {
        self.from_remote.iter().all(FileReport::is_empty)
            && self.ides.iter().all(IdeReport::is_empty)
            // An IDE left out of the run is news, even when nothing else moved.
            && self.ides.iter().all(|ide| ide.skipped.is_none())
            && self.plugins.is_empty()
    }

    /// Whether there is anything to print at the requested detail level.
    pub fn has_detail(&self, verbose: bool) -> bool {
        if !self.is_empty() {
            return true;
        }
        verbose
            && (self.from_remote.iter().any(FileReport::has_detail)
                || self.ides.iter().any(IdeReport::has_detail))
    }
}

const SETTING_WIDTH: usize = 46;

fn describe(value: Option<&String>) -> &str {
    value.map_or("(default)", String::as_str)
}

fn render_change(output: &mut String, marker: char, change: &Change, indent: &str) {
    let arrow = match (&change.from, &change.to) {
        (None, Some(to)) => to.clone(),
        (Some(from), None) => format!("{from} -> (default)"),
        (from, to) => format!("{} -> {}", describe(from.as_ref()), describe(to.as_ref())),
    };
    let _ = writeln!(
        output,
        "{indent}{marker} {:<SETTING_WIDTH$} {arrow}",
        change.setting
    );
}

fn render_conflict(output: &mut String, conflict: &Conflict, indent: &str) {
    let resolution = match conflict.resolved_to {
        Some(Side::Local) => "kept this machine's value",
        Some(Side::Remote) => "took the incoming value",
        None => "unresolved",
    };
    let _ = writeln!(
        output,
        "{indent}! {:<SETTING_WIDTH$} here {} / there {} -> {resolution}",
        conflict.setting,
        describe(conflict.local.as_ref()),
        describe(conflict.remote.as_ref()),
    );
}

fn render_files(output: &mut String, files: &[FileReport], indent: &str, verbose: bool) {
    let shown = files.iter().filter(|file| {
        if verbose {
            file.has_detail()
        } else {
            !file.is_empty()
        }
    });
    for file in shown {
        let _ = writeln!(output, "{indent}{}", file.path);
        let inner = format!("{indent}    ");
        for change in &file.incoming {
            render_change(output, '<', change, &inner);
        }
        for change in &file.outgoing {
            render_change(output, '>', change, &inner);
        }
        for conflict in &file.conflicts {
            render_conflict(output, conflict, &inner);
        }
        if verbose {
            for removal in &file.pruned {
                let _ = writeln!(
                    output,
                    "{inner}- {:<SETTING_WIDTH$} {}",
                    removal.path, removal.reason
                );
            }
        }
    }
}

/// Renders the report. `verbose` additionally lists settings that were dropped
/// for not being user choices.
pub fn render(report: &SyncReport, verbose: bool) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "machine {}  |  {}{}",
        report.machine,
        report.backend,
        if report.dry_run { "  |  dry run" } else { "" }
    );

    if !report.has_detail(verbose) {
        output.push_str("\nEverything is already in sync.\n");
        return output;
    }

    output.push_str("\nLegend: < incoming   > outgoing   ! conflict");
    if verbose {
        output.push_str("   - pruned");
    }
    output.push('\n');

    if report.from_remote.iter().any(|file| !file.is_empty()) {
        output.push_str("\nFrom other machines\n");
        render_files(&mut output, &report.from_remote, "  ", verbose);
    }

    // Every IDE is listed, even an idle one, so the report doubles as
    // confirmation that jbsync actually looked at it.
    for ide in &report.ides {
        let label = if ide.display_name.is_empty() || ide.display_name == ide.directory {
            ide.directory.clone()
        } else {
            format!("{} ({})", ide.directory, ide.display_name)
        };
        let _ = writeln!(output, "\n{label}");
        if let Some(reason) = &ide.skipped {
            let _ = writeln!(output, "  skipped: {reason}");
            continue;
        }
        let interesting = if verbose {
            ide.has_detail()
        } else {
            !ide.is_empty()
        };
        if interesting {
            render_files(&mut output, &ide.files, "  ", verbose);
        } else {
            output.push_str("  no changes\n");
        }
    }

    if !report.plugins.is_empty() {
        output.push_str("\nPlugins\n");
        for line in &report.plugins {
            let _ = writeln!(output, "  {line}");
        }
    }

    let conflicts = report.conflicts();
    output.push('\n');
    if conflicts > 0 {
        let _ = writeln!(
            output,
            "{conflicts} conflict(s) resolved. Re-run with --prefer remote to flip the choice."
        );
    }
    match &report.published {
        Some(summary) if !report.dry_run => {
            let _ = writeln!(output, "Committed: {summary}");
        }
        _ if report.dry_run => output.push_str("Nothing was written.\n"),
        _ => {}
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(setting: &str, from: Option<&str>, to: Option<&str>) -> Change {
        Change {
            path: setting.to_string(),
            setting: setting.to_string(),
            from: from.map(str::to_string),
            to: to.map(str::to_string),
        }
    }

    #[test]
    fn an_idle_sync_says_so_plainly() {
        let report = SyncReport {
            machine: "mac".to_string(),
            backend: "git (local only)".to_string(),
            ..SyncReport::default()
        };
        let rendered = render(&report, false);
        assert!(rendered.contains("Everything is already in sync."));
    }

    #[test]
    fn settings_are_named_per_ide_with_direction() {
        let report = SyncReport {
            machine: "mac".to_string(),
            backend: "git".to_string(),
            ides: vec![IdeReport {
                directory: "IntelliJIdea2026.2".to_string(),
                display_name: "IntelliJ IDEA".to_string(),
                skipped: None,
                files: vec![FileReport {
                    path: "options/editor.xml".to_string(),
                    incoming: vec![change("Editor/fontSize", None, Some("14"))],
                    outgoing: vec![change("Editor/tabs", Some("4"), Some("2"))],
                    ..FileReport::default()
                }],
            }],
            ..SyncReport::default()
        };
        let rendered = render(&report, false);
        assert!(rendered.contains("IntelliJIdea2026.2 (IntelliJ IDEA)"));
        assert!(rendered.contains("options/editor.xml"));
        assert!(rendered.contains("< Editor/fontSize"));
        assert!(rendered.contains("> Editor/tabs"));
        assert!(rendered.contains("4 -> 2"));
    }

    #[test]
    fn conflicts_state_both_values_and_the_resolution() {
        let report = SyncReport {
            ides: vec![IdeReport {
                directory: "CLion2026.2".to_string(),
                files: vec![FileReport {
                    path: "options/editor.xml".to_string(),
                    conflicts: vec![Conflict {
                        setting: "Editor/tabs".to_string(),
                        local: Some("2".to_string()),
                        remote: Some("8".to_string()),
                        resolved_to: Some(Side::Local),
                    }],
                    ..FileReport::default()
                }],
                ..IdeReport::default()
            }],
            ..SyncReport::default()
        };
        let rendered = render(&report, false);
        assert!(rendered.contains("here 2 / there 8"));
        assert!(rendered.contains("kept this machine's value"));
        assert!(rendered.contains("--prefer remote"));
    }

    #[test]
    fn pruned_settings_appear_only_when_verbose() {
        let report = SyncReport {
            ides: vec![IdeReport {
                directory: "PyCharm2026.2".to_string(),
                files: vec![FileReport {
                    path: "options/ide.general.xml".to_string(),
                    pruned: vec![Removal {
                        path: "Registry/ide.experimental.ui".to_string(),
                        reason: "registry key set by the IDE, not the user".to_string(),
                    }],
                    ..FileReport::default()
                }],
                ..IdeReport::default()
            }],
            ..SyncReport::default()
        };
        assert!(!render(&report, false).contains("ide.experimental.ui"));
        let verbose = render(&report, true);
        assert!(verbose.contains("- Registry/ide.experimental.ui"));
        assert!(verbose.contains("registry key set by the IDE"));
    }

    #[test]
    fn a_dry_run_promises_nothing_was_written() {
        let report = SyncReport {
            dry_run: true,
            ides: vec![IdeReport {
                directory: "CLion2026.2".to_string(),
                files: vec![FileReport {
                    path: "options/laf.xml".to_string(),
                    incoming: vec![change("LafManager/theme", None, Some("Dark"))],
                    ..FileReport::default()
                }],
                ..IdeReport::default()
            }],
            ..SyncReport::default()
        };
        let rendered = render(&report, false);
        assert!(rendered.contains("dry run"));
        assert!(rendered.contains("Nothing was written."));
    }
}
