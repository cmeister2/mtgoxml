//! Shared bounded decoding and errors for auxiliary MTGO XML resources.

use std::fmt;
use std::io::Read;

/// Bounds applied to one auxiliary client-data document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// Maximum encoded input and decoded UTF-8 source size in bytes.
    pub max_input_bytes: usize,
    /// Maximum XML nodes, including comments and text.
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
    /// Reject an input before allocating a tree or expanding its character encoding.
    fn check_size(self, size: usize) -> Result<(), Error> {
        if size > self.max_input_bytes {
            return Err(Error::InputTooLarge {
                limit: self.max_input_bytes,
            });
        }
        Ok(())
    }

    /// Parse already decoded Unicode without DTD or external-entity processing.
    pub(crate) fn document(self, xml: &str) -> Result<roxmltree::Document<'_>, Error> {
        self.check_size(xml.len())?;
        Ok(roxmltree::Document::parse_with_options(
            xml.strip_prefix('\u{feff}').unwrap_or(xml),
            roxmltree::ParsingOptions {
                allow_dtd: false,
                nodes_limit: self.max_nodes,
                ..roxmltree::ParsingOptions::default()
            },
        )?)
    }

    /// Decode UTF-8, declared ISO-8859-1/ASCII, or BOM-marked UTF-16 client XML.
    pub(crate) fn decode(self, bytes: &[u8]) -> Result<String, Error> {
        self.check_size(bytes.len())?;
        let xml = if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
            decode_utf16(bytes, true)?
        } else if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
            decode_utf16(bytes, false)?
        } else {
            // Only the ASCII declaration is inspected before selecting the byte decoder.
            let (bytes, bom) = bytes
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .map_or((bytes, false), |bytes| (bytes, true));
            let header = bytes.split(|byte| *byte == b'>').next().unwrap_or_default();
            let header = std::str::from_utf8(header).unwrap_or_default();
            let encoding = declared_encoding(header).unwrap_or("UTF-8");
            if bom && !encoding.eq_ignore_ascii_case("UTF-8") {
                return Err(Error::Encoding(
                    "UTF-8 BOM conflicts with XML declaration".into(),
                ));
            }
            match encoding.to_ascii_lowercase().as_str() {
                "utf-8" => std::str::from_utf8(bytes)
                    .map_err(|error| Error::Encoding(error.to_string()))?
                    .to_owned(),
                "iso-8859-1" => bytes.iter().map(|byte| char::from(*byte)).collect(),
                "us-ascii" if bytes.is_ascii() => String::from_utf8(bytes.to_vec())
                    .map_err(|error| Error::Encoding(error.to_string()))?,
                _ => {
                    return Err(Error::Encoding(format!(
                        "unsupported or invalid XML encoding: {encoding}"
                    )));
                }
            }
        };
        self.check_size(xml.len())?;
        Ok(xml)
    }

    /// Read at most the encoded input budget plus one byte before parsing.
    pub(crate) fn read(self, reader: impl Read) -> Result<Vec<u8>, Error> {
        let limit = u64::try_from(self.max_input_bytes).unwrap_or(u64::MAX);
        let mut bytes = Vec::new();
        reader
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)?;
        self.check_size(bytes.len())?;
        Ok(bytes)
    }
}

/// Read the XML declaration's encoding label without inspecting the document body.
fn declared_encoding(header: &str) -> Option<&str> {
    let mut attributes = header.strip_prefix("<?xml")?;
    if !attributes.starts_with(char::is_whitespace) {
        return None;
    }
    while let Some((name, value)) = attributes.trim_start().split_once('=') {
        // Attribute values may use either XML quote style and contain spaces.
        let value = value.trim_start();
        let quote = value.chars().next()?;
        if !matches!(quote, '\'' | '"') {
            return None;
        }
        let (value, rest) = value[1..].split_once(quote)?;
        if name.trim() == "encoding" {
            return Some(value);
        }
        attributes = rest;
    }
    None
}

/// Decode a BOM-selected UTF-16 document without replacing malformed surrogate pairs.
fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, Error> {
    if bytes.len() % 2 != 0 {
        return Err(Error::Encoding("odd UTF-16 byte count".into()));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    let xml = String::from_utf16(&units).map_err(|error| Error::Encoding(error.to_string()))?;
    if let Some(encoding) = declared_encoding(&xml) {
        let specific = if little_endian {
            "UTF-16LE"
        } else {
            "UTF-16BE"
        };
        if !encoding.eq_ignore_ascii_case("UTF-16") && !encoding.eq_ignore_ascii_case(specific) {
            return Err(Error::Encoding(
                "UTF-16 BOM conflicts with XML declaration".into(),
            ));
        }
    }
    Ok(xml)
}

/// Auxiliary XML data could not be decoded, parsed, or resolved unambiguously.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// XML is malformed, exceeds its node limit, or declares a DTD.
    Xml(roxmltree::Error),
    /// The document reader failed.
    Io(std::io::Error),
    /// Input bytes have an unsupported or malformed character encoding.
    Encoding(String),
    /// Encoded or decoded input exceeds its byte budget.
    InputTooLarge {
        /// Maximum accepted input size in bytes.
        limit: usize,
    },
    /// A document has invalid or unsupported structure or values.
    InvalidData(String),
    /// A logical namespace contains duplicate declarations.
    Duplicate {
        /// Category of the conflicting declaration.
        kind: &'static str,
        /// Duplicate key, preserving its exact source spelling.
        key: String,
    },
    /// A referenced declaration has not been supplied.
    MissingReference {
        /// Category of the referenced declaration.
        kind: &'static str,
        /// Missing key, preserving its exact source spelling.
        key: String,
    },
    /// Following inheritance references revisited a declaration.
    ReferenceCycle(Vec<String>),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => write!(formatter, "invalid client-data XML: {error}"),
            Self::Io(error) => write!(formatter, "could not read client data: {error}"),
            Self::Encoding(message) => write!(formatter, "invalid client-data encoding: {message}"),
            Self::InputTooLarge { limit } => {
                write!(formatter, "client-data input exceeds {limit} bytes")
            }
            Self::InvalidData(message) => write!(formatter, "invalid client data: {message}"),
            Self::Duplicate { kind, key } => {
                write!(formatter, "duplicate client-data {kind}: {key}")
            }
            Self::MissingReference { kind, key } => {
                write!(formatter, "missing client-data {kind}: {key}")
            }
            Self::ReferenceCycle(chain) => {
                write!(formatter, "client-data inheritance cycle: {chain:?}")
            }
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
