//! Capture a manifest snapshot and produce its verified JSON Lines release assets.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::{Compression, GzBuilder};
use mtgoxml::carddata::{CardDataFile, SetMetadata};
use mtgoxml::clickonce::{DigestAlgorithm, Hash, HashTransform, Manifest};
use mtgoxml::{
    Application, ArtworkResolver, CardDatabase, Deployment, LIVE_DEPLOYMENT_URL, ResourceKind, Url,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ureq::ResponseExt;

/// Verified style and product-image projections released alongside the card catalog.
mod auxiliary;
/// Captured avatar sources and native lossless animated WebP generation.
mod avatar_assets;
/// Offline browser-bundle generation from a completed data release.
mod site;

/// Version of the published JSON representation.
const SCHEMA_VERSION: u32 = 3;
/// Bound for a raw manifest or card-data component before parsing.
const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
/// Bound the complete ordinary JSON avatar index before whole-document parsing.
const MAX_AVATAR_INDEX_BYTES: usize = 64 * 1024 * 1024;
/// Assets hashed in the published checksum file, in their stable listing order.
const CHECKSUM_ASSETS: [&str; 5] = [
    "card-data.jsonl.gz",
    "snapshot.json",
    "MTGO.application",
    "MTGO.exe.manifest",
    "MTGO.exe.config",
];
/// Optional rendered-avatar index and preserved source/rendering bundle.
const ANIMATED_AVATAR_ASSETS: [&str; 2] = ["animated-avatars.json", "animated-avatars.tar.gz"];
/// Errors from networking, parsing, verification, serialization, or file writes.
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Provenance captured before any component downloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Source {
    /// Final deployment response URL, used to resolve its application reference.
    deployment_url: String,
    /// Final application response URL, used to resolve all component resources.
    application_manifest_url: String,
    /// Four-part application identity version.
    application_version: String,
    /// SHA-256 of the original, undecoded application-manifest bytes.
    application_manifest_sha256: String,
}

/// Verified configuration used to construct the client's primary artwork URLs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArtworkSource {
    /// Final URL from which the declared client configuration was fetched.
    config_url: String,
    /// SHA-256 of the original configuration bytes included in this release.
    config_sha256: String,
    /// Configured artwork directory, including a trailing slash.
    base_url: String,
}

/// Independently versioned completion evidence for the optional animated avatars.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnimatedAvatarsMetadata {
    /// Version of the avatar index and bundle contract.
    schema_version: u32,
    /// Number of effect-bearing avatar definitions represented by the index.
    definition_count: usize,
    /// Definitions with a completed WebP render.
    rendered_count: usize,
    /// Definitions whose required source texture was unavailable.
    missing_count: usize,
    /// Definitions whose effect has no supported rendering implementation.
    unsupported_count: usize,
    /// SHA-256 of the complete ordinary JSON avatar index.
    index_sha256: String,
    /// SHA-256 of the compressed source, renderer, and WebP archive.
    archive_sha256: String,
    /// Renderer version, independent of the upstream client version.
    renderer_version: String,
}

/// Saved input to the export and release phases.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    /// Output schema version recorded in the export metadata.
    schema_version: u32,
    /// Source fields are flat so release automation can inspect them directly.
    #[serde(flatten)]
    source: Source,
    /// Non-SemVer tag, independent of semantic-release's crate tags.
    tag: String,
    /// Present only after every catalog object has been exported successfully.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    object_count: Option<usize>,
    /// Set metadata deduplicated by the source set code after a complete export.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sets: BTreeMap<String, SetMetadata>,
    /// Configuration provenance, present after successful artwork discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artwork: Option<ArtworkSource>,
    /// Optional, independently versioned layout and product-image projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_data: Option<auxiliary::Metadata>,
    /// Optional source-preserving avatar animations generated after the native export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    animated_avatars: Option<AnimatedAvatarsMetadata>,
}

/// Dispatch manifest discovery, data export, or an offline browser build.
fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments
        .next()
        .ok_or("expected probe, export, avatars, or site")?;
    let directory = PathBuf::from(arguments.next().ok_or("expected output directory")?);
    let source = arguments.next();
    if arguments.next().is_some() {
        return Err(
            "usage: mtgoxml-export probe DIR [DEPLOYMENT_URL] | mtgoxml-export export DIR | mtgoxml-export avatars DIR | mtgoxml-export site SNAPSHOT_DIR OUTPUT_DIR".into(),
        );
    }
    match command.as_str() {
        "probe" => probe(&directory, source.as_deref().unwrap_or(LIVE_DEPLOYMENT_URL)),
        "export" if source.is_none() => export(&directory),
        "export" => Err("export accepts only the captured snapshot directory".into()),
        "avatars" if source.is_none() => export_avatars(&directory),
        "avatars" => Err("avatars accepts only the completed snapshot directory".into()),
        "site" => site::build(
            &directory,
            Path::new(
                source
                    .as_deref()
                    .ok_or("site requires an output directory")?,
            ),
        ),
        _ => Err("expected probe, export, avatars, or site".into()),
    }
}

/// Fetch both manifests once, validate their relationship, and save their exact bytes.
fn probe(directory: &Path, deployment_url: &str) -> Result<()> {
    let agent = agent();
    let (deployment_url, deployment_bytes) = fetch(&agent, &Url::parse(deployment_url)?)?;
    let deployment = Deployment::from_bytes(deployment_url.as_str(), &deployment_bytes)?;
    let (application_url, application_bytes) =
        fetch(&agent, deployment.application_manifest_url())?;
    verify_application(&deployment, &application_bytes)?;
    let application =
        deployment.with_application(application_url, Manifest::from_bytes(&application_bytes)?)?;
    let snapshot = snapshot(&application, &application_bytes)?;

    // Persist the verified source before publishing metadata that refers to it.
    fs::create_dir_all(directory)?;
    fs::write(directory.join("MTGO.application"), deployment_bytes)?;
    fs::write(directory.join("MTGO.exe.manifest"), application_bytes)?;
    save_metadata(directory, &snapshot)?;
    if let Some(output) = std::env::var_os("GITHUB_OUTPUT") {
        append_action_outputs(Path::new(&output), &snapshot)?;
    }
    println!("{}", snapshot.tag);
    Ok(())
}

/// Download the pinned components and publish the local gzip only after full validation.
fn export(directory: &Path) -> Result<()> {
    for name in ["card-data.jsonl.gz", "SHA256SUMS", "release-notes.md"]
        .into_iter()
        .chain(auxiliary::CHECKSUM_ASSETS)
        .chain(ANIMATED_AVATAR_ASSETS)
    {
        // Clear all completed outputs so a failed retry cannot retain stale release files.
        match fs::remove_file(directory.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    // Invalidate prior completion before manifest validation or networking can fail.
    let mut snapshot: Snapshot =
        serde_json::from_slice(&read_bounded(File::open(directory.join("snapshot.json"))?)?)?;
    snapshot.object_count = None;
    snapshot.sets.clear();
    snapshot.artwork = None;
    snapshot.client_data = None;
    snapshot.animated_avatars = None;
    save_metadata(directory, &snapshot)?;
    let (mut snapshot, application) = load_snapshot(directory)?;
    let agent = agent();
    let (artwork, config_bytes, artwork_source) = load_artwork(&application, &agent)?;
    let database = load_database(&application, |resource| Ok(fetch(&agent, &resource.url)?.1))?;
    let sets = database.set_metadata()?;
    let client_data = auxiliary::export(directory, &application, &database, |resource| {
        fetch(&agent, &resource.url)
    })?;
    let avatars = avatar_assets::collect_definitions(&database)?;

    // An interrupted export leaves only a temporary gzip, never a completed asset.
    let temporary = directory.join("card-data.jsonl.gz.tmp");
    write_database(File::create(&temporary)?, &database, &artwork)?;
    fs::rename(temporary, directory.join("card-data.jsonl.gz"))?;
    fs::write(directory.join("MTGO.exe.config"), config_bytes)?;
    snapshot.object_count = Some(database.len());
    snapshot.sets = sets;
    snapshot.artwork = Some(artwork_source);
    snapshot.client_data = Some(client_data);
    snapshot.animated_avatars = Some(avatar_assets::generate(
        directory,
        &application,
        &snapshot,
        &avatars,
        &artwork,
    )?);
    save_metadata(directory, &snapshot)?;
    write_release_files(directory, &snapshot)?;
    println!("Exported {} objects", database.len());
    Ok(())
}

/// Attach native avatar assets to an older completed local export without republishing it.
fn export_avatars(directory: &Path) -> Result<()> {
    let (snapshot, application) = load_snapshot(directory)?;
    if snapshot.animated_avatars.is_some() {
        return Err(
            "animated avatars are already finalized; rerun export before regenerating them".into(),
        );
    }
    let checksums = site::verify_checksums(directory, &snapshot)?;
    site::verify_completion(&snapshot, &checksums)?;
    let config_url = &snapshot
        .artwork
        .as_ref()
        .ok_or("missing artwork provenance")?
        .config_url;
    let artwork = ArtworkResolver::from_config(
        Url::parse(config_url)?,
        &read_bounded(File::open(directory.join("MTGO.exe.config"))?)?,
    )?;
    let avatars = avatar_assets::definitions(directory, &snapshot)?;
    let metadata = avatar_assets::generate(directory, &application, &snapshot, &avatars, &artwork)?;
    finalize_avatars(directory, metadata)?;
    println!(
        "Exported animated avatars for {}",
        snapshot.source.application_version
    );
    Ok(())
}

/// Create the bounded HTTP agent used by discovery and component loading.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent()
}

/// Download raw bytes while retaining the final URL and enforcing HTTP status and size.
fn fetch(agent: &ureq::Agent, url: &Url) -> Result<(Url, Vec<u8>)> {
    check_url(url)?;
    let mut response = agent.get(url.as_str()).call()?;
    if !response.status().is_success() {
        return Err(format!("HTTP {} for {url}", response.status()).into());
    }
    let final_url = Url::parse(&response.get_uri().to_string())?;
    check_url(&final_url)?;
    let bytes = read_bounded(response.body_mut().as_reader())?;
    Ok((final_url, bytes))
}

/// Require HTTP or HTTPS for all manifest and resource requests.
fn check_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("unsupported URL scheme: {}", url.scheme()).into());
    }
    Ok(())
}

/// Read at most one byte beyond the input limit so oversized sources fail closed.
fn read_bounded(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err("input exceeds the 16 MiB limit".into());
    }
    Ok(bytes)
}

/// Reconstruct the captured application, rejecting edited metadata or manifest bytes.
fn load_snapshot(directory: &Path) -> Result<(Snapshot, Application)> {
    let saved: Snapshot =
        serde_json::from_slice(&read_bounded(File::open(directory.join("snapshot.json"))?)?)?;
    let deployment_bytes = read_bounded(File::open(directory.join("MTGO.application"))?)?;
    let application_bytes = read_bounded(File::open(directory.join("MTGO.exe.manifest"))?)?;
    let deployment = Deployment::from_bytes(&saved.source.deployment_url, &deployment_bytes)?;
    verify_application(&deployment, &application_bytes)?;
    let application = deployment.with_application(
        Url::parse(&saved.source.application_manifest_url)?,
        Manifest::from_bytes(&application_bytes)?,
    )?;
    let expected = snapshot(&application, &application_bytes)?;
    if saved.schema_version != expected.schema_version
        || saved.tag != expected.tag
        || saved.source != expected.source
    {
        return Err("snapshot metadata does not match its captured manifests".into());
    }
    Ok((saved, application))
}

/// Record manifest provenance and derive the release tag from the upstream version.
fn snapshot(application: &Application, bytes: &[u8]) -> Result<Snapshot> {
    let hash = digest_bytes(bytes)?;
    let version = application
        .version()
        .ok_or("application has no version")?
        .to_string();
    Ok(Snapshot {
        schema_version: SCHEMA_VERSION,
        source: Source {
            deployment_url: application.deployment().url().to_string(),
            application_manifest_url: application.url().to_string(),
            application_version: version.clone(),
            application_manifest_sha256: hash.clone(),
        },
        tag: format!("mtgo-data-{version}"),
        object_count: None,
        sets: BTreeMap::new(),
        artwork: None,
        client_data: None,
        animated_avatars: None,
    })
}

/// Check the raw application bytes against the deployment's selected application reference.
fn verify_application(deployment: &Deployment, bytes: &[u8]) -> Result<()> {
    let reference = deployment.application_reference();
    verify_bytes(
        "application manifest",
        bytes,
        reference.size,
        &reference.hashes,
    )
}

/// Require declared byte size and every supported raw-byte digest to match.
fn verify_bytes(label: &str, bytes: &[u8], size: Option<u64>, hashes: &[Hash]) -> Result<()> {
    if size.is_some_and(|size| size != bytes.len() as u64) {
        return Err(format!("size mismatch for {label}").into());
    }
    if hashes.is_empty() {
        return Err(format!("no digest declared for {label}").into());
    }
    for hash in hashes {
        // Reject unsupported transforms before calculating and comparing this digest.
        if hash
            .transforms
            .iter()
            .any(|transform| *transform != HashTransform::Identity)
        {
            return Err(format!("unsupported digest transform for {label}").into());
        }
        let digest = match hash.digest_method {
            DigestAlgorithm::Sha1 => sha1::Sha1::digest(bytes).to_vec(),
            DigestAlgorithm::Sha256 => Sha256::digest(bytes).to_vec(),
            DigestAlgorithm::Sha384 => sha2::Sha384::digest(bytes).to_vec(),
            DigestAlgorithm::Sha512 => sha2::Sha512::digest(bytes).to_vec(),
            _ => return Err(format!("unsupported digest algorithm for {label}").into()),
        };
        if digest != hash.digest_value.as_bytes() {
            return Err(format!("digest mismatch for {label}").into());
        }
    }
    Ok(())
}

/// Load, verify, and identify each component before resolving the complete database.
fn load_database(
    application: &Application,
    mut read: impl FnMut(&mtgoxml::Resource) -> Result<Vec<u8>>,
) -> Result<CardDatabase> {
    let mut database = application.card_database();
    for resource in application
        .card_data_resources()
        .filter(|resource| resource.is_card_data_component())
    {
        // Verify bytes before interpreting XML or accepting a source identity.
        let bytes = read(resource)?;
        verify_bytes(&resource.path, &bytes, resource.size, &resource.hashes)?;
        let file = CardDataFile::from_bytes(&bytes)?;
        file.validate_resource(resource)?;
        database.insert(application, file)?;
    }
    if database.is_empty() {
        return Err("snapshot contains no digital objects".into());
    }
    database.validate()?;
    Ok(database)
}

/// Verify the declared client configuration before resolving its artwork directory.
fn load_artwork(
    application: &Application,
    agent: &ureq::Agent,
) -> Result<(ArtworkResolver, Vec<u8>, ArtworkSource)> {
    let resource = application
        .resource("MTGO.exe.config")
        .filter(|resource| resource.kind == ResourceKind::File)
        .ok_or("application has no MTGO.exe.config resource")?;
    let (url, bytes) = fetch(agent, &resource.url)?;
    verify_bytes(&resource.path, &bytes, resource.size, &resource.hashes)?;
    let artwork = ArtworkResolver::from_config(url.clone(), &bytes)?;
    let provenance = ArtworkSource {
        config_url: url.to_string(),
        config_sha256: digest_bytes(&bytes)?,
        base_url: artwork.base_url().to_string(),
    };
    Ok((artwork, bytes, provenance))
}

/// Stream friendly objects with artwork URLs into deterministic, LF-terminated JSONL.
fn write_database(
    writer: impl Write,
    database: &CardDatabase,
    artwork: &ArtworkResolver,
) -> Result<()> {
    let mut gzip = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(writer, Compression::new(6));
    for object in database.objects() {
        // Resolve and serialize one object at a time in stable numeric ID order.
        let object = object?;
        let mut friendly = object.friendly()?;
        friendly.art_url = artwork.for_object(&object).map(|url| url.to_string());
        serde_json::to_writer(&mut gzip, &friendly)?;
        writeln!(gzip)?;
    }
    gzip.finish()?.flush()?;
    Ok(())
}

/// Format a raw byte digest without interpreting its text encoding.
fn digest_bytes(bytes: &[u8]) -> Result<String> {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(hex, "{byte:02x}")?;
    }
    Ok(hex)
}

/// Write human-readable metadata only after the preceding operation succeeded.
fn save_metadata(directory: &Path, snapshot: &Snapshot) -> Result<()> {
    let mut file = File::create(directory.join("snapshot.json"))?;
    serde_json::to_writer_pretty(&mut file, snapshot)?;
    writeln!(file)?;
    Ok(())
}

/// Append derived single-line manifest outputs for subsequent GitHub Actions steps.
fn append_action_outputs(path: &Path, snapshot: &Snapshot) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "tag={}", snapshot.tag)?;
    writeln!(
        file,
        "application_version={}",
        snapshot.source.application_version
    )?;
    Ok(())
}

/// Stream a file into SHA-256 without retaining the compressed catalog in memory.
fn digest_file(path: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    let mut file = File::open(path)?;
    let mut buffer = [0; 64 * 1024];
    loop {
        // Feed each bounded read into the digest until the file reaches EOF.
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let mut hex = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(hex, "{byte:02x}")?;
    }
    Ok(hex)
}

/// Validate avatar summary counters and bind its provenance to the release checksums.
fn verify_animated_metadata(
    metadata: &AnimatedAvatarsMetadata,
    checksums: &BTreeMap<String, String>,
) -> Result<()> {
    let total = metadata
        .rendered_count
        .checked_add(metadata.missing_count)
        .and_then(|count| count.checked_add(metadata.unsupported_count));
    if metadata.schema_version != 1
        || total != Some(metadata.definition_count)
        || metadata.renderer_version.trim().is_empty()
    {
        return Err("invalid animated-avatar schema, counters, or renderer version".into());
    }
    for (name, expected) in [
        (ANIMATED_AVATAR_ASSETS[0], &metadata.index_sha256),
        (ANIMATED_AVATAR_ASSETS[1], &metadata.archive_sha256),
    ] {
        // Require canonical hex so metadata cannot disagree with the checksum representation.
        if expected.len() != 64
            || !expected
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || checksums.get(name) != Some(expected)
        {
            return Err(format!("animated-avatar provenance does not match {name}").into());
        }
    }
    Ok(())
}

/// Parse one bounded JSON array and check every avatar record without loading the tar archive.
fn verify_animated_index(
    directory: &Path,
    metadata: &AnimatedAvatarsMetadata,
) -> Result<Vec<serde_json::Value>> {
    let mut bytes = Vec::new();
    File::open(directory.join(ANIMATED_AVATAR_ASSETS[0]))?
        .take(MAX_AVATAR_INDEX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_AVATAR_INDEX_BYTES {
        return Err("animated-avatar index exceeds its document-size limit".into());
    }
    let records: Vec<serde_json::Value> = serde_json::from_slice(&bytes)?;
    let mut ids = std::collections::BTreeSet::new();
    let mut counts = [0_usize; 3];
    for record in &records {
        // Whole-document parsing rejects trailing data; semantic checks still cover every row.
        let id = record
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .filter(|id| *id > 0)
            .ok_or("animated-avatar ID must be a positive integer")?;
        if !ids.insert(id) {
            return Err(format!("duplicate animated-avatar ID: {id}").into());
        }
        let status = record
            .get("status")
            .and_then(serde_json::Value::as_str)
            .ok_or("animated-avatar record is missing its status")?;
        let index = match status {
            "rendered" => {
                // Every render is addressed inside the preserved archive by its own catalog ID.
                let path = format!("webp/{id}.webp");
                let digest = record
                    .get("sha256")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("rendered animated-avatar record is missing its SHA-256")?;
                if record.get("path").and_then(serde_json::Value::as_str) != Some(path.as_str())
                    || record
                        .get("size")
                        .and_then(serde_json::Value::as_u64)
                        .is_none_or(|size| size == 0)
                    || digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(format!("invalid rendered animated-avatar record: {id}").into());
                }
                0
            }
            "missing" => 1,
            "unsupported" => 2,
            _ => return Err(format!("unknown animated-avatar status: {status}").into()),
        };
        counts[index] += 1;
    }
    if ids.len() != metadata.definition_count
        || counts
            != [
                metadata.rendered_count,
                metadata.missing_count,
                metadata.unsupported_count,
            ]
    {
        return Err("animated-avatar index records do not match metadata counters".into());
    }
    Ok(records)
}

/// Install avatar completion metadata only after the existing release and new assets verify.
fn finalize_avatars(directory: &Path, metadata: AnimatedAvatarsMetadata) -> Result<()> {
    let (mut snapshot, _) = load_snapshot(directory)?;
    if snapshot.animated_avatars.is_some() {
        return Err(
            "animated avatars are already finalized; rerun export before regenerating them".into(),
        );
    }
    let mut checksums = site::verify_checksums(directory, &snapshot)?;
    site::verify_completion(&snapshot, &checksums)?;
    for name in ANIMATED_AVATAR_ASSETS {
        // Bind newly generated sidecar bytes to their typed completion metadata.
        let path = directory.join(name);
        if !path.is_file() || path.metadata()?.len() == 0 {
            return Err(format!("export asset {name} is missing or empty").into());
        }
        checksums.insert(name.to_owned(), digest_file(&path)?);
    }
    verify_animated_metadata(&metadata, &checksums)?;
    verify_animated_index(directory, &metadata)?;
    snapshot.animated_avatars = Some(metadata);
    save_metadata(directory, &snapshot)?;
    write_release_files(directory, &snapshot)?;
    Ok(())
}

/// Select the complete asset contract without changing older published snapshots.
fn checksum_assets(snapshot: &Snapshot) -> impl Iterator<Item = &'static str> {
    let extras: &'static [&'static str] = if snapshot.client_data.is_some() {
        &auxiliary::CHECKSUM_ASSETS
    } else {
        &[]
    };
    let avatars: &'static [&'static str] = if snapshot.animated_avatars.is_some() {
        &ANIMATED_AVATAR_ASSETS
    } else {
        &[]
    };
    CHECKSUM_ASSETS
        .into_iter()
        .chain(extras.iter().copied())
        .chain(avatars.iter().copied())
}

/// Generate publication metadata only after all complete export assets exist.
fn write_release_files(directory: &Path, snapshot: &Snapshot) -> Result<()> {
    let count = snapshot
        .object_count
        .filter(|count| *count > 0)
        .ok_or("cannot generate release files before a nonempty export completes")?;
    let mut checksums = String::new();
    let mut digests = BTreeMap::new();
    for name in checksum_assets(snapshot) {
        // Reject missing or empty assets before installing either release metadata file.
        let path = directory.join(name);
        if !path.is_file() || path.metadata()?.len() == 0 {
            return Err(format!("export asset {name} is missing or empty").into());
        }
        let digest = digest_file(&path)?;
        writeln!(checksums, "{digest}  {name}")?;
        digests.insert(name.to_owned(), digest);
    }
    let mut notes = format!(
        "Resolved MTGO catalog: {count} objects; JSON Lines schema {SCHEMA_VERSION}.\n\n\
         Application manifest: {}\n\n\
         Manifest SHA-256: `{}`\n\n\
         Includes compressed JSON Lines, source manifests, artwork configuration, metadata, and SHA256SUMS.\n",
        snapshot.source.application_manifest_url, snapshot.source.application_manifest_sha256,
    );
    if let Some(client_data) = &snapshot.client_data {
        // Keep unresolved declarations visible rather than counting them as resolved layouts.
        if client_data.schema_version != 1
            || client_data.unresolved_style_count > client_data.card_styles.record_count
        {
            return Err("invalid auxiliary export metadata".into());
        }
        write!(
            notes,
            "\nClient data JSON Lines schema 1: {} card styles ({} unresolved); {} store-image records.\n",
            client_data.card_styles.record_count,
            client_data.unresolved_style_count,
            client_data.image_names.record_count
        )?;
    }
    if let Some(avatars) = &snapshot.animated_avatars {
        // Verify optional avatar completion before publishing its counts and release checksums.
        verify_animated_metadata(avatars, &digests)?;
        verify_animated_index(directory, avatars)?;
        writeln!(
            notes,
            "\nAnimated avatars schema 1: {} definitions; {} rendered, {} missing textures, {} unsupported effects. Includes the JSON array index and preserved source/rendering archive.",
            avatars.definition_count,
            avatars.rendered_count,
            avatars.missing_count,
            avatars.unsupported_count
        )?;
    }
    fs::write(directory.join("SHA256SUMS.tmp"), checksums)?;
    fs::write(directory.join("release-notes.md.tmp"), notes)?;
    fs::rename(
        directory.join("SHA256SUMS.tmp"),
        directory.join("SHA256SUMS"),
    )?;
    fs::rename(
        directory.join("release-notes.md.tmp"),
        directory.join("release-notes.md"),
    )?;
    Ok(())
}

/// Export regressions use captured manifests and small synthetic card-data records.
#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::MultiGzDecoder;
    use serde_json::Value;
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Unique suffixes for temporary test directories within one process.
    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    /// A temporary capture removed when its test finishes.
    struct Capture {
        /// Directory holding the two captured manifests and their metadata.
        path: PathBuf,
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Create a replayable local snapshot from the checked-in publisher manifests.
    fn capture() -> Capture {
        let capture = Capture {
            path: std::env::temp_dir().join(format!(
                "mtgoxml-export-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            )),
        };
        fs::create_dir_all(&capture.path).unwrap();
        let deployment_bytes = include_bytes!("../../../fixtures/live/MTGO.application");
        let application_bytes = include_bytes!("../../../fixtures/live/MTGO.exe.manifest");
        fs::write(capture.path.join("MTGO.application"), deployment_bytes).unwrap();
        fs::write(capture.path.join("MTGO.exe.manifest"), application_bytes).unwrap();
        fs::write(
            capture.path.join("MTGO.exe.config"),
            include_bytes!("../../../fixtures/artwork/download-source.config"),
        )
        .unwrap();
        let application = Deployment::from_bytes(LIVE_DEPLOYMENT_URL, deployment_bytes)
            .unwrap()
            .with_application_bytes(application_bytes)
            .unwrap();
        save_metadata(
            &capture.path,
            &snapshot(&application, application_bytes).unwrap(),
        )
        .unwrap();
        capture
    }

    /// Tags follow the upstream version while provenance retains raw manifest changes.
    #[test]
    fn release_tags_follow_the_application_version() {
        let capture = capture();
        let (original, application) = load_snapshot(&capture.path).unwrap();
        assert_eq!(
            original.source.application_manifest_sha256,
            "32a32d35627ba060223ddc9ebef9eff7a4c70768bb6117f404e4aeb7e44acf7f"
        );
        let changed = snapshot(&application, b"same identity, different raw bytes").unwrap();
        assert_eq!(
            changed.source.application_version,
            original.source.application_version
        );
        assert_eq!(changed.tag, original.tag);
        assert_ne!(
            changed.source.application_manifest_sha256,
            original.source.application_manifest_sha256
        );
        assert_eq!(original.tag, "mtgo-data-3.4.159.4708");
        assert!(original.object_count.is_none());
        assert!(original.animated_avatars.is_none());
        assert!(
            serde_json::to_value(&original)
                .unwrap()
                .get("animated_avatars")
                .is_none()
        );
    }

    /// Saved metadata cannot redirect the release tag away from the captured bytes.
    #[test]
    fn rejects_edited_metadata_and_application_bytes() {
        let capture = capture();
        let (mut metadata, _) = load_snapshot(&capture.path).unwrap();
        metadata.tag.push_str("-edited");
        save_metadata(&capture.path, &metadata).unwrap();
        assert!(
            load_snapshot(&capture.path)
                .unwrap_err()
                .to_string()
                .contains("metadata")
        );

        let capture = self::capture();
        let mut bytes = fs::read(capture.path.join("MTGO.exe.manifest")).unwrap();
        bytes[100] ^= 1;
        fs::write(capture.path.join("MTGO.exe.manifest"), bytes).unwrap();
        assert!(
            load_snapshot(&capture.path)
                .unwrap_err()
                .to_string()
                .contains("digest mismatch")
        );
    }

    /// A failed native retry removes prior avatar completion before validating source manifests.
    #[test]
    fn failed_export_invalidates_previous_animated_avatar_outputs() {
        let capture = capture();
        let (mut metadata, _) = load_snapshot(&capture.path).unwrap();
        metadata.object_count = Some(1);
        metadata.animated_avatars = Some(AnimatedAvatarsMetadata {
            schema_version: 1,
            definition_count: 1,
            rendered_count: 1,
            missing_count: 0,
            unsupported_count: 0,
            index_sha256: "a".repeat(64),
            archive_sha256: "b".repeat(64),
            renderer_version: "test-renderer-1".into(),
        });
        save_metadata(&capture.path, &metadata).unwrap();
        for name in ANIMATED_AVATAR_ASSETS {
            fs::write(capture.path.join(name), b"previous completed avatar asset").unwrap();
        }
        // Source corruption stops the retry before it can request any network resources.
        fs::write(capture.path.join("MTGO.exe.manifest"), b"invalid manifest").unwrap();
        assert!(export(&capture.path).is_err());
        let saved: Snapshot =
            serde_json::from_slice(&fs::read(capture.path.join("snapshot.json")).unwrap()).unwrap();
        assert!(saved.animated_avatars.is_none());
        assert!(saved.object_count.is_none());
        for name in ANIMATED_AVATAR_ASSETS {
            assert!(!capture.path.join(name).exists());
        }
    }

    /// The default export completes both avatar sidecars even when the catalog has no avatars.
    #[test]
    fn default_export_generates_zero_definition_avatar_sidecars() {
        use std::net::TcpListener;
        use std::thread;
        use std::time::Instant;

        /// Exact small payloads and independently computed base64 SHA-256 digests.
        const RESOURCES: [(&str, &str, &str); 4] = [
            (
                "MTGO.exe.config",
                r#"<configuration><downloadSource><artRoot value="https://unused.invalid/Graphics"/><cardArtSubPath value="Cards/Pics"/></downloadSource></configuration>"#,
                "YdSMJsOXZSIHS5IogJvNfA50Y6lCSZ/pm51ZYRfaywE=",
            ),
            (
                "CardDataSource/client_TEST.xml",
                r#"<CardSet id="TEST"><DigitalObject DigitalObjectCatalogID="DOC_1"><CARDNAME_STRING value="Ordinary card"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/><ARTID value="1"/></DigitalObject></CardSet>"#,
                "mZalGcE8LZjfbH6oE4Kvylnu6ZIMlu8BS+zp6jXjqIQ=",
            ),
            (
                "CardStyle.xml",
                "<AnimCache/>",
                "Exymom4ikDLtB9kH7iuusFannPhopJhLFFScT+Oi6pk=",
            ),
            (
                "DigitalObjectImageNames.xml",
                "<DigitalObjectImageNamesData/>",
                "DzZegCC9M4BXXW7MJB8W7tZhSH/u9JEyhLdSFSGsTmA=",
            ),
        ];
        let capture = capture();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let source = format!("http://{}/MTGO.application", listener.local_addr().unwrap());
        let mut application = String::from(
            r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" xmlns:ds="http://www.w3.org/2000/09/xmldsig#" manifestVersion="1.0"><assemblyIdentity name="MTGO.exe" version="1.2.3.4"/><application/>"#,
        );
        for (name, bytes, digest) in RESOURCES {
            // Each downloaded source is covered by the same manifest verification as production.
            write!(application, r#"<file name="{name}" size="{}"><hash><ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/><ds:DigestValue>{digest}</ds:DigestValue></hash></file>"#, bytes.len()).unwrap();
        }
        application.push_str("</assembly>");
        let deployment = format!(
            r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" xmlns:ds="http://www.w3.org/2000/09/xmldsig#" manifestVersion="1.0"><assemblyIdentity name="MTGO.application" version="1.2.3.4"/><deployment install="true" mapFileExtensions="false"/><dependency><dependentAssembly dependencyType="install" codebase="MTGO.exe.manifest" size="{}"><assemblyIdentity name="MTGO.exe" version="1.2.3.4"/><hash><ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/><ds:DigestValue>+1yzLYNjLyoENp1ROoWR9ccSxRzpPLwMvwjlM9PGONE=</ds:DigestValue></hash></dependentAssembly></dependency></assembly>"#,
            application.len()
        );
        let pinned = Deployment::from_bytes(&source, deployment.as_bytes())
            .unwrap()
            .with_application_bytes(application.as_bytes())
            .unwrap();
        fs::write(capture.path.join("MTGO.application"), deployment).unwrap();
        fs::write(capture.path.join("MTGO.exe.manifest"), &application).unwrap();
        save_metadata(
            &capture.path,
            &snapshot(&pinned, application.as_bytes()).unwrap(),
        )
        .unwrap();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requested = Vec::new();
            while requested.len() < RESOURCES.len() && Instant::now() < deadline {
                // Nonblocking accepts and socket deadlines also bound failed export runs.
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("local export server failed: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new((&mut stream).take(8 * 1024));
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let target = request.split_whitespace().nth(1).unwrap().to_owned();
                loop {
                    // Consume the bounded request headers before returning one closed response.
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                let payload = RESOURCES
                    .iter()
                    .find(|(name, _, _)| target == format!("/{name}"));
                let (status, body) = payload
                    .map_or(("404 Not Found", "unexpected request"), |(_, body, _)| {
                        ("200 OK", *body)
                    });
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                requested.push(target);
            }
            requested
        });
        let result = export(&capture.path);
        let requested = server.join().unwrap();
        result.unwrap();
        assert_eq!(
            requested,
            [
                "/MTGO.exe.config",
                "/CardDataSource/client_TEST.xml",
                "/CardStyle.xml",
                "/DigitalObjectImageNames.xml"
            ]
        );
        let (metadata, _) = load_snapshot(&capture.path).unwrap();
        assert_eq!(metadata.object_count, Some(1));
        let avatars = metadata.animated_avatars.as_ref().unwrap();
        assert_eq!(
            (
                avatars.definition_count,
                avatars.rendered_count,
                avatars.missing_count,
                avatars.unsupported_count
            ),
            (0, 0, 0, 0)
        );
        let checksums = site::verify_checksums(&capture.path, &metadata).unwrap();
        site::verify_completion(&metadata, &checksums).unwrap();
        for name in ANIMATED_AVATAR_ASSETS {
            assert_eq!(
                checksums[name],
                digest_file(&capture.path.join(name)).unwrap()
            );
        }
        let index = fs::read(capture.path.join(ANIMATED_AVATAR_ASSETS[0])).unwrap();
        assert_eq!(index, b"[]\n");
        assert_eq!(
            serde_json::from_slice::<Value>(&index).unwrap(),
            serde_json::json!([])
        );
        let mut archive = tar::Archive::new(MultiGzDecoder::new(
            File::open(capture.path.join(ANIMATED_AVATAR_ASSETS[1])).unwrap(),
        ));
        let mut provenance = BTreeMap::new();
        for entry in archive.entries().unwrap() {
            // Inspect provenance rather than extracting any archive-controlled paths.
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            if path == Path::new(ANIMATED_AVATAR_ASSETS[0]) {
                // The archive retains the exact ordinary JSON sidecar, including its formatting.
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes, index);
                provenance.insert(path, serde_json::from_slice::<Value>(&bytes).unwrap());
            } else if path == Path::new("bundle.json") || path == Path::new("sources.json") {
                // Decode only the provenance records needed to establish this capture's source.
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                provenance.insert(path, serde_json::from_slice::<Value>(&bytes).unwrap());
            }
        }
        let bundle = &provenance[Path::new("bundle.json")];
        assert_eq!(
            bundle["application_manifest_sha256"],
            metadata.source.application_manifest_sha256
        );
        assert_eq!(bundle["catalog_sha256"], checksums["card-data.jsonl.gz"]);
        assert!(bundle["driver"].is_null());
        assert_eq!(
            provenance[Path::new(ANIMATED_AVATAR_ASSETS[0])],
            serde_json::json!([])
        );
        assert_eq!(provenance[Path::new("sources.json")], serde_json::json!([]));
        site::build(&capture.path, &capture.path.join("site")).unwrap();
    }

    /// Reject oversized ordinary JSON before parsing, independently of release checksum checks.
    #[test]
    fn avatar_index_rejects_oversized_documents_before_parsing() {
        let capture = capture();
        let metadata = AnimatedAvatarsMetadata {
            schema_version: 1,
            definition_count: 0,
            rendered_count: 0,
            missing_count: 0,
            unsupported_count: 0,
            index_sha256: "0".repeat(64),
            archive_sha256: "0".repeat(64),
            renderer_version: "test".into(),
        };
        File::create(capture.path.join(ANIMATED_AVATAR_ASSETS[0]))
            .unwrap()
            .set_len(MAX_AVATAR_INDEX_BYTES as u64 + 1)
            .unwrap();
        assert!(
            verify_animated_index(&capture.path, &metadata)
                .unwrap_err()
                .to_string()
                .contains("document-size limit")
        );
    }

    /// Byte verification rejects corruption and unknown algorithms rather than skipping checks.
    #[test]
    fn digest_verification_fails_closed() {
        let hash = Hash {
            transforms: vec![HashTransform::Identity],
            digest_method: DigestAlgorithm::Sha1,
            digest_value: "qZk+NkcGgWq6PiVxeFDCbJzQ2J0=".parse().unwrap(),
        };
        verify_bytes("test", b"abc", Some(3), std::slice::from_ref(&hash)).unwrap();
        assert!(verify_bytes("test", b"abd", Some(3), std::slice::from_ref(&hash)).is_err());
        assert!(verify_bytes("test", b"abc", Some(4), std::slice::from_ref(&hash)).is_err());
        assert!(verify_bytes("test", b"abc", None, &[]).is_err());
        let mut unknown = hash.clone();
        unknown.digest_method = DigestAlgorithm::Other("urn:future".into());
        assert!(verify_bytes("test", b"abc", None, &[unknown]).is_err());
        let mut transformed = hash;
        transformed.transforms = vec![HashTransform::Other("urn:future".into())];
        assert!(verify_bytes("test", b"abc", None, &[transformed]).is_err());
    }

    /// Each independent JSON line preserves resolved fields and escaped text reproducibly.
    #[test]
    fn resolved_json_lines_preserve_property_forms_and_all_object_types() {
        let capture = capture();
        let (_, application) = load_snapshot(&capture.path).unwrap();
        let artwork = ArtworkResolver::from_config(
            Url::parse("https://patch.example.test/MTGO.exe.config").unwrap(),
            include_bytes!("../../../fixtures/artwork/download-source.config"),
        )
        .unwrap();
        let mut database = application.card_database();
        database
            .insert(
                &application,
                CardDataFile::parse(
                    r#"<NAME_ITEMS><NAME_ITEM id="N">Name&#10;line&#13;&quot;quoted&quot; café 🧙</NAME_ITEM></NAME_ITEMS>"#,
                )
                .unwrap(),
            )
            .unwrap();
        database.insert(&application, CardDataFile::parse(r#"<CardSet id="TEST" age="7" cardsetType="future">
          <DigitalObject DigitalObjectCatalogID="DOC_2"><CLONE_ID value="1"/><IS_FOIL/><EMPTY value=""/></DigitalObject>
          <DigitalObject DigitalObjectCatalogID="DOC_1"><NAME id="N"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="BSTR"/><NESTED><UNKNOWN value="quoted &amp; decoded"/></NESTED></DigitalObject>
          <DigitalObject DigitalObjectCatalogID="DOC_3"><CARDNAME_STRING value="Art card"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/><ARTID value="1982"/><FRAMESTYLE value="3"/></DigitalObject>
        </CardSet>"#).unwrap()).unwrap();
        database.validate().unwrap();
        let mut first = Vec::new();
        let mut second = Vec::new();
        write_database(&mut first, &database, &artwork).unwrap();
        write_database(&mut second, &database, &artwork).unwrap();
        assert_eq!(first, second);
        let mut decoded = String::new();
        flate2::read::GzDecoder::new(first.as_slice())
            .read_to_string(&mut decoded)
            .unwrap();
        assert!(!decoded.starts_with('\u{feff}'));
        assert!(decoded.ends_with('\n'));
        assert!(!decoded.contains('\r'));
        let lines: Vec<_> = decoded.split_terminator('\n').collect();
        assert_eq!(lines.len(), database.len());
        assert!(lines.iter().all(|line| !line.is_empty()));
        let objects: Vec<Value> = lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(objects[0]["id"], 1);
        assert_eq!(objects[1]["id"], 2);
        assert_eq!(objects[2]["id"], 3);
        assert_eq!(objects[1]["set"], "TEST");
        assert_eq!(objects[1]["mtgo"]["NAME"], "Name\nline\r\"quoted\" café 🧙");
        assert!(objects[0].get("source").is_none());
        assert!(objects[0].get("schema_version").is_none());
        assert_eq!(objects[1]["foil"], true);
        assert_eq!(objects[1]["mtgo"]["EMPTY"], "");
        assert_eq!(objects[1]["mtgo"]["NESTED"]["UNKNOWN"], "quoted & decoded");
        assert_eq!(
            objects[0]["mtgo"]["DIGITAL_OBJECT_TYPE_CODE_STRING"],
            "BSTR"
        );
        assert_eq!(objects[2]["name"], "Art card");
        assert_eq!(objects[2]["art_id"], 1982);
        assert_eq!(
            objects[2]["art_url"],
            "http://mtgo.patch.daybreakgames.com/patch/mtg/live/Graphics/Cards/Pics/01982_typ_reg_sty_010.jpg"
        );
    }

    /// The reader enforces its limit before an XML parser can allocate for oversized input.
    #[test]
    fn rejects_oversized_input() {
        assert!(read_bounded(std::io::repeat(0).take(MAX_INPUT_BYTES as u64 + 1)).is_err());
    }

    /// Action outputs preserve prior entries and expose the exact derived tag and version.
    #[test]
    fn action_outputs_append_without_changing_existing_entries() {
        let capture = capture();
        let (metadata, _) = load_snapshot(&capture.path).unwrap();
        let path = capture.path.join("action-output");
        fs::write(&path, "prior=preserved\n").unwrap();
        append_action_outputs(&path, &metadata).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "prior=preserved\ntag=mtgo-data-3.4.159.4708\napplication_version=3.4.159.4708\n"
        );
    }

    /// Release notes and checksums remain absent until a complete, nonempty export exists.
    #[test]
    fn release_files_require_completed_export() {
        let capture = capture();
        let (mut metadata, _) = load_snapshot(&capture.path).unwrap();
        assert!(write_release_files(&capture.path, &metadata).is_err());
        metadata.object_count = Some(1);
        assert!(write_release_files(&capture.path, &metadata).is_err());
        assert!(!capture.path.join("SHA256SUMS").exists());
        assert!(!capture.path.join("release-notes.md").exists());
        fs::write(capture.path.join("card-data.jsonl.gz"), []).unwrap();
        assert!(write_release_files(&capture.path, &metadata).is_err());
        assert!(!capture.path.join("SHA256SUMS").exists());
        assert!(!capture.path.join("release-notes.md").exists());
    }

    /// Declared auxiliary data makes every new source and projection mandatory for publication.
    #[test]
    fn release_files_require_and_hash_declared_client_data() {
        let capture = capture();
        let (mut metadata, _) = load_snapshot(&capture.path).unwrap();
        metadata.object_count = Some(1);
        let source = auxiliary::ResourceSource {
            source_url: "https://patch.example.test/CardStyle.xml".into(),
            source_sha256: "a".repeat(64),
            record_count: 2,
        };
        metadata.client_data = Some(auxiliary::Metadata {
            schema_version: 1,
            card_styles: source.clone(),
            image_names: source,
            unresolved_style_count: 1,
        });
        save_metadata(&capture.path, &metadata).unwrap();
        fs::write(capture.path.join("card-data.jsonl.gz"), b"card projection").unwrap();
        assert!(write_release_files(&capture.path, &metadata).is_err());
        assert!(!capture.path.join("SHA256SUMS").exists());
        assert!(!capture.path.join("release-notes.md").exists());
        for name in auxiliary::CHECKSUM_ASSETS {
            fs::write(capture.path.join(name), name.as_bytes()).unwrap();
        }
        write_release_files(&capture.path, &metadata).unwrap();
        let checksums = fs::read_to_string(capture.path.join("SHA256SUMS")).unwrap();
        for name in checksum_assets(&metadata) {
            let expected = format!("{}  {name}", digest_file(&capture.path.join(name)).unwrap());
            assert!(checksums.lines().any(|line| line == expected));
        }
        let notes = fs::read_to_string(capture.path.join("release-notes.md")).unwrap();
        assert!(notes.contains("2 card styles (1 unresolved); 2 store-image records"));
    }

    /// Checksums cover all published source and data assets, including completed metadata.
    #[test]
    fn release_files_record_asset_digests_and_exported_count() {
        let capture = capture();
        let (mut metadata, _) = load_snapshot(&capture.path).unwrap();
        metadata.object_count = Some(2);
        save_metadata(&capture.path, &metadata).unwrap();
        fs::write(capture.path.join("card-data.jsonl.gz"), b"abc").unwrap();
        write_release_files(&capture.path, &metadata).unwrap();
        let checksums = fs::read_to_string(capture.path.join("SHA256SUMS")).unwrap();
        let lines: Vec<_> = checksums.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(
            lines[0],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  card-data.jsonl.gz"
        );
        assert!(lines[1].ends_with("  snapshot.json"));
        assert_eq!(
            lines[2],
            "1e88dd53cbf6357c1a89623e961386b7a03bf7f531c3615fa1568a574df8cbc2  MTGO.application"
        );
        assert_eq!(
            lines[3],
            "32a32d35627ba060223ddc9ebef9eff7a4c70768bb6117f404e4aeb7e44acf7f  MTGO.exe.manifest"
        );
        assert!(lines[4].ends_with("  MTGO.exe.config"));
        let notes = fs::read_to_string(capture.path.join("release-notes.md")).unwrap();
        assert!(notes.contains("2 objects; JSON Lines schema 3"));
        assert!(notes.contains(&metadata.source.application_manifest_url));
        assert!(notes.contains(&metadata.source.application_manifest_sha256));
        assert!(!capture.path.join("SHA256SUMS.tmp").exists());
        assert!(!capture.path.join("release-notes.md.tmp").exists());
    }
}
