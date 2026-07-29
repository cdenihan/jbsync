//! A minimal DOM for JetBrains settings files.
//!
//! Child order is preserved because some JetBrains elements (`<list>`, ordered
//! `<filter>` chains) are order-sensitive. Attributes are held in a `BTreeMap`,
//! so serialization is canonical without reordering anything that carries
//! meaning. JetBrains already writes components and options alphabetically, so
//! preserving document order still yields stable, machine-independent files.

use std::{collections::BTreeMap, fmt::Write as _};

/// Attributes JetBrains uses to identify a sibling, most specific first. A
/// child carrying one of these is addressed by its value rather than by
/// position, so a projected path survives siblings being inserted or removed.
pub const KEY_ATTRIBUTES: [&str; 9] = [
    "name", "key", "id", "class", "type", "language", "scheme", "ext", "pattern",
];

/// Elements whose identifying attribute is only an address, never the setting
/// itself. Everything else is assumed to mean something by existing.
pub const WRAPPER_ELEMENTS: [&str; 5] = ["option", "entry", "map", "list", "set"];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Element {
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    pub children: Vec<Element>,
    /// Trimmed character data. JetBrains settings files never mix text and
    /// element children, so a single optional string is sufficient.
    pub text: Option<String>,
}

#[derive(Debug)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

impl Element {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// The identifying attribute for this element, if it carries one.
    pub fn key_attribute(&self) -> Option<(&str, &str)> {
        KEY_ATTRIBUTES.iter().find_map(|attribute| {
            self.attributes
                .get(*attribute)
                .map(|value| (*attribute, value.as_str()))
        })
    }

    /// True when the element carries no value of its own, and can therefore be
    /// pruned once a merge or a prune rule removed its last leaf.
    ///
    /// The distinction that matters: in a *wrapper* element the name is only an
    /// address and the value lives in a sibling attribute, so `<option
    /// name="map" />` with an emptied map holds nothing. In a domain element
    /// the name is the payload — `<global_color_scheme name="Islands Dark" />`
    /// *is* the colour-scheme setting — so it must survive. Anything with
    /// children, text, or an attribute beyond its own address is kept either
    /// way.
    pub fn is_valueless(&self) -> bool {
        if !self.children.is_empty() || self.text.is_some() {
            return false;
        }
        if self.attributes.is_empty() {
            // A bare <map />, <list /> or <set />: pure structure.
            return true;
        }
        WRAPPER_ELEMENTS.contains(&self.name.as_str())
            && self
                .attributes
                .keys()
                .all(|key| KEY_ATTRIBUTES.contains(&key.as_str()))
    }

    /// A shallow copy used when a merge needs to graft a donor path onto a
    /// tree that does not have those ancestors yet.
    #[must_use]
    pub fn shell(&self) -> Self {
        Self {
            name: self.name.clone(),
            attributes: self.attributes.clone(),
            children: Vec::new(),
            text: None,
        }
    }
}

pub fn parse(contents: &str) -> Result<Element, ParseError> {
    let parsed = xmltree::Element::parse(contents.as_bytes())
        .map_err(|error| ParseError(error.to_string()))?;
    Ok(convert(&parsed))
}

fn convert(source: &xmltree::Element) -> Element {
    let mut text = String::new();
    let mut children = Vec::new();
    for node in &source.children {
        match node {
            xmltree::XMLNode::Element(child) => children.push(convert(child)),
            xmltree::XMLNode::Text(chunk) | xmltree::XMLNode::CData(chunk) => text.push_str(chunk),
            _ => {}
        }
    }
    let trimmed = text.trim();
    Element {
        name: source.name.clone(),
        attributes: source
            .attributes
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        children,
        text: (!trimmed.is_empty()).then(|| trimmed.to_string()),
    }
}

/// Serializes to the same dialect JetBrains itself writes: a single-quoted
/// declaration, two-space indentation, and a space before every `/>`.
pub fn serialize(root: &Element) -> String {
    let mut output = String::from("<?xml version='1.0' encoding='utf-8'?>\n");
    write_element(&mut output, root, 0);
    output
}

fn write_element(output: &mut String, element: &Element, depth: usize) {
    let indent = "  ".repeat(depth);
    let _ = write!(output, "{indent}<{}", element.name);
    for (key, value) in &element.attributes {
        let _ = write!(output, " {key}=\"{}\"", escape_attribute(value));
    }

    if element.children.is_empty() && element.text.is_none() {
        output.push_str(" />\n");
        return;
    }

    output.push('>');
    if let Some(text) = &element.text
        && element.children.is_empty()
    {
        let _ = writeln!(output, "{}</{}>", escape_text(text), element.name);
        return;
    }

    output.push('\n');
    for child in &element.children {
        write_element(output, child, depth + 1);
    }
    let _ = writeln!(output, "{indent}</{}>", element.name);
}

fn escape_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            // Whitespace inside an attribute is normalized to a space by any
            // conforming parser, so it must survive as a character reference.
            '\n' => escaped.push_str("&#10;"),
            '\r' => escaped.push_str("&#13;"),
            '\t' => escaped.push_str("&#9;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn escape_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_typical_options_file() {
        let source = "<?xml version='1.0' encoding='utf-8'?>\n<application>\n  <component name=\"GeneralSettings\">\n    <option name=\"reopenLastProject\" value=\"false\" />\n  </component>\n</application>";
        let parsed = parse(source).unwrap();
        let serialized = serialize(&parsed);
        assert_eq!(parse(&serialized).unwrap(), parsed);
        assert!(serialized.contains("<option name=\"reopenLastProject\" value=\"false\" />"));
    }

    #[test]
    fn attributes_serialize_in_sorted_order() {
        let parsed = parse("<root b=\"2\" a=\"1\" c=\"3\" />").unwrap();
        assert!(serialize(&parsed).contains("<root a=\"1\" b=\"2\" c=\"3\" />"));
    }

    #[test]
    fn preserves_child_order() {
        let parsed = parse("<list><item value=\"z\" /><item value=\"a\" /></list>").unwrap();
        let serialized = serialize(&parsed);
        let first = serialized.find("\"z\"").unwrap();
        let second = serialized.find("\"a\"").unwrap();
        assert!(first < second, "ordered children must not be sorted");
    }

    #[test]
    fn escapes_significant_whitespace_in_attributes() {
        let parsed = parse("<root value=\"a&#10;b\" />").unwrap();
        let serialized = serialize(&parsed);
        assert!(serialized.contains("&#10;"));
        assert_eq!(parse(&serialized).unwrap(), parsed);
    }

    #[test]
    fn keeps_element_text() {
        let parsed = parse("<place>Java: comments</place>").unwrap();
        assert_eq!(parsed.text.as_deref(), Some("Java: comments"));
        assert_eq!(parse(&serialize(&parsed)).unwrap(), parsed);
    }

    #[test]
    fn a_domain_element_named_by_its_value_is_not_valueless() {
        // The colour scheme setting *is* the name on this element.
        let parsed = parse(r#"<global_color_scheme name="Islands Dark" />"#).unwrap();
        assert!(!parsed.is_valueless());
    }

    #[test]
    fn an_emptied_wrapper_is_valueless() {
        let parsed = parse(r#"<option name="map" />"#).unwrap();
        assert!(parsed.is_valueless());
        assert!(parse("<map />").unwrap().is_valueless());
        assert!(parse(r#"<entry key="a" />"#).unwrap().is_valueless());
    }

    #[test]
    fn a_wrapper_holding_a_value_survives() {
        let parsed = parse(r#"<option name="tabs" value="4" />"#).unwrap();
        assert!(!parsed.is_valueless());
    }

    #[test]
    fn key_attribute_prefers_name() {
        let parsed = parse("<component name=\"A\" id=\"B\" />").unwrap();
        assert_eq!(parsed.key_attribute(), Some(("name", "A")));
    }
}
