# mtgoxml

Discover and resolve Magic: The Gathering Online data from its ClickOnce
deployment. The library locates versioned resources, parses CardDataSource
components, and combines lookup tables and clone inheritance into complete
digital objects. Raw source records remain available alongside resolved views.

The live entry point is
[`MTGO.application`](http://mtgo.patch.daybreakgames.com/patch/mtg/live/client/MTGO.application).

## Live discovery

Enable the optional `http` feature to fetch the two manifests:

```toml
[dependencies]
mtgoxml = { path = "/path/to/mtgoxml", features = ["http"] }
```

```rust
use mtgoxml::Client;

let application = Client::new().live()?;
println!("MTGO version: {:?}", application.version());

for resource in application.xml_resources() {
    println!("{}: {}", resource.path, resource.url);
}

if let Some(resource) = application.resource(r"CardDataSource\CARDNAME_STRING.xml") {
    println!("Card names: {}", resource.url);
}
# Ok::<(), mtgoxml::Error>(())
```

`Client::discover(url)` supports a caller-supplied deployment endpoint. Both
manifest requests have a 30-second timeout by default. HTTP errors are propagated;
relative references resolve against final response URLs after redirects. A
deployment's advertised provider URL remains available as metadata.
`Client::with_agent` accepts a configured `mtgoxml::transport::Agent`, and
`with_parse_options` configures the parser's input limits.

Print the current resource catalog without downloading any payloads:

```sh
cargo run --locked --features http --example resources
```

Pass a deployment URL as the example's first argument to inspect another endpoint.

## Resolved card data

Load selected sets and the lookup tables referenced by their properties:

```rust
use mtgoxml::Client;

let client = Client::new();
let application = client.live()?;
let database = client.load_card_sets(&application, ["V09"])?;
let card = database.card(33915)?;

assert_eq!(card.name(), Some("Mystical Tutor"));
assert!(card.is_foil()?);
println!("{}: {:?}", card.set_code(), card.rules_text());
# Ok::<(), Box<dyn std::error::Error>>(())
```

`card(33915)` applies the foil record's overrides to its base object, then joins
each lookup reference by table name and opaque ID. Names, rules text, mana costs,
colors, rarity, and arbitrary properties are available on the resolved view.
Nested property groups retain fields for flipped faces, split cards, prototype
costs, prepared spells, and dungeon states, with their references resolved too.
Empty strings, absent fields, presence flags, and nested groups stay distinct.
Clone ancestry is followed iteratively, with explicit errors for missing bases
and cycles. A child replaces an inherited property as a whole.

```sh
cargo run --locked --features http --example card -- 33915 V09
```

Set loading accepts case-insensitive codes and includes associated `_DO.xml`
fragments. It requests only referenced lookup tables, including those inside
nested groups. Clone bases outside the selected sets must be loaded as well;
otherwise loading reports the missing object ID. For the complete dataset, use
`client.load_card_database(&application)`. This is an explicit download that can
exceed 100 MiB. Both loading methods validate all loaded objects before returning.
`live()` and `discover()` continue to fetch only the two manifests.

Use `database.object(id)` for any digital object, including tokens, boosters,
decks, and containers. `database.card(id)` requires MTGO's `CARD` object-type
code. `object.raw()` preserves the child record, `object.fields()` exposes all
resolved fields, and `database.raw_object(id)` avoids resolution entirely.
MTGO text and mana syntax are preserved; rendering and normalization are left
to callers.

## Offline or custom transport

The default feature set has no HTTP client. Use your own transport or saved
manifests with the same catalog API:

```rust
use mtgoxml::{Deployment, LIVE_DEPLOYMENT_URL};

let deployment_xml = std::fs::read("fixtures/live/MTGO.application")?;
let deployment = Deployment::from_bytes(LIVE_DEPLOYMENT_URL, &deployment_xml)?;
println!("Fetch next: {}", deployment.application_manifest_url());

let application_xml = std::fs::read("fixtures/live/MTGO.exe.manifest")?;
let application = deployment.with_application_bytes(&application_xml)?;
println!("{} resources", application.resources().len());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Use `Deployment::from_manifest` and `Deployment::with_application` with parsed
`clickonce::Manifest` values to configure parsing yourself. Supply the final
application response URL to `with_application` if your transport followed a
redirect.

The raw parsers and resolver work without the `http` feature:

```rust
use mtgoxml::carddata::CardDataFile;

// Use the application snapshot discovered or parsed above.
let mut database = application.card_database();
let set = CardDataFile::from_reader(std::fs::File::open("client_V09.xml")?)?;
database.insert(&application, set)?;
// Insert its lookup tables and any other set fragments in the same way.
database.validate()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Components may be inserted in any order. `CardDataFile` distinguishes `Set` and
`Lookup`; source properties use `FieldValue::{Reference, Literal, Flag, Fields}`.
Byte input accepts UTF-8 with an optional BOM. The parser limits each component
to 16 MiB and 250,000 XML nodes by default, and property nesting to 64 levels.
`carddata::ParseOptions` customizes byte/node limits; the HTTP client accepts
these through `with_card_data_options`. DTD processing is disabled: the manifest
already lists the component files, so the master index needs no entity expansion.
The published DTD is not enforced as a complete schema.

Each database is bound to its application's manifest URL and identity. Inserting
with another application context returns `SourceMismatch`; duplicate tables or
object IDs are rejected without changing existing data. Caller-supplied bytes
rely on the supplied source context; signatures and payload hashes are not
verified. Keep cached components associated with their original application
snapshot.

To validate a saved dataset, pass a directory containing the component basenames:

```sh
cargo run --locked --example database -- DEPLOYMENT_URL MTGO.application MTGO.exe.manifest CardDataSource 33915
```

## Resources

- `resources()` includes files and installed assemblies, including optional
  groups. Runtime and OS prerequisites are excluded. Unrecognized dependency
  relationships are excluded rather than assumed downloadable.
- `xml_resources()` selects XML files; `card_data_resources()` selects everything
  declared inside `CardDataSource`, including its DTD.
- `resource(path)` looks up a logical path, accepting either slash style and
  ignoring ASCII case. File and assembly metadata retain size, hashes, group,
  optional status, and entry kind. Full manifests remain accessible.
- File names are literal paths, so URL metacharacters are percent-encoded.
  Assembly codebases are URI references; URL escaping and queries are preserved.
  Windows directory separators resolve consistently on every platform.
- `mapFileExtensions="true"` adds `.deploy` to payload URLs. The application
  manifest itself is fetched without that suffix. An omitted mapping flag
  defaults to false.

Discovery rejects ambiguous application references, mismatched application
identities, duplicate logical paths, and unsupported URL schemes. File paths must
be relative and contain no empty or dot segments. These checks establish catalog
consistency; signatures and payload hashes are retained without verification.
The crate parses CardDataSource XML; other resource formats remain available as
download locations and manifest metadata. It does not install or launch MTGO.

## Development

MSRV: **Rust 1.85**; edition 2024.

```sh
cargo check --locked --all-targets --all-features
cargo test --locked --no-default-features
cargo test --locked --all-targets --all-features
cargo test --locked --doc --all-features
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
evenflow check . --strict
```

Tests use synthetic manifests, a captured live deployment, and small derived
CardDataSource fixtures with provenance. Assertions concern the checked-in
captures rather than the mutable service. HTTP integration tests use a local
server and check selective loading, lookup resolution, and payload limits.

This fresh implementation depends on the new `clickonce` Git API, pinned to a
specific commit. Before publishing to crates.io, release that parser and replace
the Git dependency with its registry version; the old published `clickonce`
0.0.3 has a different API. The old `mtgoxml` source is not used.

Release tooling requires Node **24.15 or newer**. `npm ci && npm test` checks
release-note generation and the Cargo plugin's version preparation. The checked-in
version is `0.0.0`; semantic-release supplies release versions. CI validates
Rust 1.85 and stable on Linux, macOS, and Windows, plus MSRV, coverage,
documentation, release tooling, and flow comments through the Evenflow GitHub
Action. Evenflow must pass before release and the required `Success` check.
Renovate tracks the pinned action and binary versions. Pull requests dry-run
release analysis; publishing runs only on pushes to `main`. Set
`CARGO_REGISTRY_TOKEN` before enabling a registry release after the dependency
is published.

The initial `0.0.0` tag is a release-history baseline. The first `feat:` commit
after it selects `0.1.0`; the baseline tag must be included when pushing the branch.
