//! A flat, stable projection of a settings document.
//!
//! Every leaf (an attribute value, or an element's text) gets an address built
//! from its ancestors. Two documents can then be compared, and merged, one
//! setting at a time instead of one file at a time. The projection is a view:
//! the XML tree remains the source of truth, and merged leaves are written
//! back into a real tree so nothing outside the changed setting is disturbed.
//!
//! Address grammar, `/`-separated:
//!   `tag[attr=value]`  element identified by its key attribute
//!   `tag#n`            the n-th sibling sharing `tag`, when it has no key
//!   `@attr`            terminal: an attribute value
//!   `#text`            terminal: character data

use std::collections::BTreeMap;

use super::dom::Element;

/// Leaf address to value. Ordered, so serialization and reports are stable.
pub type Projection = BTreeMap<String, String>;

const TEXT_LEAF: &str = "#text";

/// The address of `element` relative to its parent. `same_tag_index` is the
/// element's position among siblings that share its tag name.
fn segment(element: &Element, same_tag_index: usize) -> String {
    element.key_attribute().map_or_else(
        || format!("{}#{same_tag_index}", element.name),
        |(attribute, value)| format!("{}[{attribute}={value}]", element.name),
    )
}

/// Addresses of every child, paired with its index, in document order.
fn child_segments(parent: &Element) -> Vec<(usize, String)> {
    let mut counters: BTreeMap<&str, usize> = BTreeMap::new();
    parent
        .children
        .iter()
        .enumerate()
        .map(|(index, child)| {
            let counter = counters.entry(child.name.as_str()).or_default();
            let address = segment(child, *counter);
            *counter += 1;
            (index, address)
        })
        .collect()
}

/// Flattens `root` into leaf addresses. The root's own tag is not part of any
/// address: a document is only ever compared against the same file from
/// another machine, so the root is shared context rather than information.
pub fn project(root: &Element) -> Projection {
    let mut flattened = Projection::new();
    collect(root, "", &mut flattened);
    flattened
}

fn collect(element: &Element, prefix: &str, output: &mut Projection) {
    let join = |leaf: &str| {
        if prefix.is_empty() {
            leaf.to_string()
        } else {
            format!("{prefix}/{leaf}")
        }
    };

    // The key attribute is already encoded in this element's own address, so
    // emitting it again would only add noise to every diff.
    let key = element.key_attribute().map(|(attribute, _)| attribute);
    for (attribute, value) in &element.attributes {
        if Some(attribute.as_str()) != key {
            output.insert(join(&format!("@{attribute}")), value.clone());
        }
    }
    if let Some(text) = &element.text {
        output.insert(join(TEXT_LEAF), text.clone());
    }
    for (index, address) in child_segments(element) {
        collect(&element.children[index], &join(&address), output);
    }
}

/// Rewrites an address for display, collapsing JetBrains' boilerplate so a
/// report reads like the settings UI rather than like XPath.
///
/// `component[name=Editor]/option[name=Font]/map#0/entry[key=size]/@value`
/// becomes `Editor/Font/size`.
pub fn sugar(path: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for raw in path.split('/') {
        if matches!(raw, "map#0" | "list#0" | "set#0" | "value#0") {
            continue;
        }
        let simplified = strip_wrapper(raw, "component", "name")
            .or_else(|| strip_wrapper(raw, "option", "name"))
            .or_else(|| strip_wrapper(raw, "entry", "key"));
        match simplified {
            Some(value) => parts.push(value),
            None if raw == "@value" => {}
            None => parts.push(raw.to_string()),
        }
    }
    if parts.is_empty() {
        path.to_string()
    } else {
        parts.join("/")
    }
}

fn strip_wrapper(segment: &str, tag: &str, attribute: &str) -> Option<String> {
    segment
        .strip_prefix(&format!("{tag}[{attribute}="))?
        .strip_suffix(']')
        .map(str::to_string)
}

/// Writes `value` at `path`, creating any missing ancestors by copying their
/// shells from `donor` (the tree the leaf is coming from). Returns false when
/// `donor` does not actually contain the path, which would leave the target in
/// a half-built state.
pub fn set_leaf(target: &mut Element, donor: &Element, path: &str, value: &str) -> bool {
    let mut segments: Vec<&str> = path.split('/').collect();
    let Some(leaf) = segments.pop() else {
        return false;
    };

    let mut cursor = target;
    let mut source = donor;
    for wanted in segments {
        let Some(donor_index) = find_child(source, wanted) else {
            return false;
        };
        source = &source.children[donor_index];
        if let Some(index) = find_child(cursor, wanted) {
            cursor = &mut cursor.children[index];
        } else {
            cursor.children.push(source.shell());
            let last = cursor.children.len() - 1;
            cursor = &mut cursor.children[last];
        }
    }

    if leaf == TEXT_LEAF {
        cursor.text = Some(value.to_string());
    } else if let Some(attribute) = leaf.strip_prefix('@') {
        cursor
            .attributes
            .insert(attribute.to_string(), value.to_string());
    } else {
        return false;
    }
    true
}

/// Removes the leaf at `path`, then drops any ancestor left with nothing to
/// serialize.
pub fn remove_leaf(target: &mut Element, path: &str) {
    let mut segments: Vec<&str> = path.split('/').collect();
    let Some(leaf) = segments.pop() else {
        return;
    };

    let mut cursor = &mut *target;
    for wanted in &segments {
        match find_child(cursor, wanted) {
            Some(index) => cursor = &mut cursor.children[index],
            None => return,
        }
    }
    if leaf == TEXT_LEAF {
        cursor.text = None;
    } else if let Some(attribute) = leaf.strip_prefix('@') {
        cursor.attributes.remove(attribute);
    }
    prune_empty(target);
}

/// Drops elements that no longer carry any value. A parent is only removed
/// once all of its children have been, so pruning is bottom-up.
pub fn prune_empty(element: &mut Element) {
    for child in &mut element.children {
        prune_empty(child);
    }
    element.children.retain(|child| !child.is_valueless());
}

fn find_child(parent: &Element, wanted: &str) -> Option<usize> {
    child_segments(parent)
        .into_iter()
        .find(|(_, address)| address == wanted)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::{super::dom::parse, *};

    fn document() -> Element {
        parse(
            r#"<application>
                 <component name="GeneralSettings">
                   <option name="reopenLastProject" value="false" />
                 </component>
                 <component name="Registry">
                   <entry key="ide.experimental.ui" value="true" source="SYSTEM" />
                 </component>
               </application>"#,
        )
        .unwrap()
    }

    #[test]
    fn projects_each_setting_to_one_address() {
        let flattened = project(&document());
        assert_eq!(
            flattened
                .get("component[name=GeneralSettings]/option[name=reopenLastProject]/@value")
                .map(String::as_str),
            Some("false")
        );
        assert_eq!(
            flattened
                .get("component[name=Registry]/entry[key=ide.experimental.ui]/@source")
                .map(String::as_str),
            Some("SYSTEM")
        );
    }

    #[test]
    fn key_attribute_is_not_repeated_as_a_leaf() {
        let flattened = project(&document());
        assert!(
            !flattened
                .keys()
                .any(|path| path.ends_with("@name") || path.ends_with("@key"))
        );
    }

    #[test]
    fn sugar_reads_like_the_settings_ui() {
        assert_eq!(
            sugar("component[name=GeneralSettings]/option[name=reopenLastProject]/@value"),
            "GeneralSettings/reopenLastProject"
        );
        assert_eq!(
            sugar("component[name=Editor]/option[name=fonts]/map#0/entry[key=size]/@value"),
            "Editor/fonts/size"
        );
        assert_eq!(
            sugar("component[name=Registry]/entry[key=ide.ui]/@source"),
            "Registry/ide.ui/@source"
        );
    }

    #[test]
    fn positional_addresses_disambiguate_keyless_siblings() {
        let parsed = parse(r#"<root><laf themeId="a" /><laf themeId="b" /></root>"#).unwrap();
        let flattened = project(&parsed);
        assert_eq!(
            flattened.get("laf#0/@themeId").map(String::as_str),
            Some("a")
        );
        assert_eq!(
            flattened.get("laf#1/@themeId").map(String::as_str),
            Some("b")
        );
    }

    #[test]
    fn set_leaf_creates_missing_ancestors_from_the_donor() {
        let donor = document();
        let mut target = parse("<application />").unwrap();
        let path = "component[name=GeneralSettings]/option[name=reopenLastProject]/@value";
        assert!(set_leaf(&mut target, &donor, path, "true"));
        assert_eq!(project(&target).get(path).map(String::as_str), Some("true"));
        // Only the requested leaf is grafted, not the donor's other components.
        assert_eq!(target.children.len(), 1);
    }

    #[test]
    fn set_leaf_refuses_paths_the_donor_lacks() {
        let donor = parse("<application />").unwrap();
        let mut target = parse("<application />").unwrap();
        assert!(!set_leaf(
            &mut target,
            &donor,
            "component[name=Nope]/@value",
            "x"
        ));
        assert!(target.children.is_empty());
    }

    #[test]
    fn remove_leaf_prunes_elements_it_empties() {
        let mut target = document();
        remove_leaf(
            &mut target,
            "component[name=GeneralSettings]/option[name=reopenLastProject]/@value",
        );
        let flattened = project(&target);
        assert!(
            !flattened
                .keys()
                .any(|path| path.contains("GeneralSettings"))
        );
        assert!(flattened.keys().any(|path| path.contains("Registry")));
    }

    #[test]
    fn projection_survives_a_serialization_round_trip() {
        let original = document();
        let reparsed = parse(&super::super::dom::serialize(&original)).unwrap();
        assert_eq!(project(&original), project(&reparsed));
    }
}
