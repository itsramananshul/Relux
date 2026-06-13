// Pure helpers for the OpenRouter model picker (PrimeAiSettings).
//
// The roadmap calls out that OpenRouter model IDs are unintuitive and change, so
// operators should pick a model from a real list of names/prices rather than type
// a slug (docs/RELUX_MASTER_PLAN.md "Optional LLM-backed Prime"). These helpers
// format the raw catalog (prices/context), order it sensibly (the currently-
// configured model first), and filter it for the search box. Pure + DOM-free so
// they run under `node --strip-types` (see docs note dashboard-test-tsx-vs-ts-split).

import type { ReluxOpenRouterModel } from "./api.ts";

// Format an OpenRouter per-token USD price string (e.g. "0.0000025") into a
// per-million-tokens figure (what people actually compare), e.g. "$2.50/M".
// Returns null when the price is absent or unparseable, so the UI shows nothing
// rather than a misleading "$0".
export function formatPricePerMillion(price?: string | null): string | null {
  if (price == null) return null;
  const n = Number(price);
  if (!Number.isFinite(n) || n < 0) return null;
  const perM = n * 1_000_000;
  if (perM === 0) return "free";
  // Keep small prices readable: 2 decimals for >= $0.01, else up to 4.
  const digits = perM >= 0.01 ? 2 : 4;
  return `$${perM.toFixed(digits)}/M`;
}

// Format a context-window token count into a compact label, e.g. 128000 -> "128K".
// Returns null when absent.
export function formatContextLength(tokens?: number | null): string | null {
  if (tokens == null || !Number.isFinite(tokens) || tokens <= 0) return null;
  if (tokens >= 1_000_000) {
    const m = tokens / 1_000_000;
    return `${Number.isInteger(m) ? m : m.toFixed(1)}M ctx`;
  }
  if (tokens >= 1_000) {
    const k = tokens / 1_000;
    return `${Number.isInteger(k) ? k : Math.round(k)}K ctx`;
  }
  return `${tokens} ctx`;
}

// A one-line secondary label for a model row: name (if distinct from id),
// price (prompt -> completion per million), and context. Omits parts that are
// not advertised so a spartan model still reads cleanly.
export function modelMetaLine(model: ReluxOpenRouterModel): string {
  const parts: string[] = [];
  const ctx = formatContextLength(model.context_length);
  if (ctx) parts.push(ctx);
  const prompt = formatPricePerMillion(model.prompt_price);
  const completion = formatPricePerMillion(model.completion_price);
  if (prompt && completion) {
    parts.push(`in ${prompt} · out ${completion}`);
  } else if (prompt) {
    parts.push(`in ${prompt}`);
  }
  return parts.join(" · ");
}

// The display label for a model in the picker: the human name when present,
// otherwise the id. The id is always shown elsewhere as the saved value.
export function modelDisplayName(model: ReluxOpenRouterModel): string {
  const name = model.name?.trim();
  return name && name.length > 0 ? name : model.id;
}

// Order the catalog for the picker: float the currently-configured model to the
// very top (so the operator sees their current choice first), and otherwise keep
// OpenRouter's server order (which already groups sensibly). Stable and pure.
export function orderModels(
  models: ReluxOpenRouterModel[],
  currentModelId?: string | null,
): ReluxOpenRouterModel[] {
  const current = currentModelId?.trim();
  if (!current) return [...models];
  const head = models.filter((m) => m.id === current);
  const rest = models.filter((m) => m.id !== current);
  return [...head, ...rest];
}

// Case-insensitive filter over id, name, and description for the search box. An
// empty/whitespace query returns the list unchanged.
export function filterModels(
  models: ReluxOpenRouterModel[],
  query: string,
): ReluxOpenRouterModel[] {
  const q = query.trim().toLowerCase();
  if (!q) return models;
  return models.filter((m) => {
    const hay = `${m.id} ${m.name ?? ""} ${m.description ?? ""}`.toLowerCase();
    return hay.includes(q);
  });
}
