# Relix Codebase Audit Log

Purpose: persistent shared notes for Codex and Claude so neither agent keeps rereading the same Relix files. Treat this as the handoff map for the Relix strict codebase audit.

Location: this file lives inside `.relix-audit/`, which is excluded locally through `.git/info/exclude`. Do not move it into tracked Relix docs unless Anshul explicitly asks.

Status: strict tracked-file coverage is complete as of 2026-06-05 12:18 America/New_York. The ledger is the source of truth; do not mark a future file read unless it has actually been processed line-by-line or structurally summarized if generated/binary.

Source of truth:

```text
.relix-audit/relix-file-line-coverage.jsonl
.relix-audit/relix-file-line-coverage-progress.md
.relix-audit/relix-file-line-coverage-summary.md
```

## Initial Inventory

# Relix Strict File Coverage Summary

Generated: 2026-06-05T12:09:26.9934228-04:00

- Total tracked rows: 805
- Text files pending semantic read: 802
- Generated/lock structural-summary rows: 3
- Binary inventoried rows: 0
- Text/generated lines mechanically processed: 441479

## By Kind

- generated_or_lock_text: 3
- text: 802

## By Top Folder/File

- crates: 526 files, 354385 lines
- docs: 112 files, 48364 lines
- sdks: 35 files, 6102 lines
- apps: 27 files, 5785 lines
- scripts: 20 files, 6547 lines
- flows: 17 files, 525 lines
- configs: 13 files, 2986 lines
- specs: 12 files, 1424 lines
- examples: 9 files, 2373 lines
- .github: 7 files, 660 lines
- ops: 2 files, 760 lines
- tools: 1 files, 55 lines
- deny.toml: 1 files, 117 lines
- dev-keys: 1 files, 2 lines
- rust-toolchain.toml: 1 files, 4 lines
- tool-probe.txt: 1 files, 1 lines
- install.ps1: 1 files, 671 lines
- install.sh: 1 files, 827 lines
- SECURITY.md: 1 files, 60 lines
- CHANGELOG-SPEC.md: 1 files, 18 lines
- ADVERSARIAL_AUDIT.md: 1 files, 1892 lines
- CLAUDE.md: 1 files, 48 lines
- CHANGELOG.md: 1 files, 422 lines
- .editorconfig: 1 files, 18 lines
- .dockerignore: 1 files, 22 lines
- .yamllint: 1 files, 29 lines
- .gitignore: 1 files, 67 lines
- CODEOWNERS: 1 files, 28 lines
- LICENSE: 1 files, 21 lines
- Dockerfile: 1 files, 108 lines
- README.md: 1 files, 407 lines
- LICENSE-APACHE: 1 files, 201 lines
- Cargo.lock: 1 files, 6167 lines
- CONTRIBUTING.md: 1 files, 160 lines
- Cross.toml: 1 files, 30 lines
- Cargo.toml: 1 files, 193 lines

## Strict Coverage Checkpoint - 2026-06-05

Final tracked-row coverage:

- Total tracked Relix rows: 805
- Semantic line-read text files: 799
- Pending semantic text files: 0
- Generated/lock text structurally summarized: 6
- Binary assets inventoried: 0

Reader attribution:

- Euclid: 294 `crates/relix-runtime/**` text files.
- Goodall: 142 `crates/relix-web-bridge/**`, `crates/relix-controller/**`, `crates/relix-flow-inspect/**`, and `apps/**` source/config/test text files.
- Faraday: 119 `crates/relix-cli/**`, `crates/relix-core/**`, `crates/relix-embedded/**`, `crates/relix-sdk/**`, `crates/relix-plugin-sdk/**`, and `sdks/**` text files.
- Copernicus: 67 `crates/relix-telegram/**`, `crates/relix-discord/**`, `crates/relix-slack/**`, `configs/**`, `flows/**`, and `examples/**` source/config text files.
- Huygens: 175 `docs/**`, `specs/**`, `scripts/**`, `ops/**`, `.github/**`, and assigned root docs/config text files.
- Codex: 2 local cleanup files: `dev-keys/.gitkeep` and `tools/README.md`.

Generated/lock structural rows:

- `Cargo.lock`
- `apps/dashboard/package-lock.json`
- `examples/plugins/web-lookup/Cargo.lock`
- `crates/relix-web-bridge/dashboard-dist/assets/index-2OEgiUAQ.js`
- `crates/relix-web-bridge/dashboard-dist/assets/index-BqYOyCYS.css`
- `crates/relix-web-bridge/dashboard-dist/index.html`

High-signal findings:

- Runtime is substantial: mesh controller runtime, signed identity, replay/policy bridge, approval/audit/budget/PII/metrics, AI/memory/tool/coordinator/channel/plugin/workflow/planning surfaces, and tenant-aware stores are real, not just doc concepts.
- Biggest backend architecture gap: coordinator is durable ledger/product state, but not yet durable resumable execution; comments still say mid-flow VM state and auto-relaunch are not implemented.
- Dashboard/bridge surface is much richer than the old HTML-only impression: React dashboard, first-run/admin auth, product spine, Mandates, Briefs, Crew, Runs, Chat, Settings, maintenance, and review/apply paths exist.
- Biggest product-feel gap: there are overlapping UI surfaces (`apps/dashboard`, legacy `dashboard.html`, `/spine`), generated dist drift risk, and many dashboard API failures can degrade into empty states instead of clear operator guidance.
- CLI/SDK is broad, but CLI bridge auth is inconsistent: some commands intentionally resolve bearer tokens, while many bridge-backed helpers use direct unauthenticated `reqwest` calls and can produce raw 401/403.
- Channel integrations are strongest for Telegram; Slack/Discord are useful but runnable channel-controller config examples are missing.
- Docs broadly align with the six design docs, but status drift exists around CI/cargo-deny and streaming support.
- Security/product risks still material: static peer discovery, unsigned/TOFU-ish manifest limitations, optional replay/watermark safety in some channels, FS jail TOCTOU caveat, open tool policies, no fuzz coverage, and several Paperclip-class surfaces remain partial or roadmap-level.
