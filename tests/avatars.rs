//! Avatar metadata is shared by resolved-source and saved-dataset consumers.

use mtgoxml::avatar::Avatar;
use mtgoxml::carddata::CardDataFile;
use mtgoxml::{ArtworkResolver, CardDatabase, Deployment, Url};

/// Construct one source avatar with caller-selected client fields.
fn database(id: u64, fields: &str) -> CardDatabase {
    let application = Deployment::from_bytes(
        "https://patch.example/client/MTGO.application",
        include_bytes!("../fixtures/synthetic/unmapped.application"),
    )
    .unwrap()
    .with_application_bytes(include_bytes!(
        "../fixtures/synthetic/resources.exe.manifest"
    ))
    .unwrap();
    let mut database = application.card_database();
    database.insert(&application, CardDataFile::parse(&format!(r#"<CardSet id="TEST"><DigitalObject DigitalObjectCatalogID="DOC_{id}"><CARDNAME_STRING value="Test avatar"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>{fields}</DigitalObject></CardSet>"#)).unwrap()).unwrap();
    database
}

/// Client integer flags and filename fallbacks have one interpretation.
#[test]
fn integer_flags_and_fallbacks_match_primary_artwork() {
    let resolver = ArtworkResolver::from_config(
        Url::parse("https://patch.example/client/config").unwrap(),
        br#"<configuration><downloadSource><artRoot value="https://art.example/Graphics?version=1"/><cardArtSubPath value="Cards/Pics"/><avatarArtSubPath value="Custom/Portraits"/></downloadSource></configuration>"#,
    ).unwrap();
    for art in ["", r#"<ARTID value="0"/>"#, r#"<ARTID value="23"/>"#] {
        let database = database(
            156681,
            &format!(r#"<IS_VANGUARD value="2"/><PREMIUM_EFFECT value="24"/>{art}"#),
        );
        let object = database.object(156681).unwrap();
        let avatar = Avatar::from_object(&object).unwrap().unwrap();
        assert_eq!(avatar.premium_effect(), 24);
        let name = if art.contains("23") {
            "00023_avatar.jpg"
        } else {
            "156681c_avatar.jpg"
        };
        assert_eq!(avatar.file_name(), name);
        assert_eq!(
            resolver.for_object(&object).unwrap().path(),
            format!("/Graphics/Custom/Portraits/{name}")
        );
        assert_eq!(avatar.asset_paths()[0], format!("Avatars/Regular/{name}"));
        assert_eq!(avatar.asset_paths().len(), 6);
    }
    assert_eq!(
        resolver.art_root_url().as_str(),
        "https://art.example/Graphics?version=1"
    );
}

/// An explicit invalid artwork identity never selects the catalog fallback.
#[test]
fn malformed_artwork_is_rejected_and_unknown_effects_are_retained() {
    for art in ["bad", "-1", "2147483648", ""] {
        let database = database(10, &format!(r#"<IS_VANGUARD/><ARTID value="{art}"/>"#));
        assert!(Avatar::from_object(&database.object(10).unwrap()).is_err());
    }
    let database = database(
        10,
        r#"<IS_VANGUARD/><ARTID value="20"/><PREMIUM_EFFECT value="999"/>"#,
    );
    let avatar = Avatar::from_object(&database.object(10).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(avatar.premium_effect(), 999);
    assert_eq!(avatar.asset_paths().len(), 6);
    let database = self::database(10, r#"<IS_VANGUARD value="0"/>"#);
    assert!(
        Avatar::from_object(&database.object(10).unwrap())
            .unwrap()
            .is_none()
    );
}

/// Friendly records preserve source semantics through their public serialization format.
#[cfg(feature = "serde")]
#[test]
fn saved_and_resolved_avatar_metadata_are_identical() {
    for fields in [
        r#"<IS_VANGUARD value="2"/><PREMIUM_EFFECT value="24"/>"#,
        r#"<IS_VANGUARD/><ARTID value="00023"/><PREMIUM_EFFECT value="999"/>"#,
        r#"<IS_VANGUARD value="0"/>"#,
        r#"<IS_VANGUARD/><ARTID value="not an ID"/>"#,
        r#"<IS_VANGUARD/><ARTID/>"#,
        r#"<IS_VANGUARD/><ARTID value="23"/><COLOR value="COLOR_BLUE"/><RARITY_STATUS value="MA_RARITY_PROMO"/><UNKNOWN><FLAG/><TEXT value="001"/></UNKNOWN>"#,
    ] {
        let database = database(10, fields);
        let object = database.object(10).unwrap();
        let friendly = object.friendly().unwrap();
        let bytes = serde_json::to_vec(&friendly).unwrap();
        let loaded = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            serde_json::to_value(&friendly).unwrap(),
            serde_json::to_value(&loaded).unwrap()
        );
        match (Avatar::from_object(&object), Avatar::from_friendly(&loaded)) {
            (Ok(source), Ok(saved)) => assert_eq!(source, saved),
            (Err(_), Err(_)) => {}
            result => panic!("source and saved metadata disagree: {result:?}"),
        }
    }
}

/// Animated export filtering does not validate unused artwork for static avatars.
#[cfg(feature = "serde")]
#[test]
fn static_invalid_artwork_is_ignored_only_by_animated_projection() {
    for effect in ["", r#"<PREMIUM_EFFECT value="0"/>"#] {
        let database = database(
            10,
            &format!(r#"<IS_VANGUARD value="2"/><ARTID value="invalid"/>{effect}"#),
        );
        let object = database.object(10).unwrap();
        let friendly = object.friendly().unwrap();
        assert!(Avatar::from_object(&object).is_err());
        assert!(Avatar::from_friendly(&friendly).is_err());
        assert!(Avatar::animated_from_object(&object).unwrap().is_none());
        assert!(Avatar::animated_from_friendly(&friendly).unwrap().is_none());
    }
    let database = database(
        10,
        r#"<IS_VANGUARD value="2"/><ARTID value="invalid"/><PREMIUM_EFFECT value="1"/>"#,
    );
    let object = database.object(10).unwrap();
    assert!(Avatar::animated_from_object(&object).is_err());
    assert!(Avatar::animated_from_friendly(&object.friendly().unwrap()).is_err());
}
