// Preset replacement pack for WORDS (§5.3). Data + a client-side merge. These
// wire as REPLACEMENT RULES, not vocabulary: the highest-value entries are
// spelling/multi-word transforms ("get hub" -> "GitHub") that casing-only
// vocabulary cannot express. Every entry is visible/editable/removable in the
// REPLACEMENTS ledger.
import type { Replacement } from "../bindings";

// Spoken punctuation -> literal. Compound forms precede their singles: rules
// apply in array order (deterministic_rule_order_cascades), so "colon" must not
// run before "double colon" or it eats the compound's tail.
export const CODE_SYMBOLS: Replacement[] = [
  { heard: "open brace", printed: "{" }, { heard: "close brace", printed: "}" },
  { heard: "open bracket", printed: "[" }, { heard: "close bracket", printed: "]" },
  { heard: "open paren", printed: "(" }, { heard: "close paren", printed: ")" },
  { heard: "open angle bracket", printed: "<" }, { heard: "close angle bracket", printed: ">" },
  { heard: "fat arrow", printed: "=>" }, { heard: "thin arrow", printed: "->" },
  { heard: "triple backtick", printed: "```" },
  { heard: "double colon", printed: "::" },
  { heard: "double ampersand", printed: "&&" },
  { heard: "double pipe", printed: "||" },
  { heard: "backtick", printed: "`" },
  { heard: "colon", printed: ":" }, { heard: "semicolon", printed: ";" },
  { heard: "pipe symbol", printed: "|" }, { heard: "ampersand", printed: "&" },
  { heard: "underscore", printed: "_" }, { heard: "backslash", printed: "\\" },
  { heard: "forward slash", printed: "/" }, { heard: "dollar sign", printed: "$" },
  { heard: "hash symbol", printed: "#" }, { heard: "at sign", printed: "@" },
  { heard: "percent sign", printed: "%" }, { heard: "asterisk", printed: "*" },
  { heard: "tilde", printed: "~" }, { heard: "caret", printed: "^" },
]; // 29 entries

/** Append pack entries missing from `existing` (dedupe by heard, trimmed +
 *  lower-cased). Mutates `existing`; returns count added. Re-adding = no-op;
 *  a user-edited row (same heard, changed printed) is preserved. */
export function addPack(existing: Replacement[], pack: Replacement[]): number {
  const have = new Set(existing.map((r) => r.heard.trim().toLowerCase()));
  let added = 0;
  for (const e of pack) {
    const key = e.heard.trim().toLowerCase();
    if (have.has(key)) continue;
    existing.push({ heard: e.heard, printed: e.printed });
    have.add(key);
    added++;
  }
  return added;
}

// Self-check (no test runner in repo): a re-add must be a no-op, not a dup.
{
  const l: Replacement[] = [];
  const first = addPack(l, CODE_SYMBOLS);
  const again = addPack(l, CODE_SYMBOLS);
  console.assert(again === 0 && l.length === first, "addPack must dedupe by heard on re-add");
}
