# Development

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

CI runs stable and the MSRV on Linux, macOS, and Windows, plus separate MSRV,
coverage, documentation, and release-tooling checks. The Evenflow GitHub Action
must pass before release and the required `Success` check. Renovate tracks the
pinned action and binary versions.

## Documentation

The README is the quick start. `docs/usage.md` and `docs/card-data.md` are included
directly in the crate and module Rustdoc; their Rust examples are doctests. Keep
method-specific behavior and errors on the corresponding public items.

```sh
cargo doc --locked --all-features --no-deps --open
RUSTDOCFLAGS='-D warnings' cargo doc --locked --all-features --no-deps
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-default-features --no-deps
```

Docs.rs is configured to show all features. New Markdown guides included by
Rustdoc must also be included in the Cargo package.

## Fixtures

Tests use synthetic manifests, a captured deployment, and small derived
CardDataSource samples with provenance. Assertions describe those checked-in
captures rather than the mutable live service. HTTP integration tests use a
local server. Third-party fixture rights remain with their original owners.

## Releases

The crate uses the published `clickonce` 0.1 parser. Configure the repository's
`CARGO_REGISTRY_TOKEN` secret before enabling registry publication.

Release tooling requires Node **24.15 or newer**. Run `npm ci && npm test` after
changing it. The regression checks release-note generation and the Cargo
plugin's preparation of manifest and lockfile versions.

The checked-in crate version stays `0.0.0`; semantic-release injects release
versions. Tags use plain SemVer. The initial `0.0.0` tag is a history baseline,
so the first `feat:` after it selects `0.1.0`. Include that tag when pushing the
branch.

Pull requests dry-run release analysis. Publishing runs only on pushes to `main`.

The separate [data-release workflow](data-releases.md) publishes MTGO snapshots
on a six-hour schedule. Its publication lifecycle is covered by offline tests:

```sh
python3 -m unittest discover -s scripts -p 'test_*.py' -v
```
