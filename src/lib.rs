#![doc = include_str!("../docs/usage.md")]

mod artwork;
mod dataxml;
mod error;
mod location;
mod resource;

pub mod carddata;
pub mod cardstyle;

#[cfg(feature = "http")]
mod client;

/// ClickOnce parsing and manifest metadata used by this crate.
pub use clickonce;
/// URL type used for manifest and payload locations.
pub use url::Url;

pub use artwork::{ArtworkError, ArtworkResolver};
pub use carddata::{CardDatabase, CardView, FriendlyObject, ResolvedObject};
#[cfg(feature = "http")]
pub use client::{Client, transport};
pub use error::Error;
pub use resource::{Resource, ResourceKind};

use std::collections::HashSet;

use clickonce::{
    AssemblyDependency, AssemblyIdentity, AssemblyVersion, Dependency, DependencyType, Manifest,
    ManifestKind,
};

/// The official live MTGO ClickOnce deployment endpoint.
pub const LIVE_DEPLOYMENT_URL: &str =
    "http://mtgo.patch.daybreakgames.com/patch/mtg/live/client/MTGO.application";

/// A deployment and its discovered application-manifest location.
#[derive(Debug, Clone)]
pub struct Deployment {
    /// The location against which the deployment's codebase is resolved.
    url: Url,
    /// The complete parsed deployment metadata.
    manifest: Manifest,
    /// The resolved application-manifest URL, without payload extension mapping.
    application_manifest_url: Url,
    /// The identity required by the selected application reference.
    application_identity: AssemblyIdentity,
}

impl Deployment {
    /// Parse a deployment from bytes fetched from the given URL.
    ///
    /// Use the final response URL when the request followed redirects.
    ///
    /// # Errors
    /// Returns URL or parsing errors, a wrong manifest kind, or an error if there
    /// is not exactly one downloadable application-manifest reference.
    pub fn from_bytes(url: &str, bytes: &[u8]) -> Result<Self, Error> {
        Self::from_manifest(Url::parse(url)?, Manifest::from_bytes(bytes)?)
    }

    /// Resolve an already parsed deployment's application-manifest reference.
    ///
    /// # Errors
    /// Returns errors for unsupported URLs, a wrong manifest kind, invalid
    /// codebases, or a missing or ambiguous application-manifest reference.
    pub fn from_manifest(url: Url, manifest: Manifest) -> Result<Self, Error> {
        location::check_url(&url)?;
        require_kind(&manifest, ManifestKind::Deployment)?;
        let references: Vec<_> = downloadable_assemblies(&manifest)
            .filter(|assembly| assembly.codebase.is_some())
            .collect();
        if references.len() != 1 {
            return Err(Error::ApplicationReferenceCount(references.len()));
        }
        let reference = references[0];
        let application_manifest_url = location::codebase_url(
            &url,
            reference.codebase.as_deref().unwrap_or_default(),
            false,
        )?;
        let application_identity = reference.identity.clone();
        Ok(Self {
            url,
            manifest,
            application_manifest_url,
            application_identity,
        })
    }

    /// The deployment's source URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The complete deployment metadata, including declared signatures and hashes.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The application manifest to fetch next.
    pub fn application_manifest_url(&self) -> &Url {
        &self.application_manifest_url
    }

    /// Whether payload URLs must end in `.deploy`; omission defaults to false.
    pub fn map_file_extensions(&self) -> bool {
        self.manifest
            .deployment
            .as_ref()
            .and_then(|deployment| deployment.map_file_extensions)
            .unwrap_or(false)
    }

    /// Parse the referenced application manifest and discover its resources.
    ///
    /// This assumes the application manifest was fetched from
    /// [`Self::application_manifest_url`] without a redirect. For a redirected
    /// response, use [`Self::with_application`] with the final URL instead.
    ///
    /// # Errors
    /// Returns parsing errors or the errors described by [`Self::with_application`].
    pub fn with_application_bytes(self, bytes: &[u8]) -> Result<Application, Error> {
        let url = self.application_manifest_url.clone();
        self.with_application(url, Manifest::from_bytes(bytes)?)
    }

    /// Build a resource catalog using the application manifest's final source URL.
    ///
    /// # Errors
    /// Returns errors for unsupported URLs, a wrong manifest kind, an identity
    /// that differs from the deployment reference, invalid resource locations,
    /// or duplicate resource paths.
    pub fn with_application(self, url: Url, manifest: Manifest) -> Result<Application, Error> {
        location::check_url(&url)?;
        require_kind(&manifest, ManifestKind::Application)?;
        if manifest.identity != self.application_identity {
            return Err(Error::ApplicationIdentityMismatch);
        }
        let mapped = self.map_file_extensions();
        let mut resources = Vec::new();
        for file in &manifest.files {
            let path = file.name.replace('\\', "/");
            resources.push(Resource {
                url: location::file_url(&url, &path, mapped)?,
                path,
                kind: ResourceKind::File,
                size: file.size,
                hashes: file.hashes.clone(),
                group: file.group.clone(),
                optional: file.optional,
            });
        }
        for assembly in downloadable_assemblies(&manifest) {
            if let Some(codebase) = &assembly.codebase {
                resources.push(Resource {
                    path: codebase.replace('\\', "/"),
                    url: location::codebase_url(&url, codebase, mapped)?,
                    kind: ResourceKind::Assembly,
                    size: assembly.size,
                    hashes: assembly.hashes.clone(),
                    group: assembly.group.clone(),
                    optional: assembly.optional,
                });
            }
        }
        let mut paths = HashSet::new();
        for resource in &resources {
            if !paths.insert(resource.path.to_ascii_lowercase()) {
                return Err(Error::DuplicateResource(resource.path.clone()));
            }
        }
        Ok(Application {
            deployment: self,
            url,
            manifest,
            resources,
        })
    }
}

/// A versioned MTGO application and all of its declared downloadable resources.
#[derive(Debug, Clone)]
pub struct Application {
    /// The deployment that selected this application version.
    deployment: Deployment,
    /// The application manifest's final source URL.
    url: Url,
    /// The complete parsed application metadata.
    manifest: Manifest,
    /// Nonassembly files followed by downloadable assemblies, in their original order.
    resources: Vec<Resource>,
}

impl Application {
    /// Create an empty card database bound to this application snapshot.
    ///
    /// Insert parsed components with [`CardDatabase::insert`], or enable `http`
    /// and use `Client::load_card_sets` or `Client::load_card_database`.
    pub fn card_database(&self) -> CardDatabase {
        CardDatabase::new(self)
    }

    /// The deployment that selected this application.
    pub fn deployment(&self) -> &Deployment {
        &self.deployment
    }

    /// The source URL used to resolve payload locations.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The complete application metadata.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The application's declared four-part assembly version.
    pub fn version(&self) -> Option<AssemblyVersion> {
        self.manifest.identity.version
    }

    /// All files and downloadable assemblies, including optional resources.
    pub fn resources(&self) -> &[Resource] {
        &self.resources
    }

    /// Find a declared resource, ignoring ASCII case and directory separators.
    ///
    /// Lookup uses the logical path rather than the download URL or its `.deploy`
    /// suffix. URL-escaped codebases retain their declared spelling.
    pub fn resource(&self, path: &str) -> Option<&Resource> {
        let path = path.replace('\\', "/");
        self.resources
            .iter()
            .find(|resource| resource.path.eq_ignore_ascii_case(&path))
    }

    /// Enumerate resources whose declared filenames end in `.xml`.
    pub fn xml_resources(&self) -> impl Iterator<Item = &Resource> {
        self.resources.iter().filter(|resource| resource.is_xml())
    }

    /// Enumerate resources declared inside MTGO's `CardDataSource` directory.
    pub fn card_data_resources(&self) -> impl Iterator<Item = &Resource> {
        self.resources
            .iter()
            .filter(|resource| resource.is_card_data())
    }
}

/// Check the manifest's declared role before interpreting its metadata.
fn require_kind(manifest: &Manifest, expected: ManifestKind) -> Result<(), Error> {
    if manifest.kind != expected {
        return Err(Error::UnexpectedManifest {
            expected,
            actual: manifest.kind,
        });
    }
    Ok(())
}

/// Iterate installed or legacy unspecified assemblies, excluding prerequisites.
fn downloadable_assemblies(manifest: &Manifest) -> impl Iterator<Item = &AssemblyDependency> {
    manifest
        .dependencies
        .iter()
        .filter_map(|dependency| match dependency {
            Dependency::Assembly(assembly)
                if assembly.prerequisite != Some(true)
                    && matches!(
                        assembly.dependency_type,
                        None | Some(DependencyType::Install)
                    ) =>
            {
                Some(assembly)
            }
            _ => None,
        })
}
