/** Normalize search comparisons while preserving the original display names. */
export function searchKey(value) {
  return value.normalize("NFKD").toLowerCase().replace(/\p{M}/gu, "")
    .replace(/[’‘']/gu, "").replace(/[^\p{L}\p{N}]+/gu, " ").trim();
}

/** Validate and prepare the small index once, before handling any queries. */
export function prepareIndex(rows) {
  if (!Array.isArray(rows)) throw new Error("The name index is invalid.");
  return rows.map((row) => {
    // Fail loading instead of suggesting a name with an unusable shard address.
    if (!Array.isArray(row) || row.length !== 3 || typeof row[0] !== "string" ||
        !Number.isInteger(row[1]) || row[1] < 0 || row[1] > 255 ||
        !Number.isInteger(row[2]) || row[2] < 1 || row[2] > 15) {
      throw new Error("The name index contains an invalid entry.");
    }
    return { name: row[0], shard: row[1], kindMask: row[2], key: searchKey(row[0]) };
  });
}

/** Find a bounded list of exact, prefix, word-prefix, then substring matches. */
export function findNames(index, query, includeOther = false, limit = 20) {
  const key = searchKey(query);
  if (!key) return [];
  const matches = [];
  for (const entry of index) {
    // Normal card search includes component names, but keeps products optional.
    if (!includeOther && !(entry.kindMask & 3)) continue;
    const at = entry.key.indexOf(key);
    if (at < 0) continue;
    const rank = entry.key === key ? 0 : at === 0 ? 1 : entry.key[at - 1] === " " ? 2 : 3;
    matches.push({ entry, rank });
  }
  matches.sort((a, b) => a.rank - b.rank || a.entry.name.localeCompare(b.entry.name));
  return matches.slice(0, limit).map(({ entry }) => ({
    name: entry.name, shard: entry.shard, kindMask: entry.kindMask,
  }));
}

/** Keep ordinary image URLs and upgrade the verified MTGO artwork host to HTTPS. */
export function artworkUrl(value) {
  if (typeof value !== "string" || !value) return null;
  try {
    // Source data is text, so only explicit network URLs become clickable links.
    const url = new URL(value);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    if (url.hostname === "mtgo.patch.daybreakgames.com") url.protocol = "https:";
    return url.href;
  } catch {
    return null;
  }
}
