//! Publish quality-90 derivatives of checksum-verified, indexed release WebPs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use flate2::read::MultiGzDecoder;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    ANIMATED_AVATAR_ASSETS, Result, Snapshot, avatar_assets::webp, digest_bytes,
    verify_animated_index,
};

/// Match the maximum definition count accepted by the avatar generator.
const MAX_AVATARS: usize = 4096;
/// Bound traversal of decompressed preserved sources as well as published WebPs.
const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Match the generator's bound on ordinary files in one preserved bundle.
const MAX_ARCHIVE_FILES: usize = 16_384;
/// GNU long names are needed by preserved source files, but never need large buffers.
const MAX_ARCHIVE_PATH_BYTES: u64 = 4096;
/// Reject an individual preview too large for a static gallery asset.
const MAX_WEBP_BYTES: u64 = 128 * 1024 * 1024;
/// Bound the total indexed preview bytes before writing any public assets.
const MAX_WEBP_TOTAL: u64 = 1024 * 1024 * 1024;

/// Archive-entry evidence already checked structurally by the JSON array reader.
struct Expected {
    /// Canonical SHA-256 of the lossless release WebP.
    digest: String,
    /// Exact source file size recorded after native encoding and decoding.
    size: u64,
}

/// Discoverable latest-snapshot manifest, with URLs relative to this JSON file.
#[derive(Serialize)]
struct Manifest<'a> {
    /// Public avatar manifest format version.
    schema_version: u32,
    /// Upstream client identity from the verified snapshot.
    application_version: &'a str,
    /// Data-release tag identifying the underlying source export.
    tag: &'a str,
    /// Native renderer version retained by the release.
    renderer_version: &'a str,
    /// Digest of the complete ordinary JSON source index.
    index_sha256: &'a str,
    /// Digest of the complete preserved source and rendering archive.
    archive_sha256: &'a str,
    /// All source definitions, including explicit missing and unsupported outcomes.
    definition_count: usize,
    /// Definitions with a verified public WebP URL.
    rendered_count: usize,
    /// Definitions whose required source texture was unavailable.
    missing_count: usize,
    /// Definitions whose effect or rendering source was not supported.
    unsupported_count: usize,
    /// Original release rows extended with verified quality-90 preview metadata.
    avatars: Vec<Value>,
}

/// Build the optional gallery inside the caller's unpublished staging directory.
///
/// # Errors
/// Returns an error for invalid index records, unsafe or duplicate archive paths,
/// nonordinary entries, missing or corrupt indexed WebPs, size limits, or I/O failures.
pub(super) fn build(input: &Path, output: &Path, snapshot: &Snapshot) -> Result<()> {
    let Some(metadata) = &snapshot.animated_avatars else {
        return Ok(());
    };
    let mut rows = verify_animated_index(input, metadata)?;
    if rows.len() > MAX_AVATARS {
        return Err("avatar publication exceeds its definition limit".into());
    }
    let mut expected = BTreeMap::new();
    let mut total = 0_u64;
    for row in &mut rows {
        // Only verified rendered outcomes receive a public URL, regardless of extra source fields.
        let record = row
            .as_object_mut()
            .ok_or("avatar index row must be an object")?;
        record.remove("url");
        record.remove("preview");
        if record.get("status").and_then(Value::as_str) == Some("rendered") {
            // Apply publication limits before trusting a row as an extraction target.
            let path = record
                .get("path")
                .and_then(Value::as_str)
                .ok_or("rendered avatar is missing its archive path")?
                .to_owned();
            let digest = record
                .get("sha256")
                .and_then(Value::as_str)
                .ok_or("rendered avatar is missing its digest")?
                .to_owned();
            let size = record
                .get("size")
                .and_then(Value::as_u64)
                .filter(|size| *size <= MAX_WEBP_BYTES)
                .ok_or("rendered avatar exceeds its publication size limit")?;
            total = total
                .checked_add(size)
                .filter(|size| *size <= MAX_WEBP_TOTAL)
                .ok_or("avatar publication exceeds its total size limit")?;
            expected.insert(path, Expected { digest, size });
        }
    }
    let directory = output.join("avatars");
    fs::create_dir_all(directory.join("assets"))?;
    let mut previews = publish_assets(input, &directory.join("assets"), &expected)?;
    for row in &mut rows {
        // Release paths, hashes and encoding metadata keep their original meaning.
        if row.get("status").and_then(Value::as_str) == Some("rendered") {
            // Attach the derived file only after every archive entry has been validated.
            let path = row
                .get("path")
                .and_then(Value::as_str)
                .ok_or("rendered avatar is missing its archive path")?;
            let preview = previews
                .remove(path)
                .ok_or("rendered avatar is missing its published preview")?;
            row.as_object_mut()
                .ok_or("avatar index row must be an object")?
                .insert("preview".into(), preview);
        }
    }
    let manifest = Manifest {
        schema_version: 2,
        application_version: &snapshot.source.application_version,
        tag: &snapshot.tag,
        renderer_version: &metadata.renderer_version,
        index_sha256: &metadata.index_sha256,
        archive_sha256: &metadata.archive_sha256,
        definition_count: metadata.definition_count,
        rendered_count: metadata.rendered_count,
        missing_count: metadata.missing_count,
        unsupported_count: metadata.unsupported_count,
        avatars: rows,
    };
    super::write_json(&directory.join("index.json"), &manifest)?;
    fs::write(
        directory.join("index.html"),
        super::render_template("avatars.html", &manifest)?,
    )?;
    Ok(())
}

/// Accept exact portable relative paths without normalizing traversal or separator aliases.
fn safe_archive_path(path: &[u8]) -> Result<&str> {
    let path = std::str::from_utf8(path)?;
    if path.contains(['\\', ':', '\0'])
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err("unsafe avatar archive path".into());
    }
    Ok(path)
}

/// Validate the complete archive and publish only derivatives of indexed WebP entries.
fn publish_assets(
    input: &Path,
    output: &Path,
    expected: &BTreeMap<String, Expected>,
) -> Result<BTreeMap<String, Value>> {
    let decoder = MultiGzDecoder::new(File::open(input.join(ANIMATED_AVATAR_ASSETS[1]))?);
    let mut archive = tar::Archive::new(decoder.take(MAX_ARCHIVE_BYTES + 1));
    // Continue through zero padding so hidden trailing archives do not escape path validation.
    archive.set_ignore_zeros(true);
    let mut seen = BTreeSet::new();
    let mut found = BTreeSet::new();
    let mut long_name = None;
    let mut published = BTreeMap::new();
    let mut cache = BTreeMap::new();
    let mut published_total = 0_u64;
    for entry in archive.entries()?.raw(true) {
        // Raw iteration prevents the tar library from allocating unbounded extension payloads.
        let mut entry = entry?;
        if entry.header().entry_type().is_gnu_longname() {
            // Our GNU-format exporter uses only this bounded extension for long source paths.
            if long_name.is_some() || entry.size() > MAX_ARCHIVE_PATH_BYTES {
                return Err("invalid avatar archive long-name extension".into());
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            if bytes.len() as u64 != entry.size() || bytes.pop() != Some(0) {
                return Err("unterminated avatar archive long name".into());
            }
            long_name = Some(safe_archive_path(&bytes)?.to_owned());
            continue;
        }
        let path = match long_name.take() {
            Some(path) => path,
            None => safe_archive_path(&entry.path_bytes())?.to_owned(),
        };
        if !entry.header().entry_type().is_file() {
            return Err(format!("avatar archive entry is not an ordinary file: {path}").into());
        }
        if !seen.insert(path.clone()) || seen.len() > MAX_ARCHIVE_FILES {
            return Err(format!("duplicate or excessive avatar archive entry: {path}").into());
        }
        if let Some(evidence) = expected.get(&path) {
            // Output names come solely from validated digests, never from the archive's paths.
            if entry.size() != evidence.size {
                return Err(format!("avatar archive size mismatch: {path}").into());
            }
            let source = read_webp(&mut entry, evidence)?;
            let preview = match cache.get(&evidence.digest) {
                Some(preview) => Value::clone(preview),
                None => {
                    // Encode each verified source once and name its derivative by the new bytes.
                    let (encoded, metadata) = webp::compress_preview(&source)?;
                    if encoded.len() as u64 > MAX_WEBP_BYTES {
                        return Err("compressed avatar exceeds its publication size limit".into());
                    }
                    let digest = digest_bytes(&encoded)?;
                    let path = output.join(format!("{digest}.webp"));
                    write_webp(&path, &encoded)?;
                    let mut preview = serde_json::to_value(metadata)?;
                    preview
                        .as_object_mut()
                        .ok_or("compressed avatar metadata must be an object")?
                        .insert("url".into(), json!(format!("assets/{digest}.webp")));
                    cache.insert(evidence.digest.clone(), preview.clone());
                    preview
                }
            };
            published_total = published_total
                .checked_add(
                    preview
                        .get("size")
                        .and_then(Value::as_u64)
                        .ok_or("compressed avatar is missing its size")?,
                )
                .filter(|size| *size <= MAX_WEBP_TOTAL)
                .ok_or("compressed avatars exceed their total size limit")?;
            published.insert(path.clone(), preview);
            found.insert(path);
        } else if path.starts_with("webp/") {
            return Err(format!("unindexed WebP in avatar archive: {path}").into());
        } else {
            std::io::copy(&mut entry, &mut std::io::sink())?;
        }
    }
    let mut remaining = archive.into_inner();
    std::io::copy(&mut remaining, &mut std::io::sink())?;
    if remaining.limit() == 0 {
        return Err("avatar archive exceeds its decompressed-size limit".into());
    }
    if long_name.is_some() {
        return Err("avatar archive ends with an unused long name".into());
    }
    if found.len() != expected.len() {
        return Err("avatar archive is missing an indexed WebP".into());
    }
    Ok(published)
}

/// Read one bounded release WebP and verify its complete byte hash, size and header.
fn read_webp(input: &mut impl Read, expected: &Expected) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take(expected.size + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected.size
        || digest_bytes(&bytes)? != expected.digest
        || bytes.len() < 12
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WEBP"
        || u64::from(u32::from_le_bytes(bytes[4..8].try_into()?)) + 8 != expected.size
    {
        return Err("avatar WebP bytes do not match the verified index".into());
    }
    Ok(bytes)
}

/// Install a content-addressed preview once within the unpublished staging directory.
fn write_webp(path: &Path, bytes: &[u8]) -> Result<()> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(bytes)?;
            file.flush()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
