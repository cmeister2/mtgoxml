import { findNames, prepareIndex } from "./search.js";

let index = [];
let names = new Map();

self.addEventListener("message", async ({ data }) => {
  try {
    // Load and normalize once; later keystrokes only scan the compact name index.
    if (data.type === "load") {
      const response = await fetch(data.url);
      if (!response.ok) {
        const error = new Error(`Name index request failed (${response.status}).`);
        error.missingIndex = response.status === 404;
        throw error;
      }
      index = prepareIndex(await response.json());
      names = new Map(index.map((entry) => [entry.name, entry]));
      self.postMessage({ type: "ready", count: index.length });
      return;
    }
    if (data.type === "search") {
      self.postMessage({ requestId: data.requestId,
        value: findNames(index, data.query, data.includeOther) });
    } else if (data.type === "resolve") {
      const entry = names.get(data.name);
      self.postMessage({ requestId: data.requestId,
        value: entry ? { name: entry.name, shard: entry.shard, kindMask: entry.kindMask } : null });
    }
  } catch (error) {
    self.postMessage({ type: "error", requestId: data.requestId,
      error: error.message, missingIndex: Boolean(error.missingIndex) });
  }
});
