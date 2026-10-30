Discover and resolve Magic: The Gathering Online data from its ClickOnce
deployment. Resource locations follow the application's versioned manifests;
`CardDatabase` combines the card-data files into resolved objects.

## Live card data

Enable the optional `http` feature to use `Client`. The default build contains
the manifest catalog, card-data parsers, and resolver without an HTTP client.

```no_run
# #[cfg(feature = "http")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = mtgoxml::Client::new();
    let application = client.live()?;
    let database = client.load_card_sets(&application, ["V09"])?;
    let card = database.card(33915)?;

    assert_eq!(card.name(), Some("Mystical Tutor"));
    assert!(card.is_foil()?);
    let artwork = client.load_artwork(&application)?;
    let mut friendly = card.friendly()?;
    friendly.art_url = artwork.for_object(card.object()).map(|url| url.to_string());
    println!("{:?}: {:?}", friendly.name, friendly.art_url);
    Ok(())
}
# #[cfg(not(feature = "http"))]
# fn main() {}
```

Choose how much to load:

| Method | Downloads |
| --- | --- |
| `Client::live()` or `discover(url)` | The deployment and application manifests |
| `Client::load_card_sets(&application, ["V09"])` | Selected set fragments and their referenced lookup tables |
| `Client::load_card_database(&application)` | Every CardDataSource component; this can exceed 100 MiB |
| `Client::load_artwork(&application)` | The client configuration used to resolve artwork URLs |
| `Client::load_card_styles(&application)` | Named layout declarations from `CardStyle.xml` |
| `Client::load_image_names(&application)` | Ordered store-product filename rules from `DigitalObjectImageNames.xml` |

Set codes are case insensitive. Selected loading includes both `client_CODE.xml`
and `client_CODE_DO.xml` when present, and follows references inside nested
property groups. If a clone's base belongs to another set, include that set too;
otherwise loading reports the missing object ID. Both database-loading methods
validate all loaded objects before returning.

`ResolvedObject::friendly()` exposes readable, typed fields and retains every
unpromoted property in `mtgo`. Enable `serde` to serialize that view without
requiring HTTP. The `snapshot` feature includes both capabilities for the exporter.

## Resource discovery

The live entry point is
[`MTGO.application`](http://mtgo.patch.daybreakgames.com/patch/mtg/live/client/MTGO.application).
`Client::discover(url)` accepts another deployment endpoint. Discovery fetches
only manifests, so it can be used independently of card-data loading.

The resulting `Application` exposes:

- `resources()`: every file and installed assembly, including optional groups.
  Runtime and OS prerequisites and unknown dependency relationships are excluded.
- `xml_resources()` and `card_data_resources()`: XML files, or everything inside
  `CardDataSource`, respectively. The latter includes the DTD.
- `resource(path)`: a logical-path lookup accepting either slash style and
  ignoring ASCII case. Each resource retains its kind, URL, size, hashes, group,
  and optional status. Full manifests remain accessible.

File names are literal paths; URL metacharacters are percent-encoded. Assembly
codebases are URI references, retaining escaping and query strings. Windows
separators work on every platform. `mapFileExtensions="true"` adds `.deploy` to
payload URLs, while the application manifest keeps its original name. Omission
defaults to false.

Discovery rejects ambiguous application references, mismatched identities,
duplicate logical paths, unsupported URL schemes, and invalid file paths.
Signatures and payload hashes are retained without verification. Other resource
formats are available as URLs and metadata; CardDataSource has dedicated parsers.

## Offline and custom transports

Parse saved manifests using the same catalog API:

```
use mtgoxml::{Deployment, LIVE_DEPLOYMENT_URL};

let deployment = Deployment::from_bytes(
    LIVE_DEPLOYMENT_URL,
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/live/MTGO.application")),
)?;
let application = deployment.with_application_bytes(
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/live/MTGO.exe.manifest")),
)?;
assert!(application.resource("CardDataSource/CARDNAME_STRING.xml").is_some());
# Ok::<(), mtgoxml::Error>(())
```

Use `Deployment::from_manifest` and `Deployment::with_application` with parsed
`clickonce::Manifest` values to configure parsing yourself. Supply final response
URLs after redirects so relative references resolve against the correct source.
An advertised deployment-provider URL remains metadata rather than being followed.

With `http`, `Client::with_agent` accepts a custom `mtgoxml::transport::Agent` for
proxy and timeout configuration. Default requests have a 30-second timeout each.
HTTP failures are propagated, and relative references follow final response URLs.
`with_parse_options` configures manifest limits; `with_card_data_options`
configures component limits.

## Examples

```sh
# Print resource paths and URLs without fetching payloads.
cargo run --locked --features http --example resources

# Resolve one live card from a selected set.
cargo run --locked --features http --example card -- 33915 V09
```

The `resources` example accepts a deployment URL as its first argument. The `card`
example accepts an object ID and set code.

The optional `snapshot` feature enables the `mtgoxml-export` executable, which
downloads verified components and writes compressed JSON Lines. See the repository's
[data-release guide](https://github.com/cmeister2/mtgoxml/blob/main/docs/data-releases.md)
for its schema and scheduled publication workflow.
