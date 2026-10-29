//! Parse and resolve MTGO's versioned `CardDataSource` component files.
//!
//! Set documents contain digital objects with literal properties, lookup
//! references, presence flags, and nested property groups. Lookup documents map
//! opaque identifiers to text. Parse files discovered through the ClickOnce catalog;
//! the master document's external XML entities are intentionally unsupported.
//!
//! Byte input accepts UTF-8, with or without a BOM. Text retains MTGO's own
//! formatting after XML decoding. The parsers preserve unknown property names
//! and do not validate against the incomplete published DTD.
//! Nested property groups are limited to 64 levels to bound parser recursion.
//!
//! ```
//! use mtgoxml::{Deployment, LIVE_DEPLOYMENT_URL, carddata::CardDataFile};
//!
//! let application = Deployment::from_bytes(
//!     LIVE_DEPLOYMENT_URL, include_bytes!("../../fixtures/live/MTGO.application"),
//! )?.with_application_bytes(include_bytes!("../../fixtures/live/MTGO.exe.manifest"))?;
//! let mut database = application.card_database();
//! database.insert(&application, CardDataFile::parse(r#"
//!     <CARDNAME_STRING_ITEMS>
//!       <CARDNAME_STRING_ITEM id="name">Example</CARDNAME_STRING_ITEM>
//!     </CARDNAME_STRING_ITEMS>"#,
//! )?)?;
//! database.insert(&application, CardDataFile::parse(r#"
//!     <CardSet id="EXAMPLE">
//!       <DigitalObject DigitalObjectCatalogID="DOC_1">
//!         <CARDNAME_STRING id="name"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>
//!       </DigitalObject>
//!       <DigitalObject DigitalObjectCatalogID="DOC_2">
//!         <CLONE_ID value="1"/><IS_FOIL/>
//!       </DigitalObject>
//!     </CardSet>"#,
//! )?)?;
//! let card = database.card(2)?;
//! assert_eq!(card.name(), Some("Example"));
//! assert!(card.is_foil()?);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod database;
mod error;
mod model;
mod parser;

pub use database::{CardDatabase, CardView, ResolvedObject, ResolvedValue};
pub use error::Error;
pub use model::{CardDataFile, CardSet, CatalogId, DigitalObject, FieldValue, LookupTable};

use std::io::Read;

/// Bounds applied while reading and parsing one component document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// Maximum encoded input or decoded UTF-8 source size in bytes.
    pub max_input_bytes: usize,
    /// Maximum XML nodes, including text and comments.
    pub max_nodes: u32,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            max_input_bytes: 16 * 1024 * 1024,
            max_nodes: 250_000,
        }
    }
}

impl ParseOptions {
    /// Parse an already decoded component document with these limits.
    ///
    /// # Errors
    /// Returns exceeded limits, XML errors, an unsupported document root, or
    /// invalid or duplicate component metadata. DTD declarations and property
    /// groups nested more than 64 levels deep are rejected.
    pub fn parse(self, xml: &str) -> Result<CardDataFile, Error> {
        self.check_size(xml.len())?;
        parser::parse(xml, self.max_nodes)
    }

    /// Parse UTF-8 component bytes, accepting an optional UTF-8 BOM.
    ///
    /// # Errors
    /// Returns invalid UTF-8, an unsupported encoding declaration, exceeded input
    /// limits, or any error described by [`Self::parse`].
    pub fn from_bytes(self, bytes: &[u8]) -> Result<CardDataFile, Error> {
        self.check_size(bytes.len())?;
        let xml = std::str::from_utf8(bytes)
            .map_err(|error| Error::Encoding(format!("expected UTF-8 input: {error}")))?;
        parser::check_encoding(xml.strip_prefix('\u{feff}').unwrap_or(xml))?;
        self.parse(xml)
    }

    /// Read at most the input limit plus one byte, then parse the component.
    ///
    /// # Errors
    /// Returns read failures or the errors described by [`Self::from_bytes`].
    pub fn from_reader(self, reader: impl Read) -> Result<CardDataFile, Error> {
        let limit = u64::try_from(self.max_input_bytes).unwrap_or(u64::MAX);
        let mut bytes = Vec::new();
        reader
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)?;
        self.from_bytes(&bytes)
    }

    /// Reject an input before constructing its XML tree.
    fn check_size(self, size: usize) -> Result<(), Error> {
        if size > self.max_input_bytes {
            return Err(Error::InputTooLarge {
                limit: self.max_input_bytes,
            });
        }
        Ok(())
    }
}

impl CardDataFile {
    /// Parse an already decoded component using the default limits.
    ///
    /// # Errors
    /// Returns the errors described by [`ParseOptions::parse`].
    pub fn parse(xml: &str) -> Result<Self, Error> {
        ParseOptions::default().parse(xml)
    }

    /// Parse UTF-8 component bytes using the default limits.
    ///
    /// # Errors
    /// Returns the errors described by [`ParseOptions::from_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        ParseOptions::default().from_bytes(bytes)
    }

    /// Read a bounded UTF-8 component using the default limits.
    ///
    /// # Errors
    /// Returns the errors described by [`ParseOptions::from_reader`].
    pub fn from_reader(reader: impl Read) -> Result<Self, Error> {
        ParseOptions::default().from_reader(reader)
    }
}
