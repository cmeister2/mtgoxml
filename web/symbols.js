/** MTGO's compact codes are case-sensitive: p is snow, P is a pawprint. */
const SIMPLE_SYMBOLS = {
  W: ["w", "white mana", true],
  U: ["u", "blue mana", true],
  B: ["b", "black mana", true],
  R: ["r", "red mana", true],
  G: ["g", "green mana", true],
  C: ["c", "colorless mana", true],
  X: ["x", "X mana", true],
  Y: ["y", "Y mana", true],
  Z: ["z", "Z mana", true],
  H: ["h", "Phyrexian mana", true],
  p: ["s", "snow mana", true],
  P: ["paw", "pawprint", false],
  T: ["tap", "tap", true],
  Q: ["untap", "untap", true],
  E: ["e", "energy", false],
};

/** Only supported hybrid forms become glyph classes; source text never does. */
const HYBRID_SYMBOLS = {
  wu: "white or blue mana", wb: "white or black mana",
  ub: "blue or black mana", ur: "blue or red mana",
  br: "black or red mana", bg: "black or green mana",
  rw: "red or white mana", rg: "red or green mana",
  gw: "green or white mana", gu: "green or blue mana",
  cw: "colorless or white mana", cu: "colorless or blue mana",
  cb: "colorless or black mana", cr: "colorless or red mana",
  cg: "colorless or green mana",
  "2w": "2 generic or white mana", "2u": "2 generic or blue mana",
  "2b": "2 generic or black mana", "2r": "2 generic or red mana",
  "2g": "2 generic or green mana",
  wp: "white Phyrexian mana", up: "blue Phyrexian mana",
  bp: "black Phyrexian mana", rp: "red Phyrexian mana",
  gp: "green Phyrexian mana",
  wup: "white or blue Phyrexian mana", wbp: "white or black Phyrexian mana",
  ubp: "blue or black Phyrexian mana", urp: "blue or red Phyrexian mana",
  brp: "black or red Phyrexian mana", bgp: "black or green Phyrexian mana",
  rwp: "red or white Phyrexian mana", rgp: "red or green Phyrexian mana",
  gwp: "green or white Phyrexian mana", gup: "green or blue Phyrexian mana",
};

/** Construct only fixed Mana classes and a readable accessible name. */
function descriptor(code, label, cost = true) {
  return { classes: ["ms", `ms-${code}`, ...(cost ? ["ms-cost"] : [])], label };
}

/** Decode one complete MTGO brace group or compact mana cost; preserve unknowns. */
export function decodeSymbols(value) {
  if (typeof value !== "string" || !value || /[0-9]{2}/.test(value)) return null;
  const symbols = [];
  let offset = 0;
  while (offset < value.length) {
    // Compound brace groups can contain repeated hybrid, colored, or resource codes.
    const remaining = value.slice(offset);
    if (remaining.startsWith("CHAOS")) {
      symbols.push(descriptor("chaos", "chaos", false));
      offset += 5;
      continue;
    }
    if (remaining.startsWith("TK")) {
      symbols.push(descriptor("ticket", "ticket", false));
      offset += 2;
      continue;
    }
    if (remaining.startsWith("u/p")) {
      symbols.push(descriptor("up", "blue Phyrexian mana"));
      offset += 3;
      continue;
    }
    if (remaining.startsWith("#")) {
      // Two-part hybrids terminate with a dash; three-part Phyrexian codes do not.
      const match = /^#([wubrg][wubrg]p|[2cwubrg][wubrgp]-)/.exec(remaining);
      if (!match) return null;
      const code = match[1].replace(/-$/, "");
      if (!Object.hasOwn(HYBRID_SYMBOLS, code)) return null;
      symbols.push(descriptor(code, HYBRID_SYMBOLS[code]));
      offset += match[0].length;
      continue;
    }
    if (remaining[0] === "o") {
      // Training-card flavor uses legacy oW/oT spellings for individual symbols.
      const legacy = SIMPLE_SYMBOLS[remaining[1]];
      if (!legacy) return null;
      symbols.push(descriptor(...legacy));
      offset += 2;
      continue;
    }
    const code = remaining[0];
    if (/^[0-9a-k]$/.test(code)) {
      const amount = /^[0-9]$/.test(code) ? Number(code) : code.charCodeAt(0) - 87;
      symbols.push(descriptor(String(amount), `${amount} generic mana`));
      offset += 1;
      continue;
    }
    if (!Object.hasOwn(SIMPLE_SYMBOLS, code)) return null;
    symbols.push(descriptor(...SIMPLE_SYMBOLS[code]));
    offset += 1;
  }
  return symbols;
}
