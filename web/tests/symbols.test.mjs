import assert from "node:assert/strict";
import test from "node:test";
import { decodeSymbols } from "../symbols.js";

const glyphs = (value) => decodeSymbols(value)?.map(({ classes }) => classes.find((name) => name !== "ms" && name !== "ms-cost"));

test("compact costs and brace contents preserve the order of individual symbols", () => {
  assert.deepEqual(glyphs("2WUBRG"), ["ms-2", "ms-w", "ms-u", "ms-b", "ms-r", "ms-g"]);
  assert.deepEqual(glyphs("XXR"), ["ms-x", "ms-x", "ms-r"]);
  assert.deepEqual(decodeSymbols("U")[0], { classes: ["ms", "ms-u", "ms-cost"], label: "blue mana" });
});

test("case distinguishes snow and pawprints, colorless and encoded generic costs", () => {
  assert.deepEqual(glyphs("pP"), ["ms-s", "ms-paw"]);
  assert.deepEqual(decodeSymbols("pP").map(({ label }) => label), ["snow mana", "pawprint"]);
  assert.deepEqual(glyphs("cC"), ["ms-12", "ms-c"]);
  assert.equal(decodeSymbols("P")[0].classes.includes("ms-cost"), false);
});

test("MTGO letters represent generic amounts from ten through twenty", () => {
  assert.deepEqual(glyphs("aG"), ["ms-10", "ms-g"]);
  assert.deepEqual(glyphs("bfk"), ["ms-11", "ms-15", "ms-20"]);
  assert.equal(decodeSymbols("k")[0].label, "20 generic mana");
  assert.equal(decodeSymbols("10"), null);
});

test("hybrid, monocolored hybrid, and Phyrexian codes remain separate", () => {
  assert.deepEqual(glyphs("#wb-#2u-#bp-"), ["ms-wb", "ms-2u", "ms-bp"]);
  assert.deepEqual(decodeSymbols("#bp-")[0], {
    classes: ["ms", "ms-bp", "ms-cost"], label: "black Phyrexian mana",
  });
  assert.deepEqual(glyphs("#cw-#cu-#cb-#cr-#cg-"), ["ms-cw", "ms-cu", "ms-cb", "ms-cr", "ms-cg"]);
});

test("compound hybrid costs and three-part Phyrexian mana consume complete codes", () => {
  assert.deepEqual(glyphs("1#bp-#bp-"), ["ms-1", "ms-bp", "ms-bp"]);
  assert.deepEqual(glyphs("2G#gupU"), ["ms-2", "ms-g", "ms-gup", "ms-u"]);
  assert.deepEqual(glyphs("1R#rwpW"), ["ms-1", "ms-r", "ms-rwp", "ms-w"]);
});

test("resource symbols and legacy aliases have explicit readable labels", () => {
  assert.deepEqual(glyphs("TQEEE"), ["ms-tap", "ms-untap", "ms-e", "ms-e", "ms-e"]);
  assert.deepEqual(glyphs("TK"), ["ms-ticket"]);
  assert.deepEqual(glyphs("CHAOS"), ["ms-chaos"]);
  assert.deepEqual(glyphs("oW"), ["ms-w"]);
  assert.deepEqual(glyphs("oT"), ["ms-tap"]);
  assert.deepEqual(glyphs("u/pH"), ["ms-up", "ms-h"]);
});

test("unrecognized or malformed groups are preserved whole rather than partially decoded", () => {
  for (const value of ["", undefined, " ", "2Mystery", "#unknown-", "#wb", "#2p-", "W<svg>", "{U}", "__proto__"]) {
    assert.equal(decodeSymbols(value), null, String(value));
  }
});
