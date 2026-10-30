# Card styles

Parse `CardStyle.xml` into named layout declarations, then resolve a style's
`CloneFrom` inheritance when needed. The source contains geometry, text placement,
and opaque frame, model, font, color, watermark, and texture names. Loading styles
does not download those assets or select a layout for a card.

```rust
use mtgoxml::cardstyle::CardStyles;

let styles = CardStyles::parse(r#"
<AnimCache>
  <AnimCardStyle Name="Base">
    <Frame>CardFrame Default</Frame>
    <ReferenceSize><Width>196</Width><Height>280</Height></ReferenceSize>
    <rcArt><X>6</X><Y>24</Y><Width>184</Width><Height>136</Height></rcArt>
  </AnimCardStyle>
  <AnimCardStyle Name="Blue" CloneFrom="Base">
    <Frame>CardFrame Blue</Frame>
  </AnimCardStyle>
</AnimCache>"#)?;

let raw = styles.get("Blue").unwrap();
assert!(raw.properties.reference_size.is_none());
let resolved = styles.resolve("Blue")?;
assert_eq!(resolved.properties.frame.as_deref(), Some("CardFrame Blue"));
assert_eq!(resolved.properties.reference_size.unwrap().height, 280.0);
assert_eq!(resolved.properties.rectangles["rcArt"].width, 184.0);
# Ok::<(), mtgoxml::cardstyle::Error>(())
```

With `http` enabled, `Client::load_card_styles(&application)` downloads the
manifest-declared resource. `Client::with_client_data_options` customizes its
parsing limits. Offline callers can use `parse`, `from_bytes`, or `from_reader`,
with corresponding `_with_options` methods.

Resolution uses exact, case-sensitive names and supports forward references.
Present child values replace inherited values; an absent value inherits, while
an explicit empty string, `false`, zero-area rectangle, or literal `none` remains
an override. Text elements replace whole entries keyed by `TextType`; omitted
font style or bounds remain absent in that replacement. No browser or native font
defaults are inferred. The original declarations remain unchanged.

Parsing retains unresolved references. `resolve` reports missing bases and
cycles for the requested chain. The published client data contains missing base
references, so callers should handle resolution errors per style.

The default limits are 16 MiB for both encoded and decoded input and 250,000 XML
nodes. Byte parsing supports UTF-8, declared ISO-8859-1 and ASCII, and BOM-marked
UTF-16; `parse` accepts already decoded Unicode. DTDs, duplicate declarations,
invalid structure, and non-finite geometry are rejected. Coordinates may be
negative and dimensions may be zero.

This is a standalone layout catalog shipped by MTGO. Its legacy coordinate
systems and asset names do not establish how the current WPF frame templates
render a particular card.
