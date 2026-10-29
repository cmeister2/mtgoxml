# mtgoxml

Discover Magic: The Gathering Online resources from its ClickOnce deployment.
The library follows the deployment's application-manifest reference and builds
download URLs for every declared file and downloadable assembly. Resource
discovery follows versioned paths from the manifests rather than hardcoding a
particular MTGO build.

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
The crate discovers locations and metadata; it does not parse MTGO-specific XML,
download payloads, install, or launch MTGO.

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

Tests use synthetic manifests and a captured live deployment, with provenance in
`fixtures/live/capture.json`. Assertions concern the checked-in capture rather
than the mutable service. HTTP integration tests use a local server.

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
