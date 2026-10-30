# Card browser

Search by card name, choose a printing, and view MTGO details and CDN artwork.
Rules and flavor text display supported formatting and mana symbols using
[Mana](https://mana.andrewgioia.com/) from jsDelivr. **Full MTGO data** shows the
original record, including unmodified text and properties.

## Local preview

Download all assets from a schema-3 data release, then build and serve the site:

```sh
gh release download mtgo-data-3.4.159.4708 --repo cmeister2/mtgoxml --dir /tmp/mtgo-snapshot
cargo run --locked --release --bin mtgoxml-export --features snapshot -- site /tmp/mtgo-snapshot _site
npm run serve
```

Open `http://localhost:4173/`. The preview server requires Node and binds to the
local computer. It needs no npm dependencies.

The output directory must be absent or empty. Site generation verifies the
snapshot and runs offline; viewing artwork requests images from the MTGO CDN.
Images may be crops or include a frame. Some objects have no artwork URL.

## Deployment

Configure GitHub Pages to publish using **GitHub Actions**. The
[Card browser workflow](../.github/workflows/pages.yml) deploys after successful
MTGO data checks, relevant changes on `main`, or a manual run from Actions.

It uses `git-get-release-action` to select the first published, non-prerelease
release named `MTGO card data <version>` in GitHub's listing. It downloads that
snapshot and builds the site with the Rust exporter. Failed builds leave the
existing deployment intact.

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
