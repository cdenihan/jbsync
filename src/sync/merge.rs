//! Three-way merge, one setting at a time.
//!
//! Every merge answers the same question for three inputs — the last state
//! both sides agreed on (`base`), what this machine has now (`local`), and what
//! the other side has now (`remote`):
//!
//!   * both sides agree            -> take it, nothing happened
//!   * only `remote` moved         -> take `remote`, report it as incoming
//!   * only `local` moved          -> take `local`, report it as outgoing
//!   * both moved, differently     -> a conflict, resolved by policy
//!
//! Doing this on the *projection* rather than on file text is what makes two
//! IDEs editing different settings in the same file a non-event. Git's line
//! merge would call that a conflict, and its output would not always be
//! well-formed XML.

use crate::{
    settings::roamable::is_tombstone,
    xml::{dom, project},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

/// What to do when both sides changed the same setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    /// The machine running the sync wins. Predictable, and it means the
    /// settings you can see in front of you are the ones that survive.
    #[default]
    PreferLocal,
    PreferRemote,
    /// Refuse to merge, leaving both sides untouched.
    Fail,
}

impl ConflictPolicy {
    fn winner(self) -> Option<Side> {
        match self {
            Self::PreferLocal => Some(Side::Local),
            Self::PreferRemote => Some(Side::Remote),
            Self::Fail => None,
        }
    }
}

/// A single setting changing value. `None` means the setting is absent, which
/// for JetBrains means "left at its default".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Raw projection address, used to apply the change to a real document.
    /// Empty when the change is about the file as a whole.
    pub path: String,
    /// The same address rewritten for people to read.
    pub setting: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub setting: String,
    pub local: Option<String>,
    pub remote: Option<String>,
    pub resolved_to: Option<Side>,
}

#[derive(Debug, Clone, Default)]
pub struct FileMerge {
    /// Merged bytes, or `None` when the file should not exist.
    pub content: Option<Vec<u8>>,
    /// Settings this machine is taking from the other side.
    pub incoming: Vec<Change>,
    /// Settings this machine is contributing.
    pub outgoing: Vec<Change>,
    pub conflicts: Vec<Conflict>,
}

impl FileMerge {
    pub fn is_noop(&self) -> bool {
        self.incoming.is_empty() && self.outgoing.is_empty() && self.conflicts.is_empty()
    }
}

fn as_text(bytes: Option<&[u8]>) -> Option<&str> {
    bytes.and_then(|raw| std::str::from_utf8(raw).ok())
}

/// A `DELETED` tombstone means the same thing as an absent file.
fn normalize(bytes: Option<&[u8]>) -> Option<&[u8]> {
    match as_text(bytes) {
        Some(text) if is_tombstone(text) => None,
        _ => bytes,
    }
}

/// Merges one store file. `label` is only used to decide whether to parse as
/// XML and to describe changes.
pub fn merge_file(
    base: Option<&[u8]>,
    local: Option<&[u8]>,
    remote: Option<&[u8]>,
    policy: ConflictPolicy,
) -> FileMerge {
    let (base, local, remote) = (normalize(base), normalize(local), normalize(remote));

    // Both sides agree; there is nothing to do regardless of the base.
    if local == remote {
        return FileMerge {
            content: local.map(<[u8]>::to_vec),
            ..FileMerge::default()
        };
    }

    // When both sides have the file, always reconcile it entry by entry — even
    // if only one side moved. The three-way logic reaches the same answer as a
    // wholesale copy would, but it can say *which settings* are arriving,
    // which is the whole point of the report.
    if let (Some(local_text), Some(remote_text)) = (as_text(local), as_text(remote)) {
        if let (Ok(local_doc), Ok(remote_doc)) = (dom::parse(local_text), dom::parse(remote_text)) {
            let base_doc = as_text(base).and_then(|text| dom::parse(text).ok());
            return merge_documents(base_doc.as_ref(), &local_doc, &remote_doc, policy);
        }
        return merge_text(as_text(base), local_text, remote_text, policy);
    }

    // One side does not have the file at all, so this is an add or a delete.
    if local == base {
        return whole_file(local, remote, Side::Remote);
    }
    if remote == base {
        return whole_file(remote, local, Side::Local);
    }
    binary_conflict(local, remote, policy)
}

/// One side is unchanged, so the other side's version wins outright.
fn whole_file(unchanged: Option<&[u8]>, moved: Option<&[u8]>, side: Side) -> FileMerge {
    let describe = |value: Option<&[u8]>| {
        value.map(|_| {
            if side == Side::Remote {
                "updated remotely".to_string()
            } else {
                "updated locally".to_string()
            }
        })
    };
    let change = Change {
        path: String::new(),
        setting: match (unchanged.is_some(), moved.is_some()) {
            (true, false) => "(file removed)".to_string(),
            (false, true) => "(file added)".to_string(),
            _ => "(file contents)".to_string(),
        },
        from: describe(unchanged),
        to: describe(moved),
    };
    let mut merge = FileMerge {
        content: moved.map(<[u8]>::to_vec),
        ..FileMerge::default()
    };
    match side {
        Side::Remote => merge.incoming.push(change),
        Side::Local => merge.outgoing.push(change),
    }
    merge
}

/// The real work: reconcile two documents setting by setting.
fn merge_documents(
    base: Option<&dom::Element>,
    local: &dom::Element,
    remote: &dom::Element,
    policy: ConflictPolicy,
) -> FileMerge {
    let base_view = base.map(project::project).unwrap_or_default();
    let local_view = project::project(local);
    let remote_view = project::project(remote);

    let mut merged = local.clone();
    let mut result = FileMerge::default();

    let mut paths: Vec<&String> = local_view.keys().chain(remote_view.keys()).collect();
    paths.sort_unstable();
    paths.dedup();

    for path in paths {
        let in_base = base_view.get(path);
        let in_local = local_view.get(path);
        let in_remote = remote_view.get(path);

        if in_local == in_remote {
            continue;
        }
        let readable = project::sugar(path);

        let take_remote = if in_local == in_base {
            true
        } else if in_remote == in_base {
            false
        } else {
            let winner = policy.winner();
            result.conflicts.push(Conflict {
                setting: readable.clone(),
                local: in_local.cloned(),
                remote: in_remote.cloned(),
                resolved_to: winner,
            });
            match winner {
                Some(side) => side == Side::Remote,
                // Leave the local value in place; the caller aborts anyway.
                None => false,
            }
        };

        if take_remote {
            result.incoming.push(Change {
                path: path.clone(),
                setting: readable,
                from: in_local.cloned(),
                to: in_remote.cloned(),
            });
            match in_remote {
                Some(value) => {
                    project::set_leaf(&mut merged, remote, path, value);
                }
                None => project::remove_leaf(&mut merged, path),
            }
        } else if in_remote != in_local {
            result.outgoing.push(Change {
                path: path.clone(),
                setting: readable,
                from: in_remote.cloned(),
                to: in_local.cloned(),
            });
        }
    }

    project::prune_empty(&mut merged);
    result.content = Some(dom::serialize(&merged).into_bytes());
    result
}

/// Splits a JVM flag into the key two machines could disagree about, so
/// `-Xmx4g` and `-Xmx8g` conflict while `-Xmx4g` and `-XX:+UseZGC` do not.
fn flag_key(line: &str) -> String {
    let trimmed = line.trim();
    if let Some((name, _)) = trimmed.split_once('=') {
        return name.to_string();
    }
    // Size-suffixed flags such as -Xmx4g / -Xms512m.
    let stripped = trimmed.trim_end_matches(['k', 'K', 'm', 'M', 'g', 'G']);
    stripped
        .trim_end_matches(|character: char| character.is_ascii_digit())
        .to_string()
}

/// Line-oriented merge for plain-text settings, chiefly `.vmoptions`.
///
/// Treated as a set of flags: additions from both sides are kept, deletions
/// from either side are honoured, and only two different values for the *same*
/// flag count as a conflict.
fn merge_text(base: Option<&str>, local: &str, remote: &str, policy: ConflictPolicy) -> FileMerge {
    let lines = |text: &str| -> Vec<String> {
        text.lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    };
    let base_lines = base.map(lines).unwrap_or_default();
    let local_lines = lines(local);
    let remote_lines = lines(remote);

    let mut result = FileMerge::default();
    let mut merged: Vec<String> = Vec::new();

    let added = |current: &[String]| -> Vec<String> {
        current
            .iter()
            .filter(|line| !base_lines.contains(line))
            .cloned()
            .collect()
    };
    let local_added = added(&local_lines);
    let remote_added = added(&remote_lines);

    // A base line survives unless one side deliberately removed it.
    for line in &base_lines {
        let kept_locally = local_lines.contains(line);
        let kept_remotely = remote_lines.contains(line);
        if kept_locally && kept_remotely {
            merged.push(line.clone());
            continue;
        }
        let change = Change {
            path: String::new(),
            setting: flag_key(line),
            from: Some(line.clone()),
            to: None,
        };
        if kept_locally {
            result.incoming.push(change);
        } else {
            result.outgoing.push(change);
        }
    }

    // Additions. The same flag carrying different values on the two sides is
    // the only thing that can conflict here.
    for line in &local_added {
        let key = flag_key(line);
        match remote_added.iter().find(|other| flag_key(other) == key) {
            Some(other) if other == line => {
                // Both machines added the identical flag: not a change at all.
                merged.push(line.clone());
            }
            Some(other) => {
                let winner = policy.winner();
                result.conflicts.push(Conflict {
                    setting: key,
                    local: Some(line.clone()),
                    remote: Some(other.clone()),
                    resolved_to: winner,
                });
                merged.push(if winner == Some(Side::Remote) {
                    other.clone()
                } else {
                    line.clone()
                });
            }
            None => {
                merged.push(line.clone());
                result.outgoing.push(Change {
                    path: String::new(),
                    setting: key,
                    from: None,
                    to: Some(line.clone()),
                });
            }
        }
    }
    for line in &remote_added {
        let key = flag_key(line);
        // Anything sharing a key with a local addition was settled above.
        if local_added.iter().any(|other| flag_key(other) == key) {
            continue;
        }
        merged.push(line.clone());
        result.incoming.push(Change {
            path: String::new(),
            setting: key,
            from: None,
            to: Some(line.clone()),
        });
    }

    let mut text = merged.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    result.content = Some(text.into_bytes());
    result
}

/// Whole-file three-way merge, for store files that are not IDE settings
/// (`sync.toml`, `plugins.json`). No structural knowledge is applied: the side
/// that moved wins, and if both moved the policy decides.
pub fn merge_opaque(
    base: Option<&[u8]>,
    local: Option<&[u8]>,
    remote: Option<&[u8]>,
    policy: ConflictPolicy,
) -> FileMerge {
    if local == remote {
        return FileMerge {
            content: local.map(<[u8]>::to_vec),
            ..FileMerge::default()
        };
    }
    if local == base {
        return whole_file(local, remote, Side::Remote);
    }
    if remote == base {
        return whole_file(remote, local, Side::Local);
    }
    binary_conflict(local, remote, policy)
}

fn binary_conflict(
    local: Option<&[u8]>,
    remote: Option<&[u8]>,
    policy: ConflictPolicy,
) -> FileMerge {
    let winner = policy.winner();
    let content = match winner {
        Some(Side::Remote) => remote,
        _ => local,
    };
    FileMerge {
        content: content.map(<[u8]>::to_vec),
        conflicts: vec![Conflict {
            setting: "(whole file)".to_string(),
            local: local.map(|_| "local version".to_string()),
            remote: remote.map(|_| "remote version".to_string()),
            resolved_to: winner,
        }],
        ..FileMerge::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"<application>
  <component name="Editor">
    <option name="tabs" value="4" />
    <option name="wrap" value="false" />
  </component>
</application>"#;

    fn merge(base: &str, local: &str, remote: &str) -> FileMerge {
        merge_file(
            Some(base.as_bytes()),
            Some(local.as_bytes()),
            Some(remote.as_bytes()),
            ConflictPolicy::PreferLocal,
        )
    }

    fn value_of(merge: &FileMerge, setting: &str) -> Option<String> {
        let text = String::from_utf8(merge.content.clone()?).ok()?;
        let document = dom::parse(&text).ok()?;
        project::project(&document)
            .into_iter()
            .find(|(path, _)| project::sugar(path) == setting)
            .map(|(_, value)| value)
    }

    #[test]
    fn disjoint_edits_in_one_file_both_survive() {
        let local = BASE.replace(r#"name="tabs" value="4""#, r#"name="tabs" value="2""#);
        let remote = BASE.replace(
            r#"name="wrap" value="false""#,
            r#"name="wrap" value="true""#,
        );
        let merged = merge(BASE, &local, &remote);

        assert!(
            merged.conflicts.is_empty(),
            "different settings never clash"
        );
        assert_eq!(value_of(&merged, "Editor/tabs").as_deref(), Some("2"));
        assert_eq!(value_of(&merged, "Editor/wrap").as_deref(), Some("true"));
        assert_eq!(merged.incoming.len(), 1);
        assert_eq!(merged.outgoing.len(), 1);
    }

    #[test]
    fn the_same_setting_changed_twice_is_a_conflict() {
        let local = BASE.replace(r#"name="tabs" value="4""#, r#"name="tabs" value="2""#);
        let remote = BASE.replace(r#"name="tabs" value="4""#, r#"name="tabs" value="8""#);
        let merged = merge(BASE, &local, &remote);

        assert_eq!(merged.conflicts.len(), 1);
        let conflict = &merged.conflicts[0];
        assert_eq!(conflict.setting, "Editor/tabs");
        assert_eq!(conflict.local.as_deref(), Some("2"));
        assert_eq!(conflict.remote.as_deref(), Some("8"));
        assert_eq!(value_of(&merged, "Editor/tabs").as_deref(), Some("2"));
    }

    #[test]
    fn conflict_policy_can_prefer_the_remote_value() {
        let local = BASE.replace(r#"name="tabs" value="4""#, r#"name="tabs" value="2""#);
        let remote = BASE.replace(r#"name="tabs" value="4""#, r#"name="tabs" value="8""#);
        let merged = merge_file(
            Some(BASE.as_bytes()),
            Some(local.as_bytes()),
            Some(remote.as_bytes()),
            ConflictPolicy::PreferRemote,
        );
        assert_eq!(value_of(&merged, "Editor/tabs").as_deref(), Some("8"));
    }

    #[test]
    fn a_setting_added_remotely_arrives() {
        let remote = BASE.replace(
            "</component>",
            r#"  <option name="fontSize" value="14" />
  </component>"#,
        );
        let merged = merge(BASE, BASE, &remote);
        assert_eq!(value_of(&merged, "Editor/fontSize").as_deref(), Some("14"));
        assert_eq!(merged.incoming.len(), 1);
    }

    #[test]
    fn reverting_a_setting_to_its_default_propagates_as_a_removal() {
        let remote = BASE.replace(
            r#"    <option name="wrap" value="false" />
"#,
            "",
        );
        let merged = merge(BASE, BASE, &remote);
        assert_eq!(value_of(&merged, "Editor/wrap"), None);
        assert_eq!(merged.incoming.len(), 1);
        assert_eq!(merged.incoming[0].to, None);
    }

    #[test]
    fn identical_edits_on_both_sides_are_not_a_conflict() {
        let changed = BASE.replace(r#"value="4""#, r#"value="2""#);
        let merged = merge(BASE, &changed, &changed);
        assert!(merged.conflicts.is_empty());
        assert!(merged.is_noop());
    }

    #[test]
    fn a_tombstone_is_treated_as_an_absent_file() {
        let merged = merge_file(
            Some(BASE.as_bytes()),
            Some(b"DELETED"),
            Some(BASE.as_bytes()),
            ConflictPolicy::PreferLocal,
        );
        assert_eq!(
            merged.content, None,
            "local deletion wins over an idle remote"
        );
    }

    #[test]
    fn merged_output_is_always_parseable() {
        let local = BASE.replace(r#"name="tabs" value="4""#, r#"name="tabs" value="2""#);
        let remote = BASE.replace(
            r#"name="wrap" value="false""#,
            r#"name="wrap" value="true""#,
        );
        let merged = merge(BASE, &local, &remote);
        let text = String::from_utf8(merged.content.unwrap()).unwrap();
        assert!(dom::parse(&text).is_ok());
    }

    #[test]
    fn vmoptions_additions_from_both_machines_are_kept() {
        let merged = merge_file(
            Some(b"-Xmx4g\n"),
            Some(b"-Xmx4g\n-XX:+UseZGC\n"),
            Some(b"-Xmx4g\n-Dfile.encoding=UTF-8\n"),
            ConflictPolicy::PreferLocal,
        );
        let text = String::from_utf8(merged.content.unwrap()).unwrap();
        assert!(text.contains("-XX:+UseZGC"));
        assert!(text.contains("-Dfile.encoding=UTF-8"));
        assert!(text.contains("-Xmx4g"));
        assert!(merged.conflicts.is_empty());
    }

    #[test]
    fn the_same_jvm_flag_with_two_values_conflicts() {
        let merged = merge_file(
            Some(b"-Xms512m\n"),
            Some(b"-Xms512m\n-Xmx4g\n"),
            Some(b"-Xms512m\n-Xmx16g\n"),
            ConflictPolicy::PreferLocal,
        );
        assert_eq!(merged.conflicts.len(), 1);
        assert_eq!(merged.conflicts[0].setting, "-Xmx");
        let text = String::from_utf8(merged.content.unwrap()).unwrap();
        assert!(text.contains("-Xmx4g"), "local preference applied");
        assert!(!text.contains("-Xmx16g"));
    }

    #[test]
    fn flag_key_groups_values_of_the_same_option() {
        assert_eq!(flag_key("-Xmx4g"), flag_key("-Xmx16g"));
        assert_eq!(flag_key("-Dfile.encoding=UTF-8"), "-Dfile.encoding");
        assert_ne!(flag_key("-Xmx4g"), flag_key("-Xms4g"));
    }

    #[test]
    fn fail_policy_reports_the_conflict_without_choosing() {
        let local = BASE.replace(r#"value="4""#, r#"value="2""#);
        let remote = BASE.replace(r#"value="4""#, r#"value="8""#);
        let merged = merge_file(
            Some(BASE.as_bytes()),
            Some(local.as_bytes()),
            Some(remote.as_bytes()),
            ConflictPolicy::Fail,
        );
        assert_eq!(merged.conflicts.len(), 1);
        assert_eq!(merged.conflicts[0].resolved_to, None);
    }
}
