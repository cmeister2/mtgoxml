//! Offline coverage for CardDataSource parsing, joins, and clone inheritance.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::sync::atomic::{AtomicUsize, Ordering};

use mtgoxml::carddata::{
    CardDataFile, CardDatabase, CardSet, DigitalObject, Error, FieldValue, ParseOptions,
    ResolvedValue,
};
use mtgoxml::{Application, Deployment};

/// Captured set records retain one complete card and its minimal foil clone.
const LIVE_SET: &[u8] = include_bytes!("../fixtures/carddata/live/client_V09.xml");
/// A captured noncard record demonstrates the wider digital-object namespace.
const LIVE_OBJECTS: &[u8] = include_bytes!("../fixtures/carddata/live/client_10E_DO.xml");
/// Minimal captured lookup rows supply every reference in the selected objects.
const LIVE_LOOKUPS: &[&[u8]] = &[
    include_bytes!("../fixtures/carddata/live/ARTIST_NAME_STRING.xml"),
    include_bytes!("../fixtures/carddata/live/CARDNAME_STRING.xml"),
    include_bytes!("../fixtures/carddata/live/CARDNAME_TOKEN.xml"),
    include_bytes!("../fixtures/carddata/live/CARDSETNAME_STRING.xml"),
    include_bytes!("../fixtures/carddata/live/COLOR.xml"),
    include_bytes!("../fixtures/carddata/live/COLOR_IDENTITY.xml"),
    include_bytes!("../fixtures/carddata/live/CONVERTED_MANA_COST.xml"),
    include_bytes!("../fixtures/carddata/live/FLAVORTEXT_STRING.xml"),
    include_bytes!("../fixtures/carddata/live/MANA_COST_STRING.xml"),
    include_bytes!("../fixtures/carddata/live/PROMO_LABEL_STRING.xml"),
    include_bytes!("../fixtures/carddata/live/RARITY_STATUS.xml"),
    include_bytes!("../fixtures/carddata/live/REAL_ORACLETEXT_STRING.xml"),
];

/// Build a complete synthetic deployment without relying on a mutable service.
fn application_at(url: &str, version: &str) -> Application {
    let deployment = format!(
        r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
        <assemblyIdentity name="MTGO.application" version="{version}" />
        <deployment /><dependency><dependentAssembly dependencyType="install" codebase="build/MTGO.exe.manifest">
        <assemblyIdentity name="MTGO.exe" version="{version}" />
        </dependentAssembly></dependency></assembly>"#
    );
    let manifest = format!(
        r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
        <assemblyIdentity name="MTGO.exe" version="{version}" /><application /></assembly>"#
    );
    Deployment::from_bytes(url, deployment.as_bytes())
        .unwrap()
        .with_application_bytes(manifest.as_bytes())
        .unwrap()
}

/// Use one stable deployment snapshot for ordinary insertion and resolution tests.
fn source() -> Application {
    application_at(
        "https://patch.example.test/live/MTGO.application",
        "1.2.3.4",
    )
}

/// Parse a synthetic set with caller-controlled digital-object XML.
fn set(code: &str, objects: &str) -> CardDataFile {
    CardDataFile::parse(&format!(
        r#"<CardSet id="{code}" age="7" cardsetType="PAPER">{objects}</CardSet>"#
    ))
    .unwrap()
}

/// Construct a digital object while keeping test IDs explicit at call sites.
fn object(id: u64, fields: &str) -> String {
    format!(r#"<DigitalObject DigitalObjectCatalogID="DOC_{id}">{fields}</DigitalObject>"#)
}

/// Parse one named lookup table from explicit ID and value rows.
fn lookup(name: &str, rows: &str) -> CardDataFile {
    CardDataFile::parse(&format!("<{name}_ITEMS>{rows}</{name}_ITEMS>")).unwrap()
}

/// A reader that can supply unlimited bytes while recording how many were read.
struct CountingReader<'a> {
    /// Total bytes returned to the parser rather than merely requested.
    read: &'a AtomicUsize,
}

impl Read for CountingReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        buffer.fill(b'x');
        self.read.fetch_add(buffer.len(), Ordering::Relaxed);
        Ok(buffer.len())
    }
}

/// Raw records retain each distinct field form and preserve unfamiliar names.
#[test]
fn raw_set_preserves_references_literals_flags_and_empty_overrides() {
    let CardDataFile::Set(set) = set(
        "EXAMPLE",
        &object(
            42,
            r#"<CARDNAME_STRING id="opaque_name" />
            <CARDTEXTURE_NUMBER value="84" /><IS_FOIL />
            <FUTURE_PROPERTY value="" />"#,
        ),
    ) else {
        panic!("expected a set");
    };
    assert_eq!(set.id, "EXAMPLE");
    assert_eq!(set.age, Some(7));
    assert_eq!(set.kind.as_deref(), Some("PAPER"));
    let item = &set.objects[0];
    assert_eq!(item.id, 42);
    assert_eq!(
        item.fields["CARDNAME_STRING"],
        FieldValue::Reference("opaque_name".into())
    );
    assert_eq!(
        item.fields["CARDTEXTURE_NUMBER"],
        FieldValue::Literal("84".into())
    );
    assert_eq!(item.fields["IS_FOIL"], FieldValue::Flag);
    assert_eq!(
        item.fields["FUTURE_PROPERTY"],
        FieldValue::Literal(String::new())
    );
    assert!(!item.fields.contains_key("ABSENT_PROPERTY"));
}

/// Lookup text preserves whitespace, Unicode, escaped syntax, and split text nodes.
#[test]
fn lookup_keeps_decoded_text_without_interpreting_mtgo_markup() {
    let CardDataFile::Lookup(table) = CardDataFile::parse(
        r#"<TEXT_ITEMS><TEXT_ITEM id="opaque">  @iÉowyn &amp; Sam@i\n{R}#2b- <![CDATA[<literal>]]> tail </TEXT_ITEM><TEXT_ITEM id="empty" /></TEXT_ITEMS>"#,
    )
    .unwrap() else {
        panic!("expected a lookup table");
    };
    assert_eq!(table.name, "TEXT");
    assert_eq!(
        table.entries["opaque"],
        "  @iÉowyn & Sam@i\\n{R}#2b- <literal> tail "
    );
    assert_eq!(table.entries["empty"], "");
}

/// Byte decoding accepts the live UTF-8 format without replacing malformed bytes.
#[test]
fn byte_parser_accepts_utf8_bom_and_rejects_bad_unicode_or_encoding() {
    let xml = b"<N_ITEMS><N_ITEM id=\"n\">name</N_ITEM></N_ITEMS>";
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice(xml);
    assert!(CardDataFile::from_bytes(&bytes).is_ok());
    assert!(matches!(
        CardDataFile::from_bytes(b"<N_ITEMS>\xff</N_ITEMS>"),
        Err(Error::Encoding(_))
    ));
    for declaration in ["UTF-16", "windows-1252"] {
        let xml = format!(r#"<?xml version="1.0" encoding="{declaration}"?><N_ITEMS />"#);
        assert!(matches!(
            CardDataFile::from_bytes(xml.as_bytes()),
            Err(Error::Encoding(_))
        ));
    }
}

/// Bounded reads stop after the configured limit plus one detecting byte.
#[test]
fn streaming_input_stops_at_the_byte_budget() {
    let count = AtomicUsize::new(0);
    let result = ParseOptions {
        max_input_bytes: 64,
        max_nodes: 250_000,
    }
    .from_reader(CountingReader { read: &count });
    assert!(matches!(result, Err(Error::InputTooLarge { limit: 64 })));
    assert_eq!(count.load(Ordering::Relaxed), 65);
}

/// XML nodes have their own budget even when the encoded input is small.
#[test]
fn node_budget_is_enforced_independently_of_byte_limit() {
    let xml = "<N_ITEMS><N_ITEM id=\"one\" /><N_ITEM id=\"two\" /></N_ITEMS>";
    let result = ParseOptions {
        max_input_bytes: 1_024,
        max_nodes: 2,
    }
    .parse(xml);
    assert!(result.is_err());
    assert!(CardDataFile::parse(xml).is_ok());
}

/// Duplicate data cannot silently overwrite earlier source records.
#[test]
fn duplicate_objects_fields_and_lookup_rows_are_rejected() {
    let duplicate_objects = format!(
        "<CardSet id=\"DUP\">{}{}</CardSet>",
        object(1, ""),
        object(1, "")
    );
    let duplicate_fields = format!(
        "<CardSet id=\"DUP\">{}</CardSet>",
        object(1, "<F value=\"one\" /><F value=\"two\" />")
    );
    for xml in [
        duplicate_objects.as_str(),
        duplicate_fields.as_str(),
        r#"<N_ITEMS><N_ITEM id="same">first</N_ITEM><N_ITEM id="same">second</N_ITEM></N_ITEMS>"#,
    ] {
        assert!(matches!(
            CardDataFile::parse(xml),
            Err(Error::Duplicate { .. })
        ));
    }
}

/// Structural mistakes are distinguished from unfamiliar but valid field names.
#[test]
fn ambiguous_field_values_and_wrong_lookup_items_are_rejected() {
    for fields in [
        r#"<FIELD id="ref" value="literal" />"#,
        r#"<FIELD value="literal"><nested /></FIELD>"#,
        r#"<FIELD id="ref"><nested /></FIELD>"#,
        "<FIELD>unexpected text<nested /></FIELD>",
        "<FIELD>unexpected text</FIELD>",
    ] {
        let xml = format!("<CardSet id=\"BAD\">{}</CardSet>", object(1, fields));
        assert!(CardDataFile::parse(&xml).is_err());
    }
    assert!(CardDataFile::parse("<A_ITEMS><B_ITEM id=\"id\">text</B_ITEM></A_ITEMS>").is_err());
    assert!(
        CardDataFile::parse("<A_ITEMS><A_ITEM id=\"id\"><nested /></A_ITEM></A_ITEMS>").is_err()
    );
}

/// Property groups retain child field names and every supported leaf form.
#[test]
fn nested_property_groups_preserve_unknown_fields_and_value_forms() {
    let CardDataFile::Set(set) = set(
        "NESTED",
        &object(
            1,
            r#"<PROTOTYPE_ATTRIBUTES_LIST><COLOR id="red" />
            <POWER value="3" /><FUTURE_PROPERTY value="" /><IS_NEW />
            <FURTHER_GROUP><UNKNOWN id="opaque" /></FURTHER_GROUP>
            </PROTOTYPE_ATTRIBUTES_LIST>"#,
        ),
    ) else {
        panic!("expected a set");
    };
    let FieldValue::Fields(fields) = &set.objects[0].fields["PROTOTYPE_ATTRIBUTES_LIST"] else {
        panic!("expected a property group");
    };
    assert_eq!(fields["COLOR"], FieldValue::Reference("red".into()));
    assert_eq!(fields["POWER"], FieldValue::Literal("3".into()));
    assert_eq!(
        fields["FUTURE_PROPERTY"],
        FieldValue::Literal(String::new())
    );
    assert_eq!(fields["IS_NEW"], FieldValue::Flag);
    let FieldValue::Fields(inner) = &fields["FURTHER_GROUP"] else {
        panic!("expected another property group");
    };
    assert_eq!(inner["UNKNOWN"], FieldValue::Reference("opaque".into()));
}

/// Nested references join their own named lookup tables at every property level.
#[test]
fn nested_references_resolve_without_flattening_group_structure() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    database
        .insert(
            &application,
            set(
                "NESTED",
                &object(
                    1,
                    r#"<GROUP><CARDNAME_STRING id="name" />
                    <INNER><COLOR id="red" /><IS_NEW /><EMPTY value="" /></INNER>
                    </GROUP>"#,
                ),
            ),
        )
        .unwrap();
    database
        .insert(
            &application,
            lookup(
                "CARDNAME_STRING",
                r#"<CARDNAME_STRING_ITEM id="name">Nested name</CARDNAME_STRING_ITEM>"#,
            ),
        )
        .unwrap();
    assert!(
        matches!(database.object(1), Err(Error::MissingLookup { table, id }) if table == "COLOR" && id == "red")
    );
    database
        .insert(
            &application,
            lookup("COLOR", r#"<COLOR_ITEM id="red">COLOR_RED</COLOR_ITEM>"#),
        )
        .unwrap();
    let item = database.object(1).unwrap();
    let Some(ResolvedValue::Fields(fields)) = item.field("GROUP") else {
        panic!("expected a resolved property group");
    };
    assert_eq!(
        fields["CARDNAME_STRING"],
        ResolvedValue::Text("Nested name")
    );
    let ResolvedValue::Fields(inner) = &fields["INNER"] else {
        panic!("expected a nested resolved group");
    };
    assert_eq!(inner["COLOR"], ResolvedValue::Text("COLOR_RED"));
    assert_eq!(inner["IS_NEW"], ResolvedValue::Flag);
    assert_eq!(inner["EMPTY"], ResolvedValue::Text(""));
    assert_eq!(item.text("GROUP"), None);
    assert_eq!(item.name(), None);
    database.validate().unwrap();
}

/// A child property group replaces its parent's complete group, including references.
#[test]
fn clone_groups_replace_whole_lists_instead_of_merging_child_fields() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    let base = object(
        1,
        r#"<LIST><NEVER_LOADED id="missing" /><OLD value="base" /></LIST><TOP value="retained" />"#,
    );
    let child = object(
        2,
        r#"<CLONE_ID value="1" /><LIST><NEW value="child" /></LIST>"#,
    );
    database
        .insert(&application, set("NESTED", &(base + &child)))
        .unwrap();
    let item = database.object(2).unwrap();
    let Some(ResolvedValue::Fields(fields)) = item.field("LIST") else {
        panic!("expected a resolved list");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields["NEW"], ResolvedValue::Text("child"));
    assert!(!fields.contains_key("OLD"));
    assert!(!fields.contains_key("NEVER_LOADED"));
    assert_eq!(item.text("TOP"), Some("retained"));
}

/// Each nested scope independently rejects duplicate child-property names.
#[test]
fn duplicate_child_properties_are_rejected_without_overwriting_values() {
    let xml = format!(
        "<CardSet id=\"BAD\">{}</CardSet>",
        object(
            1,
            "<GROUP><INNER><F value=\"first\" /><F value=\"second\" /></INNER></GROUP>"
        )
    );
    assert!(matches!(CardDataFile::parse(&xml), Err(Error::Duplicate { key, .. }) if key == "F"));
}

/// The structural depth limit admits the boundary and rejects deeper property groups.
#[test]
fn nested_group_depth_is_bounded_independently_of_xml_node_limit() {
    for (depth, accepted) in [(64, true), (65, false)] {
        let mut fields = "<LEAF value=\"value\" />".to_owned();
        for level in 0..depth {
            fields = format!("<G{level}>{fields}</G{level}>");
        }
        let xml = format!("<CardSet id=\"DEPTH\">{}</CardSet>", object(1, &fields));
        let result = CardDataFile::parse(&xml);
        if accepted {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(Error::InvalidData(_))));
        }
    }
}

/// Public raw models cannot bypass the resolver's bound on recursive groups.
#[test]
fn manually_built_nested_fields_still_obey_the_resolution_depth_limit() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    let mut value = FieldValue::Flag;
    for level in 0..65 {
        value = FieldValue::Fields(BTreeMap::from([(format!("N{level}"), value)]));
    }
    database
        .insert(
            &application,
            CardDataFile::Set(CardSet {
                id: "MANUAL".into(),
                age: None,
                kind: None,
                objects: vec![DigitalObject {
                    id: 1,
                    fields: BTreeMap::from([("GROUP".into(), value)]),
                }],
            }),
        )
        .unwrap();
    assert!(matches!(database.object(1), Err(Error::InvalidData(_))));
}

/// A current publisher record demonstrates prototype attributes as a nested group.
#[test]
fn captured_prototype_record_retains_its_nested_attribute_list() {
    let CardDataFile::Set(set) =
        CardDataFile::from_bytes(include_bytes!("../fixtures/carddata/nested/client_BRO.xml"))
            .unwrap()
    else {
        panic!("expected a set");
    };
    assert_eq!(set.id, "BRO");
    let item = &set.objects[0];
    assert_eq!(item.id, 104_554);
    assert_eq!(item.fields["POWER"], FieldValue::Literal("8".into()));
    let FieldValue::Fields(fields) = &item.fields["PROTOTYPE_ATTRIBUTES_LIST"] else {
        panic!("expected a prototype property group");
    };
    assert_eq!(fields["POWER"], FieldValue::Literal("3".into()));
    assert_eq!(fields["TOUGHNESS"], FieldValue::Literal("3".into()));
    assert_eq!(fields["COLOR"], FieldValue::Reference("ID1286_14".into()));
    assert_eq!(
        fields["MANA_COST_STRING"],
        FieldValue::Reference("ID2327_251".into())
    );
}

/// Component parsing never enables the master index's external-entity mechanism.
#[test]
fn unsupported_roots_dtds_and_malformed_components_are_rejected() {
    assert!(matches!(
        CardDataFile::parse("<DOCUMENT />"),
        Err(Error::UnexpectedRoot(_))
    ));
    for xml in [
        "<CardSet>",
        r#"<!DOCTYPE N_ITEMS [<!ENTITY x SYSTEM "file:///not-read">]><N_ITEMS><N_ITEM id="id">&x;</N_ITEM></N_ITEMS>"#,
        r#"<!DOCTYPE N_ITEMS [<!ENTITY x "expanded">]><N_ITEMS><N_ITEM id="id">&x;</N_ITEM></N_ITEMS>"#,
    ] {
        assert!(CardDataFile::parse(xml).is_err());
    }
}

/// Digital object IDs must retain their numeric identity without truncation.
#[test]
fn invalid_or_missing_catalog_ids_and_lookup_ids_are_rejected() {
    for id in ["1", "DOC_", "DOC_-1", "DOC_18446744073709551616"] {
        let xml = format!(
            r#"<CardSet id="BAD"><DigitalObject DigitalObjectCatalogID="{id}" /></CardSet>"#
        );
        assert!(CardDataFile::parse(&xml).is_err());
    }
    assert!(CardDataFile::parse("<CardSet id=\"BAD\"><DigitalObject /></CardSet>").is_err());
    assert!(CardDataFile::parse("<N_ITEMS><N_ITEM>missing</N_ITEM></N_ITEMS>").is_err());
}

/// Opaque IDs and field names determine lookup joins, never row position.
#[test]
fn references_resolve_by_table_and_id_with_empty_values_preserved() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    database
        .insert(
            &application,
            set(
                "JOIN",
                &object(
                    10,
                    r#"<CARDNAME_STRING id="id_900" /><TYPE id="id_4" />
                    <FUTURE_PROPERTY id="id_4" /><EMPTY id="empty" />"#,
                ),
            ),
        )
        .unwrap();
    for (name, rows) in [
        (
            "CARDNAME_STRING",
            r#"<CARDNAME_STRING_ITEM id="id_1">Wrong</CARDNAME_STRING_ITEM><CARDNAME_STRING_ITEM id="id_900">Right</CARDNAME_STRING_ITEM>"#,
        ),
        ("TYPE", r#"<TYPE_ITEM id="id_4">Internal type</TYPE_ITEM>"#),
        (
            "FUTURE_PROPERTY",
            r#"<FUTURE_PROPERTY_ITEM id="id_4">Different table</FUTURE_PROPERTY_ITEM>"#,
        ),
        ("EMPTY", r#"<EMPTY_ITEM id="empty" />"#),
    ] {
        database.insert(&application, lookup(name, rows)).unwrap();
    }
    let item = database.object(10).unwrap();
    assert_eq!(item.name(), Some("Right"));
    assert!(matches!(
        item.field("TYPE"),
        Some(ResolvedValue::Text("Internal type"))
    ));
    assert!(matches!(
        item.field("FUTURE_PROPERTY"),
        Some(ResolvedValue::Text("Different table"))
    ));
    assert!(matches!(item.field("EMPTY"), Some(ResolvedValue::Text(""))));
    assert!(item.field("ABSENT").is_none());
    assert_eq!(item.set_code(), "JOIN");
    database.validate().unwrap();
}

/// Clone chains can span files and load in any order, with nearest overrides winning.
#[test]
fn multihop_clones_merge_across_set_fragments_and_keep_raw_sources() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    let child = object(3, r#"<CLONE_ID value="2" /><IS_FOIL /><NOTES value="" />"#);
    let middle = object(
        2,
        r#"<CLONE_ID value="1" /><NOTES value="middle" /><ARTID value="22" />"#,
    );
    let base = object(
        1,
        r#"<CARDNAME_STRING value="Inherited name" /><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD" /><NOTES value="base" /><ARTID value="11" />"#,
    );
    for file in [
        set("CHILD", &child),
        set("BASE", &base),
        set("CHILD", &middle),
    ] {
        database.insert(&application, file).unwrap();
    }
    let card = database.card(3).unwrap();
    assert_eq!(card.name(), Some("Inherited name"));
    assert_eq!(card.text("ARTID"), Some("22"));
    assert_eq!(card.text("NOTES"), Some(""));
    assert!(card.is_foil().unwrap());
    assert_eq!(card.raw().id, 3);
    assert!(!card.raw().fields.contains_key("CARDNAME_STRING"));
    assert_eq!(card.set().id, "CHILD");
    assert_eq!(database.len(), 3);
    assert_eq!(database.sets().count(), 3);
    assert_eq!(database.object_ids().collect::<Vec<_>>(), [1, 2, 3]);
    database.validate().unwrap();
}

/// A missing lookup in an overridden base field is irrelevant to the child result.
#[test]
fn resolution_checks_only_effective_inherited_fields() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    let base = object(1, r#"<CARDNAME_STRING id="not-loaded" />"#);
    let child = object(
        2,
        r#"<CLONE_ID value="1" /><CARDNAME_STRING value="Override" />"#,
    );
    database
        .insert(&application, set("ONE", &(base + &child)))
        .unwrap();
    assert!(matches!(
        database.object(1),
        Err(Error::MissingLookup { .. })
    ));
    assert_eq!(database.object(2).unwrap().name(), Some("Override"));
}

/// Lookup failures expose the actual table and opaque ID rather than an empty value.
#[test]
fn missing_lookup_table_and_missing_row_are_explicit_errors() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    database
        .insert(
            &application,
            set("ONE", &object(1, r#"<CARDNAME_STRING id="wanted" />"#)),
        )
        .unwrap();
    assert!(
        matches!(database.object(1), Err(Error::MissingLookup { table, id }) if table == "CARDNAME_STRING" && id == "wanted")
    );
    database
        .insert(
            &application,
            lookup(
                "CARDNAME_STRING",
                r#"<CARDNAME_STRING_ITEM id="different">Other</CARDNAME_STRING_ITEM>"#,
            ),
        )
        .unwrap();
    assert!(
        matches!(database.object(1), Err(Error::MissingLookup { table, id }) if table == "CARDNAME_STRING" && id == "wanted")
    );
    assert!(database.validate().is_err());
}

/// Missing base objects remain distinguishable from missing requested objects.
#[test]
fn unknown_objects_and_missing_clone_bases_report_the_missing_id() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    assert!(matches!(database.object(99), Err(Error::MissingObject(99))));
    database
        .insert(
            &application,
            set("ONE", &object(1, r#"<CLONE_ID value="88" />"#)),
        )
        .unwrap();
    assert!(matches!(database.object(1), Err(Error::MissingObject(88))));
}

/// Inheritance cycles report participating IDs instead of recursing indefinitely.
#[test]
fn self_and_multihop_clone_cycles_are_detected() {
    for pairs in [&[(1, 1)][..], &[(1, 2), (2, 3), (3, 2)][..]] {
        let application = source();
        let mut database = CardDatabase::new(&application);
        let objects: String = pairs
            .iter()
            .map(|(id, base)| object(*id, &format!(r#"<CLONE_ID value="{base}" />"#)))
            .collect();
        database
            .insert(&application, set("CYCLE", &objects))
            .unwrap();
        assert!(
            matches!(database.object(1), Err(Error::CloneCycle(ids)) if ids.contains(&pairs.last().unwrap().0))
        );
    }
}

/// Clone IDs cannot silently coerce malformed values or presence-only flags.
#[test]
fn invalid_clone_values_are_rejected_during_resolution() {
    for field in [
        r#"<CLONE_ID value="not-a-number" />"#,
        r#"<CLONE_ID value="-1" />"#,
        r#"<CLONE_ID value="18446744073709551616" />"#,
        "<CLONE_ID />",
        r#"<CLONE_ID><TARGET value="1" /></CLONE_ID>"#,
    ] {
        let application = source();
        let mut database = CardDatabase::new(&application);
        database
            .insert(&application, set("BAD", &object(1, field)))
            .unwrap();
        assert!(
            matches!(database.object(1), Err(Error::InvalidValue { object: 1, field, .. }) if field == "CLONE_ID")
        );
    }
}

/// Boolean access distinguishes absence, an enabled flag, and an explicit override.
#[test]
fn foil_status_accepts_native_flags_and_zero_one_literals() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    let objects = object(1, "")
        + &object(2, "<IS_FOIL />")
        + &object(3, r#"<CLONE_ID value="2" /><IS_FOIL value="0" />"#)
        + &object(4, r#"<IS_FOIL value="1" />"#)
        + &object(5, r#"<IS_FOIL value="unknown" />"#)
        + &object(6, r#"<IS_FOIL><VALUE value="1" /></IS_FOIL>"#);
    database
        .insert(&application, set("FOIL", &objects))
        .unwrap();
    for (id, expected) in [(1, false), (2, true), (3, false), (4, true)] {
        assert_eq!(database.object(id).unwrap().is_foil().unwrap(), expected);
    }
    assert!(
        matches!(database.object(5).unwrap().is_foil(), Err(Error::InvalidValue { object: 5, field, value }) if field == "IS_FOIL" && value == "unknown")
    );
    assert!(
        matches!(database.object(6).unwrap().is_foil(), Err(Error::InvalidValue { object: 6, field, .. }) if field == "IS_FOIL")
    );
    assert!(database.validate().is_err());
}

/// A database is bound to both application identity and its versioned source URL.
#[test]
fn inserting_a_different_snapshot_does_not_change_the_database() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    for other in [
        application_at(
            "https://other.example.test/live/MTGO.application",
            "1.2.3.4",
        ),
        application_at(
            "https://patch.example.test/live/MTGO.application",
            "1.2.3.5",
        ),
    ] {
        assert!(matches!(
            database.insert(&other, set("OTHER", &object(1, ""))),
            Err(Error::SourceMismatch)
        ));
        assert!(database.is_empty());
        assert_eq!(database.sets().count(), 0);
    }
    assert_eq!(database.application_url(), application.url());
    assert_eq!(database.identity(), &application.manifest().identity);
    assert_eq!(database.version(), application.version());
}

/// Duplicate insertion is transactional even if a new object precedes the conflict.
#[test]
fn duplicate_object_insertion_cannot_leak_new_records_or_set_fragments() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    database
        .insert(&application, set("ONE", &object(1, "")))
        .unwrap();
    let before = database.sets().count();
    let conflict = object(2, "") + &object(1, "");
    assert!(matches!(
        database.insert(&application, set("TWO", &conflict)),
        Err(Error::Duplicate { .. })
    ));
    assert_eq!(database.len(), 1);
    assert!(database.raw_object(2).is_none());
    assert_eq!(database.sets().count(), before);

    let CardDataFile::Set(mut fragment) = set("MANUAL", &object(2, "")) else {
        panic!("expected a set");
    };
    fragment.objects.push(fragment.objects[0].clone());
    assert!(matches!(
        database.insert(&application, CardDataFile::Set(fragment)),
        Err(Error::Duplicate { .. })
    ));
    assert_eq!(database.len(), 1);
    assert!(database.raw_object(2).is_none());
    assert_eq!(database.sets().count(), before);
}

/// Repeated lookup tables cannot replace existing rows or add partial new data.
#[test]
fn duplicate_lookup_insertion_keeps_the_original_table() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    database
        .insert(
            &application,
            lookup("N", r#"<N_ITEM id="old">Original</N_ITEM>"#),
        )
        .unwrap();
    let result = database.insert(
        &application,
        lookup(
            "N",
            r#"<N_ITEM id="new">New</N_ITEM><N_ITEM id="old">Changed</N_ITEM>"#,
        ),
    );
    assert!(matches!(result, Err(Error::Duplicate { .. })));
    let table = database.lookups().next().unwrap();
    assert_eq!(table.entries.len(), 1);
    assert_eq!(table.entries["old"], "Original");
    assert!(!table.entries.contains_key("new"));
}

/// Long inheritance chains resolve iteratively instead of exhausting the stack.
#[test]
fn long_clone_chain_resolves_without_recursion() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    let mut objects = vec![DigitalObject {
        id: 1,
        fields: BTreeMap::from([(
            "CARDNAME_STRING".into(),
            FieldValue::Literal("Inherited".into()),
        )]),
    }];
    for id in 2..=16_384 {
        objects.push(DigitalObject {
            id,
            fields: BTreeMap::from([(
                "CLONE_ID".into(),
                FieldValue::Literal((id - 1).to_string()),
            )]),
        });
    }
    database
        .insert(
            &application,
            CardDataFile::Set(CardSet {
                id: "LONG".into(),
                age: None,
                kind: None,
                objects,
            }),
        )
        .unwrap();
    assert_eq!(database.object(16_384).unwrap().name(), Some("Inherited"));
}

/// Captured rows resolve a real foil clone and retain the separate booster object.
#[test]
fn captured_card_and_foil_resolve_names_rules_and_object_kind() {
    let application = source();
    let mut database = CardDatabase::new(&application);
    database
        .insert(&application, CardDataFile::from_bytes(LIVE_SET).unwrap())
        .unwrap();
    database
        .insert(
            &application,
            CardDataFile::from_bytes(LIVE_OBJECTS).unwrap(),
        )
        .unwrap();
    for bytes in LIVE_LOOKUPS.iter().rev() {
        database
            .insert(&application, CardDataFile::from_bytes(bytes).unwrap())
            .unwrap();
    }
    database.validate().unwrap();
    assert_eq!(database.len(), 3);
    let base = database.card(33_914).unwrap();
    let foil = database.card(33_915).unwrap();
    assert_eq!(base.name(), Some("Mystical Tutor"));
    assert_eq!(foil.name(), base.name());
    assert_eq!(
        foil.rules_text(),
        Some(
            "Search your library for an instant or sorcery card, reveal it, then shuffle and put that card on top."
        )
    );
    assert_eq!(foil.mana_cost(), Some("U"));
    assert_eq!(foil.colors(), Some("COLOR_BLUE"));
    assert_eq!(foil.rarity(), Some("MA_RARITY_MYTHIC"));
    assert_eq!(foil.set_code(), "V09");
    assert_eq!(foil.text("CARDTEXTURE_NUMBER"), Some("67831"));
    assert!(!base.is_foil().unwrap());
    assert!(foil.is_foil().unwrap());
    assert_eq!(foil.text("PROMO_LABEL_STRING"), Some(""));
    let booster = database.object(23_961).unwrap();
    assert_eq!(booster.name(), Some("Tenth Edition Booster"));
    assert_eq!(booster.object_type(), Some("BSTR"));
    assert_eq!(booster.set_code(), "10E");
    assert!(matches!(database.card(23_961), Err(Error::NotCard(23_961))));
}
