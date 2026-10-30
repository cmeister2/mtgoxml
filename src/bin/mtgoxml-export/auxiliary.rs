//! Verified auxiliary XML and resolved JSON Lines assets for one application snapshot.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::{Compression, GzBuilder, write::GzEncoder};
use mtgoxml::cardstyle::{CardStyle, CardStyles, Error as StyleError, StyleProperties};
use mtgoxml::imagenames::DigitalObjectImageNames;
use mtgoxml::{Application, CardDatabase, Resource, ResourceKind, Url};
use serde::{Deserialize, Serialize};

use super::{Result, check_url, digest_bytes, verify_bytes};

/// Additional release assets covered by the completed snapshot checksum file.
pub(super) const CHECKSUM_ASSETS: [&str; 4] = [
    "card-styles.jsonl.gz",
    "digital-object-image-names.jsonl.gz",
    "CardStyle.xml",
    "DigitalObjectImageNames.xml",
];

/// Process-local suffixes prevent stale files or concurrent exports from sharing output.
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);
/// Maximum stale-name collisions retried while reserving a temporary output file.
const MAX_TEMPORARY_ATTEMPTS: usize = 64;

/// Provenance and record counts for the auxiliary-data JSONL schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Metadata {
    /// Version of the resolved auxiliary-data representation.
    pub(super) schema_version: u32,
    /// Verified source of all resolved or explicitly unresolved style rows.
    pub(super) card_styles: ResourceSource,
    /// Verified naming rules used for the store-object filename projection.
    pub(super) image_names: ResourceSource,
    /// Styles whose explicit clone chain has a missing base or a cycle.
    pub(super) unresolved_style_count: usize,
}

/// Exact source bytes and number of records produced from a client resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ResourceSource {
    /// Final response URL after any HTTP redirects.
    pub(super) source_url: String,
    /// SHA-256 of the captured, undecoded resource bytes.
    pub(super) source_sha256: String,
    /// Number of rows in the corresponding compressed JSON Lines asset.
    pub(super) record_count: usize,
}

/// One named style retains either its effective properties or its unresolved declaration.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum StyleRow<'a> {
    /// The complete explicit clone chain was resolved successfully.
    Resolved {
        /// Exact style name used as the source lookup key.
        name: &'a str,
        /// Effective properties after all available ancestors have been applied.
        properties: &'a StyleProperties,
    },
    /// The source declaration remains useful despite an incomplete clone chain.
    Unresolved {
        /// Exact style name used as the source lookup key.
        name: &'a str,
        /// Original declaration, without fabricated inherited defaults.
        declaration: &'a CardStyle,
        /// Stable structured description of the unresolved reference.
        error: UnresolvedStyle,
    },
}

/// Stable machine-readable failures that affect individual style declarations.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum UnresolvedStyle {
    /// The named style or one of its explicit clone ancestors is absent.
    MissingReference {
        /// Namespace of the missing source declaration.
        target_kind: &'static str,
        /// Exact missing declaration key.
        key: String,
    },
    /// Following clone references repeated a declaration.
    ReferenceCycle {
        /// Source traversal including the repeated name that closes the cycle.
        chain: Vec<String>,
    },
}

/// A store-pipeline object's generated filename, retaining empty and absent outcomes.
#[derive(Serialize)]
struct ImageNameRow {
    /// Catalog ID of the resolved object.
    id: u64,
    /// Generated filename; an unmatched rule is null and an empty result stays empty.
    image_name: Option<String>,
}

/// Temporary outputs owned by this operation until all four assets are complete.
struct PendingOutputs {
    /// Directory containing the captured snapshot and generated release assets.
    directory: PathBuf,
    /// Temporary files created exclusively by this operation and their final paths.
    files: Vec<(PathBuf, PathBuf)>,
}

impl PendingOutputs {
    /// Associate a group of temporary files with an existing output directory.
    fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            files: Vec::new(),
        }
    }

    /// Create one exclusive temporary file without overwriting another writer's work.
    fn create(&mut self, name: &str) -> Result<File> {
        for _ in 0..MAX_TEMPORARY_ATTEMPTS {
            // Reserve a distinct name even when a crashed process left earlier temporary files.
            let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
            let temporary = self
                .directory
                .join(format!("{name}.{}.{sequence}.tmp", std::process::id()));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => {
                    self.files.push((temporary, self.directory.join(name)));
                    return Ok(file);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err("could not reserve a distinct auxiliary temporary output file".into())
    }

    /// Publish completed bytes only after parsing and both projections have succeeded.
    fn publish(&self) -> Result<()> {
        for (temporary, destination) in &self.files {
            fs::rename(temporary, destination)?;
        }
        Ok(())
    }
}

impl Drop for PendingOutputs {
    fn drop(&mut self) {
        for (temporary, _) in &self.files {
            let _ = fs::remove_file(temporary);
        }
    }
}

/// Capture verified auxiliary resources and publish deterministic resolved JSON Lines.
///
/// # Errors
///
/// Returns an error for mismatched source identities, absent resources, failed
/// resource verification, invalid XML, failed filename generation, or filesystem
/// and serialization errors. Missing style ancestors and cycles are retained as
/// unresolved rows. The caller publishes completion metadata only after success.
pub(super) fn export(
    directory: &Path,
    application: &Application,
    database: &CardDatabase,
    mut read: impl FnMut(&Resource) -> Result<(Url, Vec<u8>)>,
) -> Result<Metadata> {
    if database.application_url() != application.url()
        || database.identity() != &application.manifest().identity
    {
        return Err("auxiliary data and card database belong to different applications".into());
    }
    let (mut style_source, style_bytes) = load_source(application, "CardStyle.xml", &mut read)?;
    let (mut image_source, image_bytes) =
        load_source(application, "DigitalObjectImageNames.xml", &mut read)?;
    let styles = CardStyles::from_bytes(&style_bytes)?;
    let image_names = DigitalObjectImageNames::from_bytes(&image_bytes)?;
    let mut pending = PendingOutputs::new(directory);
    let unresolved_style_count = write_gzip(pending.create(CHECKSUM_ASSETS[0])?, |writer| {
        write_styles(writer, &styles)
    })?;
    style_source.record_count = styles.styles.len();
    image_source.record_count = write_gzip(pending.create(CHECKSUM_ASSETS[1])?, |writer| {
        write_image_names(writer, database, &image_names)
    })?;
    pending.create("CardStyle.xml")?.write_all(&style_bytes)?;
    pending
        .create("DigitalObjectImageNames.xml")?
        .write_all(&image_bytes)?;
    pending.publish()?;
    Ok(Metadata {
        schema_version: 1,
        card_styles: style_source,
        image_names: image_source,
        unresolved_style_count,
    })
}

/// Fetch and authenticate a literal resource from the captured application catalog.
fn load_source(
    application: &Application,
    path: &str,
    read: &mut impl FnMut(&Resource) -> Result<(Url, Vec<u8>)>,
) -> Result<(ResourceSource, Vec<u8>)> {
    let resource = application
        .resource(path)
        .filter(|resource| resource.kind == ResourceKind::File)
        .ok_or_else(|| format!("application has no {path} file resource"))?;
    let (url, bytes) = read(resource)?;
    check_url(&url)?;
    verify_bytes(&resource.path, &bytes, resource.size, &resource.hashes)?;
    let source = ResourceSource {
        source_url: url.to_string(),
        source_sha256: digest_bytes(&bytes)?,
        record_count: 0,
    };
    Ok((source, bytes))
}

/// Finish a deterministic gzip stream only after every emitted record is complete.
fn write_gzip<T>(writer: File, emit: impl FnOnce(&mut GzEncoder<File>) -> Result<T>) -> Result<T> {
    let mut gzip = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(writer, Compression::new(6));
    let result = emit(&mut gzip)?;
    gzip.finish()?.flush()?;
    Ok(result)
}

/// Serialize styles in exact name order while preserving incomplete declarations.
fn write_styles(writer: &mut impl Write, styles: &CardStyles) -> Result<usize> {
    let mut unresolved = 0;
    for (name, declaration) in &styles.styles {
        // Every declaration produces a row; unresolved chains never masquerade as resolved data.
        let resolution = styles.resolve(name);
        let row = match resolution {
            Ok(ref resolved) => StyleRow::Resolved {
                name,
                properties: &resolved.properties,
            },
            Err(StyleError::MissingReference { kind, key }) => {
                // Retain the declaration and a typed missing-reference failure rather than aborting the catalog.
                unresolved += 1;
                StyleRow::Unresolved {
                    name,
                    declaration,
                    error: UnresolvedStyle::MissingReference {
                        target_kind: kind,
                        key,
                    },
                }
            }
            Err(StyleError::ReferenceCycle(chain)) => {
                // Cycle diagnostics preserve the deterministic source traversal from the requested name.
                unresolved += 1;
                StyleRow::Unresolved {
                    name,
                    declaration,
                    error: UnresolvedStyle::ReferenceCycle { chain },
                }
            }
            Err(error) => return Err(error.into()),
        };
        serde_json::to_writer(&mut *writer, &row)?;
        writeln!(writer)?;
    }
    Ok(unresolved)
}

/// Project only the client's store pipeline, using its presence-based object marker.
fn write_image_names(
    writer: &mut impl Write,
    database: &CardDatabase,
    names: &DigitalObjectImageNames,
) -> Result<usize> {
    let mut count = 0;
    for object in database.objects() {
        // Keep catalog order and inherited markers; an explicit zero still selects this pipeline.
        let object = object?;
        if object.field_ref("IS_DIGITALOBJECT").is_none() {
            continue;
        }
        let row = ImageNameRow {
            id: object.id(),
            image_name: names.for_object(&object)?,
        };
        serde_json::to_writer(&mut *writer, &row)?;
        writeln!(writer)?;
        count += 1;
    }
    Ok(count)
}

/// Synthetic source and projection regressions without retained binary fixtures.
#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use mtgoxml::carddata::CardDataFile;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::io::Read;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Complete small styles source used by the captured manifest.
    const STYLES: &str = r#"<AnimCache><AnimCardStyle Name="Leaf" CloneFrom="Base"><Frame>none</Frame></AnimCardStyle><AnimCardStyle Name="Base"><Frame>base frame</Frame><Model>base model</Model><ArtSuffix>_base</ArtSuffix></AnimCardStyle><AnimCardStyle Name="Missing" CloneFrom="Absent"/><AnimCardStyle Name="Loop" CloneFrom="Loop"/></AnimCache>"#;
    /// Independently computed SHA-256 of the exact styles fixture bytes.
    const STYLES_DIGEST: &str = "Q0OBnBtTX2WZ+7AQD38OvwnyuLYWoQg/Rl4lFGNu1ac=";
    /// Complete small image_names source used by the captured manifest.
    const IMAGE_NAMES: &str = r#"<DigitalObjectImageNamesData><CardSetNameReplacement Original="PHYSICAL" Replacement="CODE"/><Match Rule="Equals" MatchPattern="Ticket" ValueFormat="MTG{0}_ticket.png"/><Match Rule="Contains" MatchPattern="Unused" ValueFormat=""/></DigitalObjectImageNamesData>"#;
    /// Independently computed SHA-256 of the exact image_names fixture bytes.
    const IMAGE_NAMES_DIGEST: &str = "xlQRhmmSolgdLbJypSWZN89rKDh4A689JmCskp8aRAE=";

    /// Process-local suffixes keep test output directories independent.
    static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(0);

    /// A test-owned output directory removed after success or failure.
    struct Capture {
        /// Directory holding the generated release assets.
        path: PathBuf,
    }

    impl Capture {
        /// Reserve a fresh output directory without touching existing files.
        fn new() -> Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "mtgoxml-auxiliary-{}-{}",
                std::process::id(),
                NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)?;
            Ok(Self { path })
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Build a captured application with declared sizes and digests for both source files.
    fn application() -> Result<Application> {
        let manifest = format!(
            r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" xmlns:v2="urn:schemas-microsoft-com:asm.v2" xmlns:ds="http://www.w3.org/2000/09/xmldsig#" manifestVersion="1.0">
          <assemblyIdentity name="MTGO.exe" version="1.2.3.4"/><v2:application/>
          <v2:file name="CardStyle.xml" size="{style_size}"><v2:hash><ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/><ds:DigestValue>{STYLES_DIGEST}</ds:DigestValue></v2:hash></v2:file>
          <v2:file name="DigitalObjectImageNames.xml" size="{name_size}"><v2:hash><ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/><ds:DigestValue>{IMAGE_NAMES_DIGEST}</ds:DigestValue></v2:hash></v2:file>
        </assembly>"#,
            style_size = STYLES.len(),
            name_size = IMAGE_NAMES.len()
        );
        Ok(mtgoxml::Deployment::from_bytes(
            "https://patch.example/MTGO.application",
            include_bytes!("../../../fixtures/synthetic/unmapped.application"),
        )?
        .with_application_bytes(manifest.as_bytes())?)
    }

    /// Build out-of-order source objects with both true and explicitly zero store markers.
    fn database(application: &Application) -> Result<CardDatabase> {
        let mut database = application.card_database();
        database.insert(application, CardDataFile::parse(r#"<CardSet id="PHYSICAL">
          <DigitalObject DigitalObjectCatalogID="DOC_9"><CARDNAME_STRING value="Ticket"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/></DigitalObject>
          <DigitalObject DigitalObjectCatalogID="DOC_5"><CARDNAME_STRING value="Ticket"/><IS_DIGITALOBJECT value="0"/><CARDSETNAME_STRING value="ALIAS"/></DigitalObject>
          <DigitalObject DigitalObjectCatalogID="DOC_3"><CARDNAME_STRING value="Unused Ticket"/><IS_DIGITALOBJECT/></DigitalObject>
          <DigitalObject DigitalObjectCatalogID="DOC_7"><CARDNAME_STRING value="Unknown"/><IS_DIGITALOBJECT/></DigitalObject>
          <DigitalObject DigitalObjectCatalogID="DOC_2"><CLONE_ID value="1"/></DigitalObject>
          <DigitalObject DigitalObjectCatalogID="DOC_1"><CARDNAME_STRING value="Ticket"/><IS_DIGITALOBJECT/></DigitalObject>
        </CardSet>"#)?)?;
        Ok(database)
    }

    /// Supply exact captured bytes while preserving a redirected source response URL.
    fn source(resource: &Resource) -> Result<(Url, Vec<u8>)> {
        let bytes = match resource.path.as_str() {
            "CardStyle.xml" => STYLES.as_bytes(),
            "DigitalObjectImageNames.xml" => IMAGE_NAMES.as_bytes(),
            _ => return Err("unexpected synthetic source request".into()),
        };
        Ok((
            Url::parse(&format!("https://cdn.example/captured/{}", resource.path))?,
            bytes.to_vec(),
        ))
    }

    /// Decode a generated gzip into JSON Lines while checking its line boundaries.
    fn rows(path: &Path) -> Result<Vec<Value>> {
        let mut text = String::new();
        GzDecoder::new(File::open(path)?).read_to_string(&mut text)?;
        assert!(text.is_empty() || text.ends_with('\n'));
        assert!(!text.contains('\r'));
        text.lines()
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }

    /// Publish every style with typed unresolved rows, and only native store-pipeline filenames.
    #[test]
    fn exports_resolved_styles_and_presence_selected_image_names() -> Result<()> {
        let capture = Capture::new()?;
        let application = application()?;
        let database = database(&application)?;
        let metadata = export(&capture.path, &application, &database, source)?;
        let styles = rows(&capture.path.join(CHECKSUM_ASSETS[0]))?;
        let images = rows(&capture.path.join(CHECKSUM_ASSETS[1]))?;
        assert_eq!(
            styles
                .iter()
                .map(|row| row["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["Base", "Leaf", "Loop", "Missing"]
        );
        assert_eq!(styles[1]["status"], "resolved");
        assert_eq!(styles[1]["properties"]["frame"], "none");
        assert_eq!(styles[1]["properties"]["model"], "base model");
        assert_eq!(styles[2]["status"], "unresolved");
        assert_eq!(styles[2]["declaration"]["clone_from"], "Loop");
        assert_eq!(
            styles[2]["error"],
            json!({"kind":"reference_cycle","chain":["Loop","Loop"]})
        );
        assert_eq!(
            styles[3]["error"],
            json!({"kind":"missing_reference","target_kind":"style","key":"Absent"})
        );
        assert_eq!(
            images,
            vec![
                json!({"id":1,"image_name":"MTGCODE_ticket.png"}),
                json!({"id":2,"image_name":"MTGCODE_ticket.png"}),
                json!({"id":3,"image_name":""}),
                json!({"id":5,"image_name":"MTGCODE_ticket.png"}),
                json!({"id":7,"image_name":null})
            ]
        );
        assert_eq!(metadata.schema_version, 1);
        assert_eq!(metadata.card_styles.record_count, styles.len());
        assert_eq!(metadata.image_names.record_count, images.len());
        assert_eq!(
            metadata.unresolved_style_count,
            styles
                .iter()
                .filter(|row| row["status"] == "unresolved")
                .count()
        );
        assert_eq!(
            metadata.card_styles.source_url,
            "https://cdn.example/captured/CardStyle.xml"
        );
        assert_eq!(
            metadata.image_names.source_sha256,
            digest_bytes(IMAGE_NAMES.as_bytes())?
        );
        assert_eq!(
            fs::read(capture.path.join("CardStyle.xml"))?,
            STYLES.as_bytes()
        );
        assert_eq!(
            fs::read(capture.path.join("DigitalObjectImageNames.xml"))?,
            IMAGE_NAMES.as_bytes()
        );
        Ok(())
    }

    /// Identical source and catalog snapshots produce byte-identical gzip assets.
    #[test]
    fn compressed_outputs_are_deterministic() -> Result<()> {
        let first = Capture::new()?;
        let second = Capture::new()?;
        let application = application()?;
        let database = database(&application)?;
        export(&first.path, &application, &database, source)?;
        export(&second.path, &application, &database, source)?;
        for name in CHECKSUM_ASSETS {
            // Raw XML and compressed projections both remain reproducible.
            let bytes = fs::read(first.path.join(name))?;
            assert_eq!(bytes, fs::read(second.path.join(name))?);
            if name.ends_with(".gz") {
                assert_eq!(&bytes[4..8], &[0, 0, 0, 0]);
                assert_eq!(bytes[9], 255);
            }
        }
        Ok(())
    }

    /// Stale fixed temporary paths survive a retry without blocking the new export.
    #[test]
    fn retries_ignore_stale_temporary_outputs() -> Result<()> {
        let capture = Capture::new()?;
        let application = application()?;
        let database = database(&application)?;
        for name in CHECKSUM_ASSETS {
            fs::write(
                capture.path.join(format!("{name}.tmp")),
                b"previous pending bytes",
            )?;
        }
        export(&capture.path, &application, &database, source)?;
        for name in CHECKSUM_ASSETS {
            assert!(capture.path.join(name).is_file());
            assert_eq!(
                fs::read(capture.path.join(format!("{name}.tmp")))?,
                b"previous pending bytes"
            );
        }
        let remaining: BTreeSet<_> = fs::read_dir(&capture.path)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<_>>()?;
        let expected: BTreeSet<_> = CHECKSUM_ASSETS
            .into_iter()
            .flat_map(|name| [name.to_owned(), format!("{name}.tmp")])
            .collect();
        assert_eq!(remaining, expected);
        Ok(())
    }

    /// Reject altered source bytes before XML parsing or completed output publication.
    #[test]
    fn authenticates_both_sources_before_publishing() -> Result<()> {
        let application = application()?;
        let database = database(&application)?;
        for target in ["CardStyle.xml", "DigitalObjectImageNames.xml"] {
            for truncate in [false, true] {
                // Equal-size corruption tests digest checking; truncation tests declared byte size.
                let capture = Capture::new()?;
                let result = export(&capture.path, &application, &database, |resource| {
                    let (url, mut bytes) = source(resource)?;
                    if resource.path == target {
                        if truncate {
                            bytes.pop();
                        } else {
                            bytes[0] = b'!';
                        }
                    }
                    Ok((url, bytes))
                });
                let error = result.unwrap_err().to_string();
                assert!(
                    error.contains(if truncate {
                        "size mismatch"
                    } else {
                        "digest mismatch"
                    }),
                    "{error}"
                );
                assert!(fs::read_dir(&capture.path)?.next().is_none());
            }
        }
        Ok(())
    }

    /// An unrepresentable store ID fails the projection and cleans partial temporary output.
    #[test]
    fn projection_failure_preserves_existing_completed_outputs() -> Result<()> {
        let capture = Capture::new()?;
        let application = application()?;
        let mut database = application.card_database();
        database.insert(&application, CardDataFile::parse(r#"<CardSet id="TEST"><DigitalObject DigitalObjectCatalogID="DOC_2147483648"><IS_DIGITALOBJECT/><CARDNAME_STRING value="Ticket"/></DigitalObject></CardSet>"#)?)?;
        for name in CHECKSUM_ASSETS {
            fs::write(capture.path.join(name), b"previous completed bytes")?;
        }
        let unrelated_temporary = "card-styles.jsonl.gz.tmp";
        fs::write(
            capture.path.join(unrelated_temporary),
            b"another pending output",
        )?;
        assert!(export(&capture.path, &application, &database, source).is_err());
        for name in CHECKSUM_ASSETS {
            assert_eq!(
                fs::read(capture.path.join(name))?,
                b"previous completed bytes"
            );
        }
        assert_eq!(
            fs::read(capture.path.join(unrelated_temporary))?,
            b"another pending output"
        );
        let remaining: BTreeSet<_> = fs::read_dir(&capture.path)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<_>>()?;
        let expected: BTreeSet<_> = CHECKSUM_ASSETS
            .into_iter()
            .map(str::to_owned)
            .chain(std::iter::once(unrelated_temporary.to_owned()))
            .collect();
        assert_eq!(remaining, expected);
        Ok(())
    }

    /// Ordinary cards do not enter the store projection even when their IDs exceed its range.
    #[test]
    fn excludes_non_store_objects_before_filename_generation() -> Result<()> {
        let capture = Capture::new()?;
        let application = application()?;
        let mut database = application.card_database();
        database.insert(&application, CardDataFile::parse(r#"<CardSet id="TEST"><DigitalObject DigitalObjectCatalogID="DOC_2147483648"><CARDNAME_STRING value="Ticket"/></DigitalObject></CardSet>"#)?)?;
        let metadata = export(&capture.path, &application, &database, source)?;
        assert!(rows(&capture.path.join(CHECKSUM_ASSETS[1]))?.is_empty());
        assert_eq!(metadata.image_names.record_count, 0);
        Ok(())
    }
}
