//! Errors encountered while reading and resolving MTGO card data.

use std::fmt;

use super::CatalogId;

/// A card-data component or relationship could not be parsed or resolved.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Component XML is malformed, exceeds node limits, or declares a DTD.
    Xml(roxmltree::Error),
    /// A component reader failed before parsing completed.
    Io(std::io::Error),
    /// Byte input is malformed UTF-8 or declares an unsupported encoding.
    Encoding(String),
    /// Encoded input or decoded source exceeds its byte budget.
    InputTooLarge {
        /// The configured maximum byte count.
        limit: usize,
    },
    /// A document is neither a set nor a named lookup table.
    UnexpectedRoot(String),
    /// A component has invalid or ambiguous source structure or metadata.
    InvalidData(String),
    /// Multiple records declare the same key in one logical namespace.
    Duplicate {
        /// The conflicting record category, such as `object` or `field`.
        kind: &'static str,
        /// The duplicate identifier or property name.
        key: String,
    },
    /// A requested object or one of its clone bases has not been loaded.
    MissingObject(CatalogId),
    /// A referenced table or lookup entry has not been loaded.
    MissingLookup {
        /// The property name identifying the lookup table.
        table: String,
        /// The unresolved, opaque source identifier.
        id: String,
    },
    /// Following clone bases revisited an object in the current chain.
    CloneCycle(Vec<CatalogId>),
    /// A property cannot be interpreted as its required semantic value.
    InvalidValue {
        /// The object whose resolved property is invalid.
        object: CatalogId,
        /// The property's XML name.
        field: String,
        /// The uninterpretable source value.
        value: String,
    },
    /// Components were supplied with a different application snapshot.
    SourceMismatch,
    /// The requested digital object is not declared as a card.
    NotCard(CatalogId),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => write!(formatter, "invalid card-data XML: {error}"),
            Self::Io(error) => write!(formatter, "could not read card data: {error}"),
            Self::Encoding(message) => write!(formatter, "invalid card-data encoding: {message}"),
            Self::InputTooLarge { limit } => {
                write!(formatter, "card-data input exceeds {limit} bytes")
            }
            Self::UnexpectedRoot(name) => write!(formatter, "unsupported card-data root: {name}"),
            Self::InvalidData(message) => write!(formatter, "invalid card data: {message}"),
            Self::Duplicate { kind, key } => write!(formatter, "duplicate card-data {kind}: {key}"),
            Self::MissingObject(id) => {
                write!(formatter, "card-data object DOC_{id} has not been loaded")
            }
            Self::MissingLookup { table, id } => write!(
                formatter,
                "card-data lookup {table}/{id} has not been loaded"
            ),
            Self::CloneCycle(chain) => write!(formatter, "card-data clone cycle: {chain:?}"),
            Self::InvalidValue {
                object,
                field,
                value,
            } => write!(
                formatter,
                "invalid card-data value on DOC_{object}: {field}={value:?}"
            ),
            Self::SourceMismatch => formatter
                .write_str("card-data components belong to a different application snapshot"),
            Self::NotCard(id) => write!(formatter, "card-data object DOC_{id} is not a card"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<roxmltree::Error> for Error {
    fn from(error: roxmltree::Error) -> Self {
        Self::Xml(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
