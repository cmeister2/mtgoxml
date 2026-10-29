# MTGO data releases

The **MTGO card data** workflow checks the live ClickOnce deployment every six
hours, at 00:17, 06:17, 12:17, and 18:17 UTC. It can also be started from the
Actions tab on `main`. GitHub schedules can be delayed; public repositories must
have activity within 60 days for their schedules to remain enabled.

Each check downloads the deployment and application manifests. A new application
manifest SHA-256 produces a new data release. An unchanged manifest skips the
large CardDataSource download. The tag is
`mtgo-data-v1-<application-manifest-sha256>`; the version in the release title is
informational, so a changed manifest with the same application version still
produces a snapshot.

Data releases appear alongside crate releases, with distinct tags. They do not
replace the repository's latest crate release. Each contains:

| Asset | Contents |
| --- | --- |
| `card-data.json.gz` | Gzip-compressed JSON with resolved digital objects |
| `snapshot.json` | Source URLs, application version, manifest digest, schema version, tag, and object count |
| `MTGO.application` | Original deployment-manifest bytes |
| `MTGO.exe.manifest` | Original application-manifest bytes |
| `SHA256SUMS` | SHA-256 checksums for the other four assets |

The exporter pins the downloaded manifests before fetching components. It checks
declared resource sizes and supported content digests, parses the components,
and resolves every object before producing the export. Failed downloads,
integrity checks, parsing, or resolution prevent publication. Assets are staged
in a draft release, and the release is published only after every upload is
complete. An interrupted draft is retried on the next run. Published snapshots
are retained without replacement.

## JSON schema version 1

The root contains `schema_version`, `source`, and `objects`. `source` records
the final deployment and application-manifest URLs, application version, and
SHA-256 of the original application-manifest bytes. `objects` contains every
digital object, including cards, tokens, and products, in ascending numeric ID
order:

```json
{
  "schema_version": 1,
  "source": {
    "deployment_url": "http://example.test/MTGO.application",
    "application_manifest_url": "http://example.test/MTGO.exe.manifest",
    "application_version": "3.4.159.4708",
    "application_manifest_sha256": "..."
  },
  "objects": [
    {
      "id": 33915,
      "set": {"id": "V09", "age": null, "kind": null},
      "fields": {"CARDNAME_STRING": "Mystical Tutor", "IS_FOIL": true}
    }
  ]
}
```

The illustrated fields are abbreviated. Field names preserve the source XML
names, lookup references are replaced by their text, and clone inheritance is
resolved. Values are strings, `true` for a presence-only flag, or objects for
nested property groups. Unknown fields are retained. Empty strings remain
strings. The `set` metadata describes the object's source set fragment; resolved
properties such as `CARDSETNAME_STRING` remain in `fields`.

The schema version also appears in the tag prefix. A future incompatible export
format can therefore be published without overwriting existing snapshots.

## Run the exporter locally

```sh
cargo build --locked --release --example export --features snapshot
target/release/examples/export probe /tmp/mtgo-snapshot
target/release/examples/export export /tmp/mtgo-snapshot
gzip -dc /tmp/mtgo-snapshot/card-data.json.gz > /tmp/card-data.json
```

The second command downloads manifests; the third explicitly downloads the full
CardDataSource dataset, which exceeds 100 MiB. Local export does not publish a
release. The Actions workflow uses the repository's `GITHUB_TOKEN` with
`contents: write`; no additional secret is needed.
