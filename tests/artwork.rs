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
    database_in_set("TEST", fields)
}

/// Declare a source set independently of any card-set display-name property.
fn database_in_set(set: &str, fields: &str) -> CardDatabase {
    let application = application();
    let mut database = application.card_database();
    database.insert(&application, CardDataFile::parse(&format!(r#"
      <CardSet id="{set}"><DigitalObject DigitalObjectCatalogID="DOC_1">{fields}</DigitalObject></CardSet>
    "#)).unwrap()).unwrap();
    database
}

/// Resolve a synthetic card with a small art ID through the public artwork API.
fn card_art_path(set: &str, fields: &str) -> Option<String> {
    let resolver = configured("https://art.example/Graphics", "Cards/Pics").unwrap();
    let database = database_in_set(
        set,
        &format!(r#"<ARTID value="23"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>{fields}"#),
    );
    resolver
        .for_object(&database.object(1).unwrap())
        .map(|url| url.path().to_owned())
}

/// Check a client-selected filename without probing the CDN or relying on its contents.
fn assert_art_suffix(set: &str, fields: &str, suffix: &str) {
    assert_eq!(
        card_art_path(set, fields),
        Some(format!("/Graphics/Cards/Pics/00023{suffix}.jpg")),
        "source set {set}, fields {fields}"
    );
}

/// Parse a compact test configuration with caller-selected roots.
fn configured(root: &str, subpath: &str) -> Result<ArtworkResolver, ArtworkError> {
    ArtworkResolver::from_config(
        Url::parse("https://patch.example/client/redirected/MTGO.exe.config").unwrap(),
        format!(r#"<configuration><downloadSource><artRoot value="{root}"/><cardArtSubPath value="{subpath}"/></downloadSource></configuration>"#).as_bytes(),
    )
}

/// Keep the normal card directory while supplying optional avatar settings verbatim.
fn configured_with_avatars(root: &str, settings: &str) -> Result<ArtworkResolver, ArtworkError> {
    ArtworkResolver::from_config(
        Url::parse("https://patch.example/client/redirected/MTGO.exe.config").unwrap(),
        format!(r#"<configuration><downloadSource><artRoot value="{root}"/><cardArtSubPath value="Cards/Pics"/>{settings}</downloadSource></configuration>"#).as_bytes(),
    )
}

/// Cosmetic avatars and playable Vanguards share the avatar directory and inherited ARTID.
#[test]
fn avatar_artwork_uses_art_id_and_clone_inheritance() {
    let application = application();
    let mut database = application.card_database();
    database
        .insert(
            &application,
            CardDataFile::parse(
                r#"<CardSet id="CC">
      <DigitalObject DigitalObjectCatalogID="DOC_39677">
        <ARTID value="1000129"/><FRAMESTYLE value="11"/><IS_VANGUARD value="1"/>
        <DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/><CARDTEXTURE_NUMBER value="79354"/>
        <REAL_ORACLETEXT_STRING value="At the beginning of your upkeep, put a charge counter on Avatar - Hijinks."/>
      </DigitalObject>
      <DigitalObject DigitalObjectCatalogID="DOC_39678">
        <CLONE_ID value="39677"/><CARDTEXTURE_NUMBER value="79357"/>
      </DigitalObject>
      <DigitalObject DigitalObjectCatalogID="DOC_156681">
        <FRAMESTYLE value="11"/><IS_VANGUARD/>
        <DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/><REAL_ORACLETEXT_STRING value=""/>
      </DigitalObject>
    </CardSet>"#,
            )
            .unwrap(),
        )
        .unwrap();
    let resolver = ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), CONFIG).unwrap();
    assert_eq!(
        resolver.avatar_base_url().unwrap().as_str(),
        "http://mtgo.patch.daybreakgames.com/patch/mtg/live/Graphics/Avatars/Regular/"
    );
    for id in [39677, 39678] {
        // Clone and texture IDs do not replace a nonzero inherited artwork ID.
        assert_eq!(
            resolver
                .for_object(&database.object(id).unwrap())
                .unwrap()
                .as_str(),
            "http://mtgo.patch.daybreakgames.com/patch/mtg/live/Graphics/Avatars/Regular/1000129_avatar.jpg"
        );
    }
    assert_eq!(
        resolver
            .for_object(&database.object(156681).unwrap())
            .unwrap()
            .path(),
        "/patch/mtg/live/Graphics/Avatars/Regular/156681c_avatar.jpg"
    );
}

/// Missing and zero avatar art IDs use the object's own catalog ID and c suffix.
#[test]
fn avatar_catalog_fallback_uses_each_objects_identity() {
    let resolver = ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), CONFIG).unwrap();
    for art in ["", r#"<ARTID value="0"/>"#] {
        // A clone inherits avatar properties but the fallback uses its own catalog identity.
        let application = application();
        let mut database = application.card_database();
        database
            .insert(
                &application,
                CardDataFile::parse(&format!(
                    r#"
          <CardSet id="CC">
            <DigitalObject DigitalObjectCatalogID="DOC_1">
              {art}<IS_VANGUARD/><FRAMESTYLE value="11"/>
              <DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>
            </DigitalObject>
            <DigitalObject DigitalObjectCatalogID="DOC_2"><CLONE_ID value="1"/></DigitalObject>
          </CardSet>"#
                ))
                .unwrap(),
            )
            .unwrap();
        for id in [1, 2] {
            assert_eq!(
                resolver
                    .for_object(&database.object(id).unwrap())
                    .unwrap()
                    .path(),
                format!("/patch/mtg/live/Graphics/Avatars/Regular/{id:05}c_avatar.jpg")
            );
        }
    }
}

/// Avatar routing preserves client placeholders and fails closed on malformed typed fields.
#[test]
fn avatar_lookup_rejects_ambiguous_ids_flags_and_noncard_markers() {
    let resolver = ArtworkResolver::from_config(Url::parse(CONFIG_URL).unwrap(), CONFIG).unwrap();
    for fields in [
        r#"<ARTID value="-1"/><IS_VANGUARD/>"#,
        r#"<ARTID value="2147483648"/><IS_VANGUARD/>"#,
        r#"<ARTID value="invalid"/><IS_VANGUARD/>"#,
        r#"<ARTID value=""/><IS_VANGUARD/>"#,
        r#"<ARTID value="1000129"/><IS_VANGUARD value="true"/>"#,
        r#"<ARTID value="1000129"/><IS_VANGUARD><INVALID/></IS_VANGUARD>"#,
        r#"<ARTID value="1000129"/><IS_VANGUARD/><MTGO_CLIENT_NON_CARD/>"#,
        r#"<ARTID value="1000129"/><IS_VANGUARD/><IS_DIGITALOBJECT value="0"/>"#,
        r#"<ARTID value="1000129"/>"#,
        r#"<ARTID value="1000129"/><IS_VANGUARD value="0"/>"#,
        r#"<ARTID value="1000129"/><IS_VANGUARD value="-1"/>"#,
    ] {
        // Frame 11 alone cannot establish avatar semantics or repair invalid source values.
        let database = database(&format!(
            r#"<DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/><FRAMESTYLE value="11"/>{fields}"#
        ));
        assert!(
            resolver.for_object(&database.object(1).unwrap()).is_none(),
            "{fields}"
        );
    }
    for flag in ["0", "-1"] {
        // Native integer booleans treat nonpositive values as ordinary cards.
        let database = database(&format!(
            r#"<ARTID value="23"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/><FRAMESTYLE value="3"/><IS_VANGUARD value="{flag}"/>"#
        ));
        assert_eq!(
            resolver
                .for_object(&database.object(1).unwrap())
                .unwrap()
                .path(),
            "/patch/mtg/live/Graphics/Cards/Pics/00023_typ_reg_sty_010.jpg"
        );
    }
}

/// Optional avatar settings preserve card-only configurations and literal URL segments.
#[test]
fn avatar_directory_is_optional_and_resolves_relative_roots_and_queries() {
    let card_only = configured("https://art.example/Graphics", "Cards/Pics").unwrap();
    assert!(card_only.avatar_base_url().is_none());
    let database = database(
        r#"<ARTID value="23"/><IS_VANGUARD/><FRAMESTYLE value="11"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>"#,
    );
    assert!(card_only.for_object(&database.object(1).unwrap()).is_none());
    let resolver = configured_with_avatars(
        "../Graphics?token=test",
        r#"<avatarArtSubPath value="Avatars\Regular Portraits"/>"#,
    )
    .unwrap();
    assert_eq!(
        resolver
            .for_object(&database.object(1).unwrap())
            .unwrap()
            .as_str(),
        "https://patch.example/client/Graphics/Avatars/Regular%20Portraits/00023_avatar.jpg?token=test"
    );
}

/// Optional avatar configuration must still be unambiguous and a relative directory.
#[test]
fn rejects_invalid_avatar_directories_and_qualified_values() {
    for settings in [
        r#"<avatarArtSubPath value="Avatars/Regular"/><avatarArtSubPath value="Other"/>"#,
        r#"<avatarArtSubPath/>"#,
        r#"<avatarArtSubPath value=""/>"#,
        r#"<avatarArtSubPath xmlns:other="urn:other" other:value="Avatars/Regular"/>"#,
        r#"<avatarArtSubPath value="/Avatars/Regular"/>"#,
        r#"<avatarArtSubPath value="../Avatars/Regular"/>"#,
        r#"<avatarArtSubPath value="Avatars//Regular"/>"#,
        r#"<avatarArtSubPath value="https://other.example"/>"#,
    ] {
        assert!(
            configured_with_avatars("https://art.example/Graphics", settings).is_err(),
            "{settings}"
        );
    }
}

/// The known Mystical Tutor artwork uses ARTID rather than the foil's texture number.
#[test]
fn modern_artwork_uses_inherited_art_id_and_ignores_texture_number() {
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

/// The classic Blue Elemental Blast and its foil clone use the client's classic crop.
#[test]
fn classic_artwork_and_foil_clone_use_the_client_selected_suffix() {
    let application = application();
    let mut database = application.card_database();
    database
        .insert(
            &application,
            CardDataFile::parse(
                r#"<CardSet id="1E">
      <DigitalObject DigitalObjectCatalogID="DOC_357">
        <ARTID value="23"/><FRAMESTYLE value="1"/><COLOR value="COLOR_BLUE"/>
        <DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/><CARDTEXTURE_NUMBER value="714"/>
      </DigitalObject>
      <DigitalObject DigitalObjectCatalogID="DOC_358">
        <CLONE_ID value="357"/><IS_FOIL/><CARDTEXTURE_NUMBER value="717"/>
      </DigitalObject>
    </CardSet>"#,
            )
            .unwrap(),
        )
        .unwrap();
    let resolver = configured("https://art.example/Graphics", "Cards/Pics").unwrap();
    for id in [357, 358] {
        // Inherited artwork uses the base frame, not a texture-number or foil variant.
        assert_eq!(
            resolver
                .for_object(&database.object(id).unwrap())
                .unwrap()
                .as_str(),
            "https://art.example/Graphics/Cards/Pics/00023_typ_reg_sty_001.jpg"
        );
    }
    let database = self::database(
        r#"<ARTID value="25533"/><FRAMESTYLE value="1"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="SUBC"/>"#,
    );
    assert_eq!(
        resolver
            .for_object(&database.object(1).unwrap())
            .unwrap()
            .as_str(),
        "https://art.example/Graphics/Cards/Pics/25533_typ_reg_sty_001.jpg"
    );
}

/// Distinct frame families select distinct client asset names for the same art ID.
#[test]
fn frame_families_select_their_own_artwork_variants() {
    for (frame, suffix) in [
        (3, "_typ_reg_sty_010"),
        (31, "_typ_reg_sty_010"),
        (5, "_typ_reg_sty_050"),
        (7, "_typ_reg_sty_020"),
        (9, "_typ_reg_sty_010"),
        (10, "_typ_planeswk_sty_040"),
        (13, "_typ_reg_sty_050"),
        (36, "_typ_old_sty_050"),
        (53, "_typ_reg_sty_050"),
        (55, "_typ_reg_sty_030"),
        (76, "_typ_reg_sty_050"),
        (164, "_typ_dsr_sty_010"),
        (165, "_typ_mfg_sty_030"),
        (174, "_typ_otj_sty_010"),
        (187, "_typ_blb_wl_sty_010"),
        (190, "_typ_dsk_dee_sty_010"),
        (207, "_type_inr_hrvst_planeswk_sty_050"),
    ] {
        // Expected families are independent examples of the client's selector branches.
        assert_art_suffix("TEST", &format!(r#"<FRAMESTYLE value="{frame}"/>"#), suffix);
    }
}

/// The classic dual-land exception uses the declaring set, not a resolved display alias.
#[test]
fn classic_dual_land_sort_exception_uses_the_source_set() {
    let frame = r#"<FRAMESTYLE value="2"/>"#;
    assert_art_suffix("TEST", frame, "_typ_reg_sty_001");
    assert_art_suffix("Sort", frame, "_typ_reg_sty_010");
    assert_art_suffix(
        "TEST",
        &format!(r#"{frame}<CARDSETNAME_STRING value="Sort"/>"#),
        "_typ_reg_sty_001",
    );
    assert_art_suffix(
        "Sort",
        &format!(r#"{frame}<CARDSETNAME_STRING value="TEST"/>"#),
        "_typ_reg_sty_010",
    );
}

/// Colorless nonartifacts select the wider extended-art crop; artifact and land flags opt out.
#[test]
fn color_and_boolean_properties_select_conditional_artwork() {
    let frame = r#"<FRAMESTYLE value="62"/><COLOR value="COLOR_COLORLESS"/>"#;
    assert_art_suffix("TEST", frame, "_typ_reg_sty_035");
    for (property, suffix) in [
        (r#"<IS_ARTIFACT/>"#, "_typ_reg_sty_030"),
        (r#"<IS_ARTIFACT value="1"/>"#, "_typ_reg_sty_030"),
        (r#"<IS_ARTIFACT value="2"/>"#, "_typ_reg_sty_030"),
        (r#"<IS_ARTIFACT value="0"/>"#, "_typ_reg_sty_035"),
        (r#"<IS_LAND/>"#, "_typ_reg_sty_030"),
        (r#"<IS_LAND value="0"/>"#, "_typ_reg_sty_035"),
    ] {
        // Presence and positive integer flags are true, while explicit zero stays false.
        assert_art_suffix("TEST", &format!("{frame}{property}"), suffix);
    }
    assert_art_suffix(
        "TEST",
        r#"<FRAMESTYLE value="62"/><COLOR value="COLOR_BLUE"/>"#,
        "_typ_reg_sty_030",
    );
    assert_art_suffix(
        "TEST",
        r#"<FRAMESTYLE value="80"/><COLOR value="COLOR_COLORLESS"/>"#,
        "_typ_reg_sty_050",
    );
    assert_art_suffix(
        "TEST",
        r#"<FRAMESTYLE value="163"/><COLOR value="COLOR_COLORLESS"/>"#,
        "_typ_case_sty_050",
    );
    assert_art_suffix(
        "TEST",
        r#"<FRAMESTYLE value="163"/><COLOR value="COLOR_BLUE"/>"#,
        "_typ_case_sty_010",
    );
}

/// Flip artwork depends on the client's exact color category and a true flip flag.
#[test]
fn flip_artwork_preserves_color_and_flag_semantics() {
    for color in ["COLOR_WHITE", "COLOR_BLUE", "COLOR_BLACK", "COLOR_GREEN"] {
        // Red, colorless, and multicolor are deliberately outside this client branch.
        assert_art_suffix(
            "TEST",
            &format!(r#"<FRAMESTYLE value="3"/><COLOR value="{color}"/><FLIP_CARD/>"#),
            "_typ_flip_sty_013",
        );
    }
    for color in ["COLOR_RED", "COLOR_COLORLESS", "COLOR_WHITE|COLOR_BLUE"] {
        assert_art_suffix(
            "TEST",
            &format!(r#"<FRAMESTYLE value="3"/><COLOR value="{color}"/><FLIP_CARD/>"#),
            "_typ_reg_sty_010",
        );
    }
    assert_art_suffix(
        "TEST",
        r#"<FRAMESTYLE value="3"/><COLOR value="COLOR_BLUE"/><FLIP_CARD value="1"/>"#,
        "_typ_flip_sty_013",
    );
    assert_art_suffix(
        "TEST",
        r#"<FRAMESTYLE value="3"/><COLOR value="COLOR_BLUE"/><FLIP_CARD value="0"/>"#,
        "_typ_reg_sty_010",
    );
}

/// Presence-based split metadata overrides the declared frame even when its value is zero.
#[test]
fn split_properties_override_the_frame_and_undefined_frames_use_post_m15() {
    for property in [
        r#"<FUSE/>"#,
        r#"<FUSE value="0"/>"#,
        r#"<SPLITCARD0 value="0"/>"#,
    ] {
        // The client tests presence here rather than interpreting these as booleans.
        assert_art_suffix(
            "TEST",
            &format!(r#"<FRAMESTYLE value="1"/>{property}"#),
            "_typ_reg_sty_010",
        );
    }
    for fields in [
        "",
        r#"<FRAMESTYLE value="0"/>"#,
        r#"<FRAMESTYLE value="38"/>"#,
        r#"<FRAMESTYLE value="9999"/>"#,
    ] {
        assert_art_suffix("TEST", fields, "_typ_reg_sty_010");
    }
}

/// Explicit noncard markers, artless frames, and unconfigured avatars have no artwork.
#[test]
fn noncards_artless_frames_and_unconfigured_avatars_have_no_artwork() {
    for fields in [
        r#"<FRAMESTYLE value="3"/><MTGO_CLIENT_NON_CARD/>"#,
        r#"<FRAMESTYLE value="3"/><MTGO_CLIENT_NON_CARD value="1"/>"#,
        r#"<FRAMESTYLE value="90"/>"#,
        r#"<FRAMESTYLE value="3"/><IS_VANGUARD/>"#,
        r#"<FRAMESTYLE value="3"/><IS_VANGUARD value="1"/>"#,
        r#"<FRAMESTYLE value="3"/><IS_DIGITALOBJECT/>"#,
        r#"<FRAMESTYLE value="3"/><IS_DIGITALOBJECT value="0"/>"#,
    ] {
        // A valid art ID cannot override client properties which suppress card artwork.
        assert!(card_art_path("TEST", fields).is_none(), "{fields}");
    }
    assert_art_suffix(
        "TEST",
        r#"<FRAMESTYLE value="1"/><MTGO_CLIENT_NON_CARD value="0"/>"#,
        "_typ_reg_sty_001",
    );
    assert_art_suffix(
        "TEST",
        r#"<FRAMESTYLE value="3"/><IS_VANGUARD value="0"/>"#,
        "_typ_reg_sty_010",
    );
}

/// Malformed properties suppress a derived URL instead of guessing a client value.
#[test]
fn malformed_selection_properties_have_no_artwork() {
    for fields in [
        r#"<FRAMESTYLE value="80"/><IS_ARTIFACT value="true"/>"#,
        r#"<FRAMESTYLE value="80"/><IS_LAND><INVALID value="1"/></IS_LAND>"#,
        r#"<FRAMESTYLE value="80"/><COLOR value="COLOR_FUTURE"/>"#,
        r#"<FRAMESTYLE value="not-a-number"/>"#,
    ] {
        // These values cannot select a reliable filename through the client's typed properties.
        assert!(card_art_path("TEST", fields).is_none(), "{fields}");
    }
}

/// Dungeon text is trimmed and includes room rules before token art is selected.
#[test]
fn dungeon_room_text_affects_token_artwork() {
    let frame = r#"<FRAMESTYLE value="32"/><REAL_ORACLETEXT_STRING value="  "/>"#;
    for (extra, suffix) in [
        ("", "_typ_tokwtb_sty_011"),
        ("<IS_DUNGEON/>", "_typ_tokntb_sty_012"),
        (
            r#"<IS_DUNGEON/><TRACKER_STATE_LIST0><TRACKER_STATE_RULES_TEXT_STRING value="Room ability"/></TRACKER_STATE_LIST0>"#,
            "_typ_tokwtb_sty_011",
        ),
        (
            r#"<IS_DUNGEON/><TRACKER_STATE_LIST0><TRACKER_STATE_RULES_TEXT_STRING value="  "/></TRACKER_STATE_LIST0>"#,
            "_typ_tokntb_sty_012",
        ),
    ] {
        // Whitespace-only dungeon text differs from a token's untrimmed rules string.
        assert_art_suffix("TEST", &format!("{frame}{extra}"), suffix);
    }
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
        "2147483648",
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
