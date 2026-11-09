//! Capture original avatar textures, decode bounded images, and bind shader inputs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use mtgoxml::Url;
use serde_json::{Value, json};

use super::animation::bindings::Source;
use super::{Definition, Result, download};

/// One original texture's archive identity and local capture location.
pub(super) struct Texture {
    /// Relative path retained in archive manifests and rendering provenance.
    archive_path: String,
    /// Local image file inside the current export's staging bundle.
    path: PathBuf,
}

impl Texture {
    /// Locate a texture using an archive-relative path from the avatar definition.
    pub(super) fn new(bundle: &Path, archive_path: &str) -> Self {
        Self {
            archive_path: archive_path.to_owned(),
            path: bundle.join(archive_path),
        }
    }

    /// Capture and validate the original image, retaining explicit publisher 404/410 outcomes.
    ///
    /// # Errors
    /// Returns an error for failed downloads, invalid or oversized images, or filesystem failures.
    pub(super) fn capture(&self, root: &Url, agent: &ureq::Agent) -> Result<Value> {
        let mut url = root.clone();
        let base = root.path().trim_end_matches('/');
        url.set_path(&format!(
            "{base}/{}",
            self.archive_path.trim_start_matches("textures/")
        ));
        let (bytes, mut metadata) = match download(agent, &url) {
            Ok(result) => result,
            Err(error)
                if matches!(
                    error.downcast_ref::<ureq::Error>(),
                    Some(ureq::Error::StatusCode(404 | 410))
                ) =>
            {
                return Ok(
                    json!({"path":self.archive_path,"requested_url":url.as_str(),"status":"missing","http_status":match error.downcast_ref::<ureq::Error>() { Some(ureq::Error::StatusCode(status))=>*status,_=>unreachable!() }}),
                );
            }
            Err(error) => return Err(error),
        };
        Self::decode_bytes(&bytes)?;
        fs::create_dir_all(self.path.parent().ok_or("invalid texture path")?)?;
        fs::write(&self.path, bytes)?;
        metadata["path"] = json!(self.archive_path);
        metadata["status"] = json!("captured");
        Ok(metadata)
    }

    /// Decode the local texture as straight RGBA; rendering premultiplies alpha later.
    ///
    /// # Errors
    /// Returns an error for unreadable files, unsupported images, or decoding/allocation limits.
    pub(super) fn decode(&self) -> Result<image::RgbaImage> {
        Self::decode_bytes(&fs::read(&self.path)?)
    }

    /// The local source file used for rendering and checksum verification.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// The original archive-relative source identity retained in rendering provenance.
    pub(super) fn archive_path(&self) -> &str {
        &self.archive_path
    }

    /// Decode PNG/JPEG bytes with the same pixel/allocation limits for capture and rendering.
    fn decode_bytes(bytes: &[u8]) -> Result<image::RgbaImage> {
        let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        if !matches!(
            reader.format(),
            Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
        ) {
            return Err("avatar source is not a PNG or JPEG".into());
        }
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        Ok(reader.decode()?.into_rgba8())
    }
}

/// Download unique avatar textures concurrently using bounded operation-local workers.
///
/// # Errors
/// Returns an error for a worker panic, failed capture, or malformed publisher image.
pub(super) fn capture_textures(
    bundle: &Path,
    avatars: &[Definition],
    root: &Url,
) -> Result<Vec<Value>> {
    let paths: Vec<_> = avatars
        .iter()
        .flat_map(|avatar| avatar.assets.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let chunk_size = paths.len().div_ceil(6);
    let groups = std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk_size)
            .map(|group| {
                scope.spawn(move || {
                    let agent = crate::agent();
                    group
                        .iter()
                        .map(|path| {
                            Texture::new(bundle, path)
                                .capture(root, &agent)
                                .map_err(|error| error.to_string())
                        })
                        .collect::<std::result::Result<Vec<_>, String>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| "avatar texture worker panicked".to_owned())?
            })
            .collect::<std::result::Result<Vec<_>, String>>()
    })?;
    Ok(groups.into_iter().flatten().collect())
}

/// Resolve the client's extracted sampler assignments to captured texture objects.
///
/// # Errors
/// Returns an error when an assigned avatar image is absent from the definition.
pub(super) fn samplers(
    bundle: &Path,
    avatar: &Definition,
    sources: &BTreeMap<u32, Source>,
) -> Result<BTreeMap<u32, Texture>> {
    sources
        .iter()
        .map(|(unit, source)| {
            let path = match source {
                Source::Input => avatar.assets.iter().find(|path| path.contains("/Regular/")),
                Source::AvatarOne => avatar.assets.iter().find(|path| {
                    path.contains("/Regular_Shader/") && !path.contains("_add_avatar")
                }),
                Source::AvatarTwo => avatar
                    .assets
                    .iter()
                    .find(|path| path.contains("/Regular_Shader/") && path.contains("_add_avatar")),
                Source::Shared(path) => Some(path),
            }
            .ok_or_else(|| format!("avatar {} has no source for sampler {unit}", avatar.id))?;
            Ok((*unit, Texture::new(bundle, path)))
        })
        .collect()
}
