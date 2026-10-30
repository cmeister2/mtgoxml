//! Bounded CardStyle parsing and explicit clone-resolution contracts.

use std::io::{self, Read};
use std::sync::atomic::{AtomicUsize, Ordering};

use mtgoxml::cardstyle::{CardStyles, Error, ParseOptions};

/// An unbounded source records actual bytes consumed by the reader entry point.
struct CountingReader<'a> {
    /// Bytes supplied to the parser before its configured limit stops reading.
    read: &'a AtomicUsize,
}

impl Read for CountingReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        buffer.fill(b'x');
        self.read.fetch_add(buffer.len(), Ordering::Relaxed);
        Ok(buffer.len())
    }
}

/// Forward clone references inherit fields while explicit child values win.
#[test]
fn resolves_multihop_styles_without_changing_source_declarations() {
    let catalog = CardStyles::parse(
        r#"<AnimCache>
          <AnimCardStyle Name="Leaf" CloneFrom="Middle" UseWatermarkEffectOnNonFoils="false">
            <Frame>none</Frame>
            <Model>Leaf model</Model>
            <rcArt><X>-2</X><Y>4</Y><Width>0</Width><Height>20</Height></rcArt>
            <TextElementEntry TextType="TextBox" Font="BODY" ForeColor="Black" X="1" Y="2" W="3" H="4"/>
          </AnimCardStyle>
          <AnimCardStyle Name="Base" UseWatermarkEffectOnNonFoils="true">
            <Frame>Base frame</Frame><Model>Base model</Model>
            <WatermarkEffect>Watermark Default</WatermarkEffect><ArtSuffix>_base</ArtSuffix>
            <PTBoxLeft>left</PTBoxLeft><PTBoxMiddle>middle</PTBoxMiddle><PTBoxRight>right</PTBoxRight>
            <CCBoxLeft>cc-left</CCBoxLeft><CCBoxMiddle>cc-middle</CCBoxMiddle><CCBoxRight>cc-right</CCBoxRight>
            <ReferenceSize><Width>196</Width><Height>280</Height></ReferenceSize>
            <rcArt><X>6</X><Y>24</Y><Width>184</Width><Height>136</Height></rcArt>
            <rcPTBox><X>128</X><Y>257</Y><Width>60</Width><Height>17</Height></rcPTBox>
            <TextElementEntry TextType="Title" Font="TITLE" Style="Bold" ForeColor="Black" X="12" Y="10" W="133" H="21"/>
            <TextElementEntry TextType="Copyright" Font="ARTIST" ForeColor="Black"/>
          </AnimCardStyle>
          <AnimCardStyle Name="Middle" CloneFrom="Base">
            <ArtSuffix/><PTBoxRight>middle-right</PTBoxRight>
            <TextElementEntry TextType="Title" Font="OTHER_TITLE" ForeColor="White" X="8" Y="9" W="140" H="22"/>
          </AnimCardStyle>
        </AnimCache>"#,
    )
    .unwrap();
    let resolved = catalog.resolve("Leaf").unwrap();
    assert_eq!(resolved.name, "Leaf");
    let properties = &resolved.properties;
    assert_eq!(properties.frame.as_deref(), Some("none"));
    assert_eq!(properties.model.as_deref(), Some("Leaf model"));
    assert_eq!(
        properties.watermark_effect.as_deref(),
        Some("Watermark Default")
    );
    assert_eq!(properties.art_suffix.as_deref(), Some(""));
    assert_eq!(properties.use_watermark_effect_on_non_foils, Some(false));
    assert_eq!(properties.pt_box.left.as_deref(), Some("left"));
    assert_eq!(properties.pt_box.middle.as_deref(), Some("middle"));
    assert_eq!(properties.pt_box.right.as_deref(), Some("middle-right"));
    assert_eq!(properties.cc_box.left.as_deref(), Some("cc-left"));
    assert_eq!(properties.cc_box.middle.as_deref(), Some("cc-middle"));
    assert_eq!(properties.cc_box.right.as_deref(), Some("cc-right"));
    let reference = properties.reference_size.as_ref().unwrap();
    assert_eq!((reference.width, reference.height), (196.0, 280.0));
    let art = &properties.rectangles["rcArt"];
    assert_eq!(
        (art.x, art.y, art.width, art.height),
        (-2.0, 4.0, 0.0, 20.0)
    );
    assert_eq!(properties.rectangles["rcPTBox"].width, 60.0);
    let title = &properties.text_elements["Title"];
    assert_eq!(title.text_type, "Title");
    assert_eq!(title.font, "OTHER_TITLE");
    assert_eq!(title.fore_color, "White");
    assert!(title.font_style.is_none());
    assert_eq!(title.bounds.as_ref().unwrap().x, 8.0);
    assert!(properties.text_elements["Copyright"].bounds.is_none());
    assert_eq!(properties.text_elements["TextBox"].font, "BODY");
    let raw = catalog.get("Leaf").unwrap();
    assert_eq!(raw.clone_from.as_deref(), Some("Middle"));
    assert!(raw.properties.reference_size.is_none());
    assert!(!raw.properties.text_elements.contains_key("Title"));
    assert_eq!(
        catalog
            .get("Base")
            .unwrap()
            .properties
            .art_suffix
            .as_deref(),
        Some("_base")
    );
}

/// Missing bases and cycles are resolution errors, not reasons to lose unrelated styles.
#[test]
fn retains_unresolved_declarations_and_reports_reference_failures() {
    let catalog = CardStyles::parse(
        r#"<AnimCache>
          <AnimCardStyle Name="Valid"><Frame>Frame valid</Frame></AnimCardStyle>
          <AnimCardStyle Name="Orphan" CloneFrom="Missing"/>
          <AnimCardStyle Name="A" CloneFrom="B"/>
          <AnimCardStyle Name="B" CloneFrom="A"/>
          <AnimCardStyle Name="Self" CloneFrom="Self"/>
        </AnimCache>"#,
    )
    .unwrap();
    assert_eq!(
        catalog
            .resolve("Valid")
            .unwrap()
            .properties
            .frame
            .as_deref(),
        Some("Frame valid")
    );
    assert_eq!(
        catalog.get("Orphan").unwrap().clone_from.as_deref(),
        Some("Missing")
    );
    assert!(matches!(
        catalog.resolve("Orphan"),
        Err(Error::MissingReference { .. })
    ));
    assert!(matches!(
        catalog.resolve("Missing"),
        Err(Error::MissingReference { .. })
    ));
    assert!(matches!(
        catalog.resolve("A"),
        Err(Error::ReferenceCycle(_))
    ));
    assert!(matches!(
        catalog.resolve("Self"),
        Err(Error::ReferenceCycle(_))
    ));
    assert!(catalog.get("valid").is_none());
    assert!(matches!(
        catalog.resolve("valid"),
        Err(Error::MissingReference { .. })
    ));
}

/// Duplicate identities and override keys must never silently overwrite source data.
#[test]
fn rejects_duplicate_style_fields_and_text_elements() {
    for xml in [
        r#"<AnimCache><AnimCardStyle Name="A"/><AnimCardStyle Name="A"/></AnimCache>"#,
        r#"<AnimCache><AnimCardStyle Name="A"><Frame>one</Frame><Frame>two</Frame></AnimCardStyle></AnimCache>"#,
        r#"<AnimCache><AnimCardStyle Name="A"><rcArt><X>0</X><Y>0</Y><Width>1</Width><Height>1</Height></rcArt><rcArt><X>1</X><Y>1</Y><Width>2</Width><Height>2</Height></rcArt></AnimCardStyle></AnimCache>"#,
        r#"<AnimCache><AnimCardStyle Name="A"><TextElementEntry TextType="Title" Font="ONE" ForeColor="Black"/><TextElementEntry TextType="Title" Font="TWO" ForeColor="White"/></AnimCardStyle></AnimCache>"#,
    ] {
        assert!(
            matches!(CardStyles::parse(xml), Err(Error::Duplicate { .. })),
            "{xml}"
        );
    }
}

/// Invalid geometry and ambiguous XML shapes fail instead of inventing rendering data.
#[test]
fn rejects_invalid_style_structure_and_geometry() {
    for xml in [
        "<Other/>",
        r#"<AnimCache xmlns="urn:other"><AnimCardStyle Name="A"/></AnimCache>"#,
        "<AnimCache><AnimCardStyle/></AnimCache>",
        r#"<AnimCache><AnimCardStyle Name=""/></AnimCache>"#,
        r#"<AnimCache><AnimCardStyle Name="A" UseWatermarkEffectOnNonFoils="maybe"/></AnimCache>"#,
        r#"<AnimCache><AnimCardStyle Name="A"><Frame><Unexpected/></Frame></AnimCardStyle></AnimCache>"#,
        r#"<AnimCache><AnimCardStyle Name="A"><ReferenceSize><Width>1</Width></ReferenceSize></AnimCardStyle></AnimCache>"#,
        r#"<AnimCache><AnimCardStyle Name="A"><rcArt><X>NaN</X><Y>0</Y><Width>1</Width><Height>1</Height></rcArt></AnimCardStyle></AnimCache>"#,
        r#"<AnimCache><AnimCardStyle Name="A"><rcArt><X>0</X><Y>0</Y><Width>1</Width><Height>inf</Height></rcArt></AnimCardStyle></AnimCache>"#,
        r#"<AnimCache><AnimCardStyle Name="A"><TextElementEntry TextType="Title" Font="TITLE" ForeColor="Black" X="1"/></AnimCardStyle></AnimCache>"#,
    ] {
        assert!(CardStyles::parse(xml).is_err(), "{xml}");
    }
}

/// Published Latin-1 declarations and UTF BOMs decode without changing resource names.
#[test]
fn decodes_latin1_utf8_and_utf16_without_losing_text() {
    let latin1 = r#"<?xml version="1.0" encoding="ISO-8859-1"?><AnimCache><AnimCardStyle Name="café"><TextElementEntry TextType="Title" Font="Crédit" ForeColor="Black"/></AnimCardStyle></AnimCache>"#;
    let bytes: Vec<u8> = latin1
        .chars()
        .map(|character| u8::try_from(u32::from(character)).unwrap())
        .collect();
    let catalog = CardStyles::from_bytes(&bytes).unwrap();
    assert_eq!(
        catalog.get("café").unwrap().properties.text_elements["Title"].font,
        "Crédit"
    );
    let utf8 = r#"<AnimCache><AnimCardStyle Name="雪"/></AnimCache>"#;
    let mut bom = vec![0xef, 0xbb, 0xbf];
    bom.extend_from_slice(utf8.as_bytes());
    assert!(CardStyles::from_bytes(&bom).unwrap().get("雪").is_some());
    let mut utf16 = vec![0xff, 0xfe];
    for unit in utf8.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    assert!(CardStyles::from_bytes(&utf16).unwrap().get("雪").is_some());
}

/// Unsupported declarations, malformed byte sequences, and DTDs cannot enter the parser.
#[test]
fn rejects_invalid_encodings_and_external_entities() {
    for bytes in [
        &b"\xff"[..],
        &b"\xff\xfe\x00"[..],
        &b"\xff\xfe\x00\xd8"[..],
        &br#"<?xml version="1.0" encoding="UTF-32"?><AnimCache/>"#[..],
        &br#"<!DOCTYPE AnimCache [<!ENTITY frame SYSTEM "file:///private">]><AnimCache><AnimCardStyle Name="A"><Frame>&frame;</Frame></AnimCardStyle></AnimCache>"#[..],
    ] {
        assert!(CardStyles::from_bytes(bytes).is_err());
    }
}

/// Encoded bytes, decoded bytes, XML nodes, and reader consumption each have explicit bounds.
#[test]
fn enforces_input_node_and_streaming_limits() {
    let xml = r#"<AnimCache><AnimCardStyle Name="A"/></AnimCache>"#;
    let options = ParseOptions {
        max_input_bytes: xml.len() - 1,
        ..ParseOptions::default()
    };
    assert!(matches!(
        CardStyles::parse_with_options(xml, options),
        Err(Error::InputTooLarge { .. })
    ));
    assert!(matches!(
        CardStyles::from_bytes_with_options(xml.as_bytes(), options),
        Err(Error::InputTooLarge { .. })
    ));
    let options = ParseOptions {
        max_nodes: 1,
        ..ParseOptions::default()
    };
    assert!(CardStyles::parse_with_options(xml, options).is_err());
    let latin1 = r#"<?xml version="1.0" encoding="ISO-8859-1"?><AnimCache><AnimCardStyle Name="é"/></AnimCache>"#;
    let bytes: Vec<u8> = latin1
        .chars()
        .map(|character| u8::try_from(u32::from(character)).unwrap())
        .collect();
    let options = ParseOptions {
        max_input_bytes: bytes.len(),
        ..ParseOptions::default()
    };
    assert!(matches!(
        CardStyles::from_bytes_with_options(&bytes, options),
        Err(Error::InputTooLarge { .. })
    ));
    let read = AtomicUsize::new(0);
    let options = ParseOptions {
        max_input_bytes: 32,
        ..ParseOptions::default()
    };
    assert!(matches!(
        CardStyles::from_reader_with_options(CountingReader { read: &read }, options),
        Err(Error::InputTooLarge { .. })
    ));
    assert_eq!(read.load(Ordering::Relaxed), 33);
}
