//! Friendly projection contracts using small, controlled component documents.

use std::collections::BTreeMap;

use mtgoxml::Deployment;
use mtgoxml::carddata::{
    CardDataFile, CardDatabase, Color, Error, FriendlyObject, FriendlyValue, Rarity,
};

/// Build a database bound to a synthetic application without fetching any resources.
fn database(objects: &str) -> CardDatabase {
    let application = Deployment::from_bytes(
        "https://patch.example.test/MTGO.application",
        include_bytes!("../fixtures/synthetic/mapped.application"),
    )
    .unwrap()
    .with_application_bytes(include_bytes!(
        "../fixtures/synthetic/resources.exe.manifest"
    ))
    .unwrap();
    let mut database = application.card_database();
    let set = CardDataFile::parse(&format!(
        r#"<CardSet id="SOURCE" age="7" cardsetType="PAPER">{objects}</CardSet>"#
    ))
    .unwrap();
    database.insert(&application, set).unwrap();
    database
}

/// Project one object and return its owned representation after the database is dropped.
fn project(fields: &str) -> Result<FriendlyObject, Error> {
    let database = database(&format!(
        r#"<DigitalObject DigitalObjectCatalogID="DOC_1">{fields}</DigitalObject>"#
    ));
    database.object(1)?.friendly()
}

/// The explicit aliases normalize known values and preserve empty text and MTGO notation.
#[test]
fn promotes_recognized_fields_without_duplicate_source_keys() {
    let object = project(
        r##"<CARDNAME_STRING value="Mystical Tutor"/>
        <ARTIST_NAME_STRING value="David O’Connor"/>
        <REAL_ORACLETEXT_STRING value="rules&#10;text"/>
        <FLAVORTEXT_STRING value=""/><MANA_COST_STRING value="#2b-"/>
        <ARTID value="1982"/><CARDSETNAME_STRING value="V09"/>
        <COLLECTOR_INFO_STRING value="8/15"/><CONVERTED_MANA_COST value="1"/>
        <COLOR value="COLOR_BLUE"/><COLOR_IDENTITY value="COLOR_BLUE"/>
        <RARITY_STATUS value="MA_RARITY_MYTHIC"/><IS_FOIL/>
        <CARDNAME_TOKEN value="MA_CTOKEN_MYSTICAL_TUTOR"/>"##,
    )
    .unwrap();
    assert_eq!(object.id, 1);
    assert_eq!(object.name.as_deref(), Some("Mystical Tutor"));
    assert_eq!(object.artist.as_deref(), Some("David O’Connor"));
    assert_eq!(object.rules_text.as_deref(), Some("rules\ntext"));
    assert_eq!(object.flavor_text.as_deref(), Some(""));
    assert_eq!(object.mana_cost.as_deref(), Some("#2b-"));
    assert_eq!(object.art_id, Some(1982));
    assert!(object.art_url.is_none());
    assert_eq!(object.set, "V09");
    assert_eq!(object.source_set.as_deref(), Some("SOURCE"));
    assert_eq!(object.collector_number.as_deref(), Some("8/15"));
    assert_eq!(object.mana_value, Some(1));
    assert_eq!(object.colors, Some(vec![Color::Blue]));
    assert_eq!(object.color_identity, Some(vec![Color::Blue]));
    assert_eq!(object.rarity, Some(Rarity::Mythic));
    assert!(object.foil);
    assert_eq!(
        object.mtgo,
        BTreeMap::from([(
            "CARDNAME_TOKEN".into(),
            FriendlyValue::Text("MA_CTOKEN_MYSTICAL_TUTOR".into())
        )])
    );
}

/// Unknown optional forms and values remain recoverable instead of partially normalizing.
#[test]
fn unfamiliar_alias_values_and_forms_remain_in_mtgo() {
    let object = project(
        r#"<CARDNAME_STRING/><ARTIST_NAME_STRING><CHILD value="artist"/></ARTIST_NAME_STRING>
        <ARTID value="18446744073709551616"/><CARDSETNAME_STRING/>
        <COLLECTOR_INFO_STRING><FUTURE value="collector"/></COLLECTOR_INFO_STRING>
        <CONVERTED_MANA_COST value="1.5"/>
        <COLOR value="COLOR_BLUE|COLOR_FUTURE"/><COLOR_IDENTITY/>
        <RARITY_STATUS value="MA_RARITY_FUTURE"/>"#,
    )
    .unwrap();
    assert!(object.name.is_none());
    assert!(object.artist.is_none());
    assert!(object.art_id.is_none());
    assert!(object.collector_number.is_none());
    assert!(object.mana_value.is_none());
    assert!(object.colors.is_none());
    assert!(object.color_identity.is_none());
    assert!(object.rarity.is_none());
    assert_eq!(object.set, "SOURCE");
    assert!(object.source_set.is_none());
    assert!(!object.foil);
    assert_eq!(object.mtgo.len(), 9);
    assert_eq!(object.mtgo["CARDNAME_STRING"], FriendlyValue::Flag);
    assert_eq!(object.mtgo["CARDSETNAME_STRING"], FriendlyValue::Flag);
    assert_eq!(object.mtgo["COLOR_IDENTITY"], FriendlyValue::Flag);
    assert_eq!(
        object.mtgo["ARTIST_NAME_STRING"],
        FriendlyValue::Fields(BTreeMap::from([(
            "CHILD".into(),
            FriendlyValue::Text("artist".into())
        )]))
    );
    assert_eq!(
        object.mtgo["COLOR"],
        FriendlyValue::Text("COLOR_BLUE|COLOR_FUTURE".into())
    );
    assert_eq!(
        object.mtgo["RARITY_STATUS"],
        FriendlyValue::Text("MA_RARITY_FUTURE".into())
    );
}

/// Number aliases accept only unsigned decimal values and leave other source text untouched.
#[test]
fn numeric_aliases_reject_other_syntax_without_coercing_unmapped_fields() {
    for value in ["", "+1", "-1", "1.5", " 1", "18446744073709551616"] {
        let object = project(&format!(
            r#"<ARTID value="{value}"/><CONVERTED_MANA_COST value="{value}"/>"#
        ))
        .unwrap();
        assert!(object.art_id.is_none(), "{value}");
        assert!(object.mana_value.is_none(), "{value}");
        for name in ["ARTID", "CONVERTED_MANA_COST"] {
            assert_eq!(object.mtgo[name], FriendlyValue::Text(value.into()));
        }
    }
    let object = project(
        r#"<ARTID value="18446744073709551615"/><CONVERTED_MANA_COST value="0"/>
        <COMPONENT_VERSION_STRING value="001"/><LOYALTY_STRING value="X"/>
        <POWER value="1"/><POWERTOUGHNESS_STRING value="1+*/1+*"/>"#,
    )
    .unwrap();
    assert_eq!(object.art_id, Some(u64::MAX));
    assert_eq!(object.mana_value, Some(0));
    assert_eq!(
        object.mtgo["COMPONENT_VERSION_STRING"],
        FriendlyValue::Text("001".into())
    );
    assert_eq!(
        object.mtgo["LOYALTY_STRING"],
        FriendlyValue::Text("X".into())
    );
    assert_eq!(object.mtgo["POWER"], FriendlyValue::Text("1".into()));
    assert_eq!(
        object.mtgo["POWERTOUGHNESS_STRING"],
        FriendlyValue::Text("1+*/1+*".into())
    );
}

/// Color sets are canonical and deduplicated, while unknown or mixed-colorless values survive.
#[test]
fn colors_are_canonical_and_colorless_differs_from_missing() {
    let object = project(
        r#"<COLOR value="COLOR_GREEN|COLOR_RED|COLOR_WHITE|COLOR_BLACK|COLOR_BLUE|COLOR_BLUE"/>
        <COLOR_IDENTITY value="COLOR_COLORLESS"/>"#,
    )
    .unwrap();
    assert_eq!(
        object.colors,
        Some(vec![
            Color::White,
            Color::Blue,
            Color::Black,
            Color::Red,
            Color::Green
        ])
    );
    assert_eq!(object.color_identity, Some(vec![]));
    assert!(project("").unwrap().colors.is_none());
    for value in ["", "COLOR_COLORLESS|COLOR_BLUE", "COLOR_BLUE|", "BLUE"] {
        let object = project(&format!(r#"<COLOR value="{value}"/>"#)).unwrap();
        assert!(object.colors.is_none(), "{value}");
        assert_eq!(object.mtgo["COLOR"], FriendlyValue::Text(value.into()));
    }
}

/// The rarity projection includes every known nonstandard category without conflating them.
#[test]
fn all_known_rarities_have_distinct_friendly_variants() {
    for (source, expected) in [
        ("BASIC_LAND", Rarity::BasicLand),
        ("COMMON", Rarity::Common),
        ("UNCOMMON", Rarity::Uncommon),
        ("RARE", Rarity::Rare),
        ("MYTHIC", Rarity::Mythic),
        ("TOKEN", Rarity::Token),
        ("BONUS", Rarity::Bonus),
        ("PROMO", Rarity::Promo),
        ("PLAY_AID", Rarity::PlayAid),
    ] {
        let object = project(&format!(r#"<RARITY_STATUS value="MA_RARITY_{source}"/>"#)).unwrap();
        assert_eq!(object.rarity, Some(expected));
        assert!(!object.mtgo.contains_key("RARITY_STATUS"));
    }
}

/// The owned view retains clone overrides and every unpromoted nested value form.
#[test]
fn clone_inheritance_and_nested_source_fields_are_preserved() {
    let database = database(
        r#"<DigitalObject DigitalObjectCatalogID="DOC_1">
          <CARDNAME_STRING value="Base"/><ARTIST_NAME_STRING value="Artist"/>
          <GROUP><EMPTY value=""/><FLAG/><NUMERIC value="001"/>
            <NESTED><CARDNAME_STRING value="Nested name"/></NESTED></GROUP>
        </DigitalObject>
        <DigitalObject DigitalObjectCatalogID="DOC_2">
          <CLONE_ID value="1"/><CARDNAME_STRING value=""/><IS_FOIL/>
        </DigitalObject>"#,
    );
    let original = database.object(2).unwrap();
    let object = original.friendly().unwrap();
    assert_eq!(object.name.as_deref(), Some(""));
    assert_eq!(object.artist.as_deref(), Some("Artist"));
    assert!(object.foil);
    assert_eq!(object.mtgo["CLONE_ID"], FriendlyValue::Text("1".into()));
    assert_eq!(
        object.mtgo["GROUP"],
        FriendlyValue::Fields(BTreeMap::from([
            ("EMPTY".into(), FriendlyValue::Text("".into())),
            ("FLAG".into(), FriendlyValue::Flag),
            ("NUMERIC".into(), FriendlyValue::Text("001".into())),
            (
                "NESTED".into(),
                FriendlyValue::Fields(BTreeMap::from([(
                    "CARDNAME_STRING".into(),
                    FriendlyValue::Text("Nested name".into())
                )]))
            ),
        ]))
    );
    assert!(original.fields().contains_key("CARDNAME_STRING"));
}

/// Foil handling follows the existing strict contract instead of suppressing invalid values.
#[test]
fn foil_normalizes_valid_values_and_propagates_invalid_values() {
    for (source, expected) in [
        ("", false),
        (r#"<IS_FOIL/>"#, true),
        (r#"<IS_FOIL value="1"/>"#, true),
        (r#"<IS_FOIL value="TrUe"/>"#, true),
        (r#"<IS_FOIL value="0"/>"#, false),
        (r#"<IS_FOIL value="FALSE"/>"#, false),
    ] {
        let object = project(source).unwrap();
        assert_eq!(object.foil, expected);
        assert!(!object.mtgo.contains_key("IS_FOIL"));
    }
    for source in [
        r#"<IS_FOIL value="perhaps"/>"#,
        r#"<IS_FOIL><CHILD/></IS_FOIL>"#,
    ] {
        assert!(matches!(
            project(source),
            Err(Error::InvalidValue { field, .. }) if field == "IS_FOIL"
        ));
    }
}

/// Serialization emits friendly names and direct source values while omitting missing aliases.
#[cfg(feature = "serde")]
#[test]
fn serialization_preserves_option_and_source_value_forms() {
    let object = project(
        r#"<CARDNAME_STRING value=""/><ARTID value="1982"/>
        <CONVERTED_MANA_COST value="1"/><COLOR value="COLOR_BLUE"/>
        <COLOR_IDENTITY value="COLOR_COLORLESS"/>
        <RARITY_STATUS value="MA_RARITY_PLAY_AID"/>
        <UNKNOWN/><GROUP><EMPTY value=""/><NUMERIC value="001"/></GROUP>"#,
    )
    .unwrap();
    let json = serde_json::to_value(object).unwrap();
    assert_eq!(json["name"], "");
    assert!(json.get("artist").is_none());
    assert!(json.get("art_url").is_none());
    assert_eq!(json["art_id"], 1982);
    assert_eq!(json["mana_value"], 1);
    assert_eq!(json["colors"], serde_json::json!(["U"]));
    assert_eq!(json["color_identity"], serde_json::json!([]));
    assert_eq!(json["rarity"], "play_aid");
    assert_eq!(json["set"], "SOURCE");
    assert!(json.get("source_set").is_none());
    assert_eq!(json["foil"], false);
    assert_eq!(json["mtgo"]["UNKNOWN"], true);
    assert_eq!(json["mtgo"]["GROUP"]["EMPTY"], "");
    assert_eq!(json["mtgo"]["GROUP"]["NUMERIC"], "001");
    assert!(json["mtgo"].get("CARDNAME_STRING").is_none());
}
