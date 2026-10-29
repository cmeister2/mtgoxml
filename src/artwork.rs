//! Canonical artwork-crop locations from the application's download configuration.

use std::fmt;

use roxmltree::{Document, Node, ParsingOptions};

use crate::{ResolvedObject, Url};

/// Maximum encoded configuration size accepted by parsing and HTTP loading.
pub(crate) const MAX_CONFIG_BYTES: usize = 16 * 1024 * 1024;
/// Bound on XML nodes while reading a download configuration.
const MAX_CONFIG_NODES: u32 = 250_000;

/// A download configuration could not provide an unambiguous artwork location.
#[derive(Debug)]
#[non_exhaustive]
pub enum ArtworkError {
    /// Encoded configuration exceeds its byte budget.
    InputTooLarge {
        /// Maximum accepted configuration size in bytes.
        limit: usize,
    },
    /// Configuration bytes are not valid UTF-8.
    Encoding(std::str::Utf8Error),
    /// XML is malformed, exceeds node limits, or declares a DTD.
    Xml(roxmltree::Error),
    /// Required configuration is missing, duplicated, or structurally invalid.
    InvalidConfig(String),
    /// A source or configured download location is not a valid URL.
    Url(url::ParseError),
    /// A download location is not HTTP(S), lacks a host, or contains a fragment.
    UnsupportedUrl(String),
}

impl fmt::Display for ArtworkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge { limit } => {
                write!(formatter, "artwork configuration exceeds {limit} bytes")
            }
            Self::Encoding(error) => write!(formatter, "invalid configuration UTF-8: {error}"),
            Self::Xml(error) => write!(formatter, "invalid configuration XML: {error}"),
            Self::InvalidConfig(message) => formatter.write_str(message),
            Self::Url(error) => write!(formatter, "invalid artwork URL: {error}"),
            Self::UnsupportedUrl(url) => {
                write!(
                    formatter,
                    "expected an HTTP(S) URL without a fragment: {url}"
                )
            }
        }
    }
}

impl std::error::Error for ArtworkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encoding(error) => Some(error),
            Self::Xml(error) => Some(error),
            Self::Url(error) => Some(error),
            _ => None,
        }
    }
}

/// Resolve canonical JPEG artwork crops using the application's configured CDN.
///
/// This resolves artwork only, without the card frame, name, or rules text. The
/// canonical `typ_reg_sty_010` crop is independent of a card's frame style; it
/// has been observed for classic, modern, showcase, prototype, and full-art
/// cards. Unknown or unavailable CDN files are still possible: constructing a
/// URL performs no request and does not guarantee that the image exists.
///
/// Tokens, play aids, products, and avatars use other image conventions and are
/// not resolved by this type. Card sub-objects use the normal artwork scheme.
#[derive(Debug, Clone)]
pub struct ArtworkResolver {
    /// Configured artwork directory with a trailing slash.
    base_url: Url,
}

impl ArtworkResolver {
    /// Read `downloadSource/artRoot` and `cardArtSubPath` from `MTGO.exe.config`.
    ///
    /// Relative roots use `config_url`, which must be the final response URL
    /// after any HTTP redirects. The configured HTTP or HTTPS scheme is kept.
    /// Byte input must be UTF-8; an optional UTF-8 BOM is accepted. Parsing is
    /// limited to 16 MiB and 250,000 XML nodes, without DTD processing.
    ///
    /// # Errors
    /// Returns an error for invalid URLs, exceeded limits, invalid UTF-8 or XML,
    /// ambiguous or missing settings, or a nonrelative artwork subpath.
    ///
    /// # Examples
    /// ```
    /// use mtgoxml::{ArtworkResolver, Url};
    /// let config = br#"<configuration><downloadSource>
    ///   <artRoot value="https://art.example/Graphics"/>
    ///   <cardArtSubPath value="Cards/Pics"/>
    /// </downloadSource></configuration>"#;
    /// let resolver = ArtworkResolver::from_config(
    ///     Url::parse("https://patch.example/client/MTGO.exe.config")?, config,
    /// )?;
    /// assert_eq!(resolver.base_url().as_str(), "https://art.example/Graphics/Cards/Pics/");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_config(config_url: Url, bytes: &[u8]) -> Result<Self, ArtworkError> {
        check_url(&config_url)?;
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err(ArtworkError::InputTooLarge {
                limit: MAX_CONFIG_BYTES,
            });
        }
        let xml = std::str::from_utf8(bytes).map_err(ArtworkError::Encoding)?;
        let document = Document::parse_with_options(
            xml.strip_prefix('\u{feff}').unwrap_or(xml),
            ParsingOptions {
                allow_dtd: false,
                nodes_limit: MAX_CONFIG_NODES,
                ..ParsingOptions::default()
            },
        )
        .map_err(ArtworkError::Xml)?;
        let root = document.root_element();
        if !root.has_tag_name("configuration") || root.tag_name().namespace().is_some() {
            return Err(ArtworkError::InvalidConfig(
                "expected a configuration root".into(),
            ));
        }
        let source = required_child(root, "downloadSource")?;
        let art_root = required_value(source, "artRoot")?;
        let subpath = required_value(source, "cardArtSubPath")?.replace('\\', "/");
        if subpath
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
            || subpath.contains([':', '\0'])
        {
            return Err(ArtworkError::InvalidConfig(
                "cardArtSubPath must be a relative directory path".into(),
            ));
        }
        let mut base_url = config_url.join(art_root).map_err(ArtworkError::Url)?;
        check_url(&base_url)?;
        {
            // Append literal directory segments, preserving any configured query string.
            let mut segments = base_url.path_segments_mut().map_err(|()| {
                ArtworkError::InvalidConfig("artRoot must support path segments".into())
            })?;
            segments.pop_if_empty();
            for segment in subpath.split('/') {
                segments.push(segment);
            }
            segments.push("");
        }
        Ok(Self { base_url })
    }

    /// The configured artwork directory, with a trailing slash.
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// Construct the canonical artwork-crop location for a card or card sub-object.
    ///
    /// Uses the resolved positive decimal `ARTID`, including clone inheritance,
    /// and pads it to at least five digits. `CARDTEXTURE_NUMBER` is a distinct
    /// engine identifier and does not participate in artwork locations.
    ///
    /// Returns `None` for missing, malformed, zero, or overflowing artwork IDs,
    /// object types other than `CARD` or `SUBC`, and frame-style 11 avatars.
    /// Other frame styles do not change this canonical crop location. The
    /// returned URL is not checked for image availability.
    pub fn for_object(&self, object: &ResolvedObject<'_>) -> Option<Url> {
        if !matches!(object.object_type(), Some("CARD" | "SUBC"))
            || object
                .text("FRAMESTYLE")
                .and_then(|value| value.parse::<u32>().ok())
                == Some(11)
        {
            return None;
        }
        let raw_id = object.text("ARTID")?;
        if raw_id.is_empty() || !raw_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let art_id = raw_id.parse::<u64>().ok().filter(|id| *id > 0)?;
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .ok()?
            .pop_if_empty()
            .push(&format!("{art_id:05}_typ_reg_sty_010.jpg"));
        Some(url)
    }
}

/// Require one unqualified element child for a setting or section.
fn required_child<'a, 'input>(
    parent: Node<'a, 'input>,
    name: &str,
) -> Result<Node<'a, 'input>, ArtworkError> {
    let mut children = parent.children().filter(|node| {
        node.is_element() && node.has_tag_name(name) && node.tag_name().namespace().is_none()
    });
    let first = children
        .next()
        .ok_or_else(|| ArtworkError::InvalidConfig(format!("missing {name} setting")))?;
    if children.next().is_some() {
        return Err(ArtworkError::InvalidConfig(format!(
            "duplicate {name} setting"
        )));
    }
    Ok(first)
}

/// Require a nonempty value attribute without interpreting it as a host path.
fn required_value<'a>(parent: Node<'a, '_>, name: &str) -> Result<&'a str, ArtworkError> {
    required_child(parent, name)?
        .attributes()
        .find(|attribute| attribute.name() == "value" && attribute.namespace().is_none())
        .map(|attribute| attribute.value())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ArtworkError::InvalidConfig(format!("{name} requires a nonempty value")))
}

/// Restrict source and CDN locations to network URLs without fragments.
fn check_url(url: &Url) -> Result<(), ArtworkError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.fragment().is_some()
    {
        return Err(ArtworkError::UnsupportedUrl(url.to_string()));
    }
    Ok(())
}
