Parse CardDataSource components and resolve their relationships into digital
objects. Keep the raw records when inspecting unfamiliar properties or loading
only part of a dataset.

## Files and records

MTGO splits this data across `client_*.xml` set fragments and shared lookup
tables. A set may have both a card file and a `_DO.xml` fragment for associated
products. The master XML assembles components using external entities, but the
ClickOnce manifest already lists the individual files.

`CardDataFile` distinguishes `Set` and `Lookup`. Set fragments contain
`DigitalObject` records keyed by numeric catalog ID; lookup tables map opaque
IDs to text. Not every object is a card: tokens, boosters, decks, and containers
share the same namespace.

Properties preserve their XML representation:

| XML | `FieldValue` |
| --- | --- |
| `<CARDNAME_STRING id="name"/>` | `Reference` into the named lookup table |
| `<CARDTEXTURE_NUMBER value="67831"/>` | `Literal`, including an explicitly empty string |
| `<IS_FOIL/>` | A presence `Flag` |
| `<PROTOTYPE_ATTRIBUTES_LIST>...</PROTOTYPE_ATTRIBUTES_LIST>` | Nested `Fields` |

Unknown property names survive parsing. Lookup IDs are matched by table name
and exact ID, never by row position. Text retains MTGO's formatting and mana
syntax after XML decoding; rendering and normalization are left to callers.

## Building a database

Create a database from an application snapshot, then insert parsed components
in any order. Validation is explicit so a partial dataset can be assembled first.

```
use mtgoxml::{Deployment, LIVE_DEPLOYMENT_URL, carddata::CardDataFile};

let application = Deployment::from_bytes(
    LIVE_DEPLOYMENT_URL,
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/live/MTGO.application")),
)?.with_application_bytes(
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/live/MTGO.exe.manifest")),
)?;
let mut database = application.card_database();

database.insert(&application, CardDataFile::parse(r#"
    <CARDNAME_STRING_ITEMS>
      <CARDNAME_STRING_ITEM id="name">Example</CARDNAME_STRING_ITEM>
    </CARDNAME_STRING_ITEMS>"#,
)?)?;
database.insert(&application, CardDataFile::parse(r#"
    <CardSet id="EXAMPLE">
      <DigitalObject DigitalObjectCatalogID="DOC_1">
        <CARDNAME_STRING id="name"/><DIGITAL_OBJECT_TYPE_CODE_STRING value="CARD"/>
      </DigitalObject>
      <DigitalObject DigitalObjectCatalogID="DOC_2">
        <CLONE_ID value="1"/><IS_FOIL/>
      </DigitalObject>
    </CardSet>"#,
)?)?;

database.validate()?;
let card = database.card(2)?;
assert_eq!(card.name(), Some("Example"));
assert!(card.is_foil()?);
# Ok::<(), Box<dyn std::error::Error>>(())
```

For saved files, use `CardDataFile::from_reader(std::fs::File::open(path)?)`.
Insert all needed lookup tables and clone bases before calling `validate()`.
Multiple fragments with the same set ID are allowed; duplicate object IDs or
lookup-table names are rejected without changing existing records.

## Resolved views

`CardDatabase::object(id)` returns any resolved digital object.
`CardDatabase::card(id)` requires MTGO's `CARD` object-type code and returns a
`CardView`. Both expose names, rules text, set codes, mana costs, colors, rarity,
foil status, and arbitrary fields.

Resolution follows `CLONE_ID` ancestry iteratively and joins the effective
lookup references. Missing bases or lookup entries and clone cycles are errors.
Child properties replace inherited properties, including empty strings and
whole nested groups. Groups are not merged field by field.

Nested fields retain structures for flipped faces, split cards, prototype costs,
prepared spells, and dungeon states, with their own references resolved.
`field_ref()` borrows a resolved value; `field()` clones it, including a nested
group. `fields()` exposes all resolved properties. Absence, empty text, presence
flags, and groups stay distinct.

Use `object.raw()` for the original child record, `object.set()` for its declaring
fragment, or `database.raw_object(id)` to skip resolution entirely.

## Friendly views and artwork

[`ResolvedObject::friendly`] creates an owned [`FriendlyObject`] with readable
names such as `name`, `artist`, and `rules_text`. It normalizes recognized art IDs,
mana values, colors, rarities, and foil flags. Its `mtgo` map retains unpromoted
fields and unfamiliar values, including nested groups and numeric-looking text.
It performs no network requests. Printed statistics, collector information, mana
notation, and rules formatting retain their source semantics.

[`CardDatabase::set_metadata`] collects the source `age` and `kind` once per set
code. It rejects conflicting fragments rather than choosing one silently. A
friendly object's optional `source_set` identifies a different declaring fragment;
use `source_set` when present, otherwise `set`, to look up shared metadata.

With the optional `serde` feature, friendly objects serialize directly to their
JSON representation, independently of HTTP support. See the repository's
[export schema](https://github.com/cmeister2/mtgoxml/blob/main/docs/data-releases.md)
for property mappings and migration details.

[`crate::ArtworkResolver`] reads the artwork CDN root from client configuration.
Its `for_object` method resolves primary artwork URLs from supported objects,
including inherited art IDs, without downloading images. Enable `http` to use
`Client::load_artwork` to fetch that configuration separately from card data.

## Parsing limits and provenance

Byte input accepts UTF-8 with an optional BOM. `ParseOptions` defaults to 16 MiB
and 250,000 XML nodes per component; named property groups are limited to 64
levels. The parser rejects duplicate keys, ambiguous properties, unsupported
structure, and DTD declarations. The published DTD is not enforced as a complete
schema, and external entities are never expanded.

A database records its application's manifest URL and identity. Supplying
another application context to `insert` returns `SourceMismatch`. This records
the caller's source context; externally supplied bytes and manifest signatures
or payload hashes are not verified. Cache files with their application snapshot
to avoid mixing versions.

To validate a saved dataset, pass a directory containing component basenames:

```sh
cargo run --locked --example database -- DEPLOYMENT_URL MTGO.application MTGO.exe.manifest CardDataSource 33915
```
