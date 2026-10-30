//! Semantic coverage for the client's ordered digital-object image-name rules.

use std::io::{self, Read};

use mtgoxml::imagenames::{DigitalObjectImageNames, Error, MatchRule, ParseOptions};

/// Parse a complete rule file assembled from small source declarations.
fn rules(body: &str) -> DigitalObjectImageNames {
    DigitalObjectImageNames::parse(&format!(
        "<DigitalObjectImageNamesData>{body}</DigitalObjectImageNamesData>"
    ))
    .unwrap()
}

/// Generate an owned filename without conflating absent and deliberately empty results.
fn image(data: &DigitalObjectImageNames, name: &str, set: &str, id: u64) -> Option<String> {
    data.generate_image_name(name, set, id).unwrap()
}

/// Every match kind participates in source order, including deliberate empty output.
#[test]
fn matches_by_catalog_name_substring_suffix_and_regex_in_order() {
    let data = rules(
        r#"
      <Match Rule="CatalogId" MatchPattern="42" ValueFormat="catalog.png"/>
      <Match Rule="Equals" MatchPattern="Event Ticket" ValueFormat="MTGOTHER_event.png"/>
      <Match Rule="Contains" MatchPattern="Unused" ValueFormat=""/>
      <Match Rule="EndsWith" MatchPattern=" Booster" ValueFormat="MTG{0}_booster.png"/>
      <Match Rule="Regex" MatchPattern="(.+)" ValueFormat="MTG{0}_$1.png"/>
    "#,
    );
    assert_eq!(data.matches()[0].rule(), MatchRule::CatalogId);
    assert_eq!(data.matches()[1].match_pattern(), "Event Ticket");
    assert_eq!(data.matches()[2].value_format(), "");
    assert!(!data.matches()[2].is_redemption());
    for (name, set, id, expected) in [
        ("Unused Booster", "TEST", 42, "catalog.png"),
        ("  Event Ticket\n", "ETK", 1, "MTGOTHER_event.png"),
        ("Unused Booster", "TEST", 1, ""),
        ("Mirage Booster", "MI", 1, "MTGMI_booster.png"),
        ("event ticket", "ETK", 1, "MTGETK_event_ticket.png"),
    ] {
        assert_eq!(image(&data, name, set, id).as_deref(), Some(expected));
    }
}

/// Duplicate ordered rules remain significant and an unmatched name stays absent.
#[test]
fn preserves_first_matching_rule_and_distinguishes_no_match() {
    let data = rules(
        r#"
      <Match Rule="Equals" MatchPattern="Duplicate" ValueFormat="first.png"/>
      <Match Rule="Equals" MatchPattern="Duplicate" ValueFormat="second.png"/>
    "#,
    );
    assert_eq!(
        image(&data, "Duplicate", "", 1).as_deref(),
        Some("first.png")
    );
    assert!(image(&data, "duplicate", "", 1).is_none());
    let empty = rules("");
    assert!(empty.matches().is_empty());
    assert!(image(&empty, "Anything", "", 1).is_none());
}

/// Set aliases are exact, ordered substitutions rather than a recursive map.
#[test]
fn replaces_set_codes_once_using_the_first_exact_replacement() {
    let data = rules(
        r#"
      <CardSetNameReplacement Original="CHC" Replacement="CHK"/>
      <CardSetNameReplacement Original="CHC" Replacement="WRONG"/>
      <CardSetNameReplacement Original="CHK" Replacement="RECURSIVE"/>
      <Match Rule="Equals" MatchPattern="Booster" ValueFormat="MTG{0}.png"/>
    "#,
    );
    assert_eq!(data.replacements()[0].original, "CHC");
    assert_eq!(data.replace_set_name("CHC"), "CHK");
    assert_eq!(
        image(&data, "Booster", "CHC", 1).as_deref(),
        Some("MTGCHK.png")
    );
    assert_eq!(
        image(&data, "Booster", "chc", 1).as_deref(),
        Some("MTGchc.png")
    );
}

/// Deck and redemption formats use different name portions and shared punctuation rules.
#[test]
fn formats_deck_suffixes_and_redemption_prefixes() {
    let data = rules(
        r#"
      <Match Rule="Contains" MatchPattern="Duel Decks: " ValueFormat="MTG{0}_{1}.png"/>
      <Match Rule="EndsWith" MatchPattern=" Redemption Request (Premium)" ValueFormat="{1}_foil_set.png" IsRedemption="true"/>
    "#,
    );
    assert!(data.matches()[1].is_redemption());
    assert_eq!(
        image(
            &data,
            "Duel Decks: Dragons & Demons - Ajani's (Favored)",
            "DD",
            1
        )
        .as_deref(),
        Some("MTGDD_dragons_and_demons___ajanis_favored.png")
    );
    assert_eq!(
        image(&data, "Mirage Redemption Request (Premium)", "MI", 1).as_deref(),
        Some("mirage_foil_set.png")
    );
}

/// Regex replacements retain unconsumed text and literal template case.
#[test]
fn applies_case_insensitive_regex_to_lowercase_names_without_anchoring() {
    let data = rules(
        r#"<Match Rule="Regex" MatchPattern="(\w+) Event Token" ValueFormat="Event_Token_$1.png"/>"#,
    );
    assert_eq!(
        image(&data, "PREFIX Blue EVENT TOKEN SUFFIX", "", 1).as_deref(),
        Some("prefix_Event_Token_blue.png_suffix")
    );
    assert_eq!(
        image(&data, "Æther Event Token", "", 1).as_deref(),
        Some("Event_Token_æther.png")
    );
    let repeated =
        rules(r#"<Match Rule="Regex" MatchPattern="(\w+) Event Token" ValueFormat="$1"/>"#);
    assert_eq!(
        image(&repeated, "Blue Event Token Red Event Token", "", 1).as_deref(),
        Some("blue_red")
    );
}

/// Numeric captures, set placeholders, and final comma removal retain native ordering.
#[test]
fn expands_captures_before_composite_fields_and_removes_commas() {
    let data = rules(
        r#"
      <CardSetNameReplacement Original="OR5" Replacement="ORI"/>
      <Match Rule="Regex" MatchPattern="(\w+) (\w+)'s Origin(?: Booster)?" ValueFormat="MTG{0}_$1_$2.png"/>
      <Match Rule="Equals" MatchPattern="Literal" ValueFormat="{{{0}}},{{escaped}}.png"/>
      <Match Rule="Regex" MatchPattern="(.+)" ValueFormat="Dollar_$$$1.png"/>
    "#,
    );
    assert_eq!(
        image(&data, "Liliana Vess's Origin Booster", "OR5", 1).as_deref(),
        Some("MTGORI_liliana_vess.png")
    );
    assert_eq!(
        image(&data, "Literal", "SET", 1).as_deref(),
        Some("{SET}{escaped}.png")
    );
    assert_eq!(
        image(&data, "Name, With: 'Punctuation'", "", 1).as_deref(),
        Some("Dollar_$name_with_punctuation.png")
    );
}

/// Native image naming uses each child's declaring set and inherited display name.
#[test]
fn resolves_objects_using_declaring_sets_and_inherited_names() {
    let application = mtgoxml::Deployment::from_bytes(
        "https://patch.example/client/MTGO.application",
        include_bytes!("../fixtures/synthetic/unmapped.application"),
    )
    .unwrap()
    .with_application_bytes(include_bytes!(
        "../fixtures/synthetic/resources.exe.manifest"
    ))
    .unwrap();
    let mut database = application.card_database();
    database.insert(&application, mtgoxml::carddata::CardDataFile::parse(r#"
      <CardSet id="CHC">
        <DigitalObject DigitalObjectCatalogID="DOC_1"><CARDNAME_STRING value="Booster"/><CARDSETNAME_STRING value="IGNORED"/></DigitalObject>
        <DigitalObject DigitalObjectCatalogID="DOC_2"><CLONE_ID value="1"/></DigitalObject>
        <DigitalObject DigitalObjectCatalogID="DOC_3"/>
      </CardSet>"#).unwrap()).unwrap();
    database.insert(&application, mtgoxml::carddata::CardDataFile::parse(r#"
      <CardSet id="OTHER"><DigitalObject DigitalObjectCatalogID="DOC_4"><CLONE_ID value="1"/></DigitalObject></CardSet>"#).unwrap()).unwrap();
    let data = rules(
        r#"<CardSetNameReplacement Original="CHC" Replacement="CHK"/><Match Rule="CatalogId" MatchPattern="3" ValueFormat="nameless.png"/><Match Rule="Equals" MatchPattern="Booster" ValueFormat="MTG{0}.png"/>"#,
    );
    for id in [1, 2] {
        assert_eq!(
            data.for_object(&database.object(id).unwrap())
                .unwrap()
                .as_deref(),
            Some("MTGCHK.png")
        );
    }
    assert_eq!(
        data.for_object(&database.object(3).unwrap())
            .unwrap()
            .as_deref(),
        Some("nameless.png")
    );
    assert_eq!(
        data.for_object(&database.object(4).unwrap())
            .unwrap()
            .as_deref(),
        Some("MTGOTHER.png")
    );
}

/// Unsupported XML, regex, and template constructs fail before filename generation.
#[test]
fn rejects_ambiguous_or_unsupported_source_data() {
    for body in [
        r#"<Match Rule="Invalid" MatchPattern="x" ValueFormat="x"/>"#,
        r#"<Match Rule="Equals" ValueFormat="x"/>"#,
        r#"<Match Rule="Equals" MatchPattern="x"/>"#,
        r#"<Match Rule="Equals" MatchPattern="x" ValueFormat="x" IsRedemption="yes"/>"#,
        r#"<Match Rule="Equals" MatchPattern="x" ValueFormat="x" Extra="future"/>"#,
        r#"<Match xmlns="urn:wrong" Rule="Equals" MatchPattern="x" ValueFormat="x"/>"#,
        r#"<Match xmlns:other="urn:wrong" other:Rule="Equals" MatchPattern="x" ValueFormat="x"/>"#,
        r#"<Match Rule="Equals" MatchPattern="x" ValueFormat="x"><Child/></Match>"#,
        r#"<Match Rule="Equals" MatchPattern="x" ValueFormat="x">content</Match>"#,
        r#"<Match Rule="Regex" MatchPattern="(?&lt;=a)b" ValueFormat="x"/>"#,
        r#"<Match Rule="Regex" MatchPattern="(a)" ValueFormat="$2"/>"#,
        r#"<Match Rule="Regex" MatchPattern="(a)" ValueFormat="$&amp;"/>"#,
        r#"<Match Rule="Equals" MatchPattern="x" ValueFormat="{2}"/>"#,
        r#"<Match Rule="Equals" MatchPattern="x" ValueFormat="unmatched}"/>"#,
        r#"<Other/>"#,
        "unexpected text",
    ] {
        assert!(
            DigitalObjectImageNames::parse(&format!(
                "<DigitalObjectImageNamesData>{body}</DigitalObjectImageNamesData>"
            ))
            .is_err(),
            "{body}"
        );
    }
    assert!(DigitalObjectImageNames::parse("<Other/>").is_err());
    assert!(
        DigitalObjectImageNames::parse("<DigitalObjectImageNamesData xmlns='urn:wrong'/>").is_err()
    );
    assert!(
        rules(r#"<Match Rule="Equals" MatchPattern="x" ValueFormat="x"/>"#)
            .generate_image_name("x", "", u64::MAX)
            .is_err()
    );
}

/// Encoded input accepts the source's UTF-8 BOM and enforces byte/node budgets.
#[test]
fn reads_bounded_encoded_documents_without_dtd_processing() {
    let xml = "<DigitalObjectImageNamesData><Match Rule='Equals' MatchPattern='x' ValueFormat='x'/></DigitalObjectImageNamesData>";
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend_from_slice(xml.as_bytes());
    let data = DigitalObjectImageNames::from_reader(bom.as_slice()).unwrap();
    assert_eq!(image(&data, "x", "", 1).as_deref(), Some("x"));
    assert!(matches!(
        DigitalObjectImageNames::from_bytes_with_options(
            xml.as_bytes(),
            ParseOptions {
                max_input_bytes: xml.len() - 1,
                max_nodes: 100
            }
        ),
        Err(Error::InputTooLarge { .. })
    ));
    assert!(
        DigitalObjectImageNames::parse_with_options(
            xml,
            ParseOptions {
                max_input_bytes: 1024,
                max_nodes: 1
            }
        )
        .is_err()
    );
    assert!(DigitalObjectImageNames::from_bytes(b"\xff").is_err());
    assert!(DigitalObjectImageNames::parse("<!DOCTYPE DigitalObjectImageNamesData [<!ENTITY x 'test'>]><DigitalObjectImageNamesData/>").is_err());
    assert!(DigitalObjectImageNames::from_reader(FailingReader).is_err());
}

/// Reader failure injected at the I/O boundary, independent of allocation details.
struct FailingReader;

impl Read for FailingReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("injected reader failure"))
    }
}
