import { expect, test } from "@playwright/test";

const artworkHost = "mtgo.patch.daybreakgames.com";
const localOrigin = "http://127.0.0.1:4174";
const manaStylesheet = "https://cdn.jsdelivr.net/npm/mana-font@1.18.0/css/mana.min.css";
const previewImage = '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="green"/></svg>';
const artworkUrl = `http://${artworkHost}/example.jpg`;

function card(id, name, fields = {}) {
  // Supply only properties needed by the UI cases, rather than a release snapshot.
  return { id, name, set: "TEST", foil: false,
    mtgo: { DIGITAL_OBJECT_TYPE_CODE_STRING: "CARD" }, ...fields };
}

const shards = {
  "00": { objects: { "Mystical Tutor": [
    card(101, "Mystical Tutor", { art_url: artworkUrl,
      rules_text: "Search your library for an instant or sorcery card.",
      flavor_text: "@iA student searches for @imeaning@i.@i" }),
    card(102, "Mystical Tutor", { foil: true, art_url: artworkUrl,
      flavor_text: "@iAn unfinished thought",
      mtgo: { DIGITAL_OBJECT_TYPE_CODE_STRING: "CARD", CLONE_ID: "101" } }),
  ] }, relations: {} },
  "01": { objects: {
    "Delver of Secrets": [card(201, "Delver of Secrets", {
      rules_text: "Choose one @-\\n** Add {U}.\\n** Pay {#wb-} or {#bp-#bp-}. @i({p} is snow mana.)@i\\n{P} {PP} {Q} {TK} {a}",
    })],
    "Insectile Aberration": [card(202, "Insectile Aberration")],
  }, relations: {
    "201": { other_face: { id: 202, name: "Insectile Aberration", shard: 1 } },
    "202": { other_face: { id: 201, name: "Delver of Secrets", shard: 1 } },
  } },
  "02": { objects: {
    "Fire // Ice": [card(401, "Fire // Ice")],
    Fire: [card(402, "Fire", { mana_cost: "2#wb-#bp-p",
      mtgo: { DIGITAL_OBJECT_TYPE_CODE_STRING: "SUBC" } })],
    Ice: [card(403, "Ice", { mtgo: { DIGITAL_OBJECT_TYPE_CODE_STRING: "SUBC" } })],
  }, relations: {
    "401": { split_faces: [
      { id: 402, name: "Fire", shard: 2 }, { id: 403, name: "Ice", shard: 2 },
    ] },
    "402": { parent: { id: 401, name: "Fire // Ice", shard: 2 } },
  } },
  "03": { objects: {
    "Éowyn, Shieldmaiden": [card(501, "Éowyn, Shieldmaiden", {
      mtgo: { DIGITAL_OBJECT_TYPE_CODE_STRING: "CARD", PRINTED_STATS: { POWER: "*", TOUGHNESS: "X" } },
    })],
    Treasure: [card(601, "Treasure", { mtgo: { DIGITAL_OBJECT_TYPE_CODE_STRING: "TOKN" } })],
    "Testing Bundle": [card(701, "Testing Bundle", {
      mtgo: { DIGITAL_OBJECT_TYPE_CODE_STRING: "BSTR" },
      rules_text: "A [b]Toolkit [i]contents[/i][/b] description. [u]Unknown[/u] [/b] <Token.Name=Faction>.",
    })],
    "Literal <img> example": [card(801, "Literal <img> example", {
      rules_text: '<img src=x onerror="window.sourceWasExecuted=true">\\n@iFixture text@i\\n[script]Unknown[/script] {NO_SUCH_SYMBOL}',
      flavor_text: 'Plain @i<img src=x onerror="window.flavorWasExecuted=true">@i\\n@-Attribution',
    })],
  }, relations: {} },
};
// Explicit shard addresses and relationships test the browser contract; Rust tests
// cover generating those addresses and resolving the original MTGO references.
const index = [
  ["Mystical Tutor", 0, 1], ["Delver of Secrets", 1, 1], ["Insectile Aberration", 1, 1],
  ["Fire // Ice", 2, 1], ["Fire", 2, 2], ["Ice", 2, 2],
  ["Éowyn, Shieldmaiden", 3, 1], ["Treasure", 3, 4], ["Testing Bundle", 3, 8],
  ["Literal <img> example", 3, 1],
];
const manifest = {
  schema_version: 1, application_version: "1.2.3.4",
  object_count: Object.values(shards).flatMap((shard) => Object.values(shard.objects).flat()).length,
  index: "data/mock/index.json", shards: "data/mock/",
};
const data = new Map([
  ["/mtgoxml/data/current.json", manifest], ["/mtgoxml/data/mock/index.json", index],
  ...Object.entries(shards).map(([id, shard]) => [`/mtgoxml/data/mock/${id}.json`, shard]),
]);

function dataResponse(url) {
  // Unknown bundle paths reproduce deployment removal without generated files.
  return data.has(url.pathname) ? { json: data.get(url.pathname) } :
    { status: 404, body: "No such browser data." };
}

test.beforeEach(async ({ context }) => {
  // Serve frontend files normally; all catalog and CDN responses stay in memory.
  await context.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin === localOrigin) {
      if (url.pathname.startsWith("/mtgoxml/data/")) await route.fulfill(dataResponse(url));
      else await route.continue();
    } else if (url.href === manaStylesheet) {
      await route.fulfill({ contentType: "text/css", body: ".ms { display: inline-block; width: 1em; height: 1em; }" });
    } else if (url.hostname === artworkHost) {
      await route.fulfill({ contentType: "image/svg+xml", body: previewImage });
    } else {
      await route.abort("blockedbyclient");
    }
  });
});

async function openCatalog(page, fragment = "") {
  // A ready search control means both the manifest and worker-owned index loaded.
  await page.goto("./" + fragment);
  await expect(page.locator("#card-search")).toBeEnabled();
}

async function chooseName(page, query, name = query) {
  // Match the exact displayed name so the tests also catch duplicate-name results.
  await page.locator("#card-search").fill(query);
  await page.getByRole("option", { name, exact: true }).click();
  await expect(page.locator("#card-name")).toHaveText(name);
}

async function rawObject(page) {
  return JSON.parse(await page.locator("#raw-data").textContent());
}

test("loads only the index initially and one detail shard on selection", async ({ page, context }) => {
  const paths = [];
  context.on("request", (request) => paths.push(new URL(request.url()).pathname));
  await openCatalog(page);
  await expect(page.locator("#catalog-version")).toContainText(manifest.application_version);
  expect(paths.filter((pathname) => /\/data\/.+\/[0-9a-f]{2}\.json$/.test(pathname))).toEqual([]);
  expect(paths.some((pathname) => pathname.endsWith("/index.json"))).toBe(true);

  await chooseName(page, "Mystical Tutor");
  expect(paths.filter((pathname) => /\/data\/.+\/[0-9a-f]{2}\.json$/.test(pathname))).toHaveLength(1);
  expect(paths.some((pathname) => /card-data|\.xml|\.manifest/.test(pathname))).toBe(false);
  await expect(page.locator("#art-link")).toHaveAttribute("href", /^https:\/\/mtgo\.patch\.daybreakgames\.com\//);
  expect(await page.locator("#card-art").getAttribute("crossorigin")).toBeNull();
  await expect.poll(() => page.locator("#card-art").evaluate((image) => image.naturalWidth)).toBe(16);
});

test("keyboard autocomplete and foil selection preserve the exact object", async ({ page }) => {
  await openCatalog(page);
  await page.locator("#card-search").fill("Mystical");
  await expect(page.getByRole("option", { name: "Mystical Tutor", exact: true })).toBeVisible();
  await page.locator("#card-search").press("ArrowDown");
  await page.locator("#card-search").press("Enter");
  await expect(page.locator("#card-name")).toHaveText("Mystical Tutor");
  await page.locator("#printing").selectOption("102");
  await expect.poll(async () => (await rawObject(page)).id).toBe(102);
  expect((await rawObject(page)).foil).toBe(true);
  expect((await rawObject(page)).mtgo.CLONE_ID).toBe("101");
  await expect(page).toHaveURL(/id=102/);
});

test("tokens and products are opt-in and remain distinct objects", async ({ page }) => {
  await openCatalog(page);
  await page.locator("#card-search").fill("Treasure");
  await expect(page.locator("#status")).toHaveText("No matching names.");
  await expect(page.getByRole("option", { name: "Treasure", exact: true })).toHaveCount(0);
  await page.locator("#include-other").check();
  await page.getByRole("option", { name: "Treasure", exact: true }).click();
  await expect(page.locator("#card-name")).toHaveText("Treasure");
  expect((await rawObject(page)).id).toBe(601);
  await expect(page.locator("#no-art")).toBeVisible();
  await chooseName(page, "Testing Bundle");
  expect((await rawObject(page)).mtgo.DIGITAL_OBJECT_TYPE_CODE_STRING).toBe("BSTR");
});

test("double-faced links select the resolved target and expose its return link", async ({ page }) => {
  await openCatalog(page);
  await chooseName(page, "Delver of Secrets");
  await page.locator("#related-links").getByRole("link", { name: /Insectile Aberration/ }).click();
  await expect(page.locator("#card-name")).toHaveText("Insectile Aberration");
  expect((await rawObject(page)).id).toBe(202);
  await expect(page.locator("#related-links").getByRole("link", { name: /Delver of Secrets/ })).toBeVisible();
});

test("split cards expose both components and their parent", async ({ page }) => {
  await openCatalog(page);
  await chooseName(page, "Fire // Ice");
  await expect(page.locator("#related-links").getByRole("link", { name: /Ice/ })).toBeVisible();
  await page.locator("#related-links").getByRole("link", { name: /Fire/ }).click();
  await expect(page.locator("#card-name")).toHaveText("Fire");
  expect((await rawObject(page)).id).toBe(402);
  await expect(page.locator("#related-links").getByRole("link", { name: /Fire \/\/ Ice/ })).toBeVisible();
});

test("printing share links work under a GitHub Pages project prefix", async ({ page }) => {
  await openCatalog(page, "#name=Mystical+Tutor&id=102");
  await expect(page.locator("#card-name")).toHaveText("Mystical Tutor");
  await expect(page.locator("#printing")).toHaveValue("102");
  expect((await rawObject(page)).id).toBe(102);
  await expect(page.locator("#share-link")).toHaveAttribute("href", /name=Mystical\+Tutor&id=102/);
});

test("MTGO source markup stays text rather than executable HTML", async ({ page }) => {
  await openCatalog(page);
  await chooseName(page, "Literal", "Literal <img> example");
  await expect(page.locator("#rules-text")).toContainText('<img src=x onerror="window.sourceWasExecuted=true">');
  await expect(page.locator("#rules-text img")).toHaveCount(0);
  expect(await page.evaluate(() => window.sourceWasExecuted)).toBeUndefined();
  expect((await rawObject(page)).rules_text).toContain("@iFixture text@i");
  await expect(page.locator("#rules-text i")).toHaveText("Fixture text");
  await expect(page.locator("#rules-text")).toContainText("[script]Unknown[/script] {NO_SUCH_SYMBOL}");
  await expect(page.locator("#flavor-text i")).toHaveText('<img src=x onerror="window.flavorWasExecuted=true">');
  await expect(page.locator("#flavor-text img")).toHaveCount(0);
  await expect(page.locator("#flavor-text")).toContainText("\n—Attribution");
  expect(await page.evaluate(() => window.flavorWasExecuted)).toBeUndefined();
  expect((await rawObject(page)).flavor_text).toContain("@i<img");
});

test("flavor markers toggle italic and normal spans while raw text is retained", async ({ page }) => {
  await openCatalog(page);
  await chooseName(page, "Mystical Tutor");
  await expect(page.locator("#flavor-text")).toHaveText("A student searches for meaning.");
  await expect(page.locator("#flavor-text i")).toHaveText(["A student searches for ", "."]);
  expect(await page.locator("#flavor-text i").first().evaluate((span) => getComputedStyle(span).fontStyle)).toBe("italic");
  expect((await rawObject(page)).flavor_text).toBe("@iA student searches for @imeaning@i.@i");

  await page.locator("#printing").selectOption("102");
  await expect(page.locator("#flavor-text i")).toHaveText("An unfinished thought");
  await page.locator("#printing").selectOption("101");
  await expect(page.locator("#flavor-text")).toHaveText("A student searches for meaning.");
});

test("rules render modal bullets, dash separators, and italic reminders", async ({ page }) => {
  await openCatalog(page);
  await chooseName(page, "Delver of Secrets");
  const rules = page.locator("#rules-text");
  await expect(rules).toContainText("Choose one —\n• Add");
  await expect(rules).toContainText("• Pay");
  await expect(rules.locator("i")).toContainText("is snow mana.");
  expect(await rules.locator("i").evaluate((element) => getComputedStyle(element).fontStyle)).toBe("italic");
  expect((await rawObject(page)).rules_text).toContain("Choose one @-\\n** Add {U}.");
});

test("rules symbols keep snow, pawprints, hybrid, and Phyrexian meanings distinct", async ({ page }) => {
  await openCatalog(page);
  await expect(page.locator('link[href*="mana-font"]')).toHaveAttribute("href", manaStylesheet);
  await chooseName(page, "Delver of Secrets");
  const rules = page.locator("#rules-text");
  await expect(rules.getByRole("img", { name: "snow mana", exact: true })).toHaveClass(/\bms-s\b/);
  await expect(rules.getByRole("img", { name: "pawprint", exact: true })).toHaveCount(3);
  await expect(rules.getByRole("img", { name: "white or black mana", exact: true })).toHaveClass(/\bms-wb\b/);
  await expect(rules.getByRole("img", { name: "black Phyrexian mana", exact: true })).toHaveCount(2);
  await expect(rules.getByRole("img", { name: "untap", exact: true })).toBeVisible();
  await expect(rules.getByRole("img", { name: "ticket", exact: true })).toBeVisible();
  await expect(rules.getByRole("img", { name: "10 generic mana", exact: true })).toHaveClass(/\bms-10\b/);
  expect((await rawObject(page)).rules_text).toContain("{p}");
  expect((await rawObject(page)).rules_text).toContain("{P} {PP}");
});

test("compact MTGO mana costs render without modifying their source", async ({ page }) => {
  await openCatalog(page);
  await chooseName(page, "Fire");
  const cost = page.locator('#card-fields dt:text-is("Mana cost") + dd');
  await expect(cost.getByRole("img")).toHaveCount(4);
  await expect(cost.getByRole("img", { name: "2 generic mana", exact: true })).toBeVisible();
  await expect(cost.getByRole("img", { name: "white or black mana", exact: true })).toBeVisible();
  await expect(cost.getByRole("img", { name: "black Phyrexian mana", exact: true })).toBeVisible();
  await expect(cost.getByRole("img", { name: "snow mana", exact: true })).toBeVisible();
  expect((await rawObject(page)).mana_cost).toBe("2#wb-#bp-p");
});

test("product BBCode renders supported nesting and keeps unfamiliar tags literal", async ({ page }) => {
  await openCatalog(page);
  await page.locator("#include-other").check();
  await chooseName(page, "Testing Bundle");
  const rules = page.locator("#rules-text");
  await expect(rules.locator("b")).toHaveText("Toolkit contents");
  await expect(rules.locator("b > i")).toHaveText("contents");
  await expect(rules).toContainText("[u]Unknown[/u] [/b] <Token.Name=Faction>.");
  expect((await rawObject(page)).rules_text).toContain("[b]Toolkit [i]contents[/i][/b]");
});

test("accent-insensitive search preserves Unicode names and nested source fields", async ({ page }) => {
  await openCatalog(page);
  await chooseName(page, "Eowyn", "Éowyn, Shieldmaiden");
  const object = await rawObject(page);
  expect(object.name).toBe("Éowyn, Shieldmaiden");
  expect(object.mtgo.PRINTED_STATS).toEqual({ POWER: "*", TOUGHNESS: "X" });
});

test("an unavailable CDN image leaves card details and the source link usable", async ({ page }) => {
  await page.route("https://mtgo.patch.daybreakgames.com/**", (route) => route.fulfill({ status: 404, body: "Missing" }));
  await openCatalog(page);
  await chooseName(page, "Mystical Tutor");
  await expect(page.locator("#art-status")).toHaveText("Artwork could not be loaded");
  await expect(page.locator("#art-link")).toBeVisible();
  await expect(page.locator("#rules-text")).toContainText("Search your library");
});

test("a removed detail bundle refreshes the manifest once and recovers the selection", async ({ page }) => {
  const manifestRequests = [];
  await page.route("**/data/current.json*", async (route) => {
    // Simulate an older cached manifest whose details a newer deployment removed.
    manifestRequests.push(new URL(route.request().url()));
    await route.fulfill({ json: { ...manifest,
      shards: manifestRequests.length === 1 ? "data/removed-bundle/" : manifest.shards,
    } });
  });

  await openCatalog(page);
  await chooseName(page, "Mystical Tutor");
  expect((await rawObject(page)).id).toBe(101);
  expect(manifestRequests).toHaveLength(2);
  expect(manifestRequests[1].searchParams.has("reload")).toBe(true);
  await expect(page.locator("#retry")).toBeHidden();
});

test("a late previous detail response cannot replace the newer card selection", async ({ page }) => {
  await page.addInitScript(() => {
    // Force a late response to finish even when cancellation cannot stop delivery.
    const fetchNormally = window.fetch.bind(window);
    window.fetch = (input, options) => {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href);
      if (/\/data\/.+\/[0-9a-f]{2}\.json$/.test(url.pathname)) {
        return fetchNormally(input, { ...options, signal: undefined });
      }
      return fetchNormally(input, options);
    };
  });

  let releaseFirst;
  let firstRequested;
  const allowFirstResponse = new Promise((resolve) => { releaseFirst = resolve; });
  const firstRequest = new Promise((resolve) => { firstRequested = resolve; });
  let heldUrl;
  await page.route(/\/data\/.+\/[0-9a-f]{2}\.json$/, async (route) => {
    // Deliver the first selection only after a different card has rendered.
    if (!heldUrl) {
      heldUrl = route.request().url();
      firstRequested();
      const response = dataResponse(new URL(heldUrl));
      await allowFirstResponse;
      await route.fulfill(response);
    } else {
      await route.fallback();
    }
  });

  try {
    await openCatalog(page);
    await page.locator("#card-search").fill("Mystical Tutor");
    await page.getByRole("option", { name: "Mystical Tutor", exact: true }).click();
    await firstRequest;
    await chooseName(page, "Delver of Secrets");
    const lateResponse = page.waitForResponse((response) => response.url() === heldUrl);
    releaseFirst();
    await (await lateResponse).finished();
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect(page.locator("#card-name")).toHaveText("Delver of Secrets");
    expect((await rawObject(page)).id).toBe(201);
    await expect(page).toHaveURL(/name=Delver\+of\+Secrets/);
  } finally {
    releaseFirst();
  }
});
