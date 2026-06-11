// Pure helpers for the crew skills/tags field. Kept React-free (a plain .ts module)
// so it is directly unit-testable under `node --strip-types` — see
// apps/dashboard/test/skills.test.ts and the dashboard-test-tsx-vs-ts split note.
//
// These MIRROR the backend bounds in crates/relux-kernel/src/agent_config.rs
// (`sanitize_skill` / `validate_skills`): a skill is a short slug ([a-z0-9-]), each
// clamped to MAX_SKILL_CHARS and the list bounded to MAX_SKILLS. The backend is the
// authority and re-validates everything; this is only for a tidy UI + honest preview.

export const MAX_SKILL_CHARS = 32;
export const MAX_SKILLS = 16;

// Reduce one raw token to the strict slug shape the backend stores: lowercase, keep
// only [a-z0-9], collapse any other run to a single hyphen, trim hyphens, clamp length.
// Returns "" when nothing valid remains (the caller drops it).
export function slugifySkill(raw: string): string {
  const lowered = raw.trim().toLowerCase();
  let out = "";
  let lastHyphen = false;
  for (const ch of lowered) {
    if (/[a-z0-9]/.test(ch)) {
      out += ch;
      lastHyphen = false;
    } else if (!lastHyphen) {
      out += "-";
      lastHyphen = true;
    }
    if (out.length >= MAX_SKILL_CHARS) break;
  }
  return out.replace(/^-+|-+$/g, "").slice(0, MAX_SKILL_CHARS);
}

// Parse a comma-separated (or whitespace/newline-separated) skills input into a bounded,
// deduped slug list. Empty fragments are dropped; duplicates are removed (first wins);
// the result is capped at MAX_SKILLS. The backend re-validates and is the real authority.
export function parseSkillsInput(raw: string): string[] {
  const out: string[] = [];
  for (const part of raw.split(/[,\n]/)) {
    const slug = slugifySkill(part);
    if (slug && !out.includes(slug)) out.push(slug);
    if (out.length >= MAX_SKILLS) break;
  }
  return out;
}

// Render a skills array back into the comma-separated form shown in the edit field.
export function formatSkillsInput(skills: string[] | undefined): string {
  return (skills ?? []).join(", ");
}
