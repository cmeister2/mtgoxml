import assert from "node:assert/strict";
import test from "node:test";
import { manaCostTokens, textTokens } from "../format.js";

const plain = (tokens) => tokens.filter((token) => token.type === "text").map((token) => token.text).join("");

test("rules reminders toggle italics and reset for the next field", () => {
  const tokens = textTokens("Flying @i(Reminder.)@i Plain @iUnclosed");
  assert.equal(plain(tokens), "Flying (Reminder.) Plain Unclosed");
  assert.equal(tokens.find((token) => token.text === "(Reminder.)").italic, true);
  assert.equal(tokens.find((token) => token.text === " Plain ").italic, false);
  assert.equal(tokens.at(-1).italic, true);
  assert.equal(textTokens("Plain")[0].italic, false);
});

test("modal bullets and dashes preserve line breaks and ordinary asterisks", () => {
  assert.equal(plain(textTokens("Choose one @-\\n** First\\n  ** Second\\nA ** B; **not a marker")),
    "Choose one —\n• First\n  • Second\nA ** B; **not a marker");
});

test("supported product tags nest while unmatched closing and unknown tags stay literal", () => {
  const tokens = textTokens("[b]Toolkit [i]contents[/i][/b] [url]literal[/url] [/i]");
  assert.equal(plain(tokens), "Toolkit contents [url]literal[/url] [/i]");
  assert.equal(tokens.find((token) => token.text === "Toolkit ").bold, true);
  assert.equal(tokens.find((token) => token.text === "contents").italic, true);
  assert.equal(tokens.find((token) => token.text === "contents").bold, true);
  assert.equal(tokens.at(-1).bold, false);
  assert.equal(textTokens("[B]Unclosed")[0].bold, true);
});

test("source HTML and unknown brace groups remain literal", () => {
  const source = '<img src=x onerror="alert(1)"> <Token.Name=Faction> {Unknown} {outer{W}}';
  assert.equal(plain(textTokens(source)), source);
  assert.equal(textTokens(source).some((token) => token.type === "symbol"), false);
});

test("symbols remain accessible within italic reminder text", () => {
  const tokens = textTokens("@iPay {W}.@i");
  const symbol = tokens.find((token) => token.type === "symbol");
  assert.ok(symbol.classes.includes("ms"));
  assert.ok(symbol.classes.includes("ms-w"));
  assert.match(symbol.label, /white/i);
  assert.equal(symbol.italic, true);
});

test("compact mana costs are complete symbol sequences or unchanged literal fallbacks", () => {
  const symbols = manaCostTokens("2R");
  assert.equal(symbols.length, 2);
  assert.ok(symbols.every((token) => token.type === "symbol"));
  assert.equal(plain(manaCostTokens("2<unknown>R")), "2<unknown>R");
  assert.equal(plain(manaCostTokens(undefined)), "—");
});
