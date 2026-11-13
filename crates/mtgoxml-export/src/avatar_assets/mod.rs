//! Capture manifest-selected avatar sources and generate verified lossless WebPs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use animation::{Catalog, Effect};
use flate2::{Compression, GzBuilder, read::MultiGzDecoder};
use mtgoxml::avatar::Avatar;
use mtgoxml::{Application, ArtworkResolver, CardDatabase, FriendlyObject, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use texture::Texture;
use ureq::ResponseExt;

use super::{AnimatedAvatarsMetadata, Result, Snapshot, digest_bytes, digest_file, verify_bytes};

/// Structured managed metadata, bindings, and preview animation extraction.
mod animation;
/// Verified ILSpy installation and captured assembly decompilation.
mod ilspy;
/// Native headless rendering of the captured client pixel shaders.
mod render;
/// Runtime translation of captured bytecode into the renderer's GLSL.
mod shaders;
/// Publisher texture capture, decoding, and sampler locations.
mod texture;
/// Lossless release encoding and smaller website previews with playback verification.
pub(crate) mod webp;

/// Version of structured effect extraction and the deterministic preview policy.
const RENDERER_VERSION: &str = "mtgoxml-avatar-webp-v2";
/// Maximum downloaded assembly, image, or extractor package.
const MAX_RESOURCE_BYTES: u64 = 32 * 1024 * 1024;
/// Maximum decompressed record and complete source catalog sizes.
const MAX_RECORD_BYTES: u64 = 1024 * 1024;
/// Bounded complete catalog size, independent of compressed byte count.
const MAX_CATALOG_BYTES: u64 = 1024 * 1024 * 1024;
/// A changed or corrupt catalog cannot create unbounded network fan-out.
const MAX_AVATARS: usize = 4096;
/// Maximum extracted or captured files included in one preserved bundle.
const MAX_BUNDLE_FILES: usize = 16384;
/// Assemblies retaining shader programs, dispatch, configuration and resource code.
const ASSEMBLIES: [&str; 6] = [
    "ShaderEffects.dll",
    "Core.dll",
    "WotC.MtGO.Client.Model.Core.dll",
    "WotC.MtGO.Client.Model.Data.dll",
    "Mtgo.Shared.Parser.Attributes.dll",
    "SharedResources.dll",
];
/// Preview output dimensions; full captured source textures retain their own sizes.
const WIDTH: u32 = 256;
/// Regular avatar preview height for the configured output width.
const HEIGHT: u32 = 210;
/// Exact integer-millisecond frame sampling interval.
const FRAME_MS: u32 = 100;
/// Exclusive staging suffixes within one process.
static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

/// One effective source definition and every known or candidate avatar texture path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Definition {
    /// Positive digital-object catalog identifier.
    id: u64,
    /// Exact inherited name from the exported dataset.
    name: String,
    /// Positive artwork identifier, or zero for catalog-ID filename fallback.
    art_id: u64,
    /// Original client effect discriminator.
    premium_effect: u32,
    /// Original effective MTGO properties retained for later interpretation.
    properties: Value,
    /// Paths inside the archive, preserving regular and mini source files.
    assets: Vec<String>,
}

/// Operation-owned directory removed on success and every failure path.
struct Staging {
    /// Fresh child of the snapshot directory; never an existing caller directory.
    path: PathBuf,
}

impl Staging {
    /// Reserve a unique sibling of the eventual release assets.
    fn new(directory: &Path) -> Result<Self> {
        for _ in 0..64 {
            let path = directory.join(format!(
                ".avatars-{}-{}",
                std::process::id(),
                NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err("could not reserve avatar staging directory".into())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Build avatar definitions directly from resolved objects during a normal export.
pub(super) fn collect_definitions(database: &CardDatabase) -> Result<Vec<Definition>> {
    let mut avatars = Vec::new();
    for object in database.objects() {
        // Read MTGO fields through the library before allocating export-specific metadata.
        let object = object?;
        if let Some(avatar) = Avatar::animated_from_object(&object)? {
            avatars.push(definition(avatar, object.friendly()?)?);
        }
        if avatars.len() > MAX_AVATARS {
            return Err("avatar definition limit exceeded".into());
        }
    }
    Ok(avatars)
}

/// Adapt a saved friendly dataset to the same avatar metadata used by a normal export.
pub(super) fn definitions(directory: &Path, snapshot: &Snapshot) -> Result<Vec<Definition>> {
    let mut reader = BufReader::new(MultiGzDecoder::new(File::open(
        directory.join("card-data.jsonl.gz"),
    )?));
    let mut avatars = Vec::new();
    let mut seen = BTreeSet::new();
    let mut count = 0;
    let mut decoded = 0;
    loop {
        // Bound the saved export independently of the original XML source database.
        let mut line = Vec::new();
        let length = reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if length == 0 {
            break;
        }
        decoded += length as u64;
        if length as u64 > MAX_RECORD_BYTES || decoded > MAX_CATALOG_BYTES {
            return Err("avatar source catalog exceeds its size limits".into());
        }
        count += 1;
        let object: FriendlyObject = serde_json::from_slice(&line)?;
        if object.id == 0 || !seen.insert(object.id) {
            return Err("invalid or duplicate source catalog ID".into());
        }
        if let Some(avatar) = Avatar::animated_from_friendly(&object)? {
            avatars.push(definition(avatar, object)?);
        }
        if avatars.len() > MAX_AVATARS {
            return Err("avatar definition limit exceeded".into());
        }
    }
    if snapshot.object_count != Some(count) {
        return Err("avatar source object count mismatch".into());
    }
    avatars.sort_by_key(|avatar| avatar.id);
    Ok(avatars)
}

/// Preserve export-specific names, original fields, and archive paths around library metadata.
fn definition(avatar: Avatar, object: FriendlyObject) -> Result<Definition> {
    Ok(Definition {
        id: avatar.id(),
        name: object.name.ok_or("missing avatar name")?,
        art_id: avatar.art_id(),
        premium_effect: avatar.premium_effect(),
        properties: serde_json::to_value(&object.mtgo)?,
        assets: avatar
            .asset_paths()
            .into_iter()
            .map(|path| format!("textures/{path}"))
            .collect(),
    })
}

/// Restrict remote locations and upgrade the publisher's HTTP graphics endpoint.
fn transport_url(url: &Url) -> Result<Url> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid avatar source URL".into());
    }
    let mut url = url.clone();
    if url.scheme() == "http" && url.host_str() == Some("mtgo.patch.daybreakgames.com") {
        url.set_scheme("https")
            .map_err(|()| "could not upgrade publisher URL")?;
    }
    Ok(url)
}

/// Fetch bounded bytes and retain the actual observed source metadata.
fn download(agent: &ureq::Agent, url: &Url) -> Result<(Vec<u8>, Value)> {
    let mut response = agent.get(transport_url(url)?.as_str()).call()?;
    let final_url = Url::parse(&response.get_uri().to_string())?;
    transport_url(&final_url)?;
    let mut metadata = json!({"requested_url":url.as_str(),"response_url":final_url.as_str(),"http_status":response.status().as_u16(),"captured_at_unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),"content_type":response.headers().get("content-type").and_then(|v|v.to_str().ok()),"etag":response.headers().get("etag").and_then(|v|v.to_str().ok()),"last_modified":response.headers().get("last-modified").and_then(|v|v.to_str().ok())});
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_RESOURCE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RESOURCE_BYTES {
        return Err("avatar source exceeds capture byte limit".into());
    }
    metadata["size"] = json!(bytes.len());
    metadata["sha256"] = json!(digest_bytes(&bytes)?);
    Ok((bytes, metadata))
}

/// Preserve original manifest-bound assemblies and their source-like decompilation.
fn decompile_sources(
    directory: &Path,
    bundle: &Path,
    application: &Application,
    agent: &ureq::Agent,
    tool: &ilspy::Extractor,
) -> Result<Vec<Value>> {
    let original = bundle.join("original");
    fs::create_dir(&original)?;
    for name in ["MTGO.application", "MTGO.exe.manifest", "MTGO.exe.config"] {
        fs::copy(directory.join(name), original.join(name))?;
    }
    let mut sources = Vec::new();
    for name in ASSEMBLIES {
        // Authenticate each assembly against this export's application manifest.
        let resource = application
            .resource(name)
            .ok_or_else(|| format!("missing avatar source assembly: {name}"))?;
        let (bytes, mut source) = download(agent, &resource.url)?;
        verify_bytes(name, &bytes, resource.size, &resource.hashes)?;
        fs::write(original.join(name), bytes)?;
        source["path"] = json!(format!("original/{name}"));
        source["manifest_verified"] = json!(true);
        sources.push(source);
    }
    for name in [
        "ShaderEffects.dll",
        "Core.dll",
        "WotC.MtGO.Client.Model.Core.dll",
    ] {
        tool.decompile(
            &original.join(name),
            &original,
            &bundle
                .join("decompiled")
                .join(name.trim_end_matches(".dll")),
        )?;
    }
    Ok(sources)
}

/// Render a complete deterministic timeline and verify the native codec's output.
fn render_avatar(
    bundle: &Path,
    avatar: &Definition,
    effect: &Effect,
    context: &mut render::Renderer,
    textures: &BTreeMap<u32, Texture>,
    shader: &str,
) -> Result<Value> {
    let inputs = textures
        .iter()
        .map(|(unit, texture)| Ok((*unit, texture.decode()?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let frames = context.sample_frames(
        &effect.bindings.shader_resource,
        &inputs,
        shader,
        &effect.animation,
        FRAME_MS,
    )?;
    let path = format!("webp/{}.webp", avatar.id);
    let encoded = webp::write(&bundle.join(&path), WIDTH, HEIGHT, &frames)?;
    let mut row = serde_json::to_value(encoded)?;
    row["id"] = json!(avatar.id);
    row["name"] = json!(avatar.name);
    row["premium_effect"] = json!(avatar.premium_effect);
    row["path"] = json!(path);
    row["status"] = json!("rendered");
    row["timing"] = json!(effect.animation.timing());
    row["effect_class"] = json!(effect.bindings.class);
    let inputs = textures
        .values()
        .map(|texture| {
            Ok((
                texture.archive_path().to_owned(),
                json!(digest_file(texture.path())?),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    row["input_assets"] = json!(inputs);
    Ok(row)
}

/// Write a canonical JSON file for the preserved source bundle.
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = File::create(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    writeln!(file)?;
    Ok(())
}

/// Recursively list bounded ordinary files while refusing symlinks in the archive.
fn files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![directory.to_owned()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            // Preserve only ordinary bounded files under the operation-owned bundle.
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err("avatar bundle contains a symlink".into());
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
            if files.len() + pending.len() > MAX_BUNDLE_FILES {
                return Err("avatar archive file limit exceeded".into());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Package ordinary files in stable order, retaining source bytes and recorded timestamps.
fn write_archive(destination: &Path, bundle: &Path) -> Result<()> {
    let gzip = GzBuilder::new()
        .mtime(0)
        .write(File::create(destination)?, Compression::new(6));
    let mut archive = tar::Builder::new(gzip);
    for path in files(bundle)? {
        // Normalize archive metadata without changing the captured file contents.
        let mut header = tar::Header::new_gnu();
        header.set_size(path.metadata()?.len());
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(&mut header, path.strip_prefix(bundle)?, File::open(&path)?)?;
    }
    archive.into_inner()?.finish()?.flush()?;
    Ok(())
}

/// Generate both avatar sidecars from this export's captured dataset and manifests.
pub(super) fn generate(
    directory: &Path,
    application: &Application,
    snapshot: &Snapshot,
    avatars: &[Definition],
    artwork: &ArtworkResolver,
) -> Result<AnimatedAvatarsMetadata> {
    let staging = Staging::new(directory)?;
    let bundle = staging.path.join("bundle");
    fs::create_dir(&bundle)?;
    fs::create_dir(bundle.join("webp"))?;
    write_json(&bundle.join("avatars.json"), &avatars)?;
    let root = artwork.art_root_url();
    let agent = super::agent();
    let extractor = ilspy::Config::load()?;
    let mut sources = Vec::new();
    let mut assets = Vec::new();
    let mut catalog = Catalog::default();
    if !avatars.is_empty() {
        // Decompile resources selected and verified against this run's manifest.
        let tool = extractor.install(&staging.path, &agent)?;
        sources = decompile_sources(directory, &bundle, application, &agent, &tool)?;
        assets = texture::capture_textures(&bundle, avatars, root)?;
        catalog = Catalog::load(&bundle, avatars.iter().map(|avatar| avatar.premium_effect))?;
    }
    write_json(&bundle.join("sources.json"), &sources)?;
    write_json(&bundle.join("assets.json"), &assets)?;
    let mut context = None;
    let mut compiler = None;
    let mut programs = BTreeMap::new();
    let mut translations = Vec::new();
    let mut rows = Vec::new();
    let mut counts = [0_usize; 3];
    for avatar in avatars {
        // Each source definition contributes exactly one explicit completion status.
        let mut row =
            json!({"id":avatar.id,"name":avatar.name,"premium_effect":avatar.premium_effect});
        match catalog.get(avatar.premium_effect) {
            Err(reason) => {
                // Keep unsupported source definitions and the extraction diagnostic in the index.
                row["status"] = json!("unsupported");
                row["reason"] = json!("unsupported_effect_behavior");
                row["details"] = json!([reason]);
                counts[2] += 1;
            }
            Ok(effect) => {
                // Resolve required images before translating or allocating a render context.
                let textures = texture::samplers(&bundle, avatar, &effect.bindings.samplers)?;
                let missing: Vec<_> = textures
                    .values()
                    .filter(|texture| !texture.path().is_file())
                    .map(|texture| texture.archive_path().to_owned())
                    .collect();
                if !missing.is_empty() {
                    // Record every absent required image without claiming a completed preview.
                    row["status"] = json!("missing");
                    row["reason"] = json!("missing_regular_textures");
                    row["assets"] = json!(missing);
                    counts[1] += 1;
                } else {
                    // Translate the captured program and encode its extracted animation.
                    let resource = &effect.bindings.shader_resource;
                    if !programs.contains_key(resource) {
                        // Share one translation between avatars using the same shader resource.
                        if compiler.is_none() {
                            compiler = Some(shaders::Compiler::new()?);
                        }
                        let (source, provenance) = compiler
                            .as_ref()
                            .ok_or("missing shader compiler")?
                            .translate(&bundle, resource, &snapshot.source.application_version)?;
                        programs.insert(resource.clone(), source);
                        translations.push(provenance);
                    }
                    if context.is_none() {
                        context = Some(render::Renderer::new(WIDTH, HEIGHT)?);
                    }
                    row = render_avatar(
                        &bundle,
                        avatar,
                        effect,
                        context.as_mut().ok_or("missing render context")?,
                        &textures,
                        &programs[resource],
                    )?;
                    counts[0] += 1;
                }
            }
        }
        println!("Avatar {}: {}", avatar.id, row["status"]);
        rows.push(row);
    }
    let index = staging.path.join(super::ANIMATED_AVATAR_ASSETS[0]);
    write_json(&index, &rows)?;
    fs::copy(&index, bundle.join(super::ANIMATED_AVATAR_ASSETS[0]))?;
    let driver = context
        .as_ref()
        .map(render::Renderer::description)
        .transpose()?;
    write_json(
        &bundle.join("bundle.json"),
        &json!({"schema_version":1,"application_version":snapshot.source.application_version,"application_manifest_url":snapshot.source.application_manifest_url,"application_manifest_sha256":snapshot.source.application_manifest_sha256,"catalog_sha256":digest_file(&directory.join("card-data.jsonl.gz"))?,"renderer_version":RENDERER_VERSION,"effect_extraction":catalog.provenance(),"driver":driver,"dimensions":[WIDTH,HEIGHT],"sample_fps":1000/FRAME_MS,"encoding":{"format":"animated_webp","lossless":true,"quality":75,"method":4,"minimize_size":false,"loop_count":0},"texture_count":assets.iter().filter(|asset|asset["status"]=="captured").count(),"missing_texture_count":assets.iter().filter(|asset|asset["status"]=="missing").count(),"extractor":extractor.provenance(),"shader_translation":{"tools":compiler.as_ref().map(shaders::Compiler::provenance),"shaders":translations},"notes":["CDN graphics URLs are mutable; recorded bytes, timestamps and hashes identify the observed capture.","Native rendering is a derived preview; exact Windows MTGO equivalence has not been verified.","Unrecognized client binding or animation behavior is preserved with an extraction diagnostic."]}),
    )?;
    let mut inventory = String::new();
    for path in files(&bundle)? {
        use std::fmt::Write as _;
        writeln!(
            inventory,
            "{}  {}",
            digest_file(&path)?,
            path.strip_prefix(&bundle)?.to_string_lossy()
        )?;
    }
    fs::write(bundle.join("SHA256SUMS"), inventory)?;
    let archive = staging.path.join(super::ANIMATED_AVATAR_ASSETS[1]);
    write_archive(&archive, &bundle)?;
    let metadata = AnimatedAvatarsMetadata {
        schema_version: 1,
        definition_count: avatars.len(),
        rendered_count: counts[0],
        missing_count: counts[1],
        unsupported_count: counts[2],
        index_sha256: digest_file(&index)?,
        archive_sha256: digest_file(&archive)?,
        renderer_version: RENDERER_VERSION.to_owned(),
    };
    for name in super::ANIMATED_AVATAR_ASSETS {
        fs::rename(staging.path.join(name), directory.join(name))?;
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    use std::net::TcpListener;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    /// Create a completed synthetic snapshot without depending on a client version.
    fn synthetic_snapshot(count: usize) -> Result<Snapshot> {
        Ok(serde_json::from_value(json!({
            "schema_version": 3,
            "deployment_url": "https://publisher.test/MTGO.application",
            "application_manifest_url": "https://publisher.test/future/MTGO.exe.manifest",
            "application_version": "99.1.2.3",
            "application_manifest_sha256": "00".repeat(32),
            "tag": "mtgo-data-99.1.2.3",
            "object_count": count
        }))?)
    }

    /// Save the actual compressed JSONL input consumed by definition discovery.
    fn synthetic_catalog(directory: &Path, rows: &[Value]) -> Result<()> {
        let mut gzip = GzBuilder::new().mtime(0).write(
            File::create(directory.join("card-data.jsonl.gz"))?,
            Compression::fast(),
        );
        for row in rows {
            let mut row = row.clone();
            let fields = row.as_object_mut().ok_or("test row must be an object")?;
            fields.entry("set").or_insert(json!("TEST"));
            fields.entry("foil").or_insert(json!(false));
            serde_json::to_writer(&mut gzip, &row)?;
            writeln!(gzip)?;
        }
        gzip.finish()?.flush()?;
        Ok(())
    }

    /// Construct client-relative texture paths through the public avatar metadata API.
    fn texture_paths(id: u64, art_id: u64, effect: u32) -> Vec<String> {
        let object: FriendlyObject = serde_json::from_value(json!({
            "id":id, "name":"Test avatar", "set":"TEST", "foil":false,
            "art_id":art_id, "mtgo":{"IS_VANGUARD":true,"PREMIUM_EFFECT":effect.to_string()}
        }))
        .unwrap();
        Avatar::from_friendly(&object)
            .unwrap()
            .unwrap()
            .asset_paths()
            .into_iter()
            .map(|path| format!("textures/{path}"))
            .collect()
    }

    /// Serve exactly one bounded loopback request and retain its request line.
    fn local_response(
        status: u16,
        body: Vec<u8>,
    ) -> Result<(Url, JoinHandle<std::io::Result<String>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error),
                }
            };
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                if stream.read(&mut byte)? == 0 || request.len() > 8192 {
                    return Err(std::io::Error::other("invalid mock request"));
                }
                request.push(byte[0]);
            }
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )?;
            stream.write_all(&body)?;
            Ok(String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned())
        });
        Ok((Url::parse(&format!("http://{address}/Graphics/"))?, server))
    }

    /// Discover only effect-bearing avatars from this run, retaining unknown codes.
    #[test]
    fn definitions_select_this_catalog_and_preserve_fallback_and_unknown_effects() -> Result<()> {
        let stage = Staging::new(&std::env::temp_dir())?;
        let rows = vec![
            json!({"id": 1, "name": "Ordinary card", "mtgo": {"PREMIUM_EFFECT": "24"}}),
            json!({"id": 18, "name": "Static Phage", "art_id": 125770, "mtgo": {"IS_VANGUARD": "1"}}),
            json!({"id": 2, "name": "Explicitly static", "mtgo": {"IS_VANGUARD": true, "PREMIUM_EFFECT": "0"}}),
            json!({"id": 156681, "name": "Fallback avatar", "art_id": 0, "mtgo": {"IS_VANGUARD": true, "PREMIUM_EFFECT": "1", "UNKNOWN_PROPERTY": "preserve"}}),
            json!({"id": 16701, "name": "Future effect", "art_id": 17, "mtgo": {"IS_VANGUARD": "1", "PREMIUM_EFFECT": "999"}}),
            json!({"id": 7, "name": "Double texture avatar", "art_id": 123, "mtgo": {"IS_VANGUARD": "2", "PREMIUM_EFFECT": "24"}}),
        ];
        synthetic_catalog(&stage.path, &rows)?;
        let avatars = definitions(&stage.path, &synthetic_snapshot(rows.len())?)?;
        assert_eq!(
            avatars.iter().map(|avatar| avatar.id).collect::<Vec<_>>(),
            [7, 16701, 156681]
        );
        assert_eq!(avatars[0].assets.len(), 6);
        assert_eq!(avatars[1].premium_effect, 999);
        assert_eq!(avatars[1].assets.len(), 6);
        assert!(avatars[1].assets[0].ends_with("00017_avatar.jpg"));
        assert!(avatars[2].assets[0].ends_with("156681c_avatar.jpg"));
        assert_eq!(avatars[2].properties["UNKNOWN_PROPERTY"], "preserve");
        assert!(definitions(&stage.path, &synthetic_snapshot(rows.len() + 1)?).is_err());
        Ok(())
    }

    /// Normal exports and legacy attachment use identical typed avatar interpretation.
    #[test]
    fn resolved_and_saved_definitions_are_identical() -> Result<()> {
        let application = mtgoxml::Deployment::from_bytes(
            "https://patch.example/client/MTGO.application",
            include_bytes!("../../../../fixtures/synthetic/unmapped.application"),
        )?
        .with_application_bytes(include_bytes!(
            "../../../../fixtures/synthetic/resources.exe.manifest"
        ))?;
        let mut database = application.card_database();
        database.insert(&application, mtgoxml::carddata::CardDataFile::parse(r#"<CardSet id="TEST">
          <DigitalObject DigitalObjectCatalogID="DOC_10"><CARDNAME_STRING value="Avatar"/><IS_VANGUARD value="2"/><PREMIUM_EFFECT value="24"/><ARTID value="0"/><UNKNOWN><FLAG/><TEXT value="001"/></UNKNOWN></DigitalObject>
          <DigitalObject DigitalObjectCatalogID="DOC_11"><CARDNAME_STRING value="Ordinary card"/><PREMIUM_EFFECT value="24"/></DigitalObject>
          <DigitalObject DigitalObjectCatalogID="DOC_12"><CARDNAME_STRING value="Static avatar"/><IS_VANGUARD value="2"/><ARTID value="invalid"/></DigitalObject>
        </CardSet>"#)?)?;
        let resolved = collect_definitions(&database)?;
        let rows = database
            .objects()
            .map(|object| Ok(serde_json::to_value(object?.friendly()?)?))
            .collect::<Result<Vec<_>>>()?;
        let stage = Staging::new(&std::env::temp_dir())?;
        synthetic_catalog(&stage.path, &rows)?;
        let saved = definitions(&stage.path, &synthetic_snapshot(database.len())?)?;
        assert_eq!(
            serde_json::to_value(resolved)?,
            serde_json::to_value(saved)?
        );
        Ok(())
    }

    /// Invalid effective artwork data cannot silently select a different image.
    #[test]
    fn malformed_artwork_and_duplicate_catalog_ids_are_rejected() -> Result<()> {
        let stage = Staging::new(&std::env::temp_dir())?;
        let malformed = json!({"id": 3, "name": "Malformed", "mtgo": {"IS_VANGUARD": "1", "PREMIUM_EFFECT": "1", "ARTID": "not an integer"}});
        synthetic_catalog(&stage.path, &[malformed])?;
        assert!(definitions(&stage.path, &synthetic_snapshot(1)?).is_err());
        let duplicate = json!({"id": 3, "name": "Duplicate", "mtgo": {"IS_VANGUARD": "1", "PREMIUM_EFFECT": "1"}});
        synthetic_catalog(&stage.path, &[duplicate.clone(), duplicate])?;
        assert!(definitions(&stage.path, &synthetic_snapshot(2)?).is_err());
        Ok(())
    }

    /// Only publisher 404/410 become missing rows; failures and HTML abort capture.
    #[test]
    fn texture_capture_distinguishes_missing_http_errors_and_malformed_success() -> Result<()> {
        let stage = Staging::new(&std::env::temp_dir())?;
        let relative = "textures/Avatars/Regular/00012_avatar.jpg";
        for status in [404, 410, 500, 200] {
            let (root, server) = local_response(status, b"<html>not an image</html>".to_vec())?;
            let result = Texture::new(&stage.path, relative).capture(&root, &super::super::agent());
            let request = server.join().map_err(|_| "mock server panicked")??;
            assert_eq!(
                request,
                "GET /Graphics/Avatars/Regular/00012_avatar.jpg HTTP/1.1"
            );
            if matches!(status, 404 | 410) {
                let record = result?;
                assert_eq!(record["status"], "missing");
                assert_eq!(record["http_status"], status);
            } else {
                assert!(
                    result.is_err(),
                    "HTTP {status} must not produce a usable asset"
                );
            }
            assert!(!stage.path.join(relative).exists());
        }
        Ok(())
    }

    /// A valid downloaded PNG is preserved byte-for-byte and decoded as straight RGBA.
    #[test]
    fn texture_capture_preserves_original_png_bytes_and_alpha() -> Result<()> {
        let stage = Staging::new(&std::env::temp_dir())?;
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 240, 120, 20, 32,
        ];
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes).write_image(
            &pixels,
            2,
            2,
            image::ExtendedColorType::Rgba8,
        )?;
        let (root, server) = local_response(200, bytes.clone())?;
        let relative = "textures/Avatars/Regular_Shader/00012_avatar.png";
        let texture = Texture::new(&stage.path, relative);
        let record = texture.capture(&root, &super::super::agent())?;
        server.join().map_err(|_| "mock server panicked")??;
        assert_eq!(record["status"], "captured");
        assert_eq!(record["sha256"], digest_bytes(&bytes)?);
        assert_eq!(fs::read(stage.path.join(relative))?, bytes);
        assert_eq!(texture.decode()?.as_raw(), &pixels);
        Ok(())
    }

    /// Filename fallback must retain the catalog marker in all six asset roles.
    #[test]
    fn catalog_fallback_and_shader_packages() {
        let single = texture_paths(156681, 0, 1);
        assert_eq!(single.len(), 4);
        assert!(single[0].ends_with("156681c_avatar.jpg"));
        let double = texture_paths(18, 125770, 4);
        assert_eq!(double.len(), 6);
        assert!(double[4].ends_with("125770_add_avatar.png"));
        assert_eq!(texture_paths(1, 12, 99).len(), 6);
    }

    /// Malformed source payloads must not be preserved as apparently usable graphics.
    #[test]
    fn invalid_images_and_urls_are_rejected() {
        let stage = Staging::new(&std::env::temp_dir()).unwrap();
        let texture = Texture::new(&stage.path, "invalid.png");
        fs::write(texture.path(), b"<html>error</html>").unwrap();
        assert!(texture.decode().is_err());
        assert!(
            transport_url(&Url::parse("https://name:secret@example.test/art").unwrap()).is_err()
        );
        assert!(transport_url(&Url::parse("file:///etc/passwd").unwrap()).is_err());
        let url =
            transport_url(&Url::parse("http://mtgo.patch.daybreakgames.com/Graphics").unwrap())
                .unwrap();
        assert_eq!(url.scheme(), "https");
    }

    /// The shared configuration parser resolves roots against the actual response URL.
    #[test]
    fn relative_art_root_uses_configuration_source() {
        let artwork = ArtworkResolver::from_config(
            Url::parse("https://assets.example/client/version/MTGO.exe.config").unwrap(),
            br#"<configuration><downloadSource><artRoot value="../../Graphics/?v=1"/><cardArtSubPath value="Cards/Pics"/><avatarArtSubPath value="Custom/Portraits"/></downloadSource></configuration>"#,
        ).unwrap();
        assert_eq!(
            artwork.art_root_url().as_str(),
            "https://assets.example/Graphics/?v=1"
        );
        // The configured primary portrait directory does not alter the client's fixed renderer paths.
        assert_eq!(
            artwork.avatar_base_url().unwrap().path(),
            "/Graphics/Custom/Portraits/"
        );
        assert_eq!(
            texture_paths(10, 20, 1)[0],
            "textures/Avatars/Regular/00020_avatar.jpg"
        );
    }
}
