//! Keeping only settings the user actually chose.
//!
//! The IntelliJ platform already does most of this work: when a component's
//! state equals the state produced by its default constructor, nothing is
//! written to XML at all. So a live `options/*.xml` is close to a diff against
//! defaults before `jbsync` touches it.
//!
//! What remains is residue of three kinds, and each gets a declarative rule
//! rather than bespoke code:
//!
//!   * components that serialize a whole map, including untouched entries
//!     (tutorial progress, inlay-hint tables);
//!   * values the IDE set for itself rather than for the user (registry keys
//!     carrying `source="SYSTEM"` or `"MANAGER"`, one-shot migration flags);
//!   * components that persist only a schema version and no settings.
//!
//! Rules are data, so covering a newly noisy component is a table entry here
//! or an `[[xml.omit]]` block in `sync.toml` — never a new code path.

use crate::{
    config::XmlOmitRule, settings::defaults::ProductDefaults, xml::dom::Element, xml::project,
};

/// Attributes that describe the file format rather than a user's choice. A
/// component carrying only these, with no children, holds no settings.
const BOOKKEEPING_ATTRIBUTES: [&str; 2] = ["name", "version"];

struct Builtin {
    file: &'static str,
    component: Option<&'static str>,
    element: &'static str,
    attribute: &'static str,
    equals: &'static str,
    reason: &'static str,
}

/// Values written by the IDE for its own purposes. Registry keys record where
/// they came from; anything not attributable to the user is not a setting we
/// want to carry between machines.
const BUILTINS: &[Builtin] = &[
    Builtin {
        file: "options/ide.general.xml",
        component: Some("Registry"),
        element: "entry",
        attribute: "source",
        equals: "SYSTEM",
        reason: "registry key set by the IDE, not the user",
    },
    Builtin {
        file: "options/ide.general.xml",
        component: Some("Registry"),
        element: "entry",
        attribute: "source",
        equals: "MANAGER",
        reason: "registry key managed centrally, not by the user",
    },
    Builtin {
        file: "options/ide-features-trainer.xml",
        component: Some("LessonStateBase"),
        element: "entry",
        attribute: "value",
        equals: "NOT_PASSED",
        reason: "tutorial never started",
    },
    Builtin {
        file: "options/*.xml",
        component: None,
        element: "option",
        attribute: "name",
        equals: "MIGRATE_OLD_SETTINGS",
        reason: "one-shot settings migration flag",
    },
    Builtin {
        file: "options/other.xml",
        component: Some("LangManager"),
        element: "entry",
        attribute: "key",
        equals: "JAVA",
        reason: "per-installation language state",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    /// Human-readable address, as shown in reports.
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct PruneOutcome {
    pub removed: Vec<Removal>,
    /// True when nothing worth storing survived, so the file itself can be
    /// dropped from the store.
    pub is_empty: bool,
}

fn file_matches(pattern: &str, relative: &str) -> bool {
    globset::Glob::new(pattern).is_ok_and(|glob| glob.compile_matcher().is_match(relative))
}

/// True when `element` is one the rule targets, given the component it sits in.
fn rule_matches(
    element: &Element,
    component: Option<&str>,
    rule_component: Option<&str>,
    rule_element: &str,
    attribute: Option<&str>,
    option: Option<&str>,
    equals: &str,
) -> bool {
    if element.name != rule_element {
        return false;
    }
    if let Some(wanted) = rule_component
        && component != Some(wanted)
    {
        return false;
    }
    if let Some(name) = option {
        return element.attributes.get("name").map(String::as_str) == Some(name);
    }
    let Some(attribute) = attribute else {
        return false;
    };
    element.attributes.get(attribute).map(String::as_str) == Some(equals)
}

/// Removes non-setting content from `root`, reporting what went and why.
/// `defaults`, when present, are the values this product shipped with, captured
/// from an install nobody had opened yet. A setting still holding its default is
/// not a choice, so it is removed here — before the bottom-up cleanup, so the
/// component it lived in disappears too rather than lingering as an empty shell.
pub fn prune_document(
    relative: &str,
    root: &mut Element,
    rules: &[XmlOmitRule],
    use_builtins: bool,
    defaults: Option<&ProductDefaults>,
) -> PruneOutcome {
    let mut removed = Vec::new();

    if use_builtins {
        for rule in BUILTINS
            .iter()
            .filter(|rule| file_matches(rule.file, relative))
        {
            apply(
                root,
                None,
                "",
                rule.component,
                rule.element,
                Some(rule.attribute),
                None,
                rule.equals,
                rule.reason,
                &mut removed,
            );
        }
    }
    for rule in rules
        .iter()
        .filter(|rule| file_matches(&rule.file, relative))
    {
        let reason = format!("excluded by {}", rule.file);
        apply(
            root,
            None,
            "",
            rule.component.as_deref(),
            &rule.element,
            rule.attribute.as_deref(),
            rule.option.as_deref(),
            &rule.equals,
            &reason,
            &mut removed,
        );
    }

    if let Some(defaults) = defaults {
        let untouched: Vec<String> = project::project(root)
            .into_iter()
            .filter(|(address, value)| defaults.value(relative, address) == Some(value.as_str()))
            .map(|(address, _)| address)
            .collect();
        for address in untouched {
            project::remove_leaf(root, &address);
            removed.push(Removal {
                path: project::sugar(&address),
                reason: "unchanged from this product's default".to_string(),
            });
        }
    }

    // Prune bottom-up first: a component only looks settingless once the
    // wrappers its removed entries lived in have themselves gone.
    project::prune_empty(root);
    drop_settingless_components(root, &mut removed);

    PruneOutcome {
        is_empty: root.children.is_empty() && root.attributes.is_empty(),
        removed,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply(
    element: &mut Element,
    component: Option<&str>,
    path: &str,
    rule_component: Option<&str>,
    rule_element: &str,
    attribute: Option<&str>,
    option: Option<&str>,
    equals: &str,
    reason: &str,
    removed: &mut Vec<Removal>,
) {
    // A `<component name=...>` establishes the scope rules are written against.
    let scope = if element.name == "component" {
        element.attributes.get("name").map(String::as_str)
    } else {
        component
    };

    element.children.retain(|child| {
        let matched = rule_matches(
            child,
            scope,
            rule_component,
            rule_element,
            attribute,
            option,
            equals,
        );
        if matched {
            removed.push(Removal {
                path: describe(path, child),
                reason: reason.to_string(),
            });
        }
        !matched
    });

    let scope = scope.map(str::to_string);
    for child in &mut element.children {
        let child_path = describe(path, child);
        apply(
            child,
            scope.as_deref(),
            &child_path,
            rule_component,
            rule_element,
            attribute,
            option,
            equals,
            reason,
            removed,
        );
    }
}

/// A readable address for an element, used only in reports.
fn describe(parent_path: &str, element: &Element) -> String {
    let own = element
        .key_attribute()
        .map_or_else(|| element.name.clone(), |(_, value)| value.to_string());
    if parent_path.is_empty() {
        own
    } else {
        format!("{parent_path}/{own}")
    }
}

/// Drops components that persist a schema version but no settings.
fn drop_settingless_components(root: &mut Element, removed: &mut Vec<Removal>) {
    root.children.retain(|child| {
        let settingless = child.name == "component"
            && child.children.is_empty()
            && child.text.is_none()
            && child
                .attributes
                .keys()
                .all(|key| BOOKKEEPING_ATTRIBUTES.contains(&key.as_str()));
        if settingless {
            removed.push(Removal {
                path: describe("", child),
                reason: "component stores no settings".to_string(),
            });
        }
        !settingless
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::dom::parse;

    fn prune(relative: &str, source: &str) -> (Element, PruneOutcome) {
        let mut root = parse(source).unwrap();
        let outcome = prune_document(relative, &mut root, &[], true, None);
        (root, outcome)
    }

    #[test]
    fn keeps_user_registry_keys_and_drops_ide_owned_ones() {
        let (root, outcome) = prune(
            "options/ide.general.xml",
            r#"<application>
                 <component name="Registry">
                   <entry key="ide.experimental.ui" value="true" source="SYSTEM" />
                   <entry key="trace.url" value="https://x" source="MANAGER" />
                   <entry key="my.tweak" value="true" source="USER" />
                   <entry key="older.style" value="true" />
                 </component>
               </application>"#,
        );
        let kept: Vec<&str> = root.children[0]
            .children
            .iter()
            .filter_map(|entry| entry.attributes.get("key").map(String::as_str))
            .collect();
        assert_eq!(kept, vec!["my.tweak", "older.style"]);
        assert_eq!(outcome.removed.len(), 2);
    }

    /// A setting still holding the value its product shipped with is not a
    /// choice, and the component it lived in should go with it.
    #[test]
    fn a_value_matching_the_products_default_is_not_a_choice() {
        let mut root = crate::xml::dom::parse(
            "<application>\n  <component name=\"Editor\">\n    <option name=\"tabs\" value=\"4\" />\n    <option name=\"wrap\" value=\"true\" />\n  </component>\n</application>",
        )
        .unwrap();
        let mut defaults = ProductDefaults::new("WebStorm", "262.1");
        defaults.record(
            "options/editor.xml",
            crate::xml::project::project(&root).into_iter().collect(),
        );

        // Everything matches the default, so nothing is worth sharing.
        let mut untouched = root.clone();
        let outcome = prune_document(
            "options/editor.xml",
            &mut untouched,
            &[],
            true,
            Some(&defaults),
        );
        assert!(
            outcome.is_empty,
            "an untouched file has no opinion, and leaves no empty shells"
        );

        // Change one of the two; only that one survives.
        let donor = root.clone();
        crate::xml::project::set_leaf(
            &mut root,
            &donor,
            "component[name=Editor]/option[name=tabs]/@value",
            "8",
        );
        let outcome = prune_document("options/editor.xml", &mut root, &[], true, Some(&defaults));
        assert!(!outcome.is_empty);
        let kept: Vec<String> = crate::xml::project::project(&root)
            .into_keys()
            .map(|address| crate::xml::project::sugar(&address))
            .collect();
        assert_eq!(kept, vec!["Editor/tabs".to_string()]);
    }

    #[test]
    fn drops_untouched_tutorial_progress_entirely() {
        let (_, outcome) = prune(
            "options/ide-features-trainer.xml",
            r#"<application>
                 <component name="LessonStateBase">
                   <option name="map">
                     <map>
                       <entry key="actions" value="NOT_PASSED" />
                       <entry key="collapse" value="NOT_PASSED" />
                     </map>
                   </option>
                 </component>
               </application>"#,
        );
        assert!(
            outcome.is_empty,
            "a file of untouched defaults should not reach the store"
        );
    }

    #[test]
    fn keeps_tutorials_the_user_completed() {
        let (root, _) = prune(
            "options/ide-features-trainer.xml",
            r#"<application>
                 <component name="LessonStateBase">
                   <option name="map">
                     <map>
                       <entry key="actions" value="NOT_PASSED" />
                       <entry key="collapse" value="PASSED" />
                     </map>
                   </option>
                 </component>
               </application>"#,
        );
        let flattened = crate::xml::project::project(&root);
        assert_eq!(flattened.len(), 1);
        assert!(flattened.keys().next().unwrap().contains("collapse"));
    }

    #[test]
    fn drops_version_only_components_but_keeps_real_settings() {
        let (root, _) = prune(
            "options/databaseDrivers.xml",
            r#"<application>
                 <component name="LocalDatabaseDriverManager" version="201" />
                 <component name="LafManager" autodetect="true" />
               </application>"#,
        );
        assert_eq!(root.children.len(), 1);
        assert_eq!(
            root.children[0].attributes.get("name").map(String::as_str),
            Some("LafManager")
        );
    }

    #[test]
    fn drops_one_shot_migration_flags() {
        let (_, outcome) = prune(
            "options/diff.xml",
            r#"<application>
                 <component name="ExternalDiffSettings">
                   <option name="MIGRATE_OLD_SETTINGS" value="true" />
                 </component>
               </application>"#,
        );
        assert!(outcome.is_empty);
    }

    #[test]
    fn user_rules_apply_on_top_of_builtins() {
        let mut root = parse(
            r#"<application>
                 <component name="EditorSettings">
                   <option name="SHOW_INTENTION_BULB" value="false" />
                   <option name="IS_CARET_BLINKING" value="true" />
                 </component>
               </application>"#,
        )
        .unwrap();
        let rules = vec![XmlOmitRule {
            file: "options/editor.xml".to_string(),
            component: Some("EditorSettings".to_string()),
            element: "option".to_string(),
            option: Some("SHOW_INTENTION_BULB".to_string()),
            attribute: None,
            equals: String::new(),
        }];
        let outcome = prune_document("options/editor.xml", &mut root, &rules, true, None);
        assert_eq!(outcome.removed.len(), 1);
        assert_eq!(root.children[0].children.len(), 1);
    }

    #[test]
    fn rules_scoped_to_other_files_do_not_fire() {
        let mut root = parse(
            r#"<application><component name="Registry">
                 <entry key="a" value="1" source="SYSTEM" />
               </component></application>"#,
        )
        .unwrap();
        let outcome = prune_document("options/editor.xml", &mut root, &[], true, None);
        assert!(outcome.removed.is_empty());
    }
}
