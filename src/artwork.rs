//! Client artwork locations from the application's download configuration.

mod color;
mod frame;
mod properties;
mod style;

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

/// Resolve primary artwork using the application's configured CDN and frame rules.
///
/// The effective frame style and card properties select the same primary image
/// filename as the MTGO client. Some styles use a crop, while others include a
/// frame. Constructing a URL performs no request and does not guarantee that
/// the image exists.
///
/// Avatars and Vanguard cards use their separately configured artwork directory.
/// Tokens, play aids, and products are not resolved by this type. Card
/// sub-objects use the normal artwork scheme.
#[derive(Debug, Clone)]
pub struct ArtworkResolver {
    /// Configured artwork directory with a trailing slash.
    base_url: Url,
    /// Optional configured avatar and Vanguard artwork directory.
    avatar_base_url: Option<Url>,
}

impl ArtworkResolver {
    /// Read `downloadSource/artRoot` and artwork subpaths from `MTGO.exe.config`.
    ///
    /// `cardArtSubPath` is required. An optional `avatarArtSubPath` enables
    /// artwork for avatars and Vanguard cards; card-only configurations remain
    /// valid when it is absent.
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
        let root_url = config_url.join(art_root).map_err(ArtworkError::Url)?;
        check_url(&root_url)?;
        let base_url = artwork_directory(
            &root_url,
            required_value(source, "cardArtSubPath")?,
            "cardArtSubPath",
        )?;
        let avatar_base_url = optional_child(source, "avatarArtSubPath")?
            .map(|setting| {
                artwork_directory(
                    &root_url,
                    setting_value(setting, "avatarArtSubPath")?,
                    "avatarArtSubPath",
                )
            })
            .transpose()?;
        Ok(Self {
            base_url,
            avatar_base_url,
        })
    }

    /// The configured artwork directory, with a trailing slash.
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// The configured avatar and Vanguard directory, when supplied by the client.
    pub fn avatar_base_url(&self) -> Option<&Url> {
        self.avatar_base_url.as_ref()
    }

    /// Construct primary artwork for a card, avatar, Vanguard, or card sub-object.
    ///
    /// Uses the resolved positive decimal `ARTID` within the client's signed
    /// 32-bit range, including clone inheritance, and pads it to at least five
    /// digits. `CARDTEXTURE_NUMBER` is a distinct
    /// engine identifier and does not participate in artwork locations.
    ///
    /// `IS_VANGUARD` selects the avatar directory and `_avatar` suffix. Avatars
    /// with missing or zero `ARTID` use their catalog ID and `c_avatar` suffix.
    /// This fallback requires a positive catalog ID within the same signed
    /// 32-bit range. Both cosmetic avatars and playable Vanguard cards use it.
    ///
    /// Uses frame-style overrides, color, and effective card properties to select
    /// the image suffix. Returns `None` for malformed or overflowing IDs,
    /// unsupported object types, artless styles, missing required artwork
    /// configuration, or malformed properties needed for image selection.
    /// Non-avatar cards also require a positive `ARTID`. The returned URL is
    /// not checked for image availability.
    pub fn for_object(&self, object: &ResolvedObject<'_>) -> Option<Url> {
        if !matches!(object.object_type(), Some("CARD" | "SUBC")) {
            return None;
        }
        let avatar = properties::boolean(object, "IS_VANGUARD")?;
        let raw_id = match object.field_ref("ARTID") {
            None if avatar => "0",
            _ => object.text("ARTID")?,
        };
        let art_id = artwork_id(raw_id)?;
        let suffix = style::primary_suffix(object)?;
        let (mut url, art_id, fallback) = if avatar {
            // The client substitutes the catalog ID only when the avatar art ID is zero.
            let base = self.avatar_base_url.as_ref()?.clone();
            if art_id == 0 {
                // Catalog-based avatar filenames retain the client's signed integer bounds.
                let id = object.id();
                if !(1..=i32::MAX as u64).contains(&id) {
                    return None;
                }
                (base, id, "c")
            } else {
                (base, art_id, "")
            }
        } else {
            if art_id == 0 {
                return None;
            }
            (self.base_url.clone(), art_id, "")
        };
        url.path_segments_mut()
            .ok()?
            .pop_if_empty()
            .push(&format!("{art_id:05}{fallback}{suffix}.jpg"));
        Some(url)
    }
}

/// Parse a nonnegative art ID without interpreting malformed text as a fallback.
fn artwork_id(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|id| *id <= i32::MAX as u64)
}

/// Append a configured relative artwork directory while preserving root URL queries.
fn artwork_directory(root: &Url, subpath: &str, name: &str) -> Result<Url, ArtworkError> {
    let subpath = subpath.replace('\\', "/");
    if subpath
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || subpath.contains([':', '\0'])
    {
        return Err(ArtworkError::InvalidConfig(format!(
            "{name} must be a relative directory path"
        )));
    }
    let mut url = root.clone();
    {
        // Push literal path components so a subpath cannot replace the configured host.
        let mut segments = url.path_segments_mut().map_err(|()| {
            ArtworkError::InvalidConfig("artRoot must support path segments".into())
        })?;
        segments.pop_if_empty();
        for segment in subpath.split('/') {
            segments.push(segment);
        }
        segments.push("");
    }
    Ok(url)
}

/// Require one unqualified element child for a setting or section.
fn required_child<'a, 'input>(
    parent: Node<'a, 'input>,
    name: &str,
) -> Result<Node<'a, 'input>, ArtworkError> {
    optional_child(parent, name)?
        .ok_or_else(|| ArtworkError::InvalidConfig(format!("missing {name} setting")))
}

/// Read an optional unqualified setting while still rejecting ambiguous duplicates.
fn optional_child<'a, 'input>(
    parent: Node<'a, 'input>,
    name: &str,
) -> Result<Option<Node<'a, 'input>>, ArtworkError> {
    let mut children = parent.children().filter(|node| {
        node.is_element() && node.has_tag_name(name) && node.tag_name().namespace().is_none()
    });
    let first = children.next();
    if children.next().is_some() {
        return Err(ArtworkError::InvalidConfig(format!(
            "duplicate {name} setting"
        )));
    }
    Ok(first)
}

/// Require a nonempty value attribute without interpreting it as a host path.
fn required_value<'a>(parent: Node<'a, '_>, name: &str) -> Result<&'a str, ArtworkError> {
    setting_value(required_child(parent, name)?, name)
}

/// Require the selected setting's plain, nonempty value attribute.
fn setting_value<'a>(setting: Node<'a, '_>, name: &str) -> Result<&'a str, ArtworkError> {
    setting
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
