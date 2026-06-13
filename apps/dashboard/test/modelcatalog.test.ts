// Unit tests for the OpenRouter model-picker helpers (modelcatalog.ts), pinning
// the doc-specified UX (docs/RELUX_MASTER_PLAN.md "Optional LLM-backed Prime":
// pick a model by name/price, current model first, manual slug as fallback). Pure
// helpers — run under `node --strip-types`, no DOM (docs note
// dashboard-test-tsx-vs-ts-split).
//
// Run: `npm test` (auto-discovered) or `node --test test/modelcatalog.test.ts`.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  formatPricePerMillion,
  formatContextLength,
  modelMetaLine,
  modelDisplayName,
  orderModels,
  filterModels,
} from "../src/modelcatalog.ts";
import type { ReluxOpenRouterModel } from "../src/api.ts";

function m(partial: Partial<ReluxOpenRouterModel> & { id: string }): ReluxOpenRouterModel {
  return { ...partial };
}

test("formatPricePerMillion converts per-token USD to a per-million figure", () => {
  // $0.0000025/token -> $2.50 per million tokens.
  assert.equal(formatPricePerMillion("0.0000025"), "$2.50/M");
  // A sub-cent-per-million price keeps more precision so it isn't rounded to $0.00.
  assert.equal(formatPricePerMillion("0.000000005"), "$0.0050/M");
  // A price >= $0.01/M uses two decimals.
  assert.equal(formatPricePerMillion("0.00000015"), "$0.15/M");
  // Zero reads as "free", not "$0.00/M".
  assert.equal(formatPricePerMillion("0"), "free");
  // Absent / unparseable -> null (UI shows nothing, never a misleading $0).
  assert.equal(formatPricePerMillion(null), null);
  assert.equal(formatPricePerMillion(undefined), null);
  assert.equal(formatPricePerMillion("n/a"), null);
});

test("formatContextLength renders a compact token window", () => {
  assert.equal(formatContextLength(128000), "128K ctx");
  assert.equal(formatContextLength(1000000), "1M ctx");
  assert.equal(formatContextLength(900), "900 ctx");
  assert.equal(formatContextLength(null), null);
  assert.equal(formatContextLength(0), null);
});

test("modelMetaLine combines context and prompt/completion price", () => {
  const line = modelMetaLine(
    m({ id: "a/b", context_length: 128000, prompt_price: "0.0000025", completion_price: "0.00001" }),
  );
  assert.match(line, /128K ctx/);
  assert.match(line, /in \$2\.50\/M/);
  assert.match(line, /out \$10\.00\/M/);
});

test("modelMetaLine omits parts that aren't advertised", () => {
  // Only an id -> empty meta line (the row still shows the id as its name).
  assert.equal(modelMetaLine(m({ id: "a/b" })), "");
});

test("modelDisplayName prefers the human name, falls back to the id", () => {
  assert.equal(modelDisplayName(m({ id: "a/b", name: "Cool Model" })), "Cool Model");
  assert.equal(modelDisplayName(m({ id: "a/b" })), "a/b");
  assert.equal(modelDisplayName(m({ id: "a/b", name: "   " })), "a/b");
});

test("orderModels floats the currently-configured model to the top", () => {
  const models = [m({ id: "a/1" }), m({ id: "a/2" }), m({ id: "a/3" })];
  const ordered = orderModels(models, "a/3");
  assert.deepEqual(ordered.map((x) => x.id), ["a/3", "a/1", "a/2"]);
  // No current model -> server order preserved (a copy, not mutated input).
  const same = orderModels(models, null);
  assert.deepEqual(same.map((x) => x.id), ["a/1", "a/2", "a/3"]);
  assert.notEqual(same, models);
});

test("filterModels matches id, name, and description, case-insensitively", () => {
  const models = [
    m({ id: "openai/gpt-4o", name: "OpenAI: GPT-4o" }),
    m({ id: "anthropic/claude", name: "Anthropic Claude", description: "great at code" }),
    m({ id: "meta/llama" }),
  ];
  assert.deepEqual(filterModels(models, "gpt").map((x) => x.id), ["openai/gpt-4o"]);
  assert.deepEqual(filterModels(models, "CLAUDE").map((x) => x.id), ["anthropic/claude"]);
  // Matches on description too.
  assert.deepEqual(filterModels(models, "code").map((x) => x.id), ["anthropic/claude"]);
  // Empty query -> unchanged.
  assert.equal(filterModels(models, "  ").length, 3);
});
