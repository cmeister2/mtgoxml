import { decodeSymbols } from "./symbols.js";

/** Find the end of a complete brace group, leaving nested or unfinished groups literal. */
function braceEnd(text, start) {
  let depth = 0;
  for (let index = start; index < text.length; index += 1) {
    // A group is consumed whole so unknown syntax cannot expose an inner symbol.
    if (text[index] === "{") depth += 1;
    if (text[index] === "}" && --depth === 0) return index + 1;
  }
  return text.length;
}

/** Parse supported MTGO formatting without treating source strings as HTML. */
export function textTokens(value) {
  if (typeof value !== "string" || !value) return [];
  const source = value.replace(/\\n/g, "\n")
    .replace(/^([ \t]*)\*\*(?=\s|$)[ \t]*/gm, "$1• ");
  const tokens = [];
  let italic = false;
  let italicDepth = 0;
  let boldDepth = 0;
  let position = 0;

  /** Append literal text, joining adjacent runs with the same formatting. */
  function appendText(text) {
    if (!text) return;
    const previous = tokens[tokens.length - 1];
    const style = { italic: italic || italicDepth > 0, bold: boldDepth > 0 };
    if (previous?.type === "text" && previous.italic === style.italic && previous.bold === style.bold) {
      previous.text += text;
    } else {
      tokens.push({ type: "text", text, ...style });
    }
  }

  const markers = /@i|@-|\[\/?[bBiI]\]|\{/g;
  for (let match = markers.exec(source); match; match = markers.exec(source)) {
    // Only recognized markers alter formatting; everything between them stays text.
    appendText(source.slice(position, match.index));
    position = markers.lastIndex;
    const marker = match[0];
    if (marker === "@i") {
      italic = !italic;
    } else if (marker === "@-") {
      appendText("—");
    } else if (marker === "{") {
      const end = braceEnd(source, match.index);
      const group = source.slice(match.index, end);
      const symbols = group.endsWith("}") ? decodeSymbols(group.slice(1, -1)) : null;
      if (symbols) {
        // Symbol classes and labels come from the local, explicit decoder.
        for (const symbol of symbols) {
          tokens.push({ type: "symbol", ...symbol, italic: italic || italicDepth > 0, bold: boldDepth > 0 });
        }
      } else {
        appendText(group);
      }
      position = end;
      markers.lastIndex = end;
    } else {
      const tag = marker.toLowerCase();
      if (tag === "[i]") italicDepth += 1;
      else if (tag === "[b]") boldDepth += 1;
      else if (tag === "[/i]" && italicDepth > 0) italicDepth -= 1;
      else if (tag === "[/b]" && boldDepth > 0) boldDepth -= 1;
      else appendText(marker);
    }
  }
  appendText(source.slice(position));
  return tokens;
}

/** Decode one complete compact mana cost, preserving unfamiliar costs as literal text. */
export function manaCostTokens(value) {
  if (typeof value !== "string" || !value) {
    return [{ type: "text", text: "—", italic: false, bold: false }];
  }
  const symbols = decodeSymbols(value);
  return symbols ? symbols.map((symbol) => ({ type: "symbol", ...symbol, italic: false, bold: false })) :
    [{ type: "text", text: value, italic: false, bold: false }];
}

/** Build explicit style elements and accessible symbol spans from trusted tokens. */
function renderTokens(container, tokens) {
  const document = container.ownerDocument;
  const fragment = document.createDocumentFragment();
  const stack = [];
  for (const token of tokens) {
    // Reuse an outer style when only a nested italic or bold run changes.
    const styles = [...(token.bold ? ["b"] : []), ...(token.italic ? ["i"] : [])];
    let common = 0;
    while (common < stack.length && stack[common].tag === styles[common]) common += 1;
    stack.length = common;
    let parent = stack.length ? stack[stack.length - 1].node : fragment;
    for (const tag of styles.slice(common)) {
      const node = document.createElement(tag);
      parent.append(node);
      stack.push({ tag, node });
      parent = node;
    }
    if (token.type === "symbol") {
      const icon = document.createElement("span");
      icon.classList.add(...token.classes);
      icon.setAttribute("role", "img");
      icon.setAttribute("aria-label", token.label);
      icon.title = token.label;
      parent.append(icon);
    } else {
      parent.append(document.createTextNode(token.text));
    }
  }
  container.replaceChildren(fragment);
}

/** Render rules or flavor text with supported markup and literal fallback for unknown syntax. */
export function renderText(container, value) {
  renderTokens(container, textTokens(value));
}

/** Render a compact mana-cost field with Mana's symbol classes. */
export function renderManaCost(container, value) {
  renderTokens(container, manaCostTokens(value));
}
