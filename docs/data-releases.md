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
| `snapshot.json` | Source URLs, application version, manifest digest, schema version, tag, and object count |
| `MTGO.application` | Original deployment-manifest bytes |
| `MTGO.exe.manifest` | Original application-manifest bytes |
| `SHA256SUMS` | SHA-256 checksums for the other four assets |

The exporter pins the downloaded manifests before fetching components. It checks
declared resource sizes and supported content digests, parses the components,
and resolves every object before producing the export. Failed downloads,
integrity checks, parsing, or resolution prevent publication. The workflow uses
`softprops/action-gh-release` to stage assets in a draft and publish after every
upload completes. An interrupted draft is retried on the next run. Published
snapshots are retained without replacement.

## JSON Lines schema version 2

Each [JSON Lines](https://jsonlines.org/) record is one complete JSON object,
including cards, tokens, and products, in ascending numeric ID order.
Records use UTF-8 without a byte-order mark and
end with LF. Newlines inside strings are escaped, so each record stays on one
physical line:

```jsonl
{"id":33914,"set":{"id":"V09","age":null,"kind":null},"fields":{"CARDNAME_STRING":"Mystical Tutor"}}
{"id":33915,"set":{"id":"V09","age":null,"kind":null},"fields":{"CARDNAME_STRING":"Mystical Tutor","IS_FOIL":true}}
```

The illustrated fields are abbreviated. Field names preserve the source XML
names, lookup references are replaced by their text, and clone inheritance is
resolved. Values are strings, `true` for a presence-only flag, or objects for
nested property groups. Unknown fields are retained. Empty strings remain
strings. The `set` metadata describes the object's source set fragment; resolved
properties such as `CARDSETNAME_STRING` remain in `fields`.

`snapshot.json` records `schema_version: 2`, the source URLs, application
version, manifest SHA-256, release tag, and object count. Provenance is kept
there rather than repeated on every record. Release tags continue to track the
upstream MTGO application version.

The format supports streaming through tools such as `jq` without reading a
single enclosing array. For example, `gzip -dc card-data.jsonl.gz | head -n 1`
prints the first object.

### Earlier snapshots

Schema version 1 used a single JSON document with an `objects` array in
`card-data.json.gz`. Already published releases retain their original assets;
the new format starts with the next application version. A downloaded schema 1
file can be converted locally:

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
