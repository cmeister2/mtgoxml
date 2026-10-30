# Digital-object image names

`DigitalObjectImageNames.xml` maps product names, set codes, and catalog IDs to
store image filenames. Parse it with `imagenames::DigitalObjectImageNames`,
inspect its ordered replacements and matching rules, or call
`generate_image_name` to apply them. The first matching rule wins, including an
explicitly empty result. `for_object` uses the resolved name, declaring `CardSet`
code, and catalog ID, matching the client rather than a set-name property alias.

These filenames belong to the client's store image pipeline; they do not replace
card or avatar artwork selection. URL construction and image availability remain
separate. See the Rustdoc for supported regex syntax, formatting behavior, and
bounded parsing options.

```rust
use mtgoxml::imagenames::DigitalObjectImageNames;

let names = DigitalObjectImageNames::parse(r#"
<DigitalObjectImageNamesData>
  <CardSetNameReplacement Original="CHC" Replacement="CHK"/>
  <Match Rule="Contains" MatchPattern="Unused" ValueFormat=""/>
  <Match Rule="EndsWith" MatchPattern="Booster" ValueFormat="MTG{0}_booster.png"/>
</DigitalObjectImageNamesData>"#)?;

assert_eq!(names.generate_image_name("Champions of Kamigawa Booster", "CHC", 1)?,
    Some("MTGCHK_booster.png".into()));
assert_eq!(names.generate_image_name("Unused Booster", "CHC", 2)?, Some("".into()));
assert_eq!(names.generate_image_name("Unknown product", "CHC", 3)?, None);
# Ok::<(), mtgoxml::imagenames::Error>(())
```

With `http` enabled, `Client::load_image_names(&application)` downloads the
manifest-declared file. `Client::with_client_data_options` sets its byte and XML
node limits. Offline callers can use `parse`, `from_bytes`, and `from_reader`,
or their `_with_options` variants. DTDs and unsupported source syntax are rejected.
