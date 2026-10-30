import assert from "node:assert/strict";
import test from "node:test";
import { artworkUrl, findNames, prepareIndex, searchKey } from "../search.js";

test("search compares accents and punctuation without changing display names", () => {
  const index = prepareIndex([["Jötun Grunt", 1, 1], ["Lim-Dûl’s Vault", 2, 1], ["Fire // Ice", 3, 2]]);
  assert.equal(searchKey("Lim-Dûl’s Vault"), "lim duls vault");
  assert.deepEqual(findNames(index, "jotun").map((entry) => entry.name), ["Jötun Grunt"]);
  assert.deepEqual(findNames(index, "lim dul's vault").map((entry) => entry.name), ["Lim-Dûl’s Vault"]);
  assert.deepEqual(findNames(index, "lim duls vault").map((entry) => entry.name), ["Lim-Dûl’s Vault"]);
  assert.deepEqual(findNames(index, "fire ice").map((entry) => entry.name), ["Fire // Ice"]);
});

test("ranking prefers exact, prefix, word prefix, then internal substrings", () => {
  const index = prepareIndex([
    ["Icebreaker Kraken", 1, 1], ["Fire // Ice", 2, 1], ["Ice", 3, 2], ["Splice", 4, 1],
  ]);
  assert.deepEqual(findNames(index, "ice").map((entry) => entry.name),
    ["Ice", "Icebreaker Kraken", "Fire // Ice", "Splice"]);
});

test("card components are searchable by default while other types are optional", () => {
  const index = prepareIndex([["Fire", 1, 2], ["Fire token", 2, 4], ["Fire booster", 3, 8]]);
  assert.deepEqual(findNames(index, "Fire").map((entry) => entry.name), ["Fire"]);
  assert.equal(findNames(index, "Fire", true).length, 3);
  assert.equal(findNames(index, "Fire", true, 1).length, 1);
  assert.deepEqual(findNames(index, "   "), []);
});

test("normalization never merges exact names or loses shard identities", () => {
  const index = prepareIndex([["Jötun", 1, 1], ["Jotun", 2, 1]]);
  assert.deepEqual(new Set(findNames(index, "jotun").map((entry) => entry.shard)), new Set([1, 2]));
  assert.throws(() => prepareIndex([["Broken", 256, 1]]), /invalid entry/);
  assert.throws(() => prepareIndex([["Broken", 1, 0]]), /invalid entry/);
});

test("artwork links upgrade only the verified host and reject executable URLs", () => {
  assert.equal(artworkUrl("http://mtgo.patch.daybreakgames.com/art.jpg"),
    "https://mtgo.patch.daybreakgames.com/art.jpg");
  assert.equal(artworkUrl("http://example.com/art.jpg"), "http://example.com/art.jpg");
  assert.equal(artworkUrl("javascript:alert(1)"), null);
  assert.equal(artworkUrl("not a URL"), null);
  assert.equal(artworkUrl(undefined), null);
});
