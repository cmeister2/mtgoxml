import { artworkUrl } from "./search.js";
import { renderManaCost, renderText } from "./format.js";

const elements = Object.fromEntries([
  "catalog-version", "search-form", "card-search", "search-results", "include-other",
  "status", "retry", "card-details", "printing", "card-name", "card-fields",
  "rules-text", "flavor-section", "flavor-text", "related", "related-links",
  "artwork", "card-art", "art-status", "art-link", "no-art", "share-link", "raw-data",
].map((id) => [id, document.getElementById(id)]));

let manifest;
let worker;
let catalogGeneration = 0;
let requestId = 0;
let searchGeneration = 0;
let selectionGeneration = 0;
let hashGeneration = 0;
let selectionController;
let results = [];
let activeResult = -1;
let currentGroup;
let catalogReady;
const pending = new Map();
const shardCache = new Map();

/** Set the one live status without leaving a previous retry button visible. */
function status(message, failed = false) {
  elements.status.textContent = message;
  elements.retry.hidden = !failed;
}

/** Resolve generated paths against the project root rather than the domain root. */
function dataUrl(path) {
  const url = new URL(path, document.baseURI);
  if (url.origin !== location.origin) throw new Error("Catalog data must be served with this page.");
  return url;
}

/** Send a request to the current index worker and track its matching response. */
function askWorker(type, values = {}) {
  return new Promise((resolve, reject) => {
    // Each response is matched to its caller; keystrokes can finish out of order.
    const id = ++requestId;
    pending.set(id, { resolve, reject });
    worker.postMessage({ type, requestId: id, ...values });
  });
}

/** Load one consistent manifest and replace the worker and cached detail shards. */
async function loadCatalog(fresh = false) {
  const generation = ++catalogGeneration;
  searchGeneration += 1;
  worker?.terminate();
  for (const request of pending.values()) request.reject(new Error("Catalog reloaded."));
  pending.clear();
  shardCache.clear();
  hideResults();
  elements["card-search"].disabled = true;
  elements["include-other"].disabled = true;
  status("Loading the name index…");
  const url = dataUrl("./data/current.json");
  if (fresh) url.searchParams.set("reload", Date.now().toString());
  const response = await fetch(url, { cache: "no-cache" });
  if (!response.ok) throw new Error(`Catalog request failed (${response.status}).`);
  const next = await response.json();
  if (next.schema_version !== 1 || typeof next.index !== "string" ||
      typeof next.shards !== "string" || typeof next.application_version !== "string") {
    throw new Error("This catalog format is not supported by this page.");
  }
  if (generation !== catalogGeneration) return;
  manifest = next;
  worker = new Worker(new URL("./search-worker.js", import.meta.url), { type: "module" });
  try {
    await new Promise((resolve, reject) => {
      // The worker owns the name index, leaving the page free to handle input.
      worker.addEventListener("error", () => {
        const error = new Error("The search worker could not load.");
        for (const request of pending.values()) request.reject(error);
        pending.clear();
        elements["card-search"].disabled = true;
        elements["include-other"].disabled = true;
        status(error.message, true);
        reject(error);
      });
      worker.addEventListener("message", ({ data }) => {
        if (generation !== catalogGeneration) return;
        if (data.type === "ready") {
          resolve();
        } else if (data.requestId !== undefined) {
          const request = pending.get(data.requestId);
          pending.delete(data.requestId);
          if (data.type === "error") request?.reject(new Error(data.error));
          else request?.resolve(data.value);
        } else if (data.type === "error") {
          reject(Object.assign(new Error(data.error), { missingIndex: data.missingIndex }));
        }
      });
      worker.postMessage({ type: "load", url: dataUrl(manifest.index).href });
    });
  } catch (error) {
    // Cached manifests can refer to an index removed by a newer Pages deployment.
    if (error.missingIndex && !fresh) return loadCatalog(true);
    throw error;
  }
  if (generation !== catalogGeneration) return;
  elements["catalog-version"].textContent =
    `MTGO ${manifest.application_version} · ${Number(manifest.object_count).toLocaleString()} objects`;
  elements["card-search"].disabled = false;
  elements["include-other"].disabled = false;
  status("Type a card name to search.");
}

/** Close the popup and its active-descendant relationship together. */
function hideResults() {
  results = [];
  activeResult = -1;
  elements["search-results"].replaceChildren();
  elements["search-results"].hidden = true;
  elements["card-search"].setAttribute("aria-expanded", "false");
  elements["card-search"].removeAttribute("aria-activedescendant");
}

/** Update keyboard highlighting without moving focus away from the search box. */
function activateResult(index) {
  activeResult = index;
  for (const [at, item] of [...elements["search-results"].children].entries()) {
    // Keep assistive technology and the visible highlight on the same result.
    item.setAttribute("aria-selected", String(at === index));
    if (at === index) {
      elements["card-search"].setAttribute("aria-activedescendant", item.id);
      item.scrollIntoView({ block: "nearest" });
    }
  }
}

/** Render only the newest worker result, using text nodes for every source name. */
async function search() {
  const generation = ++searchGeneration;
  const query = elements["card-search"].value;
  if (!query.trim()) {
    hideResults();
    status("Type a card name to search.");
    return;
  }
  try {
    const found = await askWorker("search", {
      query, includeOther: elements["include-other"].checked,
    });
    if (generation !== searchGeneration) return;
    hideResults();
    results = found;
    for (const [index, entry] of results.entries()) {
      // Pointer selection preserves textbox focus and follows the same path as Enter.
      const item = document.createElement("li");
      item.id = `result-${index}`;
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", "false");
      item.textContent = entry.name;
      item.addEventListener("pointerdown", (event) => {
        event.preventDefault();
        void selectEntry(entry);
      });
      elements["search-results"].append(item);
    }
    elements["search-results"].hidden = results.length === 0;
    elements["card-search"].setAttribute("aria-expanded", String(results.length > 0));
    status(results.length ? `${results.length} matching names. Use arrow keys and Enter to choose.` : "No matching names.");
  } catch (error) {
    if (generation === searchGeneration) status(error.message, true);
  }
}

/** Fetch only the requested detail shard and retain at most four recent shards. */
async function loadShard(shard, signal) {
  const filename = `${shard.toString(16).padStart(2, "0")}.json`;
  const url = dataUrl(`${manifest.shards}${filename}`).href;
  if (shardCache.has(url)) return shardCache.get(url);
  const response = await fetch(url, { signal });
  if (!response.ok) {
    const error = new Error(`Card details request failed (${response.status}).`);
    error.missingShard = response.status === 404;
    throw error;
  }
  const data = await response.json();
  if (!data.objects || !data.relations) throw new Error("The card detail file is invalid.");
  shardCache.set(url, data);
  if (shardCache.size > 4) shardCache.delete(shardCache.keys().next().value);
  return data;
}

/** Select an exact-name group, retrying stale bundle paths only once. */
async function selectEntry(entry, selectedId = null, retry = true) {
  const generation = ++selectionGeneration;
  searchGeneration += 1;
  selectionController?.abort();
  selectionController = new AbortController();
  hideResults();
  elements["card-search"].value = entry.name;
  status(`Loading ${entry.name}…`);
  try {
    const shard = await loadShard(entry.shard, selectionController.signal);
    if (generation !== selectionGeneration) return;
    const records = Object.hasOwn(shard.objects, entry.name) ? shard.objects[entry.name] : null;
    if (!Array.isArray(records) || !records.length) throw new Error("This name is missing from the detail file.");
    currentGroup = { entry, records, relations: shard.relations };
    if (selectedId !== null) {
      const record = records.find((item) => String(item.id) === String(selectedId));
      if (!record) throw new Error("This printing is not present in the current catalog.");
      if (!isCard(record)) elements["include-other"].checked = true;
    }
    showPrintings(selectedId);
  } catch (error) {
    if (generation !== selectionGeneration || error.name === "AbortError") return;
    if (error.missingShard && retry) {
      // An older tab can hold a manifest whose bundle a new deployment removed.
      try {
        catalogReady = loadCatalog(true);
        await catalogReady;
        if (generation !== selectionGeneration) return;
        const fresh = await askWorker("resolve", { name: entry.name });
        if (generation !== selectionGeneration) return;
        if (!fresh) throw new Error("This name is no longer in the current catalog.");
        return await selectEntry(fresh, selectedId, false);
      } catch (refreshError) {
        if (generation === selectionGeneration) status(refreshError.message, true);
        return;
      }
    }
    currentGroup = undefined;
    elements["card-details"].hidden = true;
    status(error.message, true);
  }
}

/** Classify records by the explicit MTGO object type, including card components. */
function isCard(record) {
  return ["CARD", "SUBC"].includes(record.mtgo?.DIGITAL_OBJECT_TYPE_CODE_STRING);
}

/** Populate native printing controls while preserving distinct digital objects. */
function showPrintings(selectedId = null) {
  const visible = currentGroup.records.filter((record) => elements["include-other"].checked || isCard(record));
  if (!visible.length) {
    elements["card-details"].hidden = true;
    status("Enable tokens and other objects to view this name.");
    return;
  }
  const selected = visible.find((record) => String(record.id) === String(selectedId)) ||
    visible.find((record) => !record.foil && record.art_url) || visible.find((record) => !record.foil) || visible[0];
  elements.printing.replaceChildren();
  for (const record of visible) {
    // Printing labels stay useful even when a set or collector number is absent.
    const option = document.createElement("option");
    option.value = String(record.id);
    option.textContent = [record.set, record.collector_number, record.foil ? "Foil" : "Nonfoil", `ID ${record.id}`]
      .filter((value) => value !== undefined && value !== "").join(" · ");
    elements.printing.append(option);
  }
  elements.printing.value = String(selected.id);
  renderCard(selected);
  status(`${visible.length.toLocaleString()} printing${visible.length === 1 ? "" : "s"}.`);
}

/** Render a source value as text, distinguishing zero and empty color lists. */
function displayValue(value) {
  if (Array.isArray(value)) return value.length ? value.join(", ") : "Colorless";
  return value === undefined || value === null || value === "" ? "—" : String(value);
}

/** Render the selected object without interpreting any source property as HTML. */
function renderCard(record) {
  const mtgo = record.mtgo || {};
  const fields = [
    ["MTGO ID", record.id], ["Set", record.set], ["Collector number", record.collector_number],
    ["Foil", record.foil ? "Yes" : "No"], ["Mana cost", record.mana_cost],
    ["Mana value", record.mana_value], ["Colors", record.colors], ["Color identity", record.color_identity],
    ["Rarity", record.rarity?.replaceAll("_", " ")], ["Artist", record.artist],
    ["Object type", mtgo.DIGITAL_OBJECT_TYPE_CODE_STRING],
  ];
  if (mtgo.POWERTOUGHNESS_STRING !== undefined) fields.push(["Power / toughness", mtgo.POWERTOUGHNESS_STRING]);
  if (mtgo.LOYALTY !== undefined) fields.push(["Loyalty", mtgo.LOYALTY]);
  elements["card-fields"].replaceChildren();
  for (const [label, value] of fields) {
    // Explicit elements keep nested or unusual source values out of HTML parsing.
    const term = document.createElement("dt");
    const description = document.createElement("dd");
    term.textContent = label;
    if (label === "Mana cost") renderManaCost(description, value);
    else description.textContent = displayValue(value);
    elements["card-fields"].append(term, description);
  }
  elements["card-name"].textContent = record.name;
  renderText(elements["rules-text"], record.rules_text || "No rules text.");
  elements["flavor-section"].hidden = !record.flavor_text;
  renderText(elements["flavor-text"], record.flavor_text);
  elements["raw-data"].textContent = JSON.stringify(record, null, 2);
  const hash = `#${new URLSearchParams({ name: record.name, id: String(record.id) })}`;
  history.replaceState(null, "", hash);
  elements["share-link"].href = hash;
  renderArtwork(record);
  renderRelations(record);
  elements["card-details"].hidden = false;
}

/** Display a normal cross-origin image and a separate direct artwork link. */
function renderArtwork(record) {
  const url = artworkUrl(record.art_url);
  elements.artwork.hidden = !url;
  elements["no-art"].hidden = Boolean(url);
  const image = elements["card-art"];
  image.removeAttribute("src");
  image.hidden = false;
  if (!url) return;
  elements["art-link"].href = url;
  elements["art-status"].textContent = "Artwork";
  image.alt = `${record.name} artwork`;
  image.onerror = () => {
    // Keep the direct URL available even when the CDN image cannot be loaded.
    if (image.src !== url) return;
    image.hidden = true;
    elements["art-status"].textContent = "Artwork could not be loaded";
  };
  image.src = url;
}

/** Link validated relations using their resolved exact names and catalog IDs. */
function renderRelations(record) {
  const relations = currentGroup.relations[String(record.id)] || {};
  const entries = [
    ["Other face", relations.other_face], ["Parent card", relations.parent],
    ["Other half", relations.other_half],
    ...(relations.split_faces || []).map((target) => ["Half", target]),
  ].filter(([, target]) => target);
  elements["related-links"].replaceChildren();
  for (const [label, target] of entries) {
    // Fragments also work as standalone shared links on a project Pages site.
    const link = document.createElement("a");
    link.href = `#${new URLSearchParams({ name: target.name, id: String(target.id) })}`;
    link.textContent = `${label}: ${target.name} (ID ${target.id})`;
    elements["related-links"].append(link);
  }
  elements.related.hidden = entries.length === 0;
}

/** Resolve a shared name through the index before fetching its details. */
async function navigateHash() {
  const generation = ++hashGeneration;
  try {
    await catalogReady;
    if (generation !== hashGeneration) return;
    const parameters = new URLSearchParams(location.hash.slice(1));
    const name = parameters.get("name");
    if (!name) return;
    const entry = await askWorker("resolve", { name });
    if (generation !== hashGeneration) return;
    if (!entry) throw new Error("This name is not present in the current catalog.");
    await selectEntry(entry, parameters.get("id"));
  } catch (error) {
    if (generation === hashGeneration) status(error.message, true);
  }
}

elements["card-search"].addEventListener("input", () => void search());
elements["card-search"].addEventListener("keydown", (event) => {
  // Arrow navigation leaves focus in the combobox until a result is selected.
  if (event.key === "Escape") hideResults();
  if (!results.length) return;
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    const change = event.key === "ArrowDown" ? 1 : -1;
    activateResult(activeResult < 0 && change < 0 ? results.length - 1 :
      (activeResult + change + results.length) % results.length);
  }
});
elements["search-form"].addEventListener("submit", (event) => {
  event.preventDefault();
  if (results.length) void selectEntry(results[Math.max(activeResult, 0)]);
});
elements["include-other"].addEventListener("change", () => {
  if (currentGroup) showPrintings(elements.printing.value);
  void search();
});
elements.printing.addEventListener("change", () => {
  const record = currentGroup?.records.find((item) => String(item.id) === elements.printing.value);
  if (record) renderCard(record);
});
document.addEventListener("pointerdown", (event) => {
  if (!elements["search-form"].contains(event.target)) hideResults();
});
elements.retry.addEventListener("click", () => {
  // Cancel details from the old bundle before installing a fresh manifest.
  selectionGeneration += 1;
  selectionController?.abort();
  currentGroup = undefined;
  elements["card-details"].hidden = true;
  catalogReady = loadCatalog(true);
  void navigateHash();
});
window.addEventListener("hashchange", () => void navigateHash());
catalogReady = loadCatalog();
void navigateHash();
