//! Saying what a sync did, in terms of settings rather than files.

use std::fmt::Write as _;

use super::merge::{Change, Conflict, Side};
use crate::settings::prune::Removal;
use crate::style::Style;

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

/// Width of the direction column. Sized to the longest label, so the setting
/// names line up in a single ragged-right column that is easy to scan.
const LABEL_WIDTH: usize = 8;

fn describe(value: Option<&String>) -> &str {
    value.map_or("(default)", String::as_str)
}

/// `from -> to`, or just the new value when there was nothing before.
fn transition(style: Style, from: Option<&String>, to: Option<&String>) -> String {
    let arrow = style.dim("->");
    match (from, to) {
        (None, Some(to)) => to.clone(),
        (Some(from), None) => format!("{from} {arrow} {}", style.dim("(default)")),
        (from, to) => format!("{} {arrow} {}", describe(from), describe(to)),
    }
}

fn render_change(output: &mut String, label: &str, change: &Change, indent: &str, style: Style) {
    // A whole-file change has no setting to name and no before/after value, so
    // forcing it into those columns produced rows like
    // "(file added)   updated locally", which read as jargon in both columns.
    if whole_file(change) {
        let _ = writeln!(
            output,
            "{indent}{label}  {}",
            style.dim(whole_file_phrase(change))
        );
        return;
    }
    let _ = writeln!(
        output,
        "{indent}{:<LABEL_WIDTH$}  {:<34}  {}",
        label,
        change.setting,
        transition(style, change.from.as_ref(), change.to.as_ref())
    );
}

fn whole_file_phrase(change: &Change) -> &'static str {
    match (change.from.is_some(), change.to.is_some()) {
        (false, true) => "the whole file, new here",
        (true, false) => "the whole file, no longer synced",
        _ => "the whole file",
    }
}

/// Conflicts get several lines each. They are the part of a report a person
/// most needs to actually understand, and the one-line form put four values and
/// a verdict on the same row.
fn render_conflict(output: &mut String, conflict: &Conflict, indent: &str, style: Style) {
    let _ = writeln!(
        output,
        "{indent}{}  {}",
        style.yellow(&format!("{:<LABEL_WIDTH$}", "conflict")),
        style.bold(&conflict.setting)
    );
    let detail = format!("{indent}{:LABEL_WIDTH$}  ", "");
    let kept = |side: Side| {
        if conflict.resolved_to == Some(side) {
            style.green("  <- kept")
        } else {
            String::new()
        }
    };
    let _ = writeln!(
        output,
        "{detail}this machine   {}{}",
        describe(conflict.local.as_ref()),
        kept(Side::Local)
    );
    let _ = writeln!(
        output,
        "{detail}other machine  {}{}",
        describe(conflict.remote.as_ref()),
        kept(Side::Remote)
    );
    if conflict.resolved_to.is_none() {
        let _ = writeln!(output, "{detail}{}", style.yellow("unresolved"));
    }
}

/// A change with no projection address is about the file as a whole — it was
/// added or removed, rather than having individual settings edited.
fn whole_file(change: &Change) -> bool {
    change.path.is_empty()
}

/// Files whose only news is that they arrived or left, counted rather than
/// listed. The first sync of an IDE is dozens of these, and a page of identical
/// rows hides the one file that had a real change.
struct BulkFiles {
    to_ide: Vec<String>,
    to_store: Vec<String>,
    removed: Vec<String>,
}

impl BulkFiles {
    fn is_empty(&self) -> bool {
        self.to_ide.is_empty() && self.to_store.is_empty() && self.removed.is_empty()
    }
}

fn split_bulk(files: &[FileReport]) -> BulkFiles {
    let mut bulk = BulkFiles {
        to_ide: Vec::new(),
        to_store: Vec::new(),
        removed: Vec::new(),
    };
    for file in files {
        if !file.conflicts.is_empty() {
            continue;
        }
        let only_whole = |changes: &[Change]| !changes.is_empty() && changes.iter().all(whole_file);
        let gone = |changes: &[Change]| changes.iter().all(|change| change.to.is_none());
        if file.incoming.is_empty() && only_whole(&file.outgoing) {
            if gone(&file.outgoing) {
                bulk.removed.push(file.path.clone());
            } else {
                bulk.to_store.push(file.path.clone());
            }
        } else if file.outgoing.is_empty() && only_whole(&file.incoming) {
            if gone(&file.incoming) {
                bulk.removed.push(file.path.clone());
            } else {
                bulk.to_ide.push(file.path.clone());
            }
        }
    }
    bulk
}

fn render_bulk(output: &mut String, bulk: &BulkFiles, indent: &str, verbose: bool, style: Style) {
    let mut line = |label: &str, paths: &[String], coloured: String| {
        if paths.is_empty() {
            return;
        }
        let _ = writeln!(
            output,
            "{indent}{coloured}  {} {label}",
            plural(paths.len(), "file")
        );
        if verbose {
            for path in paths {
                let _ = writeln!(output, "{indent}{:LABEL_WIDTH$}  {}", "", style.dim(path));
            }
        }
    };
    line(
        "copied into this IDE",
        &bulk.to_ide,
        style.cyan(&format!("{:<LABEL_WIDTH$}", "to IDE")),
    );
    line(
        "published from this IDE",
        &bulk.to_store,
        style.green(&format!("{:<LABEL_WIDTH$}", "to store")),
    );
    line(
        "no longer synced",
        &bulk.removed,
        style.dim(&format!("{:<LABEL_WIDTH$}", "removed")),
    );
}

fn render_files(
    output: &mut String,
    files: &[FileReport],
    indent: &str,
    verbose: bool,
    style: Style,
) {
    let bulk = split_bulk(files);
    let bulked: std::collections::BTreeSet<&String> = bulk
        .to_ide
        .iter()
        .chain(&bulk.to_store)
        .chain(&bulk.removed)
        .collect();

    for file in files {
        // Counted in the bulk summary below; listing them here as well would
        // undo the point of collapsing them.
        if bulked.contains(&file.path) {
            continue;
        }
        let interesting = if verbose {
            file.has_detail()
        } else {
            !file.is_empty()
        };
        if !interesting {
            continue;
        }
        let _ = writeln!(output, "{indent}{}", style.bold(&file.path));
        let inner = format!("{indent}  ");
        for change in &file.incoming {
            render_change(
                output,
                &style.cyan(&format!("{:<LABEL_WIDTH$}", "to IDE")),
                change,
                &inner,
                style,
            );
        }
        for change in &file.outgoing {
            render_change(
                output,
                &style.green(&format!("{:<LABEL_WIDTH$}", "to store")),
                change,
                &inner,
                style,
            );
        }
        for conflict in &file.conflicts {
            render_conflict(output, conflict, &inner, style);
        }
        if verbose {
            for removal in &file.pruned {
                let _ = writeln!(
                    output,
                    "{inner}{}  {:<34}  {}",
                    style.dim(&format!("{:<LABEL_WIDTH$}", "dropped")),
                    removal.path,
                    style.dim(&removal.reason)
                );
            }
        }
    }

    if !bulk.is_empty() {
        render_bulk(output, &bulk, indent, verbose, style);
    }
}

/// Counts for the one-line summary, so the shape of a run is visible without
/// reading every row.
#[derive(Default)]
struct Tally {
    settings_in: usize,
    settings_out: usize,
    files_in: usize,
    files_out: usize,
    conflicts: usize,
}

fn tally(report: &SyncReport) -> Tally {
    let mut tally = Tally {
        conflicts: report.conflicts(),
        ..Tally::default()
    };
    for file in report
        .from_remote
        .iter()
        .chain(report.ides.iter().flat_map(|ide| &ide.files))
    {
        // Whole files and individual settings are different units. Adding them
        // together made a first sync claim hundreds of "settings".
        for change in &file.incoming {
            if whole_file(change) {
                tally.files_in += 1;
            } else {
                tally.settings_in += 1;
            }
        }
        for change in &file.outgoing {
            if whole_file(change) {
                tally.files_out += 1;
            } else {
                tally.settings_out += 1;
            }
        }
    }
    tally
}

/// "1 setting" but "2 settings".
fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Renders the report. `verbose` additionally lists settings that were dropped
/// for not being user choices, and names the files behind each bulk count.
pub fn render(report: &SyncReport, verbose: bool) -> String {
    render_with(report, verbose, Style::plain())
}

/// As [`render`], with ANSI styling when the destination is a terminal.
pub fn render_with(report: &SyncReport, verbose: bool, style: Style) -> String {
    let mut output = String::new();
    let mut header = format!("{}  ·  {}", report.machine, report.backend);
    if report.dry_run {
        header.push_str("  ·  dry run, nothing will be written");
    }
    let _ = writeln!(output, "{}", style.dim(&header));

    if !report.has_detail(verbose) {
        output.push_str("\nEverything is already in sync.\n");
        return output;
    }

    let counts = tally(report);
    let conflicts = counts.conflicts;
    let mut parts = Vec::new();
    let into_ide = counts.settings_in + counts.files_in;
    if into_ide > 0 {
        let mut piece = Vec::new();
        if counts.settings_in > 0 {
            piece.push(plural(counts.settings_in, "setting"));
        }
        if counts.files_in > 0 {
            piece.push(plural(counts.files_in, "file"));
        }
        parts.push(format!("{} into IDEs", piece.join(" and ")));
    }
    let into_store = counts.settings_out + counts.files_out;
    if into_store > 0 {
        let mut piece = Vec::new();
        if counts.settings_out > 0 {
            piece.push(plural(counts.settings_out, "setting"));
        }
        if counts.files_out > 0 {
            piece.push(plural(counts.files_out, "file"));
        }
        parts.push(format!("{} into the store", piece.join(" and ")));
    }
    if conflicts > 0 {
        parts.push(plural(conflicts, "conflict"));
    }
    if !parts.is_empty() {
        let _ = writeln!(output, "{}", style.bold(&parts.join("  ·  ")));
    }

    if report.from_remote.iter().any(|file| !file.is_empty()) {
        let _ = writeln!(output, "\n{}", style.bold("From other machines"));
        render_files(&mut output, &report.from_remote, "  ", verbose, style);
    }

    // Every IDE is listed, even an idle one, so the report doubles as
    // confirmation that jbsync actually looked at it.
    for ide in &report.ides {
        let label = if ide.display_name.is_empty() || ide.display_name == ide.directory {
            ide.directory.clone()
        } else {
            format!("{}  ({})", ide.directory, ide.display_name)
        };
        let _ = writeln!(output, "\n{}", style.bold(&label));
        if let Some(reason) = &ide.skipped {
            let _ = writeln!(output, "  {} {reason}", style.dim("skipped:"));
            continue;
        }
        let interesting = if verbose {
            ide.has_detail()
        } else {
            !ide.is_empty()
        };
        if interesting {
            render_files(&mut output, &ide.files, "  ", verbose, style);
        } else {
            let _ = writeln!(output, "  {}", style.dim("no changes"));
        }
    }

    if !report.plugins.is_empty() {
        let _ = writeln!(output, "\n{}", style.bold("Plugins"));
        for line in &report.plugins {
            let _ = writeln!(output, "  {line}");
        }
    }

    output.push('\n');
    if conflicts > 0 {
        let _ = writeln!(
            output,
            "{} resolved in favour of this machine. \
             Re-run with --prefer remote to flip the choice.",
            plural(conflicts, "conflict")
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
        assert!(rendered.contains("IntelliJIdea2026.2"));
        assert!(rendered.contains("options/editor.xml"));
        // Direction is stated in words, naming where the value is going.
        assert!(rendered.contains("to IDE    Editor/fontSize"));
        assert!(rendered.contains("to store  Editor/tabs"));
        assert!(rendered.contains("4 -> 2"));
        assert!(rendered.contains("1 setting into IDEs"));
        assert!(rendered.contains("1 setting into the store"));
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
        assert!(rendered.contains("conflict  Editor/tabs"));
        assert!(rendered.contains("this machine   2"));
        assert!(rendered.contains("other machine  8"));
        assert!(rendered.contains("<- kept"));
        assert!(rendered.contains("1 conflict resolved in favour of this machine"));
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
        assert!(verbose.contains("dropped   Registry/ide.experimental.ui"));
        assert!(verbose.contains("registry key set by the IDE"));
    }

    /// The first sync of an IDE is dozens of whole-file additions. Listing each
    /// one buried the files that had real changes.
    #[test]
    fn bulk_file_additions_are_counted_not_listed() {
        let whole = |to: Option<&str>| Change {
            path: String::new(),
            setting: "(file added)".to_string(),
            from: None,
            to: to.map(str::to_string),
        };
        let bulk: Vec<FileReport> = (0..12)
            .map(|index| FileReport {
                path: format!("options/file{index}.xml"),
                outgoing: vec![whole(Some("updated locally"))],
                ..FileReport::default()
            })
            .collect();
        let report = SyncReport {
            ides: vec![IdeReport {
                directory: "PyCharm2026.2".to_string(),
                files: bulk,
                ..IdeReport::default()
            }],
            ..SyncReport::default()
        };

        let rendered = render(&report, false);
        assert!(rendered.contains("12 files published from this IDE"));
        assert!(
            !rendered.contains("options/file3.xml"),
            "individual paths are noise at this volume: {rendered}"
        );
        // Whole files are counted as files, never as settings.
        assert!(rendered.contains("12 files into the store"));
        assert!(!rendered.contains("setting"));

        // --verbose is how you see exactly which files.
        assert!(render(&report, true).contains("options/file3.xml"));
    }

    /// A file that changed as a whole *and* had a setting edited still lists
    /// both, and the whole-file part reads as a sentence rather than as a
    /// setting named "(file added)".
    #[test]
    fn a_whole_file_change_is_described_not_tabulated() {
        let report = SyncReport {
            ides: vec![IdeReport {
                directory: "CLion2026.2".to_string(),
                files: vec![FileReport {
                    path: "options/laf.xml".to_string(),
                    incoming: vec![change("LafManager/themeId", Some("Light"), Some("Dark"))],
                    outgoing: vec![Change {
                        path: String::new(),
                        setting: "(file added)".to_string(),
                        from: None,
                        to: Some("updated locally".to_string()),
                    }],
                    ..FileReport::default()
                }],
                ..IdeReport::default()
            }],
            ..SyncReport::default()
        };
        let rendered = render(&report, false);
        assert!(rendered.contains("the whole file, new here"), "{rendered}");
        assert!(!rendered.contains("(file added)"), "{rendered}");
        assert!(!rendered.contains("updated locally"), "{rendered}");
        assert!(rendered.contains("to IDE    LafManager/themeId"));
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
