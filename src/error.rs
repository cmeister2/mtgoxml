//! Errors encountered while discovering deployment resources.

use std::fmt;

use clickonce::ManifestKind;

/// A manifest, resource location, or discovery request could not be resolved.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// ClickOnce input could not be read or parsed.
    Manifest(clickonce::Error),
    /// A URL could not be parsed or resolved.
    Url(url::ParseError),
    /// A location does not use HTTP or HTTPS, or contains a fragment.
    UnsupportedUrl(String),
    /// A file path is empty, absolute, or contains empty or dot segments.
    InvalidPath(String),
    /// A manifest has the wrong role in the deployment.
    UnexpectedManifest {
        /// The role required at this stage.
        expected: ManifestKind,
        /// The role indicated by the parsed manifest.
        actual: ManifestKind,
    },
    /// A deployment must reference exactly one downloadable application manifest.
    ApplicationReferenceCount(usize),
    /// The application identity differs from the deployment's assembly reference.
    ApplicationIdentityMismatch,
    /// Multiple resources declare the same path, ignoring case and separators.
    DuplicateResource(String),
    /// CardDataSource parsing, snapshot consistency, or object resolution failed.
    CardData(crate::carddata::Error),
    /// A requested set or required lookup table is absent from the manifest.
    MissingCardDataResource(String),
    /// An HTTP request failed, including unsuccessful status codes.
    #[cfg(feature = "http")]
    Http(Box<ureq::Error>),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => write!(formatter, "invalid ClickOnce manifest: {error}"),
            Self::Url(error) => write!(formatter, "invalid resource URL: {error}"),
            Self::UnsupportedUrl(url) => write!(
                formatter,
                "expected an HTTP(S) URL without a fragment: {url}"
            ),
            Self::InvalidPath(path) => write!(formatter, "invalid resource path: {path:?}"),
            Self::UnexpectedManifest { expected, actual } => write!(
                formatter,
                "expected a {expected:?} manifest, found {actual:?}"
            ),
            Self::ApplicationReferenceCount(count) => write!(
                formatter,
                "expected one application manifest reference, found {count}"
            ),
            Self::ApplicationIdentityMismatch => {
                formatter.write_str("application identity does not match the deployment reference")
            }
            Self::DuplicateResource(path) => write!(formatter, "duplicate resource path: {path}"),
            Self::CardData(error) => write!(formatter, "invalid CardDataSource data: {error}"),
            Self::MissingCardDataResource(path) => {
                write!(formatter, "CardDataSource resource not found: {path}")
            }
            #[cfg(feature = "http")]
            Self::Http(error) => write!(formatter, "HTTP request failed: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Manifest(error) => Some(error),
            Self::Url(error) => Some(error),
            Self::CardData(error) => Some(error),
            #[cfg(feature = "http")]
            Self::Http(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<crate::carddata::Error> for Error {
    fn from(error: crate::carddata::Error) -> Self {
        Self::CardData(error)
    }
}

impl From<clickonce::Error> for Error {
    fn from(error: clickonce::Error) -> Self {
        Self::Manifest(error)
    }
}

impl From<url::ParseError> for Error {
    fn from(error: url::ParseError) -> Self {
        Self::Url(error)
    }
}

#[cfg(feature = "http")]
impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        Self::Http(Box::new(error))
    }
}
