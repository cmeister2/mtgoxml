//! Offline artwork configuration, source-identity, and URL construction coverage.

use mtgoxml::carddata::CardDataFile;
use mtgoxml::{Application, ArtworkError, ArtworkResolver, CardDatabase, Deployment, Url};

/// Minimal configuration with the artwork roots observed in the official client.
const CONFIG: &[u8] = include_bytes!("../fixtures/artwork/download-source.config");
/// Final response URL for the captured client configuration.
const CONFIG_URL: &str = "http://mtgo.patch.daybreakgames.com/patch/mtg/live/client/3.4.159.4708.20261014120111/MTGO.exe.config";

/// Create a synthetic application identity for offline card-data insertion.
fn application() -> Application {
    Deployment::from_bytes(
        "https://patch.example/client/MTGO.application",
        include_bytes!("../fixtures/synthetic/unmapped.application"),
    )
    .unwrap()
    .with_application_bytes(include_bytes!(
        "../fixtures/synthetic/resources.exe.manifest"
    ))
    .unwrap()
}

/// Build a complete one-object database using literal fields supplied by a test.
fn database(fields: &str) -> CardDatabase {
    let application = application();
    let mut database = application.card_database();
    database.insert(&application, CardDataFile::parse(&format!(r#"
      <CardSet id="TEST"><DigitalObject DigitalObjectCatalogID="DOC_1">{fields}</DigitalObject></CardSet>
    "#)).unwrap()).unwrap();
    database
}

/// Parse a compact test configuration with caller-selected roots.
fn configured(root: &str, subpath: &str) -> Result<ArtworkResolver, ArtworkError> {
    ArtworkResolver::from_config(
        Url::parse("https://patch.example/client/redirected/MTGO.exe.config").unwrap(),
        format!(r#"<configuration><downloadSource><artRoot value="{root}"/><cardArtSubPath value="{subpath}"/></downloadSource></configuration>"#).as_bytes(),
    )
}

/// The known Mystical Tutor artwork uses ARTID rather than the foil's texture number.
#[test]
fn canonical_artwork_uses_inherited_art_id_and_ignores_texture_number() {
    let application = application();
    let mut database = application.card_database();
    database
        .insert(
            &application,
            CardDataFile::parse(
                r#"<CardSet id="V09">
      <DigitalObject DigitalObjectCatalogID="DOC_33914">
        <ARTID value="1982"/><FRAMESTYLE value="3"/>
        <DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/><CARDTEXTURE_NUMBER value="67828"/>
      </DigitalObject>
      <DigitalObject DigitalObjectCatalogID="DOC_33915">
        <CLONE_ID value="33914"/><IS_FOIL/><CARDTEXTURE_NUMBER value="67831"/>
      </DigitalObject>
    </CardSet>"#,
            )
            .unwrap(),
        )
        .unwrap();
    let resolver = ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), CONFIG).unwrap();
    let expected = "http://mtgo.patch.daybreakgames.com/patch/mtg/live/Graphics/Cards/Pics/01982_typ_reg_sty_010.jpg";
    assert_eq!(
        resolver
            .for_object(&database.object(33914).unwrap())
            .unwrap()
            .as_str(),
        expected
    );
    assert_eq!(
        resolver
            .for_object(&database.object(33915).unwrap())
            .unwrap()
            .as_str(),
        expected
    );
}

/// Canonical crop naming is independent of the source's modern frame-style code.
#[test]
fn modern_frames_and_sub_cards_use_canonical_crop() {
    let resolver = configured("https://art.example/Graphics", "Cards/Pics").unwrap();
    let database = database(
        r#"<ARTID value="431903"/><FRAMESTYLE value="111"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>"#,
    );
    assert_eq!(
        resolver
            .for_object(&database.object(1).unwrap())
            .unwrap()
            .as_str(),
        "https://art.example/Graphics/Cards/Pics/431903_typ_reg_sty_010.jpg"
    );
    let database = self::database(
        r#"<ARTID value="25533"/><FRAMESTYLE value="1"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="SUBC"/>"#,
    );
    assert_eq!(
        resolver
            .for_object(&database.object(1).unwrap())
            .unwrap()
            .as_str(),
        "https://art.example/Graphics/Cards/Pics/25533_typ_reg_sty_010.jpg"
    );
}

/// Unsupported object kinds and known avatar frames do not advertise a normal-card URL.
#[test]
fn products_tokens_play_aids_and_avatars_have_no_normal_crop() {
    let resolver = configured("https://art.example/Graphics", "Cards/Pics").unwrap();
    for kind in ["BSTR", "TOKN", "PLAD", "UNKNOWN"] {
        // A valid-looking artwork identifier cannot substitute for a supported object kind.
        let database = database(&format!(
            r#"<ARTID value="1982"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="{kind}"/>"#
        ));
        assert!(resolver.for_object(&database.object(1).unwrap()).is_none());
    }
    let database = database(
        r#"<ARTID value="124294"/><FRAMESTYLE value="11"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>"#,
    );
    assert!(resolver.for_object(&database.object(1).unwrap()).is_none());
    let database = self::database(r#"<ARTID value="1982"/>"#);
    assert!(resolver.for_object(&database.object(1).unwrap()).is_none());
    let database = self::database(
        r#"<ARTID value="124294"/><FRAMESTYLE value="011"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>"#,
    );
    assert!(resolver.for_object(&database.object(1).unwrap()).is_none());
}

/// Missing and malformed IDs cannot become paths, and valid IDs retain all their digits.
#[test]
fn artwork_ids_are_positive_bounded_decimal_integers() {
    let resolver = configured("https://art.example/Graphics", "Cards/Pics").unwrap();
    for id in [
        "",
        "0",
        "-1",
        "+1982",
        " 1982",
        "1.5",
        "1/2",
        "18446744073709551616",
    ] {
        // Reject malformed IDs before formatting a URL or interpreting any path syntax.
        let database = database(&format!(
            r#"<ARTID value="{id}"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>"#
        ));
        assert!(resolver.for_object(&database.object(1).unwrap()).is_none());
    }
    let database =
        database(r#"<ARTID value="00001"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>"#);
    assert!(
        resolver
            .for_object(&database.object(1).unwrap())
            .unwrap()
            .path()
            .ends_with("/00001_typ_reg_sty_010.jpg")
    );
    let database =
        self::database(r#"<ARTID value="123456"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>"#);
    assert!(
        resolver
            .for_object(&database.object(1).unwrap())
            .unwrap()
            .path()
            .ends_with("/123456_typ_reg_sty_010.jpg")
    );
    let database = self::database(r#"<DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>"#);
    assert!(resolver.for_object(&database.object(1).unwrap()).is_none());
}

/// Final response URLs resolve relative roots while directory segments remain literal.
#[test]
fn relative_roots_windows_separators_and_queries_resolve_without_path_loss() {
    let resolver = configured("../Graphics?token=test", r"Cards\Card Pics").unwrap();
    assert_eq!(
        resolver.base_url().as_str(),
        "https://patch.example/client/Graphics/Cards/Card%20Pics/?token=test"
    );
    let database =
        database(r#"<ARTID value="1982"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>"#);
    assert_eq!(
        resolver
            .for_object(&database.object(1).unwrap())
            .unwrap()
            .as_str(),
        "https://patch.example/client/Graphics/Cards/Card%20Pics/01982_typ_reg_sty_010.jpg?token=test"
    );
}

/// Parse only one unambiguous source section and required setting at each level.
#[test]
fn rejects_missing_duplicate_and_namespaced_configuration() {
    for xml in [
        "<other/>",
        "<configuration/>",
        "<configuration><downloadSource/><downloadSource/></configuration>",
        r#"<configuration><downloadSource><artRoot value="https://art.example"/><artRoot value="https://other.example"/><cardArtSubPath value="Cards/Pics"/></downloadSource></configuration>"#,
        r#"<configuration><downloadSource><artRoot value="https://art.example"/><cardArtSubPath/></downloadSource></configuration>"#,
        r#"<configuration xmlns="urn:other"><downloadSource/></configuration>"#,
        r#"<configuration><downloadSource xmlns="urn:other"><artRoot value="https://art.example"/><cardArtSubPath value="Cards/Pics"/></downloadSource></configuration>"#,
        r#"<configuration><downloadSource><artRoot value="https://art.example"/><cardArtSubPath value=""/></downloadSource></configuration>"#,
    ] {
        // Every ambiguous or incomplete source must fail instead of silently choosing a CDN.
        assert!(
            ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), xml.as_bytes()).is_err()
        );
    }
}

/// Namespace-qualified attributes cannot provide or shadow the required plain value.
#[test]
fn value_attributes_must_be_unqualified() {
    let config = br#"<configuration xmlns:other="urn:other"><downloadSource>
      <artRoot other:value="https://wrong.example" value="https://right.example/Graphics"/>
      <cardArtSubPath other:value="Wrong" value="Cards/Pics"/>
    </downloadSource></configuration>"#;
    let resolver = ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), config).unwrap();
    assert_eq!(
        resolver.base_url().as_str(),
        "https://right.example/Graphics/Cards/Pics/"
    );
    let only_qualified = br#"<configuration xmlns:other="urn:other"><downloadSource>
      <artRoot other:value="https://wrong.example"/>
      <cardArtSubPath value="Cards/Pics"/>
    </downloadSource></configuration>"#;
    assert!(ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), only_qualified).is_err());
}

/// Source locations cannot redirect artwork through unsupported schemes or absolute subpaths.
#[test]
fn rejects_unsupported_urls_and_nonrelative_subpaths() {
    assert!(configured("file:///tmp/Graphics", "Cards/Pics").is_err());
    assert!(configured("https://art.example/Graphics#fragment", "Cards/Pics").is_err());
    for subpath in [
        "/Cards/Pics",
        "../Cards/Pics",
        "Cards/./Pics",
        "Cards//Pics",
        "https://other.example",
    ] {
        assert!(configured("https://art.example/Graphics", subpath).is_err());
    }
    assert!(
        ArtworkResolver::from_config(Url::parse("file:///tmp/config").unwrap(), CONFIG).is_err()
    );
}

/// Configuration parsing accepts a UTF-8 BOM but rejects DTDs and oversized byte input.
#[test]
fn bounded_utf8_configuration_does_not_process_dtds() {
    let mut bom = vec![0xef, 0xbb, 0xbf];
    bom.extend_from_slice(CONFIG);
    ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), &bom).unwrap();
    assert!(ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), &[0xff]).is_err());
    let dtd = br#"<!DOCTYPE configuration [<!ENTITY root "https://art.example">]><configuration><downloadSource><artRoot value="&root;"/><cardArtSubPath value="Cards/Pics"/></downloadSource></configuration>"#;
    assert!(ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), dtd).is_err());
    let oversized = vec![b' '; 16 * 1024 * 1024 + 1];
    assert!(matches!(
        ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), &oversized),
        Err(ArtworkError::InputTooLarge { .. })
    ));
    let too_many_nodes = format!(
        "<configuration>{}</configuration>",
        "<extra/>".repeat(250_000)
    );
    assert!(matches!(
        ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), too_many_nodes.as_bytes()),
        Err(ArtworkError::Xml(_))
    ));
}

/// Missing configuration is reported before any network request is attempted.
#[cfg(feature = "http")]
#[test]
fn client_reports_missing_configuration_resource() {
    assert!(
        matches!(mtgoxml::Client::new().load_artwork(&application()),
        Err(mtgoxml::Error::MissingResource(path)) if path == "MTGO.exe.config")
    );
}
