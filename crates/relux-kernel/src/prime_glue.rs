//! Brain-AUTHORED tool-glue program parsing — the chat-turn counterpart of the
//! operator-driven `POST /v1/relux/prime/glue/preview` route
//! (`docs/RELUX_MASTER_PLAN.md` §23; `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §2).
//!
//! ## Why this exists
//!
//! `relux_core::tool_glue` grounds a structured `(plugin, tool, args)` program into an
//! INERT preview, and the glue-preview HTTP route lets an OPERATOR author one by hand. The
//! remaining gap (the one §23 "What remains" records) is letting Prime's BRAIN author the
//! same program from a natural-language multi-step request inside a chat turn and surface
//! the SAME inert preview card. This module is the parse half of that: it lifts a validated
//! [`BrainGluePlan`] out of the unified decision envelope's `glue` section so the kernel can
//! ground it through the EXISTING [`crate::KernelState::preview_tool_glue_plan`] path — no
//! new execution model, no new authority.
//!
//! ## What makes this safe (binding)
//!
//! - **Parsing is structural only — it never grounds or executes.** This module decides
//!   only whether the brain emitted a well-formed program; whether each step names a REAL
//!   tool is decided later by [`relux_core::ground_tool_glue_plan`] against the live
//!   catalog (the allowlist-before-dispatch gate), which fails an unknown tool closed and
//!   blocks the one-click commit. So an unknown / made-up tool is NOT rejected here — it is
//!   carried through and rendered honestly as `unknown`.
//! - **A glue program only ever shapes a turn whose reconciled intent is
//!   `ToolPlanRequest`.** That intent is in the SENSITIVE set
//!   ([`crate::prime_intent::is_sensitive_intent`]), so the fail-closed
//!   [`crate::prime_intent::reconcile_intent`] gate forbids guarded chat (a greeting, an
//!   insult, frustration, a vague musing/question, a brainstorm) from ever being promoted
//!   to it. Casual chat therefore can never become a glue plan — exactly the boundary the
//!   keyword multi-tool path already enforces.
//! - **Fail closed on a malformed shape.** An unknown top-level / per-step field, a
//!   non-array `steps`, an empty program, or a non-object step drops the WHOLE section
//!   (`Err`), so the deterministic keyword `ProposeToolPlan` path stands as the fallback.
//!   The brain is strictly additive — it can author a sharper structured program, but it
//!   can never smuggle an un-modeled key past the parser (openclaw's
//!   `additionalProperties: false` discipline).
//! - **Bounded, never silently truncated.** At most [`MAX_PARSED_GLUE_STEPS`] steps are
//!   parsed — one more than the absolute step ceiling — so an over-long program still
//!   reaches grounding with enough length for the honest "too many steps" report rather
//!   than being clipped into looking valid.
//!
//! ## Reference-driven design (see `docs/reference-driven-development.md`)
//!
//! - **openclaw** `src/agents/tools/update-plan-tool.ts` `readPlanSteps` — a structured
//!   plan is validated FIELD-BY-FIELD and per-entry against an allowlist; a bad shape is an
//!   input error, never silent acceptance/truncation. Mirrored here: every step is an
//!   object with only the allowed keys, or the section fails closed.
//! - **openclaw** `src/shared/balanced-json.ts` `extractBalancedJsonPrefix` — lift the JSON
//!   object from a noisy reply with a balanced-brace scan, reused via
//!   [`crate::prime_intent::extract_json_object`].

use crate::prime_intent::extract_json_object;
use relux_core::{ProposedGlueStep, MAX_TASK_TOOL_PLAN_STEPS_CEIL};

/// Max characters kept from the optional `goal` echo (provenance only).
const MAX_GOAL_CHARS: usize = 400;

/// Max characters kept from a step's `plugin` / `tool` reference before grounding.
const MAX_REF_CHARS: usize = 160;

/// Max steps parsed out of one brain-authored program. Deliberately ONE above the absolute
/// step ceiling ([`MAX_TASK_TOOL_PLAN_STEPS_CEIL`]): an over-long program is still bounded
/// (an adversarial brain cannot make us allocate an unbounded Vec), but it reaches
/// [`relux_core::ground_tool_glue_plan`] with `len() > max`, so the over-cap is reported
/// HONESTLY there rather than hidden by a silent clip into a valid-looking plan.
pub const MAX_PARSED_GLUE_STEPS: usize = MAX_TASK_TOOL_PLAN_STEPS_CEIL + 1;

/// The only top-level keys a `glue` section may carry. Any other key fails the section
/// closed — the brain may not smuggle an un-modeled key in as authority.
const ALLOWED_KEYS: &[&str] = &["goal", "steps", "extended"];

/// The only keys a single glue STEP may carry (mirrors [`ProposedGlueStep`]'s fields).
const ALLOWED_STEP_KEYS: &[&str] = &["plugin", "tool", "args"];

/// A validated, brain-AUTHORED tool-glue program lifted from the unified decision envelope.
///
/// Only [`parse_glue_plan`] builds this, and only after rejecting unknown fields and
/// requiring a non-empty, well-formed `steps` array. It is a PROPOSAL: the steps are NOT
/// yet grounded against the live catalog (that happens in
/// [`crate::KernelState::preview_tool_glue_plan`]), so a step here may still name a tool
/// that does not exist — it will render as `unknown` and block the one-click commit.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainGluePlan {
    /// The goal the program was authored for, echoed onto the preview card for provenance.
    /// `None` when the brain omitted it (the kernel falls back to the user's message).
    pub goal: Option<String>,
    /// The ordered `(plugin, tool, args)` steps the model wrote (structurally validated,
    /// not yet catalog-grounded).
    pub steps: Vec<ProposedGlueStep>,
    /// Whether to use the configured EXTENDED tool-plan step limit instead of the standard
    /// one. Bounded by the operator's `PrimeAgentPolicy`, never an unbounded fan-out.
    pub extended: bool,
}

/// Parse a brain reply's `glue` section into a validated [`BrainGluePlan`], or `Err` on any
/// malformed shape (no JSON object, an unknown top-level / per-step field, a non-array or
/// empty `steps`, a non-object step). On `Err` the caller drops just this section and the
/// deterministic keyword `ProposeToolPlan` path stands as the fallback.
///
/// Structural only: it requires each step to be an object carrying ONLY the allowed keys and
/// trims/clamps the references, but it does NOT require the tool to exist — grounding is the
/// allowlist gate, so an unknown tool is carried through to be rendered honestly.
pub fn parse_glue_plan(reply: &str) -> Result<BrainGluePlan, String> {
    let json = extract_json_object(reply).ok_or_else(|| "no JSON object in reply".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "reply was not valid JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "reply was not a JSON object".to_string())?;

    // Fail closed on ANY field outside the allowlist.
    for key in obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(format!("unsupported field '{key}'"));
        }
    }

    let steps_val = obj
        .get("steps")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "glue needs a 'steps' array".to_string())?;
    if steps_val.is_empty() {
        return Err("a tool-glue program needs at least one step".to_string());
    }

    let mut steps: Vec<ProposedGlueStep> = Vec::new();
    for step in steps_val.iter().take(MAX_PARSED_GLUE_STEPS) {
        let step_obj = step
            .as_object()
            .ok_or_else(|| "each glue step must be an object".to_string())?;
        // Fail closed on any per-step field outside the allowlist — the brain may not
        // smuggle a `shell` / `cwd` / `env` key into a step.
        for key in step_obj.keys() {
            if !ALLOWED_STEP_KEYS.contains(&key.as_str()) {
                return Err(format!("unsupported step field '{key}'"));
            }
        }
        let plugin = step_obj
            .get("plugin")
            .and_then(|v| v.as_str())
            .map(clamp_ref)
            .unwrap_or_default();
        let tool = step_obj
            .get("tool")
            .and_then(|v| v.as_str())
            .map(clamp_ref)
            .unwrap_or_default();
        // `args` is forwarded verbatim; a non-object / absent value becomes `{}` (the same
        // default `ProposedGlueStep` and `TaskToolCall` use). Grounding re-validates size.
        let args = match step_obj.get("args") {
            Some(v) if v.is_object() => v.clone(),
            _ => serde_json::json!({}),
        };
        steps.push(ProposedGlueStep { plugin, tool, args });
    }

    let goal = obj
        .get("goal")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().chars().take(MAX_GOAL_CHARS).collect::<String>())
        .filter(|s| !s.is_empty());

    let extended = obj
        .get("extended")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok(BrainGluePlan {
        goal,
        steps,
        extended,
    })
}

/// Trim and length-clamp a `plugin` / `tool` reference. Grounding still decides whether the
/// trimmed reference names a real tool; this only bounds the string.
fn clamp_ref(s: &str) -> String {
    s.trim().chars().take(MAX_REF_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_clean_two_step_program() {
        let raw = r#"{"goal":"inspect then build","steps":[
            {"plugin":"acme","tool":"inspect"},
            {"plugin":"acme","tool":"build","args":{"target":"x"}}
        ],"extended":true}"#;
        let plan = parse_glue_plan(raw).unwrap();
        assert_eq!(plan.goal.as_deref(), Some("inspect then build"));
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].plugin, "acme");
        assert_eq!(plan.steps[0].tool, "inspect");
        assert_eq!(plan.steps[1].args, serde_json::json!({"target":"x"}));
        assert!(plan.extended);
    }

    #[test]
    fn lifts_the_object_out_of_a_noisy_cli_reply() {
        let raw = "Sure, here is the plan:\n```json\n{\"steps\":[{\"plugin\":\"a\",\"tool\":\"b\"}]}\n```\nDone.";
        let plan = parse_glue_plan(raw).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(plan.goal.is_none());
        assert!(!plan.extended);
    }

    #[test]
    fn an_unknown_tool_is_carried_through_not_rejected_here() {
        // Parsing is structural only — grounding is the allowlist gate, so a made-up tool
        // is NOT dropped at parse time; it is carried through to render as `unknown`.
        let raw = r#"{"steps":[{"plugin":"made","tool":"up"}]}"#;
        let plan = parse_glue_plan(raw).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].plugin, "made");
    }

    #[test]
    fn empty_steps_fails_closed() {
        assert!(parse_glue_plan(r#"{"steps":[]}"#).is_err());
        assert!(parse_glue_plan(r#"{"goal":"x"}"#).is_err());
    }

    #[test]
    fn an_unknown_top_level_field_fails_the_section_closed() {
        // A smuggled authority key drops the whole section (fall back to the keyword path).
        let raw = r#"{"steps":[{"plugin":"a","tool":"b"}],"run":true}"#;
        assert!(parse_glue_plan(raw).is_err());
    }

    #[test]
    fn an_unknown_step_field_fails_closed() {
        // A step may not carry a `shell` (or any other) key beyond plugin/tool/args.
        let raw = r#"{"steps":[{"plugin":"a","tool":"b","shell":"rm -rf /"}]}"#;
        assert!(parse_glue_plan(raw).is_err());
    }

    #[test]
    fn a_non_object_step_fails_closed() {
        assert!(parse_glue_plan(r#"{"steps":["acme/build"]}"#).is_err());
    }

    #[test]
    fn over_long_program_is_bounded_but_still_reports_over_cap_length() {
        // One more than the ceiling is kept, so grounding still sees len() > max and can
        // report the over-cap honestly instead of a silently-clipped valid-looking plan.
        let steps: String = (0..(MAX_PARSED_GLUE_STEPS + 50))
            .map(|_| r#"{"plugin":"a","tool":"b"}"#)
            .collect::<Vec<_>>()
            .join(",");
        let raw = format!("{{\"steps\":[{steps}]}}");
        let plan = parse_glue_plan(&raw).unwrap();
        assert_eq!(plan.steps.len(), MAX_PARSED_GLUE_STEPS);
        assert!(plan.steps.len() > MAX_TASK_TOOL_PLAN_STEPS_CEIL);
    }

    #[test]
    fn a_half_specified_step_is_kept_for_honest_grounding() {
        // grounding flags a missing plugin/tool as unknown; parsing does not guess or drop.
        let raw = r#"{"steps":[{"plugin":"acme","tool":""}]}"#;
        let plan = parse_glue_plan(raw).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].tool, "");
    }
}
