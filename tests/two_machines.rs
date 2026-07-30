//! End-to-end behaviour across two machines and several IDEs.
//!
//! These are the scenarios the design exists for: two IDEs changing different
//! settings in the same file must both get their way, and two machines
//! changing the *same* setting must produce one clearly reported conflict
//! rather than a corrupted file.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use jbsync::{
    sync::{ConflictPolicy, Engine, SyncOptions},
    xml::{dom, project},
};

/// A JetBrains config root with one or more IDE directories in it.
struct Machine {
    _home: tempfile::TempDir,
    config_dir: PathBuf,
    jetbrains_root: PathBuf,
}

impl Machine {
    fn new(remote: &Path, ides: &[&str]) -> Self {
        let home = tempfile::tempdir().unwrap();
        let config_dir = home.path().join("jbsync");
        let jetbrains_root = home.path().join("JetBrains");
        std::fs::create_dir_all(&config_dir).unwrap();
        for ide in ides {
            std::fs::create_dir_all(jetbrains_root.join(ide).join("options")).unwrap();
            // The platform writes other.xml on first start, and jbsync treats
            // its absence as "installed but never launched". These IDEs are
            // meant to look like ones somebody has actually used.
            std::fs::write(
                jetbrains_root.join(ide).join("options/other.xml"),
                "<application />",
            )
            .unwrap();
        }
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "[repo]\nremote = {:?}\n\n[jetbrains]\nroot = {:?}\n",
                remote.to_string_lossy(),
                jetbrains_root.to_string_lossy()
            ),
        )
        .unwrap();
        Self {
            _home: home,
            config_dir,
            jetbrains_root,
        }
    }

    fn write_option(&self, ide: &str, file: &str, component: &str, options: &[(&str, &str)]) {
        let mut body = String::new();
        for (name, value) in options {
            let _ = writeln!(body, "    <option name=\"{name}\" value=\"{value}\" />");
        }
        let document = format!(
            "<?xml version='1.0' encoding='utf-8'?>\n<application>\n  <component name=\"{component}\">\n{body}  </component>\n</application>"
        );
        let path = self.jetbrains_root.join(ide).join("options").join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, document).unwrap();
    }

    /// Writes a settings file verbatim, for shapes `write_option` cannot
    /// express — `project.default.xml` nests a component per project setting
    /// inside `<component name="ProjectManager"><defaultProject>`.
    fn write_file(&self, ide: &str, file: &str, body: &str) {
        let path = self.jetbrains_root.join(ide).join("options").join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn read_file(&self, ide: &str, file: &str) -> String {
        let path = self.jetbrains_root.join(ide).join("options").join(file);
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// The value of one setting as the IDE currently has it on disk.
    fn read_option(&self, ide: &str, file: &str, setting: &str) -> Option<String> {
        let path = self.jetbrains_root.join(ide).join("options").join(file);
        let text = std::fs::read_to_string(path).ok()?;
        let document = dom::parse(&text).ok()?;
        project::project(&document)
            .into_iter()
            .find(|(address, _)| project::sugar(address) == setting)
            .map(|(_, value)| value)
    }

    fn sync(&self) -> jbsync::sync::report::SyncReport {
        self.sync_with(ConflictPolicy::PreferLocal)
    }

    fn dry_run(&self) -> jbsync::sync::report::SyncReport {
        let mut engine = Engine::open(Some(self.config_dir.clone())).unwrap();
        engine
            .sync(&SyncOptions {
                dry_run: true,
                ..SyncOptions::default()
            })
            .unwrap()
    }

    fn sync_with(&self, policy: ConflictPolicy) -> jbsync::sync::report::SyncReport {
        let mut engine = Engine::open(Some(self.config_dir.clone())).unwrap();
        engine
            .sync(&SyncOptions {
                policy,
                ..SyncOptions::default()
            })
            .unwrap()
    }
}

fn bare_remote() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "--bare", "-b", "main"])
        .arg(directory.path())
        .status()
        .expect("git must be installed to run these tests");
    assert!(status.success());
    directory
}

#[test]
fn two_ides_changing_different_settings_both_win() {
    let remote = bare_remote();
    let machine = Machine::new(remote.path(), &["IntelliJIdea2026.2", "PyCharm2026.2"]);

    // Start both IDEs from the same settings, so the baseline is unambiguous.
    for ide in ["IntelliJIdea2026.2", "PyCharm2026.2"] {
        machine.write_option(
            ide,
            "editor.xml",
            "Editor",
            &[("tabs", "4"), ("wrap", "true")],
        );
    }
    let baseline = machine.sync();
    assert_eq!(
        baseline.conflicts(),
        0,
        "identical settings cannot conflict"
    );

    // IntelliJ changes only `tabs`; PyCharm changes only `wrap`.
    machine.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "8"), ("wrap", "true")],
    );
    machine.write_option(
        "PyCharm2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "4"), ("wrap", "false")],
    );
    let report = machine.sync();

    assert_eq!(report.conflicts(), 0, "different settings must not collide");
    for ide in ["IntelliJIdea2026.2", "PyCharm2026.2"] {
        assert_eq!(
            machine
                .read_option(ide, "editor.xml", "Editor/tabs")
                .as_deref(),
            Some("8"),
            "{ide} should have IntelliJ's tab change"
        );
        assert_eq!(
            machine
                .read_option(ide, "editor.xml", "Editor/wrap")
                .as_deref(),
            Some("false"),
            "{ide} should have PyCharm's wrap change"
        );
    }
}

#[test]
fn a_second_machine_adopts_the_shared_settings() {
    let remote = bare_remote();
    let first = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    first.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "2"), ("fontSize", "15")],
    );
    first.sync();

    // A brand new machine, with the IDE present but no settings of its own.
    let second = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    let report = second.sync();

    assert_eq!(
        second
            .read_option("IntelliJIdea2026.2", "editor.xml", "Editor/tabs")
            .as_deref(),
        Some("2")
    );
    assert_eq!(
        second
            .read_option("IntelliJIdea2026.2", "editor.xml", "Editor/fontSize")
            .as_deref(),
        Some("15")
    );
    assert_eq!(report.conflicts(), 0, "adoption is not a conflict");
}

#[test]
fn the_same_setting_changed_on_two_machines_is_one_reported_conflict() {
    let remote = bare_remote();
    let first = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    first.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "2")],
    );
    first.sync();

    let second = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    second.sync();

    // Both machines now change the same setting, differently.
    first.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "4")],
    );
    first.sync();
    second.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "8")],
    );
    let report = second.sync();

    assert_eq!(
        report.conflicts(),
        1,
        "exactly one conflict, named precisely"
    );
    let conflict = report
        .from_remote
        .iter()
        .flat_map(|file| &file.conflicts)
        .chain(
            report
                .ides
                .iter()
                .flat_map(|ide| &ide.files)
                .flat_map(|file| &file.conflicts),
        )
        .next()
        .expect("the conflict should be reported");
    assert_eq!(conflict.setting, "Editor/tabs");

    // Default policy keeps the value on the machine running the sync.
    assert_eq!(
        second
            .read_option("IntelliJIdea2026.2", "editor.xml", "Editor/tabs")
            .as_deref(),
        Some("8")
    );
}

#[test]
fn preferring_remote_takes_the_incoming_value_instead() {
    let remote = bare_remote();
    let first = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    first.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "2")],
    );
    first.sync();

    let second = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    second.sync();

    first.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "4")],
    );
    first.sync();
    second.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "8")],
    );
    second.sync_with(ConflictPolicy::PreferRemote);

    assert_eq!(
        second
            .read_option("IntelliJIdea2026.2", "editor.xml", "Editor/tabs")
            .as_deref(),
        Some("4")
    );
}

#[test]
fn a_settled_sync_reports_nothing_and_stays_settled() {
    let remote = bare_remote();
    let machine = Machine::new(remote.path(), &["IntelliJIdea2026.2", "CLion2026.2"]);
    machine.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "2")],
    );

    machine.sync();
    let second_run = machine.sync();
    assert!(
        second_run.is_empty(),
        "a sync with nothing to do must converge in one run"
    );
}

#[test]
fn settings_the_ide_owns_survive_a_sync() {
    let remote = bare_remote();
    let machine = Machine::new(remote.path(), &["IntelliJIdea2026.2", "CLion2026.2"]);

    // ide.general.xml carries a real setting plus registry keys the IDE set for
    // itself. The registry keys must not reach the store, but must also not be
    // stripped out of the IDE's own file.
    let path = machine
        .jetbrains_root
        .join("IntelliJIdea2026.2/options/ide.general.xml");
    std::fs::write(
        &path,
        r#"<?xml version='1.0' encoding='utf-8'?>
<application>
  <component name="GeneralSettings">
    <option name="reopenLastProject" value="false" />
  </component>
  <component name="Registry">
    <entry key="ide.experimental.ui" value="true" source="SYSTEM" />
  </component>
</application>"#,
    )
    .unwrap();
    machine.write_option("CLion2026.2", "editor.xml", "Editor", &[("tabs", "2")]);
    machine.sync();

    let after = std::fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("ide.experimental.ui"),
        "pruning decides what is shared, not what the IDE keeps"
    );

    let engine = Engine::open(Some(machine.config_dir.clone())).unwrap();
    let stored =
        std::fs::read_to_string(engine.store_root().join("shared/options/ide.general.xml"))
            .unwrap();
    assert!(stored.contains("reopenLastProject"), "real settings sync");
    assert!(
        !stored.contains("ide.experimental.ui"),
        "IDE-owned registry keys are not user choices"
    );
}

/// An installer lays down a full `options/` of factory defaults before the IDE
/// has ever run. Harvesting those would publish the product's defaults as if
/// they were choices, and writing into that directory races the first-run
/// import wizard.
#[test]
fn an_ide_that_has_never_been_launched_takes_no_part() {
    let remote = bare_remote();
    let machine = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    machine.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "4")],
    );

    // A second IDE as an installer leaves it: options/ populated, but none of
    // the markers the platform writes on first start.
    let fresh = machine.jetbrains_root.join("WebStorm2026.2");
    std::fs::create_dir_all(fresh.join("options")).unwrap();
    std::fs::write(
        fresh.join("options/editor.xml"),
        "<?xml version='1.0' encoding='utf-8'?>\n<application>\n  <component name=\"Editor\">\n    <option name=\"tabs\" value=\"99\" />\n  </component>\n</application>",
    )
    .unwrap();

    let report = machine.sync();

    let webstorm = report
        .ides
        .iter()
        .find(|ide| ide.directory == "WebStorm2026.2")
        .expect("a skipped IDE is still reported, not silently dropped");
    assert!(
        webstorm.skipped.is_some(),
        "a never-launched IDE must be skipped"
    );

    let store = Engine::open(Some(machine.config_dir.clone()))
        .unwrap()
        .store_root()
        .to_path_buf();
    let stored = std::fs::read_to_string(store.join("shared/options/editor.xml")).unwrap();
    assert!(
        stored.contains("\"4\"") && !stored.contains("\"99\""),
        "the factory default must not reach the store, got: {stored}"
    );
}

/// An IDE nobody has opened is the only clean sample of a product's factory
/// defaults. Capturing it lets jbsync tell a choice from a shipped value.
#[test]
fn a_never_launched_ide_teaches_which_values_are_defaults() {
    let remote = bare_remote();
    let machine = Machine::new(remote.path(), &["WebStorm2026.2"]);

    // The user changed `tabs` and left `wrap` alone.
    machine.write_option(
        "WebStorm2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "8"), ("wrap", "true")],
    );

    // A second install of the same product, never opened: both values as
    // shipped. Same product name, so it is the same defaults record.
    let fresh = machine.jetbrains_root.join("WebStorm2025.3");
    std::fs::create_dir_all(fresh.join("options")).unwrap();
    std::fs::write(
        fresh.join("options/editor.xml"),
        "<?xml version='1.0' encoding='utf-8'?>\n<application>\n  <component name=\"Editor\">\n    <option name=\"tabs\" value=\"4\" />\n    <option name=\"wrap\" value=\"true\" />\n  </component>\n</application>",
    )
    .unwrap();

    machine.sync();

    let store = Engine::open(Some(machine.config_dir.clone()))
        .unwrap()
        .store_root()
        .to_path_buf();
    assert!(
        store.join("defaults/WebStorm.toml").exists(),
        "the untouched install's defaults are recorded and shared"
    );

    let stored = std::fs::read_to_string(store.join("shared/options/editor.xml")).unwrap();
    assert!(
        stored.contains("tabs") && stored.contains("\"8\""),
        "a changed value is a choice and must sync: {stored}"
    );
    assert!(
        !stored.contains("wrap"),
        "a value still at its shipped default is not a choice: {stored}"
    );
}

/// The whole point of remembering the manifest: a machine where JetBrains'
/// Backup and Sync has never run must still sync the same files.
#[test]
fn a_machine_with_no_settings_sync_tree_inherits_the_manifest() {
    let remote = bare_remote();

    // The first machine has run Backup and Sync, so it can prove that this
    // otherwise unremarkable file roams.
    let first = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    let tree = first
        .jetbrains_root
        .join("IntelliJIdea2026.2/settingsSync/options");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::write(tree.join("unusual.xml"), "<application />").unwrap();
    first.write_option(
        "IntelliJIdea2026.2",
        "unusual.xml",
        "Unusual",
        &[("on", "true")],
    );
    first.sync();

    let store = Engine::open(Some(first.config_dir.clone()))
        .unwrap()
        .store_root()
        .to_path_buf();
    assert!(
        store.join("manifest.toml").exists(),
        "the learned manifest is published to the store"
    );
    assert!(store.join("shared/options/unusual.xml").exists());

    // A second machine with no settingsSync tree anywhere still adopts it.
    let second = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    assert!(
        !second
            .jetbrains_root
            .join("IntelliJIdea2026.2/settingsSync")
            .exists()
    );
    second.sync();

    assert_eq!(
        second
            .read_option("IntelliJIdea2026.2", "unusual.xml", "Unusual/on")
            .as_deref(),
        Some("true"),
        "the manifest travelled, so the file did too"
    );
}

/// A machine that must keep one file to itself says so in `machines/<id>.toml`,
/// which lives in the store so the exclusion follows the machine.
#[test]
fn a_machine_can_exclude_a_file_from_its_own_sync() {
    let remote = bare_remote();
    let machine = Machine::new(remote.path(), &["IntelliJIdea2026.2", "PyCharm2026.2"]);
    std::fs::write(
        machine.config_dir.join("config.toml"),
        format!(
            "[repo]\nremote = {:?}\n\n[jetbrains]\nroot = {:?}\n\n[machine]\nid = \"laptop\"\n",
            remote.path().to_string_lossy(),
            machine.jetbrains_root.to_string_lossy()
        ),
    )
    .unwrap();

    machine.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "4")],
    );
    // editor-font.xml syncs by default, so excluding it proves the machine
    // override is consulted rather than the built-in exclusion list.
    machine.write_option(
        "IntelliJIdea2026.2",
        "editor-font.xml",
        "DefaultFontConfiguration",
        &[("FONT_SIZE", "18")],
    );

    // Opening the engine creates the store, so the override can be placed
    // before anything has been published.
    let store = Engine::open(Some(machine.config_dir.clone()))
        .unwrap()
        .store_root()
        .to_path_buf();
    std::fs::create_dir_all(store.join("machines")).unwrap();
    std::fs::write(
        store.join("machines/laptop.toml"),
        "[jetbrains]\nexclude = [\"options/editor-font.xml\"]\n",
    )
    .unwrap();

    machine.sync();

    assert!(
        store.join("shared/options/editor.xml").exists(),
        "an ordinary file still syncs"
    );
    assert!(
        !store.join("shared/options/editor-font.xml").exists(),
        "the excluded file must not reach the store"
    );
    assert_eq!(
        machine.read_option(
            "PyCharm2026.2",
            "editor-font.xml",
            "DefaultFontConfiguration/FONT_SIZE"
        ),
        None,
        "and must not reach the other IDEs"
    );
}

/// `status` is the command people run before trusting `sync`, so its report has
/// to be the same one `sync` produces. It only is if a dry run buffers the
/// writes it would make and later passes read them back.
#[test]
fn a_dry_run_predicts_exactly_what_a_real_sync_does() {
    fn summarize(report: &jbsync::sync::report::SyncReport) -> Vec<String> {
        let mut lines: Vec<String> = report
            .ides
            .iter()
            .flat_map(|ide| {
                ide.files.iter().flat_map(move |file| {
                    let incoming = file.incoming.iter().map(move |change| {
                        format!("< {} {} {}", ide.directory, file.path, change.setting)
                    });
                    let outgoing = file.outgoing.iter().map(move |change| {
                        format!("> {} {} {}", ide.directory, file.path, change.setting)
                    });
                    incoming.chain(outgoing)
                })
            })
            .collect();
        lines.sort();
        lines
    }

    let remote = bare_remote();
    // Several IDEs holding different files is what forces multiple passes, and
    // multiple passes are where a dry run used to diverge from a real one.
    let machine = Machine::new(
        remote.path(),
        &["IntelliJIdea2026.2", "PyCharm2026.2", "CLion2026.2"],
    );
    machine.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "2")],
    );
    machine.write_option(
        "PyCharm2026.2",
        "laf.xml",
        "LafManager",
        &[("theme", "Dark")],
    );
    machine.write_option("CLion2026.2", "debugger.xml", "Debugger", &[("steps", "9")]);

    let predicted = summarize(&machine.dry_run());
    let actual = summarize(&machine.sync());

    assert_eq!(
        predicted, actual,
        "a dry run must report the same changes the real sync makes"
    );
    assert!(
        !predicted.is_empty(),
        "the fixture should produce changes to compare"
    );
    // And nothing contradictory: no file both gains and loses the same setting.
    for line in &predicted {
        let flipped = if let Some(rest) = line.strip_prefix("< ") {
            format!("> {rest}")
        } else {
            format!("< {}", &line[2..])
        };
        assert!(
            !predicted.contains(&flipped),
            "{line} is reported in both directions"
        );
    }
}

/// The reason `project.default.xml` is synced at all: *Settings for New
/// Projects* are choices, and before this they were the one class of setting
/// jbsync could see and still declined to carry.
///
/// The same file holds dialog geometry in this machine's screen coordinates,
/// which is why the platform skips it wholesale. Pruning per component instead
/// of per file is what makes the trade unnecessary — so both halves are
/// asserted here, in both directions.
#[test]
fn settings_for_new_projects_reach_another_machine_without_the_dialog_state() {
    const DEFAULT_PROJECT: &str = r##"<?xml version='1.0' encoding='utf-8'?>
<application>
  <component name="ProjectManager">
    <defaultProject>
      <component name="TypeScriptCompiler">
        <option name="memoryAutoIncrease" value="true" />
      </component>
      <component name="WindowStateProjectService">
        <state x="356" y="76" key="#Plugins" timestamp="1785330308665">
          <screen x="0" y="33" width="1512" height="876" />
        </state>
      </component>
    </defaultProject>
  </component>
</application>"##;

    let remote = bare_remote();
    let first = Machine::new(remote.path(), &["WebStorm2026.2"]);
    first.write_file("WebStorm2026.2", "project.default.xml", DEFAULT_PROJECT);
    first.sync();

    let second = Machine::new(remote.path(), &["WebStorm2026.2"]);
    second.sync();

    assert_eq!(
        second
            .read_option(
                "WebStorm2026.2",
                "project.default.xml",
                "New Projects/TypeScriptCompiler/memoryAutoIncrease"
            )
            .as_deref(),
        Some("true"),
        "the setting the user chose has to arrive"
    );
    // Asserted against the file's text rather than a projected address: the
    // claim is that none of this component reached the other machine, and a
    // mistyped address would make a narrower assertion pass for free.
    let arrived = second.read_file("WebStorm2026.2", "project.default.xml");
    assert!(
        !arrived.contains("WindowStateProjectService"),
        "another machine's window position must not arrive with it: {arrived}"
    );

    // Pruning decides what is shared, never what an IDE keeps: the machine that
    // published still has its own geometry, untouched.
    let published_from = first.read_file("WebStorm2026.2", "project.default.xml");
    assert!(
        published_from.contains(r##"<state x="356" y="76" key="#Plugins""##),
        "the local file is filtered on the way into the store, not edited: {published_from}"
    );
}

/// A machine whose own copy of a file is all residue must still *receive*.
///
/// "Everything in it was pruned" means the IDE has no opinion, which is a
/// reason not to publish and no reason at all to refuse what others published.
/// `project.default.xml` makes the distinction matter: plenty of IDEs have
/// dialog geometry in theirs and nothing else, and those are exactly the ones
/// that need the settings for new projects to arrive.
#[test]
fn an_ide_holding_only_residue_still_adopts_what_others_published() {
    const UI_STATE_ONLY: &str = r##"<?xml version='1.0' encoding='utf-8'?>
<application>
  <component name="ProjectManager">
    <defaultProject>
      <component name="WindowStateProjectService">
        <state x="1" y="2" key="#Plugins" />
      </component>
    </defaultProject>
  </component>
</application>"##;
    const WITH_SETTING: &str = r#"<?xml version='1.0' encoding='utf-8'?>
<application>
  <component name="ProjectManager">
    <defaultProject>
      <component name="TypeScriptCompiler">
        <option name="memoryAutoIncrease" value="true" />
      </component>
    </defaultProject>
  </component>
</application>"#;

    let remote = bare_remote();
    let first = Machine::new(remote.path(), &["WebStorm2026.2"]);
    first.write_file("WebStorm2026.2", "project.default.xml", WITH_SETTING);
    first.sync();

    let second = Machine::new(remote.path(), &["WebStorm2026.2"]);
    second.write_file("WebStorm2026.2", "project.default.xml", UI_STATE_ONLY);
    second.sync();

    assert_eq!(
        second
            .read_option(
                "WebStorm2026.2",
                "project.default.xml",
                "New Projects/TypeScriptCompiler/memoryAutoIncrease"
            )
            .as_deref(),
        Some("true"),
        "an IDE with nothing to say must still be told"
    );
    // And it published nothing of its own: the geometry stayed at home.
    let stored = std::fs::read_to_string(
        Engine::open(Some(second.config_dir.clone()))
            .unwrap()
            .store_root()
            .join("shared/options/project.default.xml"),
    )
    .unwrap();
    assert!(!stored.contains("WindowStateProjectService"), "{stored}");
}

/// Excluding a file has to keep working after somebody else publishes it.
///
/// Discovery leaves the file out, but the store contributes its own list of
/// paths to reconcile. Without the exclusion applying to those too, the file
/// arrives from the store and is written into the very IDE that excluded it.
#[test]
fn an_exclusion_holds_even_once_another_machine_has_published_the_file() {
    let remote = bare_remote();
    let first = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    first.write_option(
        "IntelliJIdea2026.2",
        "editor-font.xml",
        "DefaultFontConfiguration",
        &[("FONT_SIZE", "18")],
    );
    first.write_option(
        "IntelliJIdea2026.2",
        "editor.xml",
        "Editor",
        &[("tabs", "2")],
    );
    first.sync();

    let second = Machine::new(remote.path(), &["IntelliJIdea2026.2"]);
    std::fs::write(
        second.config_dir.join("config.toml"),
        format!(
            "[repo]\nremote = {:?}\n\n[jetbrains]\nroot = {:?}\n\n[machine]\nid = \"laptop\"\n",
            remote.path().to_string_lossy(),
            second.jetbrains_root.to_string_lossy()
        ),
    )
    .unwrap();
    let store = Engine::open(Some(second.config_dir.clone()))
        .unwrap()
        .store_root()
        .to_path_buf();
    std::fs::create_dir_all(store.join("machines")).unwrap();
    std::fs::write(
        store.join("machines/laptop.toml"),
        "[jetbrains]\nexclude = [\"options/editor-font.xml\"]\n",
    )
    .unwrap();
    second.sync();

    assert!(
        second
            .read_file("IntelliJIdea2026.2", "editor-font.xml")
            .is_empty(),
        "an excluded file must not be created from the store"
    );
    assert_eq!(
        second
            .read_option("IntelliJIdea2026.2", "editor.xml", "Editor/tabs")
            .as_deref(),
        Some("2"),
        "and everything else must still arrive"
    );
}
