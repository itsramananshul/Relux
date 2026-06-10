// Pure, dependency-free derivation of the first-run "connect Prime to a brain"
// step from the live control-plane status (RELUX_MASTER_PLAN §11 Dashboard / §22
// Home: the dynamic first-run checklist reflects health status). This is the
// onboarding brain of Relux Home — given the AI status and the installed adapter
// list, it returns exactly one guided step that tells the user whether Prime is
// connected to a real brain and, if not, the precise next action and where to do
// it (always Health → Prime Brain / AI Runtime, never the legacy Crew path).
//
// Kept React-free (like ./routing) so `node --test` can assert the guidance
// without a DOM. ReluxHome renders whatever this returns; it invents nothing.

import type {
  ReluxAiStatus,
  ReluxAdapterStatus,
  ReluxPrimeBrain,
} from "./api";

export const CLAUDE_ADAPTER_ID = "relux-adapter-claude-cli";
export const CODEX_ADAPTER_ID = "relux-adapter-codex-cli";

// "done"  — Prime is connected to a working brain; nothing to do.
// "todo"  — a brain is selected but not usable yet; there is an exact next step.
// "link"  — Prime works (deterministic) but a richer brain is one click away.
export type OnboardingStatus = "done" | "todo" | "link";

export interface OnboardingStep {
  id: string;
  label: string;
  status: OnboardingStatus;
  description: string;
  linkTo: string;
}

// The dashboard surface that owns brain + adapter setup (PrimeBrainPanel).
const BRAIN_SETUP_PATH = "/health";

function adapterFor(
  adapters: ReluxAdapterStatus[] | null,
  id: string,
): ReluxAdapterStatus | null {
  return (adapters ?? []).find((a) => a.plugin_id === id) ?? null;
}

// Whether either coding-agent CLI is already installed on the user's PATH, so the
// guidance can say "detected — one click away" instead of "install it first".
export function anyCliOnPath(adapters: ReluxAdapterStatus[] | null): boolean {
  return (adapters ?? []).some(
    (a) =>
      (a.plugin_id === CLAUDE_ADAPTER_ID || a.plugin_id === CODEX_ADAPTER_ID) &&
      a.available_on_path,
  );
}

function cliLabel(brain: "claude_cli" | "codex_cli"): { name: string; bin: string; id: string } {
  return brain === "claude_cli"
    ? { name: "Claude CLI", bin: "claude", id: CLAUDE_ADAPTER_ID }
    : { name: "Codex CLI", bin: "codex", id: CODEX_ADAPTER_ID };
}

// Derive the single guided brain step. `ai` null means the control plane was not
// reachable for AI status; we still return an actionable link rather than hiding
// the step.
export function primeBrainStep(
  ai: ReluxAiStatus | null,
  adapters: ReluxAdapterStatus[] | null,
): OnboardingStep {
  const base = { id: "prime-brain", linkTo: BRAIN_SETUP_PATH } as const;

  if (!ai) {
    return {
      ...base,
      label: "Connect Prime to a brain",
      status: "link",
      description:
        "Open Health → Prime Brain / AI Runtime to choose who answers Prime: " +
        "Local (deterministic), Claude CLI, Codex CLI, or OpenRouter.",
    };
  }

  const brain: ReluxPrimeBrain = ai.brain;

  if (brain === "claude_cli" || brain === "codex_cli") {
    const cli = cliLabel(brain);
    const adapter = adapterFor(adapters, cli.id);
    if (adapter && adapter.state === "available") {
      return {
        ...base,
        label: `Prime brain: ${cli.name}`,
        status: "done",
        description: `Prime is answering through your local ${cli.name}. Ask Prime a normal message to try it.`,
      };
    }
    // A CLI brain is selected but not usable yet — give the exact reason + fix.
    let description: string;
    if (!adapter) {
      description = `The ${cli.name} adapter is not installed. Open Health → Prime Brain / AI Runtime to set it up.`;
    } else if (!adapter.available_on_path) {
      description =
        `${cli.name} is selected but \`${cli.bin}\` is not on your PATH. Install and sign in ` +
        `(the panel shows the exact command), then click "Use ${cli.name} for Prime" on ` +
        `Health → Prime Brain / AI Runtime and Refresh.`;
    } else if (!adapter.enabled) {
      description =
        `${cli.name} is selected but its adapter is disabled. Click "Use ${cli.name} for Prime" ` +
        `on Health → Prime Brain / AI Runtime to enable it.`;
    } else {
      description = adapter.detail || `${cli.name} is selected but not ready yet. See Health → Prime Brain.`;
    }
    return { ...base, label: `Prime brain: ${cli.name} (needs setup)`, status: "todo", description };
  }

  if (brain === "openrouter") {
    if (ai.configured && !ai.disabled) {
      return {
        ...base,
        label: "Prime brain: OpenRouter",
        status: "done",
        description: `Prime is answering through OpenRouter (${ai.model}).`,
      };
    }
    const description = ai.configured
      ? "OpenRouter is selected but LLM replies are disabled. Re-enable them on Health → Prime AI settings."
      : "OpenRouter is selected but no API key is set. Add one on Health → Prime AI settings.";
    return { ...base, label: "Prime brain: OpenRouter (needs setup)", status: "todo", description };
  }

  // brain === "local": Prime works, but a real coding-agent brain is one click
  // away. Not a failure — an opportunity, tailored to whether a CLI is detected.
  const detected = anyCliOnPath(adapters);
  return {
    ...base,
    label: "Connect Prime to Claude or Codex (recommended)",
    status: "link",
    description: detected
      ? "A Claude or Codex CLI is already on your PATH. Open Health → Prime Brain / AI Runtime " +
        'and click "Use Claude CLI for Prime" to give Prime a natural voice. Prime works without ' +
        "it, but stays deterministic."
      : "Prime is using the built-in deterministic brain. For natural chat, install the Claude or " +
        "Codex CLI, then connect it on Health → Prime Brain / AI Runtime. No API key or JSON editing needed.",
  };
}
