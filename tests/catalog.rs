//! Offline integration coverage for discovery and URL resolution.

use clickonce::{Dependency, Manifest, ManifestKind};
use mtgoxml::{Application, Deployment, Error, ResourceKind};
use url::Url;

/// A stable test deployment location, independent of the live patch service.
const DEPLOYMENT_URL: &str = "https://patch.example.test/live/MTGO.application";
/// An application manifest containing files and an installed assembly.
const RESOURCES: &[u8] = include_bytes!("../fixtures/synthetic/resources.exe.manifest");
/// A deployment that explicitly enables extension mapping.
const MAPPED: &[u8] = include_bytes!("../fixtures/synthetic/mapped.application");
/// A deployment that explicitly disables extension mapping.
const UNMAPPED: &[u8] = include_bytes!("../fixtures/synthetic/unmapped.application");
/// A deployment that leaves extension mapping at its ClickOnce default.
const DEFAULT: &[u8] = include_bytes!("../fixtures/synthetic/default.application");

/// Resolve the synthetic application under a selected deployment fixture.
fn application(deployment: &[u8]) -> Application {
    Deployment::from_bytes(DEPLOYMENT_URL, deployment)
        .unwrap()
        .with_application_bytes(RESOURCES)
        .unwrap()
}

/// Construct a minimal application while retaining the deployment identity.
fn application_xml(contents: &str) -> String {
    format!(
        r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
          <assemblyIdentity name="MTGO.exe" version="1.2.3.4" />
          <application />{contents}</assembly>"#
    )
}

/// Add arbitrary file and dependency entries to the synthetic deployment.
fn with_contents(contents: &str) -> Result<Application, Error> {
    Deployment::from_bytes(DEPLOYMENT_URL, UNMAPPED)?
        .with_application_bytes(application_xml(contents).as_bytes())
}

/// The captured publisher manifests form a complete, offline resource catalog.
#[test]
fn captured_mtgo_catalog_includes_every_file_and_downloadable_assembly() {
    let deployment = Deployment::from_bytes(
        mtgoxml::LIVE_DEPLOYMENT_URL,
        include_bytes!("../fixtures/live/MTGO.application"),
    )
    .unwrap();
    let application = deployment
        .with_application_bytes(include_bytes!("../fixtures/live/MTGO.exe.manifest"))
        .unwrap();

    // Counts belong to this checked-in capture, not to the mutable live service.
    assert_eq!(application.resources().len(), 1_028);
    assert_eq!(
        application
            .resources()
            .iter()
            .filter(|r| r.kind == ResourceKind::File)
            .count(),
        914
    );
    assert_eq!(
        application
            .resources()
            .iter()
            .filter(|r| r.kind == ResourceKind::Assembly)
            .count(),
        114
    );
    assert_eq!(application.xml_resources().count(), 723);
    assert_eq!(application.card_data_resources().count(), 714);
    assert_eq!(
        application.version(),
        application.manifest().identity.version
    );
    assert!(!application.deployment().map_file_extensions());

    for file in &application.manifest().files {
        let resource = application.resource(&file.name).unwrap();
        assert_eq!(resource.kind, ResourceKind::File);
        assert_eq!(resource.path, file.name.replace('\\', "/"));
        assert_eq!(resource.size, file.size);
        assert_eq!(resource.hashes, file.hashes);
        assert_eq!(resource.group, file.group);
        assert_eq!(resource.optional, file.optional);
        assert!(!resource.url.as_str().contains('\\'));
        assert_eq!(
            resource.url,
            application
                .url()
                .join(&file.name.replace('\\', "/"))
                .unwrap()
        );
    }
    for dependency in &application.manifest().dependencies {
        if let Dependency::Assembly(assembly) = dependency {
            if let Some(codebase) = &assembly.codebase {
                let resource = application.resource(codebase).unwrap();
                assert_eq!(resource.kind, ResourceKind::Assembly);
                assert_eq!(resource.size, assembly.size);
                assert_eq!(resource.hashes, assembly.hashes);
            }
        }
    }
    let card_names = application
        .resource(r"CardDataSource\CARDNAME_STRING.xml")
        .unwrap();
    assert!(card_names.is_xml());
    assert!(card_names.is_card_data());
    assert_eq!(card_names.size, Some(3_051_947));
    assert!(!application.resource("legality.xml").unwrap().is_card_data());
    assert!(application.resource("does-not-exist.xml").is_none());
}

/// The live deployment URL remains the documented entry point.
#[test]
fn live_entry_point_matches_supplied_url() {
    assert_eq!(
        mtgoxml::LIVE_DEPLOYMENT_URL,
        "http://mtgo.patch.daybreakgames.com/patch/mtg/live/client/MTGO.application"
    );
}

/// Windows separators and spaces resolve without host filesystem semantics.
#[test]
fn deployment_resolves_version_directory_and_retains_metadata() {
    let deployment = Deployment::from_bytes(DEPLOYMENT_URL, MAPPED).unwrap();
    assert_eq!(deployment.url().as_str(), DEPLOYMENT_URL);
    assert_eq!(deployment.manifest().kind, ManifestKind::Deployment);
    assert!(deployment.map_file_extensions());
    assert_eq!(
        deployment.application_manifest_url().as_str(),
        "https://patch.example.test/live/Application%20Files/MTGO_1_2_3_4/MTGO.exe.manifest"
    );
}

/// Both absent and explicit false mapping leave payload names unchanged.
#[test]
fn unmapped_payloads_and_default_mapping_use_original_extensions() {
    for fixture in [UNMAPPED, DEFAULT] {
        let application = application(fixture);
        assert!(!application.deployment().map_file_extensions());
        assert_eq!(application.resources().len(), 5);
        assert_eq!(
            application.resource("bin/Card.dll").unwrap().url.as_str(),
            "https://patch.example.test/live/Application%20Files/MTGO_1_2_3_4/bin/Card.dll"
        );
        assert_eq!(application.xml_resources().count(), 3);
        assert_eq!(application.card_data_resources().count(), 0);
        assert!(application.resource("framework.dll").is_none());
    }
}

/// Mapping changes every payload URL while preserving logical resource names.
#[test]
fn mapping_appends_deploy_to_files_and_assemblies_only() {
    let application = application(MAPPED);
    assert!(application.url().as_str().ends_with("MTGO.exe.manifest"));
    for resource in application.resources() {
        assert!(resource.url.path().ends_with(".deploy"));
        assert!(!resource.path.ends_with(".deploy"));
    }
    let resource = application.resource("ExternalData/Cards.xml").unwrap();
    assert!(resource.is_xml());
    assert_eq!(resource.path, "ExternalData/Cards.xml");
    assert!(
        resource
            .url
            .path()
            .ends_with("/ExternalData/Cards.xml.deploy")
    );
}

/// Lookup follows the case-insensitive Windows logical resource namespace.
#[test]
fn lookup_accepts_either_separator_and_case_without_changing_metadata() {
    let application = application(UNMAPPED);
    let resource = application.resource(r"EXTERNALDATA\CARDS.XML").unwrap();
    assert_eq!(resource.path, "ExternalData/Cards.xml");
    assert_eq!(resource.size, Some(123));
    assert_eq!(resource.group.as_deref(), Some("data"));
    assert_eq!(resource.optional, Some(true));
    assert_eq!(resource.hashes.len(), 1);
    let assembly = application.resource(r"BIN\card.DLL").unwrap();
    assert_eq!(assembly.kind, ResourceKind::Assembly);
    assert_eq!(assembly.size, Some(4_294_967_297));
    assert_eq!(assembly.group.as_deref(), Some("core"));
    assert_eq!(assembly.optional, Some(false));
    assert_eq!(assembly.hashes.len(), 1);
    assert_eq!(
        application.resource("Images/card.png").unwrap().size,
        Some(0)
    );
}

/// A file name is literal text rather than a URI with queries or fragments.
#[test]
fn file_names_encode_reserved_characters_and_existing_percent_sequences() {
    let application = application(UNMAPPED);
    let special = application
        .resource("ExternalData/Card Data #1?100%.xml")
        .unwrap();
    assert_eq!(
        special.url.as_str(),
        "https://patch.example.test/live/Application%20Files/MTGO_1_2_3_4/ExternalData/Card%20Data%20%231%3F100%25.xml"
    );
    assert_eq!(special.url.query(), None);
    assert_eq!(special.url.fragment(), None);
    let percent = application
        .resource("ExternalData/already%20encoded.xml")
        .unwrap();
    assert!(percent.url.path().ends_with("/already%2520encoded.xml"));
    assert!(
        application
            .resource("ExternalData/already encoded.xml")
            .is_none()
    );
}

/// Logical file extensions cannot be confused with URL syntax or deploy mapping.
#[test]
fn xml_filter_uses_literal_file_names_and_excludes_deploy_lookalikes() {
    let application = with_contents(
        r#"<file name="CardDataSource/question?.XML" />
        <file name="CardDataSource/archive.xml.deploy" />
        <file name="CardDataSource/notes.txt" />"#,
    )
    .unwrap();
    let names: Vec<_> = application
        .xml_resources()
        .map(|resource| resource.path.as_str())
        .collect();
    assert_eq!(names, ["CardDataSource/question?.XML"]);
    assert_eq!(application.card_data_resources().count(), 3);
}

/// A caller can supply the response URL after an application-manifest redirect.
#[test]
fn redirected_application_uses_actual_response_directory() {
    let deployment = Deployment::from_bytes(DEPLOYMENT_URL, MAPPED).unwrap();
    let response_url = Url::parse("https://cdn.example.test/release/MTGO.exe.manifest").unwrap();
    let manifest = Manifest::from_bytes(RESOURCES).unwrap();
    let application = deployment
        .with_application(response_url.clone(), manifest)
        .unwrap();
    assert_eq!(application.url(), &response_url);
    assert_eq!(application.deployment().url().as_str(), DEPLOYMENT_URL);
    assert_eq!(
        application.resource("bin/Card.dll").unwrap().url.as_str(),
        "https://cdn.example.test/release/bin/Card.dll.deploy"
    );
}

/// Assembly codebases retain URI queries and are mapped before the query.
#[test]
fn assembly_codebases_preserve_http_url_queries() {
    let deployment = Deployment::from_bytes(DEPLOYMENT_URL, MAPPED).unwrap();
    let manifest = application_xml(
        r#"<dependency><dependentAssembly dependencyType="install"
        codebase="https://cdn.example.test/bin/Card.dll?revision=7&amp;channel=live">
        <assemblyIdentity name="Card" /></dependentAssembly></dependency>"#,
    );
    let application = deployment
        .with_application_bytes(manifest.as_bytes())
        .unwrap();
    assert_eq!(application.resources().len(), 1);
    assert_eq!(
        application.resources()[0].url.as_str(),
        "https://cdn.example.test/bin/Card.dll.deploy?revision=7&channel=live"
    );
}

/// Runtime prerequisites and unrecognized dependency relationships are omitted.
#[test]
fn only_install_and_unspecified_dependencies_are_downloadable() {
    let application = with_contents(
        r#"<dependency><dependentAssembly dependencyType="install" codebase="installed.dll">
             <assemblyIdentity name="Installed" /></dependentAssembly></dependency>
           <dependency><dependentAssembly codebase="implicit.dll">
             <assemblyIdentity name="Implicit" /></dependentAssembly></dependency>
           <dependency><dependentAssembly dependencyType="preRequisite" codebase="runtime.dll">
             <assemblyIdentity name="Runtime" /></dependentAssembly></dependency>
           <dependency><dependentAssembly preRequisite="true" dependencyType="install" codebase="legacy-runtime.dll">
             <assemblyIdentity name="LegacyRuntime" /></dependentAssembly></dependency>
           <dependency><dependentAssembly dependencyType="future" codebase="unknown.dll">
             <assemblyIdentity name="Unknown" /></dependentAssembly></dependency>"#,
    )
    .unwrap();
    assert_eq!(application.resources().len(), 2);
    assert!(application.resource("installed.dll").is_some());
    assert!(application.resource("implicit.dll").is_some());
    assert!(application.resource("runtime.dll").is_none());
    assert!(application.resource("unknown.dll").is_none());
    assert!(application.resource("legacy-runtime.dll").is_none());
}

/// Duplicate logical paths cannot silently replace a file or an assembly.
#[test]
fn duplicate_resources_reject_case_and_separator_aliases() {
    for contents in [
        r#"<file name="Data\Cards.xml" /><file name="data/cards.XML" />"#,
        r#"<file name="bin\Card.dll" /><dependency><dependentAssembly
          dependencyType="install" codebase="BIN/card.DLL"><assemblyIdentity name="Card" />
          </dependentAssembly></dependency>"#,
    ] {
        assert!(matches!(
            with_contents(contents),
            Err(Error::DuplicateResource(_))
        ));
    }
}

/// Malformed XML is reported through the underlying manifest error.
#[test]
fn malformed_deployment_and_application_fail() {
    assert!(matches!(
        Deployment::from_bytes(DEPLOYMENT_URL, b"<assembly>"),
        Err(Error::Manifest(_))
    ));
    assert!(matches!(
        Deployment::from_bytes(DEPLOYMENT_URL, UNMAPPED)
            .unwrap()
            .with_application_bytes(b"bad XML"),
        Err(Error::Manifest(_))
    ));
}

/// Manifest roles are checked independently of the URL extension.
#[test]
fn wrong_manifest_roles_fail() {
    assert!(matches!(
        Deployment::from_bytes(DEPLOYMENT_URL, RESOURCES),
        Err(Error::UnexpectedManifest {
            expected: ManifestKind::Deployment,
            actual: ManifestKind::Application
        })
    ));
    assert!(matches!(
        Deployment::from_bytes(DEPLOYMENT_URL, UNMAPPED)
            .unwrap()
            .with_application_bytes(UNMAPPED),
        Err(Error::UnexpectedManifest {
            expected: ManifestKind::Application,
            actual: ManifestKind::Deployment
        })
    ));
}

/// A valid manifest for another build must not be paired with the deployment.
#[test]
fn mismatched_application_identity_is_rejected() {
    let manifest = String::from_utf8(RESOURCES.to_vec()).unwrap();
    for mismatched in [
        manifest.replacen("name=\"MTGO.exe\"", "name=\"Other.exe\"", 1),
        manifest.replacen("version=\"1.2.3.4\"", "version=\"1.2.3.5\"", 1),
    ] {
        assert!(matches!(
            Deployment::from_bytes(DEPLOYMENT_URL, UNMAPPED)
                .unwrap()
                .with_application_bytes(mismatched.as_bytes()),
            Err(Error::ApplicationIdentityMismatch)
        ));
    }
}

/// No reference or multiple install references cannot identify one application.
#[test]
fn deployment_requires_exactly_one_application_reference() {
    let xml = String::from_utf8(UNMAPPED.to_vec()).unwrap();
    let start = xml.find("  <v2:dependency>").unwrap();
    let end = xml.find("  </v2:dependency>").unwrap() + "  </v2:dependency>".len();
    let dependency = &xml[start..end];
    let missing = format!("{}{}", &xml[..start], &xml[end..]);
    let multiple = xml.replacen("</assembly>", &format!("{dependency}</assembly>"), 1);
    assert!(matches!(
        Deployment::from_bytes(DEPLOYMENT_URL, missing.as_bytes()),
        Err(Error::ApplicationReferenceCount(0))
    ));
    assert!(matches!(
        Deployment::from_bytes(DEPLOYMENT_URL, multiple.as_bytes()),
        Err(Error::ApplicationReferenceCount(2))
    ));
}

/// Manifest-declared filesystem paths must remain relative and unambiguous.
#[test]
fn invalid_file_paths_are_rejected_before_url_resolution() {
    for path in [
        "",
        "/root.xml",
        r"\root.xml",
        "../outside.xml",
        "dir/../outside.xml",
        "./cards.xml",
        "dir//cards.xml",
        r"C:\cards.xml",
    ] {
        let xml = format!(r#"<file name="{path}" />"#);
        assert!(
            matches!(with_contents(&xml), Err(Error::InvalidPath(_))),
            "path {path:?}"
        );
    }
}

/// Deployment URLs support the HTTP transports used by patch services.
#[test]
fn unsupported_deployment_schemes_are_rejected() {
    for url in [
        "file:///tmp/MTGO.application",
        "ftp://patch.example.test/MTGO.application",
    ] {
        assert!(matches!(
            Deployment::from_bytes(url, UNMAPPED),
            Err(Error::UnsupportedUrl(_))
        ));
    }
    assert!(matches!(
        Deployment::from_bytes("not a URL", UNMAPPED),
        Err(Error::Url(_))
    ));
}
