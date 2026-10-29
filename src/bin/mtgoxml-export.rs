//! Capture a manifest snapshot and produce its verified JSON Lines release assets.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::{Compression, GzBuilder};
use mtgoxml::carddata::{CardDataFile, ResolvedValue};
use mtgoxml::clickonce::{
    Dependency, DependencyType, DigestAlgorithm, Hash, HashTransform, Manifest,
};
use mtgoxml::{Application, CardDatabase, Deployment, LIVE_DEPLOYMENT_URL, ResourceKind, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ureq::ResponseExt;

/// Version of the published JSON representation.
const SCHEMA_VERSION: u32 = 2;
/// Bound for a raw manifest or card-data component before parsing.
const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
/// Assets hashed in the published checksum file, in their stable listing order.
const CHECKSUM_ASSETS: [&str; 4] = [
    "card-data.jsonl.gz",
    "snapshot.json",
    "MTGO.application",
    "MTGO.exe.manifest",
];
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
}

/// Dispatch manifest discovery or export of the previously captured manifest.
fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next().ok_or("expected probe or export")?;
    let directory = PathBuf::from(arguments.next().ok_or("expected output directory")?);
    let source = arguments.next();
    if arguments.next().is_some() {
        return Err(
            "usage: mtgoxml-export probe DIR [DEPLOYMENT_URL] | mtgoxml-export export DIR".into(),
        );
    }
    match command.as_str() {
        "probe" => probe(&directory, source.as_deref().unwrap_or(LIVE_DEPLOYMENT_URL)),
        "export" if source.is_none() => export(&directory),
        "export" => Err("export accepts only the captured snapshot directory".into()),
        _ => Err("expected probe or export".into()),
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
    for name in ["card-data.jsonl.gz", "SHA256SUMS", "release-notes.md"] {
        // Clear all completed outputs so a failed retry cannot retain stale release files.
        match fs::remove_file(directory.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let (mut snapshot, application) = load_snapshot(directory)?;
    snapshot.object_count = None;
    save_metadata(directory, &snapshot)?;
    let agent = agent();
    let database = load_database(&application, |resource| Ok(fetch(&agent, &resource.url)?.1))?;

    // An interrupted export leaves only a temporary gzip, never a completed asset.
    let temporary = directory.join("card-data.jsonl.gz.tmp");
    write_database(File::create(&temporary)?, &database)?;
    fs::rename(temporary, directory.join("card-data.jsonl.gz"))?;
    snapshot.object_count = Some(database.len());
    save_metadata(directory, &snapshot)?;
    write_release_files(directory, &snapshot)?;
    println!("Exported {} objects", database.len());
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
    let mut hash = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(hash, "{byte:02x}")?;
    }
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
    })
}

/// Check the raw application bytes against the deployment's selected application reference.
fn verify_application(deployment: &Deployment, bytes: &[u8]) -> Result<()> {
    let reference = deployment
        .manifest()
        .dependencies
        .iter()
        .find_map(|dependency| match dependency {
            Dependency::Assembly(assembly)
                if assembly.codebase.is_some()
                    && assembly.prerequisite != Some(true)
                    && matches!(
                        assembly.dependency_type,
                        None | Some(DependencyType::Install)
                    ) =>
            {
                Some(assembly)
            }
            _ => None,
        })
        .ok_or("deployment has no application reference")?;
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
    for resource in application.card_data_resources().filter(|resource| {
        resource.kind == ResourceKind::File
            && resource.is_xml()
            && !resource
                .path
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("MTGOV3CardData.xml"))
    }) {
        // Verify bytes before interpreting XML or accepting a source identity.
        let bytes = read(resource)?;
        verify_bytes(&resource.path, &bytes, resource.size, &resource.hashes)?;
        let file = CardDataFile::from_bytes(&bytes)?;
        let filename = resource.path.rsplit('/').next().unwrap_or_default();
        let matches = match &file {
            CardDataFile::Set(set) => {
                filename.eq_ignore_ascii_case(&format!("client_{}.xml", set.id))
                    || filename.eq_ignore_ascii_case(&format!("client_{}_DO.xml", set.id))
            }
            CardDataFile::Lookup(table) => {
                filename.eq_ignore_ascii_case(&format!("{}.xml", table.name))
            }
        };
        if !matches {
            return Err(format!("component identity does not match {}", resource.path).into());
        }
        database.insert(application, file)?;
    }
    if database.is_empty() {
        return Err("snapshot contains no digital objects".into());
    }
    database.validate()?;
    Ok(database)
}

/// Stream one compact JSON object per LF-terminated line into a deterministic gzip.
fn write_database(writer: impl Write, database: &CardDatabase) -> Result<()> {
    let mut gzip = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(writer, Compression::new(6));
    for object in database.objects() {
        // Resolve and serialize one object at a time in stable numeric ID order.
        let object = object?;
        serde_json::to_writer(
            &mut gzip,
            &json!({
                "id": object.id(),
                "set": { "id": object.set().id, "age": object.set().age, "kind": object.set().kind },
                "fields": fields_json(object.fields()),
            }),
        )?;
        writeln!(gzip)?;
    }
    gzip.finish()?.flush()?;
    Ok(())
}

/// Preserve text, presence flags, and recursive property groups as distinct JSON values.
fn fields_json(fields: &BTreeMap<&str, ResolvedValue<'_>>) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|(name, value)| {
                let value = match value {
                    ResolvedValue::Text(value) => Value::String((*value).to_owned()),
                    ResolvedValue::Flag => Value::Bool(true),
                    ResolvedValue::Fields(fields) => fields_json(fields),
                };
                ((*name).to_owned(), value)
            })
            .collect(),
    )
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

/// Generate publication metadata only after all four complete export assets exist.
fn write_release_files(directory: &Path, snapshot: &Snapshot) -> Result<()> {
    let count = snapshot
        .object_count
        .filter(|count| *count > 0)
        .ok_or("cannot generate release files before a nonempty export completes")?;
    let mut checksums = String::new();
    for name in CHECKSUM_ASSETS {
        // Reject missing or empty assets before installing either release metadata file.
        let path = directory.join(name);
        if !path.is_file() || path.metadata()?.len() == 0 {
            return Err(format!("export asset {name} is missing or empty").into());
        }
        writeln!(checksums, "{}  {name}", digest_file(&path)?)?;
    }
    let notes = format!(
        "Resolved MTGO catalog: {count} objects; JSON Lines schema {SCHEMA_VERSION}.\n\n\
         Application manifest: {}\n\n\
         Manifest SHA-256: `{}`\n\n\
         Includes compressed JSON Lines, source manifests, metadata, and SHA256SUMS.\n",
        snapshot.source.application_manifest_url, snapshot.source.application_manifest_sha256,
    );
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
        let deployment_bytes = include_bytes!("../../fixtures/live/MTGO.application");
        let application_bytes = include_bytes!("../../fixtures/live/MTGO.exe.manifest");
        fs::write(capture.path.join("MTGO.application"), deployment_bytes).unwrap();
        fs::write(capture.path.join("MTGO.exe.manifest"), application_bytes).unwrap();
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
        </CardSet>"#).unwrap()).unwrap();
        database.validate().unwrap();
        let mut first = Vec::new();
        let mut second = Vec::new();
        write_database(&mut first, &database).unwrap();
        write_database(&mut second, &database).unwrap();
        assert_eq!(first, second);
        let mut decoded = String::new();
        flate2::read::GzDecoder::new(first.as_slice())
            .read_to_string(&mut decoded)
            .unwrap();
        assert!(!decoded.starts_with('\u{feff}'));
        assert!(decoded.ends_with('\n'));
        assert!(!decoded.contains('\r'));
        let lines: Vec<_> = decoded.split_terminator('\n').collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| !line.is_empty()));
        let objects: Vec<Value> = lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0]["id"], 1);
        assert_eq!(objects[1]["id"], 2);
        assert_eq!(
            objects[1]["set"],
            json!({"id":"TEST", "age":7,"kind":"future"})
        );
        assert_eq!(
            objects[1]["fields"]["NAME"],
            "Name\nline\r\"quoted\" café 🧙"
        );
        assert!(objects[0].get("source").is_none());
        assert!(objects[0].get("schema_version").is_none());
        assert_eq!(objects[1]["fields"]["IS_FOIL"], true);
        assert_eq!(objects[1]["fields"]["EMPTY"], "");
        assert_eq!(
            objects[1]["fields"]["NESTED"]["UNKNOWN"],
            "quoted & decoded"
        );
        assert_eq!(
            objects[0]["fields"]["DIGITAL_OBJECT_TYPE_CODE_STRING"],
            "BSTR"
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

    /// Checksums cover all published source and data assets, including completed metadata.
    #[test]
    fn release_files_record_asset_digests_and_exported_count() {
        let capture = capture();
        let (mut metadata, _) = load_snapshot(&capture.path).unwrap();
        metadata.object_count = Some(156735);
        save_metadata(&capture.path, &metadata).unwrap();
        fs::write(capture.path.join("card-data.jsonl.gz"), b"abc").unwrap();
        write_release_files(&capture.path, &metadata).unwrap();
        let checksums = fs::read_to_string(capture.path.join("SHA256SUMS")).unwrap();
        let lines: Vec<_> = checksums.lines().collect();
        assert_eq!(lines.len(), 4);
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
        let notes = fs::read_to_string(capture.path.join("release-notes.md")).unwrap();
        assert!(notes.contains("156735 objects; JSON Lines schema 2"));
        assert!(notes.contains(&metadata.source.application_manifest_url));
        assert!(notes.contains(&metadata.source.application_manifest_sha256));
        assert!(!capture.path.join("SHA256SUMS.tmp").exists());
        assert!(!capture.path.join("release-notes.md.tmp").exists());
    }
}
