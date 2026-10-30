# MTGO data releases

The **MTGO card data** workflow checks the live ClickOnce deployment every six
hours, at 00:17, 06:17, 12:17, and 18:17 UTC. It can also be started from the
Actions tab on `main`. GitHub schedules can be delayed; public repositories must
have activity within 60 days for their schedules to remain enabled.

Each check downloads the deployment and application manifests. Each application
version has one data release, tagged `mtgo-data-<application-version>`, for example
`mtgo-data-3.4.159.4708`. If that version already has a published release, the
workflow skips the large CardDataSource download. Manifest hashes are retained
in the export metadata and checksums for provenance and integrity verification.

The workflow uses `cardinalby/git-get-release-action` to look for a published
release with that exact tag before downloading the catalog.

Data releases appear alongside crate releases, with distinct tags. They do not
replace the repository's latest crate release. New snapshots contain:

| Asset | Contents |
| --- | --- |
| `card-data.jsonl.gz` | Gzip-compressed JSON Lines, one resolved digital object per line |
| `card-styles.jsonl.gz` | Named styles with resolved inheritance or an explicit resolution error |
| `digital-object-image-names.jsonl.gz` | Resolved store-image filenames keyed by catalog ID |
| `snapshot.json` | Source provenance, schema version, object count, shared set metadata, and artwork configuration |
| `MTGO.application` | Original deployment-manifest bytes |
| `MTGO.exe.manifest` | Original application-manifest bytes |
| `MTGO.exe.config` | Original client configuration used for artwork URLs |
| `CardStyle.xml` | Original layout declarations used for the style export |
| `DigitalObjectImageNames.xml` | Original ordered rules used for store-image filenames |
| `SHA256SUMS` | SHA-256 checksums for the other assets |

The exporter pins the downloaded manifests before fetching components. It checks
declared resource sizes and supported content digests for components, artwork
configuration, and both auxiliary XML files, parses the components,
and resolves every object before producing the export. Failed downloads,
integrity checks, parsing, or resolution prevent publication. The workflow uses
`softprops/action-gh-release` to stage assets in a draft and publish after every
upload completes. An interrupted draft is retried on the next run. Published
snapshots are retained without replacement.

## JSON Lines schema version 3

Each [JSON Lines](https://jsonlines.org/) record is one complete JSON object,
including cards, tokens, and products, in ascending numeric ID order.
Records use UTF-8 without a byte-order mark and
end with LF. Newlines inside strings are escaped, so each record stays on one
physical line:

```jsonl
{"id":33914,"name":"Mystical Tutor","artist":"David O’Connor","set":"V09","art_id":1982,"mana_value":1,"colors":["U"],"rarity":"mythic","foil":false,"mtgo":{"FRAMESTYLE":"3"}}
{"id":33915,"name":"Mystical Tutor","artist":"David O’Connor","set":"V09","art_id":1982,"mana_value":1,"colors":["U"],"rarity":"mythic","foil":true,"mtgo":{"FRAMESTYLE":"3","CLONE_ID":"33914"}}
```

The illustrated records are abbreviated. Lookup references and clone inheritance
are resolved before the library's friendly projection assigns these aliases:

| Source property | JSON field | Value |
| --- | --- | --- |
| `CARDNAME_STRING` | `name` | Display name |
| `ARTIST_NAME_STRING` | `artist` | Artist text |
| `ARTID` | `art_id` | Unsigned integer |
| `REAL_ORACLETEXT_STRING` | `rules_text` | Original MTGO text |
| `FLAVORTEXT_STRING` | `flavor_text` | Original MTGO text |
| `MANA_COST_STRING` | `mana_cost` | Original MTGO mana notation |
| `CONVERTED_MANA_COST` | `mana_value` | Unsigned integer |
| `CARDSETNAME_STRING` | `set` | Set code, falling back to the source fragment |
| `COLLECTOR_INFO_STRING` | `collector_number` | Text, including values such as `8/15` |
| `COLOR`, `COLOR_IDENTITY` | `colors`, `color_identity` | Arrays of `W`, `U`, `B`, `R`, `G`; `[]` for colorless |
| `RARITY_STATUS` | `rarity` | `basic_land`, `common`, `uncommon`, `rare`, `mythic`, `token`, `bonus`, `promo`, or `play_aid` |
| `IS_FOIL` | `foil` | Boolean; absent means false |

Optional fields are omitted when absent. Explicit empty text stays empty. A field
with an unfamiliar value or representation remains in `mtgo`, without an invented
normalized value. Every other effective property also stays in `mtgo`, using its
original name. This includes numeric-looking codes such as `"001"`, printed stats
such as `"X"` or `"*/*"`, and recursive face, prototype, or dungeon groups. Raw
values remain strings, presence flags serialize as `true`, and groups as objects.
Mana and rules text retain MTGO's formatting; they are not converted to Scryfall
notation or plain Oracle text.

`art_url` follows the client's primary artwork selection using the inherited art ID,
effective frame style, and card properties. Some styles provide an artwork crop;
others include a frame. Avatars and playable Vanguard cards use the configured
avatar directory with an `_avatar.jpg` suffix. Missing or zero avatar art IDs
fall back to the object's catalog ID and `c_avatar.jpg`. Texture IDs remain
separate from art IDs. Tokens, play aids, products, and artless styles have no
generated URL.
Constructing a URL does not request the image or guarantee its availability.

`snapshot.json` records `schema_version: 3`, source URLs and digests, the release
tag, object count, and `sets`, a dictionary of source set codes to their `age` and
`kind`. These values retain exact spelling, empty strings, and nulls; `age` is an
opaque source value rather than a calendar year. Records reference shared metadata
by `set`, or by the optional `source_set` when the declaring fragment differs from
the resolved set code. The `artwork` object records `config_url`, `config_sha256`,
and the resolved image `base_url`. Release tags still use only the MTGO version.

New exports also contain an optional `client_data` object describing the auxiliary
JSONL format version, source URLs, raw XML SHA-256 digests, record counts, and
unresolved style count. Older schema-3 snapshots remain valid without these assets.

The format supports streaming through tools such as `jq` without reading a
single enclosing array. For example, `gzip -dc card-data.jsonl.gz | head -n 1`
prints the first object.

## Client data JSON Lines schema version 1

These two additional gzip files follow the same UTF-8, LF-terminated JSONL
conventions. Their schema version is `snapshot.json`'s
`client_data.schema_version`, independently of the card catalog's schema 3.
All source and generated assets appear in `SHA256SUMS`.

`card-styles.jsonl.gz` contains every style in exact-name order. A resolved row
has `name`, `status: "resolved"`, and `properties` from the inherited style.
Properties preserve opaque asset/font/color names, explicit empty values, and
the geometry described by [CardStyles](card-styles.md). For example, with
properties abbreviated:

```jsonl
{"name":"Base","status":"resolved","properties":{"frame":"CardFrame Default"}}
{"name":"Child","status":"unresolved","declaration":{"name":"Child","clone_from":"Missing","properties":{}},"error":{"kind":"missing_reference","target_kind":"style","key":"Missing"}}
```

Missing bases and cycles produce unresolved rows with the original declaration
and a typed error. Cycle errors use `kind: "reference_cycle"` and a `chain` array.
They remain visible in the export and its unresolved count. Other parsing or
export failures prevent publication.

`digital-object-image-names.jsonl.gz` contains one row per store-image object,
in numeric catalog-ID order:

```jsonl
{"id":1,"image_name":"MTGOTHER_event_1_sty_001.png"}
```

The client selects this pipeline by the presence of `IS_DIGITALOBJECT`, including
an explicit zero value. Join rows to `card-data.jsonl.gz` by `id`. Filenames use
the object's declaring set, inherited display name, and the ordered rules in
[DigitalObjectImageNames](image-names.md). `image_name: null` means no rule
matched; `""` means the winning rule deliberately produced an empty name. These
values remain distinct. The file contains filenames rather than image URLs;
directory selection and availability fallback remain with the consumer.

## Earlier snapshots

Schema version 2 used the same JSONL filename, with records containing `id`, a
repeated source `set` object, and the original property map in `fields`. Schema
version 1 used a single JSON document with an `objects` array in `card-data.json.gz`.
Already published releases retain their original schemas; the scheduled workflow
skips versions that are already published. A schema 1 download can be converted
to the schema 2 record layout locally:

```sh
jq -c '.objects[]' card-data.json > card-data.jsonl
```

## Run the exporter locally

```sh
cargo build --locked --release --bin mtgoxml-export --features snapshot
target/release/mtgoxml-export probe /tmp/mtgo-snapshot
target/release/mtgoxml-export export /tmp/mtgo-snapshot
gzip -dc /tmp/mtgo-snapshot/card-data.jsonl.gz > /tmp/card-data.jsonl
```

The second command downloads manifests; the third explicitly downloads the full
CardDataSource dataset, which exceeds 100 MiB. Local export does not publish a
release. The executable also writes checksums and release notes for the release
action. The Actions workflow uses the repository's `GITHUB_TOKEN` with
`contents: write`; no additional secret is needed.
