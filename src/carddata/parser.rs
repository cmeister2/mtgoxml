//! Structural parsing of independent MTGO card-data component documents.

use std::collections::{BTreeMap, BTreeSet};

use roxmltree::{Document, Node};

use super::{CardDataFile, CardSet, CatalogId, DigitalObject, Error, FieldValue, LookupTable};

/// Maximum nested property-group depth before rejecting further recursion.
const MAX_FIELD_DEPTH: usize = 64;

/// Parse a bounded XML tree without enabling DTD or external-entity processing.
pub(super) fn parse(xml: &str, max_nodes: u32) -> Result<CardDataFile, Error> {
    let document = Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            nodes_limit: max_nodes,
            ..roxmltree::ParsingOptions::default()
        },
    )?;
    let root = document.root_element();
    require_no_namespace(root)?;
    match root.tag_name().name() {
        "CardSet" => parse_set(root).map(CardDataFile::Set),
        name if name
            .strip_suffix("_ITEMS")
            .is_some_and(|name| !name.is_empty()) =>
        {
            parse_lookup(root).map(CardDataFile::Lookup)
        }
        name => Err(Error::UnexpectedRoot(name.to_owned())),
    }
}

/// Parse one fragment while checking its objects' numeric identifiers.
fn parse_set(root: Node<'_, '_>) -> Result<CardSet, Error> {
    let id = required_attribute(root, "id")?.to_owned();
    let age = root
        .attribute("age")
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| Error::InvalidData(format!("invalid CardSet age: {value:?}")))
        })
        .transpose()?;
    let mut ids = BTreeSet::new();
    let mut objects = Vec::new();
    for node in element_children(root)? {
        // Keep objects in source order while rejecting aliases of a numeric ID.
        require_name(node, "DigitalObject")?;
        let object = parse_object(node)?;
        if !ids.insert(object.id) {
            return Err(Error::Duplicate {
                kind: "object",
                key: object.id.to_string(),
            });
        }
        objects.push(object);
    }
    Ok(CardSet {
        id,
        age,
        kind: root.attribute("cardsetType").map(str::to_owned),
        objects,
    })
}

/// Parse an object's properties without inferring MTGO-specific field types.
fn parse_object(node: Node<'_, '_>) -> Result<DigitalObject, Error> {
    let id = parse_catalog_id(required_attribute(node, "DigitalObjectCatalogID")?)?;
    let fields = parse_fields(node, 0)?;
    Ok(DigitalObject { id, fields })
}

/// Parse one named property collection and reject duplicate names at that level.
fn parse_fields(node: Node<'_, '_>, depth: usize) -> Result<BTreeMap<String, FieldValue>, Error> {
    let mut fields = BTreeMap::new();
    for field in element_children(node)? {
        // Apply the same value forms at every nesting level, retaining unknown names.
        let name = field.tag_name().name().to_owned();
        let value = parse_field(field, depth)?;
        if fields.insert(name.clone(), value).is_some() {
            return Err(Error::Duplicate {
                kind: "field",
                key: name,
            });
        }
    }
    Ok(fields)
}

/// Parse a leaf or bounded property group without accepting mixed value forms.
fn parse_field(node: Node<'_, '_>, depth: usize) -> Result<FieldValue, Error> {
    if !element_children(node)?.is_empty() {
        // A group owns only its child fields; attributes would ambiguously mix
        // a literal or lookup value with those nested properties.
        if node.attributes().len() != 0 {
            return Err(Error::InvalidData(format!(
                "property group {} contains attributes",
                node.tag_name().name()
            )));
        }
        if depth >= MAX_FIELD_DEPTH {
            return Err(Error::InvalidData(format!(
                "property groups exceed {MAX_FIELD_DEPTH} nested levels"
            )));
        }
        return parse_fields(node, depth + 1).map(FieldValue::Fields);
    }
    require_leaf_attributes(node)?;
    match (node.attribute("id"), node.attribute("value")) {
        (Some(id), None) if !id.trim().is_empty() => Ok(FieldValue::Reference(id.to_owned())),
        (None, Some(value)) => Ok(FieldValue::Literal(value.to_owned())),
        (None, None) => Ok(FieldValue::Flag),
        _ => Err(Error::InvalidData(format!(
            "property {} needs one nonempty id, one value, or no attributes",
            node.tag_name().name()
        ))),
    }
}

/// Parse a named lookup, retaining decoded PCDATA including empty strings.
fn parse_lookup(root: Node<'_, '_>) -> Result<LookupTable, Error> {
    let name = root
        .tag_name()
        .name()
        .strip_suffix("_ITEMS")
        .unwrap_or_default()
        .to_owned();
    let row_name = format!("{name}_ITEM");
    let mut entries = BTreeMap::new();
    for row in element_children(root)? {
        // Join all text nodes so comments and CDATA boundaries cannot truncate text.
        require_name(row, &row_name)?;
        let id = required_attribute(row, "id")?.to_owned();
        if row.children().any(|child| child.is_element()) {
            return Err(Error::InvalidData(format!(
                "lookup {name}/{id} contains nested elements"
            )));
        }
        let value: String = row
            .children()
            .filter_map(|child| child.text().filter(|_| child.is_text()))
            .collect();
        if entries.insert(id.clone(), value).is_some() {
            return Err(Error::Duplicate {
                kind: "lookup entry",
                key: id,
            });
        }
    }
    Ok(LookupTable { name, entries })
}

/// Obtain structural element children while rejecting unexpected text or namespaces.
fn element_children<'a, 'input>(node: Node<'a, 'input>) -> Result<Vec<Node<'a, 'input>>, Error> {
    let mut elements = Vec::new();
    for child in node.children() {
        // Formatting whitespace and comments may surround structural elements.
        if child.is_element() {
            require_no_namespace(child)?;
            elements.push(child);
        } else if child.is_text()
            && child
                .text()
                .is_some_and(|text| text.chars().any(|character| !xml_space(character)))
        {
            return Err(Error::InvalidData(format!(
                "unexpected text inside {}",
                node.tag_name().name()
            )));
        }
    }
    Ok(elements)
}

/// Reject attributes outside the supported reference and literal leaf forms.
fn require_leaf_attributes(node: Node<'_, '_>) -> Result<(), Error> {
    if node.attributes().any(|attribute| {
        attribute.namespace().is_some() || !matches!(attribute.name(), "id" | "value")
    }) {
        return Err(Error::InvalidData(format!(
            "property {} contains unsupported attributes",
            node.tag_name().name()
        )));
    }
    Ok(())
}

/// Require a nonempty structural attribute without altering its spelling.
fn required_attribute<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str, Error> {
    node.attribute(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            Error::InvalidData(format!(
                "{} requires nonempty {name}",
                node.tag_name().name()
            ))
        })
}

/// Match a component element's exact local name.
fn require_name(node: Node<'_, '_>, expected: &str) -> Result<(), Error> {
    if node.tag_name().name() != expected {
        return Err(Error::InvalidData(format!(
            "expected {expected}, found {}",
            node.tag_name().name()
        )));
    }
    Ok(())
}

/// Component XML does not declare namespaces for its source records.
fn require_no_namespace(node: Node<'_, '_>) -> Result<(), Error> {
    if node.tag_name().namespace().is_some() {
        return Err(Error::InvalidData(format!(
            "unexpected namespace on {}",
            node.tag_name().name()
        )));
    }
    Ok(())
}

/// Decode the numeric suffix of a strict `DOC_` catalog identifier.
fn parse_catalog_id(value: &str) -> Result<CatalogId, Error> {
    value
        .strip_prefix("DOC_")
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
        .ok_or_else(|| Error::InvalidData(format!("invalid DigitalObjectCatalogID: {value:?}")))
}

/// Check a byte input's declaration while leaving malformed syntax to the XML parser.
pub(super) fn check_encoding(xml: &str) -> Result<(), Error> {
    let Some(declaration) = xml
        .strip_prefix("<?xml")
        .filter(|rest| rest.starts_with(xml_space))
    else {
        return Ok(());
    };
    let Some((mut attributes, _)) = declaration.split_once("?>") else {
        return Ok(());
    };
    while !attributes.trim_start_matches(xml_space).is_empty() {
        // Inspect complete quoted values, avoiding substring matches inside other attributes.
        attributes = attributes.trim_start_matches(xml_space);
        let name_end = attributes
            .find(|character: char| xml_space(character) || character == '=')
            .unwrap_or(attributes.len());
        let (name, rest) = attributes.split_at(name_end);
        let Some(rest) = rest.trim_start_matches(xml_space).strip_prefix('=') else {
            return Ok(());
        };
        let rest = rest.trim_start_matches(xml_space);
        let Some(quote) = rest
            .chars()
            .next()
            .filter(|character| matches!(character, '\'' | '"'))
        else {
            return Ok(());
        };
        let Some(end) = rest[1..].find(quote) else {
            return Ok(());
        };
        let value = &rest[1..end + 1];
        if name == "encoding" && !value.eq_ignore_ascii_case("utf-8") {
            return Err(Error::Encoding(format!(
                "only UTF-8 byte input is supported, found {value:?}"
            )));
        }
        attributes = &rest[end + 2..];
    }
    Ok(())
}

/// XML's four whitespace characters, used in declarations and structural metadata.
fn xml_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n')
}
