//! Owned source records parsed from MTGO card-data component documents.

use std::collections::BTreeMap;

/// The numeric portion of a digital object's `DOC_` catalog identifier.
pub type CatalogId = u64;

/// One property's representation in its source digital object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    /// An opaque identifier in the lookup table named by this property.
    Reference(String),
    /// A literal attribute value, including an explicitly empty string.
    Literal(String),
    /// A presence-only element, such as `<IS_FOIL/>`.
    Flag,
    /// A named group of nested properties, such as tracker states or card faces.
    ///
    /// Child property names and values are retained recursively. A group has no
    /// `id` or `value` attribute of its own.
    Fields(BTreeMap<String, FieldValue>),
}

/// A raw card, booster, deck, container, or other MTGO digital object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigitalObject {
    /// The object's numeric catalog ID.
    pub id: CatalogId,
    /// Properties keyed by their exact XML element names.
    ///
    /// Unknown property names are retained. An absent property has no entry;
    /// empty literal values and presence flags remain distinct.
    pub fields: BTreeMap<String, FieldValue>,
}

/// A fragment of a set's objects, as declared in one `client_*.xml` resource.
///
/// A set can have multiple fragments, such as a card document and a `_DO`
/// document containing associated products. These fragments share a set ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSet {
    /// The set code declared by the document.
    pub id: String,
    /// The source's optional `age` metadata, without interpreting its semantics.
    pub age: Option<u32>,
    /// The source's optional `cardsetType`, retaining unfamiliar values.
    pub kind: Option<String>,
    /// The objects declared in this fragment, in source order.
    pub objects: Vec<DigitalObject>,
}

/// One named lookup table mapping opaque source IDs to decoded text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupTable {
    /// The root element's name without its `_ITEMS` suffix.
    pub name: String,
    /// Entries keyed by their exact, case-sensitive source identifiers.
    ///
    /// Empty strings and MTGO formatting remain intact after XML decoding.
    pub entries: BTreeMap<String, String>,
}

/// One independently parseable `CardDataSource` XML component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardDataFile {
    /// A set fragment containing digital objects.
    Set(CardSet),
    /// A lookup table shared by digital-object properties.
    Lookup(LookupTable),
}
