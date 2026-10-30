//! Build a complete static card browser without contacting the upstream service.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::read::MultiGzDecoder;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{Result, Snapshot, checksum_assets, digest_file, load_snapshot, read_bounded};

/// Version of the browser manifest, index, shard wrapper, and relationship links.
const BROWSER_SCHEMA_VERSION: u32 = 1;
/// Maximum size of one decompressed JSONL record, including its line ending.
const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;
/// Bound on the decompressed catalog before generating a Pages-sized bundle.
const MAX_CATALOG_BYTES: u64 = 512 * 1024 * 1024;
/// Largest integer ID represented exactly by a JavaScript number.
const MAX_BROWSER_ID: u64 = 9_007_199_254_740_991;
/// Unique suffixes prevent simultaneous builds from sharing a staging directory.
static NEXT_BUILD: AtomicU64 = AtomicU64::new(0);
/// Frontend sources are embedded so the installed executable can build a site.
const WEB_ASSETS: [(&str, &[u8]); 7] = [
    ("index.html", include_bytes!("../../../web/index.html")),
    ("app.js", include_bytes!("../../../web/app.js")),
    ("styles.css", include_bytes!("../../../web/styles.css")),
    ("search.js", include_bytes!("../../../web/search.js")),
    ("format.js", include_bytes!("../../../web/format.js")),
    ("symbols.js", include_bytes!("../../../web/symbols.js")),
    (
        "search-worker.js",
        include_bytes!("../../../web/search-worker.js"),
    ),
];

/// One autocomplete entry: exact name, numeric shard, and object-kind mask.
type IndexEntry = (String, u8, u8);

/// Full original records and browser-only links for one portion of the catalog.
#[derive(Default, Serialize)]
struct Shard {
    /// A name's complete records remain together and preserve every source field.
    objects: BTreeMap<String, Vec<Value>>,
    /// Links use explicit namespaces while original source properties stay intact.
    relations: BTreeMap<String, Relationships>,
}

/// A resolved target can be loaded without an additional global ID index.
#[derive(Serialize)]
struct Link {
    /// Catalog ID of the target, rather than its card texture number.
    id: u64,
    /// Exact group name used as a key in the target shard.
    name: String,
    /// First SHA-256 byte of the target's UTF-8 exact name.
    shard: u8,
}

/// Known cross-object relationships resolved without modifying source records.
#[derive(Default, Serialize)]
struct Relationships {
    /// Another face, looked up through the texture-number namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    other_face: Option<Link>,
    /// Split-card components, in their declared left-to-right order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    split_faces: Vec<Link>,
    /// Whole split card containing this component.
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<Link>,
    /// The companion component of a split card.
    #[serde(skip_serializing_if = "Option::is_none")]
    other_half: Option<Link>,
}

impl Relationships {
    /// Omit empty link maps rather than repeating empty objects for ordinary cards.
    fn is_empty(&self) -> bool {
        self.other_face.is_none()
            && self.split_faces.is_empty()
            && self.parent.is_none()
            && self.other_half.is_none()
    }
}

/// Stable entry point that pins every browser request to one generated bundle.
#[derive(Serialize)]
struct BrowserManifest<'a> {
    /// Browser representation version, independent of the source JSONL schema.
    schema_version: u32,
    /// Upstream application identity for display and provenance.
    application_version: &'a str,
    /// Durable data-release tag used to rebuild this site.
    tag: &'a str,
    /// Verified number of complete source records in all shards.
    object_count: usize,
    /// Number of exact-name autocomplete entries, including noncard objects.
    name_count: usize,
    /// Digest of the downloaded, compressed JSONL archive.
    source_sha256: &'a str,
    /// Index path relative to the project site's root.
    index: String,
    /// Directory of hexadecimal shard filenames relative to the site root.
    shards: String,
    /// Shared set metadata, retaining the source's opaque values.
    sets: &'a BTreeMap<String, mtgoxml::carddata::SetMetadata>,
}

/// Temporary output is removed on every error before a completed site is installed.
struct StagingDirectory {
    /// Newly created sibling of the requested output directory.
    path: PathBuf,
}

impl StagingDirectory {
    /// Reserve a fresh build directory without altering existing nonempty output.
    fn create(output: &Path) -> Result<Self> {
        let name = output
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("site output must name a directory")?;
        if let Ok(metadata) = fs::symlink_metadata(output) {
            // Only an ordinary empty directory may be replaced by the completed build.
            if !metadata.is_dir() || fs::read_dir(output)?.next().is_some() {
                return Err("site output directory must be absent or empty".into());
            }
        }
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let path = parent.join(format!(
            ".{name}-building-{}-{}",
            std::process::id(),
            NEXT_BUILD.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    /// Install a validated site with a single directory rename on the same filesystem.
    fn publish(&self, output: &Path) -> Result<()> {
        if output.exists() {
            // A concurrent writer makes removal fail rather than losing its files.
            fs::remove_dir(output)?;
        }
        fs::rename(&self.path, output)?;
        Ok(())
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Verify a completed export and atomically create its offline browser bundle.
///
/// # Errors
///
/// Returns an error for invalid checksums or metadata, unsupported records,
/// duplicate IDs or textures, missing relationship targets, size limits, a
/// nonempty output directory, or filesystem and serialization failures.
pub(super) fn build(input: &Path, output: &Path) -> Result<()> {
    let staging = StagingDirectory::create(output)?;
    let (snapshot, _) = load_snapshot(input)?;
    let checksums = verify_checksums(input, &snapshot)?;
    verify_completion(&snapshot, &checksums)?;
    let records = read_catalog(input, &snapshot)?;
    let (index, shards) = partition(records)?;
    let data_directory = staging.path.join("data");
    let temporary_bundle = data_directory.join("building");
    fs::create_dir_all(&temporary_bundle)?;
    write_json(&temporary_bundle.join("index.json"), &index)?;
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"mtgoxml-browser\0");
    fingerprint.update(BROWSER_SCHEMA_VERSION.to_le_bytes());
    fingerprint.update(serde_json::to_vec(&snapshot)?);
    fingerprint.update(digest_file(&temporary_bundle.join("index.json"))?);
    for (number, shard) in shards.iter().enumerate() {
        // The complete serialized output, not just the source version, identifies a bundle.
        let filename = format!("{number:02x}.json");
        let path = temporary_bundle.join(&filename);
        write_json(&path, shard)?;
        fingerprint.update(filename.as_bytes());
        fingerprint.update(digest_file(&path)?);
    }
    let mut fingerprint_hex = String::with_capacity(64);
    for byte in fingerprint.finalize() {
        // The SHA-2 output array has no aggregate LowerHex implementation.
        write!(fingerprint_hex, "{byte:02x}")?;
    }
    let bundle = format!(
        "{}-{}",
        snapshot.source.application_version,
        &fingerprint_hex[..16]
    );
    fs::rename(&temporary_bundle, data_directory.join(&bundle))?;
    let manifest = BrowserManifest {
        schema_version: BROWSER_SCHEMA_VERSION,
        application_version: &snapshot.source.application_version,
        tag: &snapshot.tag,
        object_count: snapshot.object_count.ok_or("missing object count")?,
        name_count: index.len(),
        source_sha256: &checksums["card-data.jsonl.gz"],
        index: format!("data/{bundle}/index.json"),
        shards: format!("data/{bundle}/"),
        sets: &snapshot.sets,
    };
    write_json(&data_directory.join("current.json"), &manifest)?;
    for (name, bytes) in WEB_ASSETS {
        // Frontend files belong to the same complete deployment as the data manifest.
        fs::write(staging.path.join(name), bytes)?;
    }
    staging.publish(output)?;
    println!(
        "Built browser for {}: {} objects, {} names",
        snapshot.source.application_version, manifest.object_count, manifest.name_count
    );
    Ok(())
}

/// Validate exactly the expected release assets without accepting arbitrary paths.
fn verify_checksums(input: &Path, snapshot: &Snapshot) -> Result<BTreeMap<String, String>> {
    let bytes = read_bounded(File::open(input.join("SHA256SUMS"))?)?;
    let assets: Vec<_> = checksum_assets(snapshot).collect();
    let mut expected = BTreeMap::new();
    for line in std::str::from_utf8(&bytes)?.lines() {
        // Restrict filenames before opening any paths supplied by a checksum line.
        let (digest, name) = line.split_once("  ").ok_or("invalid SHA256SUMS line")?;
        if digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !assets.contains(&name)
        {
            return Err(format!("invalid SHA256SUMS entry: {name}").into());
        }
        if expected
            .insert(name.to_owned(), digest.to_ascii_lowercase())
            .is_some()
        {
            return Err(format!("duplicate SHA256SUMS entry: {name}").into());
        }
    }
    for name in assets {
        // Missing entries fail instead of weakening verification to a partial asset list.
        let expected_digest = expected
            .get(name)
            .ok_or_else(|| format!("missing SHA256SUMS entry: {name}"))?;
        if digest_file(&input.join(name))? != *expected_digest {
            return Err(format!("checksum mismatch: {name}").into());
        }
    }
    Ok(expected)
}

/// Require metadata produced only after the source export completed successfully.
fn verify_completion(snapshot: &Snapshot, checksums: &BTreeMap<String, String>) -> Result<()> {
    if snapshot.object_count.is_none_or(|count| count == 0) {
        return Err("site requires a completed nonempty export".into());
    }
    let artwork = snapshot
        .artwork
        .as_ref()
        .ok_or("site requires completed artwork provenance")?;
    if artwork.config_sha256 != checksums["MTGO.exe.config"] {
        return Err("artwork provenance does not match MTGO.exe.config".into());
    }
    if let Some(client_data) = &snapshot.client_data {
        // Auxiliary release data is optional, but a declared bundle must be coherent.
        if client_data.schema_version != 1
            || client_data.unresolved_style_count > client_data.card_styles.record_count
        {
            return Err("invalid client-data schema version or unresolved style count".into());
        }
        for (name, source) in [
            ("CardStyle.xml", &client_data.card_styles),
            ("DigitalObjectImageNames.xml", &client_data.image_names),
        ] {
            // Raw XML provenance must identify the same bytes verified from the release.
            if source.source_sha256 != checksums[name] {
                return Err(format!("client-data provenance does not match {name}").into());
            }
            let url = mtgoxml::Url::parse(&source.source_url)?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || url.fragment().is_some()
            {
                return Err(format!("invalid client-data source URL for {name}").into());
            }
        }
    }
    Ok(())
}

/// Stream bounded JSON Lines while retaining every record exactly once by catalog ID.
fn read_catalog(input: &Path, snapshot: &Snapshot) -> Result<BTreeMap<u64, Value>> {
    let gzip = MultiGzDecoder::new(File::open(input.join("card-data.jsonl.gz"))?);
    let mut reader = BufReader::new(gzip);
    let mut records = BTreeMap::new();
    let mut decompressed_bytes = 0;
    loop {
        // Read one bounded record, including EOF so gzip trailers and checksums are checked.
        let mut line = Vec::new();
        let length = reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if length == 0 {
            break;
        }
        decompressed_bytes += length as u64;
        if length as u64 > MAX_RECORD_BYTES || decompressed_bytes > MAX_CATALOG_BYTES {
            return Err("catalog exceeds its record or decompressed-size limit".into());
        }
        let record: Value = serde_json::from_slice(&line)
            .map_err(|error| format!("invalid catalog record {}: {error}", records.len() + 1))?;
        let id = record
            .get("id")
            .and_then(Value::as_u64)
            .filter(|id| *id <= MAX_BROWSER_ID)
            .ok_or("catalog ID must be an unsigned JavaScript-safe integer")?;
        exact_name(&record, id)?;
        if !record.get("mtgo").is_some_and(Value::is_object) {
            return Err(format!("catalog object {id} has no MTGO property map").into());
        }
        if records.insert(id, record).is_some() {
            return Err(format!("duplicate catalog ID: {id}").into());
        }
    }
    if snapshot.object_count != Some(records.len()) {
        return Err(format!(
            "catalog object count mismatch: metadata {:?}, read {}",
            snapshot.object_count,
            records.len()
        )
        .into());
    }
    Ok(records)
}

/// Extract the exact group identity without normalizing names or inventing aliases.
fn exact_name(record: &Value, id: u64) -> Result<&str> {
    record
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("catalog object {id} has no string name").into())
}

/// Select an evenly distributed shard while keeping all exact-name printings together.
fn shard_number(name: &str) -> u8 {
    Sha256::digest(name.as_bytes())[0]
}

/// Separate card, component, token, and other objects without discarding any kind.
fn kind_mask(record: &Value) -> u8 {
    match record["mtgo"]["DIGITAL_OBJECT_TYPE_CODE_STRING"].as_str() {
        Some("CARD") => 1,
        Some("SUBC") => 2,
        Some("TOKN") => 4,
        _ => 8,
    }
}

/// Resolve relationships before moving original records into their final shards.
fn partition(records: BTreeMap<u64, Value>) -> Result<(Vec<IndexEntry>, Vec<Shard>)> {
    let mut textures = BTreeMap::new();
    for (&id, record) in &records {
        // Texture numbers occupy a separate namespace from catalog IDs.
        if let Some(texture) = reference(record, id, "CARDTEXTURE_NUMBER")? {
            if let Some(previous) = textures.insert(texture, id) {
                return Err(format!(
                    "ambiguous texture {texture}: catalog IDs {previous} and {id}"
                )
                .into());
            }
        }
    }
    let mut relations = BTreeMap::new();
    for (&id, record) in &records {
        // Source-specific namespaces prevent a coincidental numeric ID match winning.
        let mut resolved = Relationships::default();
        if let Some(texture) = reference(record, id, "OTHER_FACE")? {
            let target = textures
                .get(&texture)
                .ok_or_else(|| format!("object {id} has missing OTHER_FACE texture {texture}"))?;
            resolved.other_face = Some(link(&records, *target, id, "OTHER_FACE")?);
        }
        for field in ["SPLITCARD0", "SPLITCARD0_1"] {
            // The source ordering records the split card's first and second components.
            if let Some(target) = reference(record, id, field)? {
                resolved
                    .split_faces
                    .push(link(&records, target, id, field)?);
            }
        }
        if let Some(target) = reference(record, id, "SPLITCARD_PARENTCARD")? {
            resolved.parent = Some(link(&records, target, id, "SPLITCARD_PARENTCARD")?);
        }
        if let Some(target) = reference(record, id, "SPLITCARD_OTHERCARD")? {
            resolved.other_half = Some(link(&records, target, id, "SPLITCARD_OTHERCARD")?);
        }
        if !resolved.is_empty() {
            relations.insert(id, resolved);
        }
    }
    let mut names: BTreeMap<String, u8> = BTreeMap::new();
    let mut shards: Vec<Shard> = (0..256).map(|_| Shard::default()).collect();
    for (id, record) in records {
        // Move records instead of duplicating the catalog's potentially large JSON values.
        let name = exact_name(&record, id)?.to_owned();
        *names.entry(name.clone()).or_default() |= kind_mask(&record);
        let shard = &mut shards[usize::from(shard_number(&name))];
        shard.objects.entry(name).or_default().push(record);
        if let Some(links) = relations.remove(&id) {
            shard.relations.insert(id.to_string(), links);
        }
    }
    let index = names
        .into_iter()
        .map(|(name, mask)| {
            let number = shard_number(&name);
            (name, number, mask)
        })
        .collect();
    Ok((index, shards))
}

/// Recognize a declared decimal reference without coercing unrelated source values.
fn reference(record: &Value, id: u64, field: &str) -> Result<Option<u64>> {
    let Some(value) = record["mtgo"].get(field) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| format!("object {id} has invalid {field} reference"))?;
    Ok(Some(text.parse().map_err(|_| {
        format!("object {id} has out-of-range {field} reference")
    })?))
}

/// Resolve one catalog-ID reference and include its exact name and target shard.
fn link(records: &BTreeMap<u64, Value>, target: u64, id: u64, field: &str) -> Result<Link> {
    let record = records
        .get(&target)
        .ok_or_else(|| format!("object {id} has missing {field} catalog ID {target}"))?;
    let name = exact_name(record, target)?.to_owned();
    Ok(Link {
        id: target,
        shard: shard_number(&name),
        name,
    })
}

/// Write compact JSON with buffered I/O and a conventional terminal newline.
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    serde_json::to_writer(&mut writer, value)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}

/// Synthetic exports exercise source preservation and data-integrity failures.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::auxiliary::{Metadata, ResourceSource};
    use crate::{ArtworkSource, LIVE_DEPLOYMENT_URL, save_metadata, snapshot, write_release_files};
    use flate2::{Compression, GzBuilder};
    use mtgoxml::Deployment;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// Independent temporary input and output trees for one test.
    struct TestDirectory {
        /// Parent directory removed after each success or failure assertion.
        path: PathBuf,
    }

    impl TestDirectory {
        /// Allocate a path safe for parallel test execution.
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "mtgoxml-site-test-{}-{}",
                std::process::id(),
                NEXT_BUILD.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        /// Write a small completed export with authentic manifest relationships.
        fn capture(&self, records: &[Value]) -> PathBuf {
            let input = self.path.join("snapshot");
            fs::create_dir_all(&input).unwrap();
            let deployment = include_bytes!("../../../fixtures/live/MTGO.application");
            let application = include_bytes!("../../../fixtures/live/MTGO.exe.manifest");
            let config = include_bytes!("../../../fixtures/artwork/download-source.config");
            for (name, bytes) in [
                ("MTGO.application", deployment.as_slice()),
                ("MTGO.exe.manifest", application.as_slice()),
                ("MTGO.exe.config", config.as_slice()),
            ] {
                // Fixture manifests pin the same source identity used by the real exporter.
                fs::write(input.join(name), bytes).unwrap();
            }
            let application = Deployment::from_bytes(LIVE_DEPLOYMENT_URL, deployment)
                .unwrap()
                .with_application_bytes(application)
                .unwrap();
            let mut metadata = snapshot(
                &application,
                include_bytes!("../../../fixtures/live/MTGO.exe.manifest"),
            )
            .unwrap();
            metadata.object_count = Some(records.len());
            metadata.artwork = Some(ArtworkSource {
                config_url: "https://patch.example.test/MTGO.exe.config".into(),
                config_sha256: crate::digest_bytes(config).unwrap(),
                base_url: "https://patch.example.test/art/".into(),
            });
            let mut gzip = GzBuilder::new().mtime(0).write(
                File::create(input.join("card-data.jsonl.gz")).unwrap(),
                Compression::new(6),
            );
            for record in records {
                // Emit the same one-record-per-line representation as a complete export.
                serde_json::to_writer(&mut gzip, record).unwrap();
                writeln!(gzip).unwrap();
            }
            gzip.finish().unwrap();
            save_metadata(&input, &metadata).unwrap();
            write_release_files(&input, &metadata).unwrap();
            input
        }

        /// Extend a legacy capture with small, checksummed auxiliary release assets.
        fn capture_with_client_data(&self, records: &[Value]) -> (PathBuf, Snapshot) {
            let input = self.capture(records);
            let (mut metadata, _) = load_snapshot(&input).unwrap();
            for (name, bytes) in [
                ("CardStyle.xml", br#"<AnimCache><AnimCardStyle Name="Unresolved" CloneFrom="Missing"/></AnimCache>"#.as_slice()),
                ("DigitalObjectImageNames.xml", br#"<DigitalObjectImageNamesData><Match Rule="Equals" MatchPattern="Ticket" ValueFormat="ticket.png"/></DigitalObjectImageNamesData>"#.as_slice()),
            ] {
                // Standalone XML samples keep the test independent of mutable client data.
                fs::write(input.join(name), bytes).unwrap();
            }
            for (name, value) in [
                ("card-styles.jsonl.gz", json!({"name":"Unresolved"})),
                (
                    "digital-object-image-names.jsonl.gz",
                    json!({"rule":"Equals","match_pattern":"Ticket","value_format":"ticket.png"}),
                ),
            ] {
                // Site generation verifies these archive bytes without consuming their records.
                let mut gzip = GzBuilder::new()
                    .mtime(0)
                    .write(File::create(input.join(name)).unwrap(), Compression::new(6));
                serde_json::to_writer(&mut gzip, &value).unwrap();
                writeln!(gzip).unwrap();
                gzip.finish().unwrap();
            }
            metadata.client_data = Some(Metadata {
                schema_version: 1,
                card_styles: ResourceSource {
                    source_url: "https://patch.example.test/CardStyle.xml".into(),
                    source_sha256: digest_file(&input.join("CardStyle.xml")).unwrap(),
                    record_count: 1,
                },
                image_names: ResourceSource {
                    source_url: "https://patch.example.test/DigitalObjectImageNames.xml".into(),
                    source_sha256: digest_file(&input.join("DigitalObjectImageNames.xml")).unwrap(),
                    record_count: 1,
                },
                unresolved_style_count: 1,
            });
            save_capture_checksums(&input, &metadata);
            (input, metadata)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Minimal source object with unpromoted fields retained in a nested map.
    fn record(id: u64, name: &str, kind: &str, texture: u64) -> Value {
        json!({
            "id":id,"name":name,"set":"TEST","foil":false,
            "mtgo":{"DIGITAL_OBJECT_TYPE_CODE_STRING":kind,"CARDTEXTURE_NUMBER":texture.to_string(),
            "UNKNOWN":{"TEXT":"001 / X / café 🧙","FLAG":true}}
        })
    }

    /// Read JSON output for exact structure and source-value comparisons.
    fn json_file(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    /// Rewrite checksums after intentional metadata changes to isolate semantic validation.
    fn save_capture_checksums(input: &Path, metadata: &Snapshot) {
        save_metadata(input, metadata).unwrap();
        let mut checksums = String::new();
        for name in checksum_assets(metadata) {
            writeln!(
                checksums,
                "{}  {name}",
                digest_file(&input.join(name)).unwrap()
            )
            .unwrap();
        }
        fs::write(input.join("SHA256SUMS"), checksums).unwrap();
    }

    /// Resolve a name's generated shard using the public manifest paths.
    fn output_shard(output: &Path, manifest: &Value, name: &str) -> Value {
        let filename = format!("{:02x}.json", shard_number(name));
        json_file(
            &output
                .join(manifest["shards"].as_str().unwrap())
                .join(filename),
        )
    }

    /// Generated shards preserve every field and object while grouping printings by name.
    #[test]
    fn round_trip_preserves_records_and_kind_masks_deterministically() {
        let directory = TestDirectory::new();
        let mut foil = record(2, "O’Connor's café", "CARD", 12);
        foil["foil"] = json!(true);
        let records = vec![
            record(1, "O’Connor's café", "CARD", 10),
            foil,
            record(3, "O’Connor's café", "TOKN", 14),
            record(4, "Booster", "BSTR", 16),
        ];
        let input = directory.capture(&records);
        let (metadata, _) = load_snapshot(&input).unwrap();
        assert!(metadata.client_data.is_none());
        assert_eq!(
            verify_checksums(&input, &metadata)
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
                "MTGO.application",
                "MTGO.exe.config",
                "MTGO.exe.manifest",
                "card-data.jsonl.gz",
                "snapshot.json"
            ]
        );
        let first = directory.path.join("first");
        let second = directory.path.join("second");
        build(&input, &first).unwrap();
        build(&input, &second).unwrap();
        let manifest = json_file(&first.join("data/current.json"));
        assert_eq!(manifest["object_count"], records.len());
        assert_eq!(manifest["schema_version"], BROWSER_SCHEMA_VERSION);
        assert_eq!(manifest, json_file(&second.join("data/current.json")));
        let index = json_file(&first.join(manifest["index"].as_str().unwrap()));
        let entry = index
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry[0] == "O’Connor's café")
            .unwrap();
        assert_eq!(entry[2], 5);
        let mut restored = BTreeMap::new();
        for number in 0..256 {
            // Read every shard, rather than assuming a fixed number of names or records.
            let filename = format!("{number:02x}.json");
            let shard = json_file(
                &first
                    .join(manifest["shards"].as_str().unwrap())
                    .join(filename),
            );
            for objects in shard["objects"].as_object().unwrap().values() {
                // Arrays retain distinct foil IDs and all nested source values.
                for record in objects.as_array().unwrap() {
                    assert!(
                        restored
                            .insert(record["id"].as_u64().unwrap(), record.clone())
                            .is_none()
                    );
                }
            }
        }
        assert_eq!(restored.len(), records.len());
        for record in records {
            assert_eq!(restored[&record["id"].as_u64().unwrap()], record);
        }
        assert!(first.join("index.html").is_file());
        assert!(first.join("search-worker.js").is_file());
    }

    /// Optional auxiliary assets are verified while card records and browser schema stay intact.
    #[test]
    fn verifies_client_data_assets_without_changing_browser_records() {
        let directory = TestDirectory::new();
        let original = record(1, "Card", "CARD", 10);
        let (input, metadata) = directory.capture_with_client_data(std::slice::from_ref(&original));
        let checksums = verify_checksums(&input, &metadata).unwrap();
        for name in crate::auxiliary::CHECKSUM_ASSETS {
            assert_eq!(checksums[name], digest_file(&input.join(name)).unwrap());
        }
        let output = directory.path.join("site");
        build(&input, &output).unwrap();
        let manifest = json_file(&output.join("data/current.json"));
        assert_eq!(manifest["schema_version"], BROWSER_SCHEMA_VERSION);
        assert_eq!(
            output_shard(&output, &manifest, "Card")["objects"]["Card"][0],
            original
        );
    }

    /// Every declared auxiliary asset and checksum entry is required before publication.
    #[test]
    fn rejects_corrupted_missing_or_unlisted_client_data_assets() {
        for name in crate::auxiliary::CHECKSUM_ASSETS {
            // Exercise all four files rather than only the first new checksum entry.
            for variant in ["corrupted", "missing file", "missing checksum"] {
                // Each capture starts complete so one semantic failure is isolated.
                let directory = TestDirectory::new();
                let (input, _) =
                    directory.capture_with_client_data(&[record(1, "Card", "CARD", 10)]);
                match variant {
                    "corrupted" => fs::write(input.join(name), b"changed asset").unwrap(),
                    "missing file" => fs::remove_file(input.join(name)).unwrap(),
                    _ => {
                        // Retain every other valid checksum while omitting this required entry.
                        let listing = fs::read_to_string(input.join("SHA256SUMS")).unwrap();
                        let omitted = format!("  {name}");
                        let mut updated = String::new();
                        for line in listing.lines().filter(|line| !line.ends_with(&omitted)) {
                            writeln!(updated, "{line}").unwrap();
                        }
                        fs::write(input.join("SHA256SUMS"), updated).unwrap();
                    }
                }
                let output = directory.path.join("site");
                assert!(build(&input, &output).is_err(), "{name}: {variant}");
                assert!(!output.exists());
            }
        }
        let directory = TestDirectory::new();
        let (input, metadata) =
            directory.capture_with_client_data(&[record(1, "Card", "CARD", 10)]);
        let mut listing = fs::read_to_string(input.join("SHA256SUMS")).unwrap();
        listing.push_str(&format!("{}  unknown-client-data.xml\n", "0".repeat(64)));
        fs::write(input.join("SHA256SUMS"), listing).unwrap();
        assert!(
            verify_checksums(&input, &metadata)
                .unwrap_err()
                .to_string()
                .contains("invalid SHA256SUMS entry")
        );
    }

    /// Auxiliary checksum entries cannot silently extend a legacy export's asset contract.
    #[test]
    fn rejects_auxiliary_checksums_without_declared_client_data() {
        let directory = TestDirectory::new();
        let (input, mut metadata) =
            directory.capture_with_client_data(&[record(1, "Card", "CARD", 10)]);
        metadata.client_data = None;
        save_metadata(&input, &metadata).unwrap();
        let listing = fs::read_to_string(input.join("SHA256SUMS")).unwrap();
        let listing = listing
            .lines()
            .map(|line| {
                if line.ends_with("  snapshot.json") {
                    format!(
                        "{}  snapshot.json\n",
                        digest_file(&input.join("snapshot.json")).unwrap()
                    )
                } else {
                    format!("{line}\n")
                }
            })
            .collect::<String>();
        fs::write(input.join("SHA256SUMS"), listing).unwrap();
        let output = directory.path.join("site");
        assert!(
            build(&input, &output)
                .unwrap_err()
                .to_string()
                .contains("invalid SHA256SUMS entry")
        );
        assert!(!output.exists());
    }

    /// Correct checksums do not repair unsupported metadata or mismatched raw XML provenance.
    #[test]
    fn rejects_invalid_client_data_metadata_and_provenance() {
        for variant in [
            "schema",
            "count",
            "style source",
            "image source",
            "source URL",
            "fragment",
        ] {
            // Rehash the edited metadata to reach semantic validation after file verification.
            let directory = TestDirectory::new();
            let (input, mut metadata) =
                directory.capture_with_client_data(&[record(1, "Card", "CARD", 10)]);
            let client_data = metadata.client_data.as_mut().unwrap();
            match variant {
                "schema" => client_data.schema_version = 2,
                "count" => {
                    client_data.unresolved_style_count = client_data.card_styles.record_count + 1
                }
                "style source" => client_data.card_styles.source_sha256 = "0".repeat(64),
                "image source" => client_data.image_names.source_sha256 = "0".repeat(64),
                "source URL" => {
                    client_data.card_styles.source_url =
                        "ftp://patch.example.test/CardStyle.xml".into()
                }
                _ => client_data.image_names.source_url.push_str("#fragment"),
            }
            save_capture_checksums(&input, &metadata);
            let output = directory.path.join("site");
            assert!(build(&input, &output).is_err(), "{variant}");
            assert!(!output.exists());
        }
    }

    /// Texture references must not accidentally resolve to equal-valued catalog IDs.
    #[test]
    fn relationships_use_their_declared_namespaces() {
        let directory = TestDirectory::new();
        let mut front = record(1, "Front", "CARD", 10);
        front["mtgo"]["OTHER_FACE"] = json!("20");
        let mut back = record(2, "Back", "CARD", 20);
        back["mtgo"]["OTHER_FACE"] = json!("10");
        let mut parent = record(3, "Fire // Ice", "CARD", 30);
        parent["mtgo"]["SPLITCARD0"] = json!("4");
        parent["mtgo"]["SPLITCARD0_1"] = json!("5");
        let mut half = record(4, "Fire", "SUBC", 40);
        half["mtgo"]["SPLITCARD_PARENTCARD"] = json!("3");
        half["mtgo"]["SPLITCARD_OTHERCARD"] = json!("5");
        let records = [
            front,
            back,
            parent,
            half,
            record(5, "Ice", "SUBC", 50),
            record(20, "Wrong", "CARD", 60),
        ];
        let input = directory.capture(&records);
        let output = directory.path.join("site");
        build(&input, &output).unwrap();
        let manifest = json_file(&output.join("data/current.json"));
        let front = output_shard(&output, &manifest, "Front");
        assert_eq!(front["relations"]["1"]["other_face"]["id"], 2);
        assert_eq!(front["relations"]["1"]["other_face"]["name"], "Back");
        assert_eq!(
            front["relations"]["1"]["other_face"]["shard"],
            shard_number("Back")
        );
        let parent = output_shard(&output, &manifest, "Fire // Ice");
        assert_eq!(parent["relations"]["3"]["split_faces"][0]["id"], 4);
        assert_eq!(parent["relations"]["3"]["split_faces"][1]["id"], 5);
        let half = output_shard(&output, &manifest, "Fire");
        assert_eq!(half["relations"]["4"]["parent"]["id"], 3);
        assert_eq!(half["relations"]["4"]["other_half"]["id"], 5);
    }

    /// Checksum failures happen before publishing even when a completed output is requested.
    #[test]
    fn rejects_corrupted_or_unlisted_assets() {
        let directory = TestDirectory::new();
        let input = directory.capture(&[record(1, "Card", "CARD", 10)]);
        let output = directory.path.join("site");
        fs::write(input.join("MTGO.exe.config"), b"changed").unwrap();
        assert!(
            build(&input, &output)
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch")
        );
        assert!(!output.exists());
        let line = format!("{}  ../unlisted\n", "0".repeat(64));
        fs::write(input.join("SHA256SUMS"), line).unwrap();
        assert!(
            build(&input, &output)
                .unwrap_err()
                .to_string()
                .contains("invalid SHA256SUMS entry")
        );
        assert!(!output.exists());
    }

    /// Counts and identity ambiguity fail closed instead of silently removing records.
    #[test]
    fn rejects_duplicate_ids_textures_and_incorrect_counts() {
        for variant in ["id", "texture", "count"] {
            // Each invalid export has valid checksums, isolating semantic validation.
            let directory = TestDirectory::new();
            let records = [
                record(1, "First", "CARD", 10),
                record(
                    if variant == "id" { 1 } else { 2 },
                    "Second",
                    "CARD",
                    if variant == "texture" { 10 } else { 20 },
                ),
            ];
            let input = directory.capture(&records);
            if variant == "count" {
                let (mut metadata, _) = load_snapshot(&input).unwrap();
                metadata.object_count = Some(records.len() + 1);
                save_metadata(&input, &metadata).unwrap();
                write_release_files(&input, &metadata).unwrap();
            }
            let output = directory.path.join("site");
            let error = build(&input, &output).unwrap_err().to_string();
            assert!(
                error.contains(match variant {
                    "id" => "duplicate catalog ID",
                    "texture" => "ambiguous texture",
                    _ => "count mismatch",
                }),
                "{error}"
            );
            assert!(!output.exists());
        }
    }

    /// Missing known targets prevent installing incomplete navigation links.
    #[test]
    fn rejects_missing_relationship_targets_and_preserves_existing_output() {
        let directory = TestDirectory::new();
        let mut missing = record(1, "Front", "CARD", 10);
        missing["mtgo"]["OTHER_FACE"] = json!("99");
        let input = directory.capture(&[missing]);
        let output = directory.path.join("site");
        assert!(
            build(&input, &output)
                .unwrap_err()
                .to_string()
                .contains("missing OTHER_FACE")
        );
        assert!(!output.exists());
        fs::create_dir(&output).unwrap();
        fs::write(output.join("index.html"), "previous site").unwrap();
        assert!(
            build(&input, &output)
                .unwrap_err()
                .to_string()
                .contains("absent or empty")
        );
        assert_eq!(
            fs::read_to_string(output.join("index.html")).unwrap(),
            "previous site"
        );
        let leftovers: BTreeSet<_> = fs::read_dir(&directory.path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 2);
    }
}
