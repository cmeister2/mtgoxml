# Card browser

Search by card name, choose a printing, and view MTGO details and CDN artwork.
Rules and flavor text display supported formatting and mana symbols using
[Mana](https://mana.andrewgioia.com/) from jsDelivr. **Full MTGO data** shows the
original record, including unmodified text and properties.

The [animated avatar gallery](https://cmeister2.github.io/mtgoxml/avatars/)
provides quality-90 WebP previews and a
[JSON index](https://cmeister2.github.io/mtgoxml/avatars/index.json). Rendered
entries point to `/mtgoxml/avatars/assets/<sha256>.webp`, so identical previews
share one asset URL. Pages publishes the selected snapshot; it does not promise
to retain historical avatar URLs. Versioned
[data releases](data-releases.md#animated-avatars-schema-version-1) preserve the
complete avatar index and source/rendering archive.

The gallery index uses schema 2. Each avatar retains its release metadata,
including the path, size and checksum of its lossless WebP. Rendered avatars
also have a `preview` object with the website copy's `url`, `sha256`, `size`,
frame counts and encoding settings (`lossless: false`, `quality: 90`). URLs are
relative to the gallery index. The top-level index and archive checksums identify
the original release files.

The site build verifies the release assets before compressing them, preserving
their canvas, playback duration and infinite looping. It writes the smaller
copies only into the generated website; the release files remain lossless.

## Local preview

Download all assets from a schema-3 data release, then build and serve the site:

```sh
gh release download mtgo-data-3.4.159.4708 --repo cmeister2/mtgoxml --dir /tmp/mtgo-snapshot
cargo run --locked --release -p mtgoxml-export -- site /tmp/mtgo-snapshot _site
npm run serve
```

Open `http://localhost:4173/`. The preview server requires Node and binds to the
local computer. It needs no npm dependencies.

The output directory must be absent or empty. Site generation verifies the
snapshot and runs offline; viewing artwork requests images from the MTGO CDN.
Images may be crops or include a frame. Some objects have no artwork URL.
The avatar gallery uses locally generated site assets when the selected snapshot
contains completed avatar sidecars. Legacy releases require the native
[`avatars` command](data-releases.md#run-the-exporter-locally) to add those assets locally.

## Deployment

Configure GitHub Pages to publish using **GitHub Actions**. The
[Card browser workflow](../.github/workflows/pages.yml) deploys after successful
MTGO data checks, relevant changes on `main`, or a manual run from Actions.

It uses `git-get-release-action` to select the first published, non-prerelease
release named `MTGO card data <version>` in GitHub's listing. It downloads that
snapshot and builds the site with the Rust exporter. New data releases include
the completed avatar sidecars and are used directly.

For an older selected release without `animated_avatars` metadata, the workflow
copies the downloaded files into an isolated `avatar-snapshot` directory and
runs the native `avatars` command against its captured manifest. The downloaded
snapshot and published release remain unchanged. Headless rendering libraries
are installed only when this generation is needed.

The completed legacy copy is cached using the exact downloaded files, including
`snapshot.json` and `SHA256SUMS`, plus the native renderer, exporter sources, and
Cargo dependency lockfile. The cache saves work; the versioned data release is
the archival source. Cache hits still undergo the exporter's snapshot, checksum,
and avatar validation before site publication. Failed generation or validation
does not publish or save a partial cache and leaves the existing deployment
intact. If a legacy cache is unavailable, generation captures the CDN bytes
available at that time.

Frontend sources live in `web/` and are embedded in the exporter. The generated
site contains a name index and detail shards loaded on selection; generated data
stays out of Git history. See [data releases](data-releases.md) for the source
format.

## Tests

```sh
npm ci
npx --no-install playwright install chromium
npm run test:web
npm run test:browser
```

Browser tests serve `web/` under the `/mtgoxml/` project path and supply catalog
and image responses in memory.
