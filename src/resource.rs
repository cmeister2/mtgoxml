//! Downloadable payload metadata and MTGO resource filters.

use crate::Url;

/// The application-manifest entry that declares a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// A nonassembly file, such as XML, an image, or audio.
    File,
    /// An assembly installed with the application.
    Assembly,
}

/// One downloadable file declared by the application manifest.
///
/// Sizes and hashes are manifest declarations; this crate does not verify them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// The declared file path or assembly codebase, with `/` separators.
    ///
    /// Assembly codebases may be absolute URLs rather than relative paths.
    pub path: String,
    /// The resolved download URL, including `.deploy` when requested.
    pub url: Url,
    /// Whether this entry came from a file or assembly dependency.
    pub kind: ResourceKind,
    /// The declared payload size in bytes.
    pub size: Option<u64>,
    /// The declared digest metadata, without verification.
    pub hashes: Vec<clickonce::Hash>,
    /// The optional download group.
    pub group: Option<String>,
    /// Whether the manifest marks this payload as optional.
    pub optional: Option<bool>,
}

impl Resource {
    /// Whether the declared filename ends in `.xml`, ignoring case.
    ///
    /// File names are literal paths; assembly codebases are URI references and
    /// may include a query string. ClickOnce's `.deploy` mapping has no effect.
    pub fn is_xml(&self) -> bool {
        let path = match self.kind {
            ResourceKind::File => self.path.as_str(),
            ResourceKind::Assembly => self.path.split('?').next().unwrap_or(&self.path),
        };
        path.to_ascii_lowercase().ends_with(".xml")
    }

    /// Whether the logical resource path is inside MTGO's `CardDataSource` directory.
    pub fn is_card_data(&self) -> bool {
        self.path
            .split('/')
            .next()
            .is_some_and(|directory| directory.eq_ignore_ascii_case("CardDataSource"))
    }
}
