// apps/dashboard/scripts/browser-smoke.mjs
//
// Live-browser click smoke for the Relux dashboard. This is the link the
// browser-free render harness (test/*-render.test.mjs) deliberately cannot
// close: the real binding from a button's `onClick` → network → re-render. A
// first-paint test renders a route through StaticRouter, so it never fires an
// effect, never dispatches a click, and never sees a route's data load fail at
// runtime. The reported failure — a page that goes blank AFTER you click
// View/Query/Send/Inspect, or a raw JSON envelope leaking into the chat — only
// shows up in a real browser driving the real bundle against the real kernel.
//
// It adds ZERO npm dependencies and commits NO browser binary (the explicit
// objection recorded in apps/dashboard/README.md). It drives the operator's
// already-installed Chrome/Edge over the Chrome DevTools Protocol using only
// Node's built-in global `fetch` + global `WebSocket` (Node >= 21). The README's
// stated bar was: "If a live-DOM smoke is ever wanted, it should reuse an
// already-present engine, not commit a browser binary." This is exactly that.
//
// What it catches (each is a real user-visible regression a render test misses):
//   - a blank main content area on any of the 8 Relux shell routes,
//   - a sidebar nav link whose click does not actually route / renders nothing,
//   - an uncaught render crash (the ErrorBoundary "This page hit an error" card),
//   - a console error or thrown exception during a route's runtime data load,
//   - an HTTP 5xx (or a failed JS/CSS chunk) behind any clicked surface,
//   - Prime turning a greeting into auto-created work, or leaking the raw turn
//     JSON envelope into the chat instead of the shaped reply,
//   - the Work "Inspect" button → task-detail panel binding (open + close).
//
// It does NOT run on its own: it needs a running kernel that serves /dashboard
// + the live /v1/relux/* control plane on ONE origin (so cookie auth and the
// SPA share an origin, exactly like production). The one-command wrapper
// `scripts/relux-browser-smoke.ps1` boots a release kernel against a throwaway
// DB, seeds one task, and invokes this script. To run it by hand against an
// already-running kernel:
//
//   RELUX_SMOKE_BASE=http://127.0.0.1:19891 \
//   RELUX_SMOKE_USER=admin RELUX_SMOKE_PASS=secret-pass \
//   node apps/dashboard/scripts/browser-smoke.mjs
//
// Env:
//   RELUX_SMOKE_BASE    required. Origin serving /dashboard + /v1/relux (no trailing slash).
//   RELUX_SMOKE_USER    operator username (default "admin").
//   RELUX_SMOKE_PASS    operator password (required for login).
//   RELUX_SMOKE_BROWSER explicit path to chrome.exe / msedge.exe (else auto-detect).
//   RELUX_SMOKE_HEADFUL set to "1" to watch it run (debugging).

import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const BASE = (process.env.RELUX_SMOKE_BASE || "").replace(/\/+$/, "");
const USER = process.env.RELUX_SMOKE_USER || "admin";
const PASS = process.env.RELUX_SMOKE_PASS || "";
const HEADFUL = process.env.RELUX_SMOKE_HEADFUL === "1";

if (!BASE) {
  console.error("RELUX_SMOKE_BASE is required (e.g. http://127.0.0.1:19891).");
  process.exit(2);
}

// ---- tiny PASS/FAIL reporter --------------------------------------------
const results = [];
function record(name, ok, detail = "") {
  results.push({ name, ok, detail });
  const tag = ok ? "PASS" : "FAIL";
  console.log(`  ${tag.padEnd(4)} ${name}${detail ? "  — " + detail : ""}`);
}
function section(t) {
  console.log(`\n>> ${t}`);
}

// ---- locate a Chromium-family browser -----------------------------------
function findBrowser() {
  if (process.env.RELUX_SMOKE_BROWSER) return process.env.RELUX_SMOKE_BROWSER;
  const candidates = [
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
    "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  ];
  return candidates.find((p) => existsSync(p)) || null;
}

function freePort() {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.on("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const port = srv.address().port;
      srv.close(() => resolve(port));
    });
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---- minimal CDP client over the browser WebSocket ----------------------
class CDP {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.listeners = [];
    ws.addEventListener("message", (ev) => {
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch {
        return;
      }
      if (msg.id != null && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) reject(new Error(msg.error.message || JSON.stringify(msg.error)));
        else resolve(msg.result);
      } else if (msg.method) {
        for (const fn of this.listeners) fn(msg.method, msg.params, msg.sessionId);
      }
    });
  }
  on(fn) {
    this.listeners.push(fn);
  }
  send(method, params = {}, sessionId) {
    const id = ++this.id;
    const payload = { id, method, params };
    if (sessionId) payload.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify(payload));
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          reject(new Error(`CDP timeout: ${method}`));
        }
      }, 30000);
    });
  }
}

function connect(url) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    ws.addEventListener("open", () => resolve(ws));
    ws.addEventListener("error", (e) => reject(new Error("ws error: " + (e.message || e.type))));
  });
}

async function main() {
  const browserPath = findBrowser();
  if (!browserPath) {
    console.error("No Chrome/Edge found. Set RELUX_SMOKE_BROWSER to a chrome.exe/msedge.exe path.");
    process.exit(2);
  }
  if (!PASS) {
    console.error("RELUX_SMOKE_PASS is required (the operator password to sign in).");
    process.exit(2);
  }

  console.log("== Relux dashboard live-browser click smoke ==");
  console.log(`  base:    ${BASE}`);
  console.log(`  browser: ${browserPath}`);

  const port = await freePort();
  const userDataDir = mkdtempSync(join(tmpdir(), "relux-smoke-profile-"));
  const args = [
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${userDataDir}`,
    "--remote-allow-origins=*",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-gpu",
    "--disable-extensions",
    "--disable-background-networking",
    "--disable-features=Translate,MediaRouter",
    "--mute-audio",
    "--window-size=1280,900",
  ];
  if (!HEADFUL) args.push("--headless=new");

  const proc = spawn(browserPath, args, { stdio: ["ignore", "ignore", "pipe"] });
  let procExited = false;
  proc.on("exit", () => (procExited = true));
  let cdp = null;

  const cleanup = () => {
    try {
      if (cdp) cdp.ws.close();
    } catch {}
    try {
      if (!procExited) proc.kill();
    } catch {}
    try {
      rmSync(userDataDir, { recursive: true, force: true });
    } catch {}
  };

  // ---- runtime error capture (filled by event listeners) ----------------
  const consoleErrors = [];
  const pageExceptions = [];
  const httpFailures = []; // 5xx or failed document/script chunk
  const apiWarnings = []; // /v1/relux 4xx

  try {
    // Wait for the DevTools endpoint to come up, then grab the browser WS URL.
    let wsUrl = null;
    const deadline = Date.now() + 20000;
    while (Date.now() < deadline) {
      if (procExited) throw new Error("browser process exited before DevTools came up");
      try {
        const r = await fetch(`http://127.0.0.1:${port}/json/version`);
        if (r.ok) {
          const j = await r.json();
          wsUrl = j.webSocketDebuggerUrl;
          if (wsUrl) break;
        }
      } catch {
        /* not up yet */
      }
      await sleep(200);
    }
    if (!wsUrl) throw new Error("DevTools endpoint never reported a webSocketDebuggerUrl");

    const ws = await connect(wsUrl);
    cdp = new CDP(ws);

    // Open a fresh page target and attach to it (flat session protocol).
    const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
    const sid = sessionId;

    await cdp.send("Page.enable", {}, sid);
    await cdp.send("Runtime.enable", {}, sid);
    await cdp.send("Network.enable", {}, sid);

    cdp.on((method, params, msgSid) => {
      if (msgSid !== sid) return;
      if (method === "Runtime.consoleAPICalled") {
        if (params.type === "error") {
          const text = (params.args || [])
            .map((a) => (a.value !== undefined ? a.value : a.description ?? ""))
            .join(" ");
          consoleErrors.push(text.slice(0, 300));
        }
      } else if (method === "Runtime.exceptionThrown") {
        const d = params.exceptionDetails || {};
        const text = d.exception?.description || d.text || "exception";
        pageExceptions.push(String(text).slice(0, 300));
      } else if (method === "Network.responseReceived") {
        const { url, status } = params.response || {};
        if (status >= 500) httpFailures.push(`${status} ${url}`);
        else if (status >= 400 && url && url.includes("/v1/relux")) apiWarnings.push(`${status} ${url}`);
      } else if (method === "Network.loadingFailed") {
        const t = params.type;
        if (
          (t === "Document" || t === "Script" || t === "Stylesheet") &&
          params.errorText !== "net::ERR_ABORTED"
        ) {
          httpFailures.push(`${t} load failed: ${params.errorText}`);
        }
      }
    });

    // ---- DOM helpers (all via Runtime.evaluate on the page session) ------
    async function evaluate(expression) {
      const r = await cdp.send(
        "Runtime.evaluate",
        { expression, returnByValue: true, awaitPromise: true },
        sid,
      );
      if (r.exceptionDetails) {
        const d = r.exceptionDetails;
        throw new Error("evaluate threw: " + (d.exception?.description || d.text));
      }
      return r.result.value;
    }
    async function waitFor(expr, { timeout = 10000, interval = 150, desc = "" } = {}) {
      const end = Date.now() + timeout;
      let last;
      while (Date.now() < end) {
        last = await evaluate(`(() => { try { return !!(${expr}); } catch { return false; } })()`);
        if (last) return true;
        await sleep(interval);
      }
      throw new Error(`waitFor timed out${desc ? " (" + desc + ")" : ""}: ${expr}`);
    }
    async function navigate(url) {
      await cdp.send("Page.navigate", { url }, sid);
    }
    // Snapshot error counts so a step reports only the NEW problems it caused.
    function snapshot() {
      return {
        c: consoleErrors.length,
        e: pageExceptions.length,
        h: httpFailures.length,
      };
    }
    function newProblems(s) {
      const probs = [];
      if (consoleErrors.length > s.c) probs.push(...consoleErrors.slice(s.c).map((x) => "console: " + x));
      if (pageExceptions.length > s.e) probs.push(...pageExceptions.slice(s.e).map((x) => "exception: " + x));
      if (httpFailures.length > s.h) probs.push(...httpFailures.slice(s.h).map((x) => "http: " + x));
      return probs;
    }
    // The Relux shell's main content; non-blank text that is not just the route
    // fallback "Loading…" means the route actually rendered something real.
    const WORKSPACE_TEXT = `(() => { const w = document.querySelector('.workspace'); return w ? (w.innerText||'').trim() : null; })()`;
    async function waitForContent(desc) {
      await waitFor(
        `(() => { const w = document.querySelector('.workspace'); if(!w) return false; const t=(w.innerText||'').trim(); return t.length>1 && t!=='Loading…'; })()`,
        { timeout: 12000, desc },
      );
    }
    async function assertNotBlank(name) {
      const txt = await evaluate(WORKSPACE_TEXT);
      if (txt == null) {
        record(name, false, "no .workspace element (shell not mounted)");
        return false;
      }
      if (txt.length < 2) {
        record(name, false, "main content is blank");
        return false;
      }
      if (txt.includes("This page hit an error")) {
        record(name, false, "ErrorBoundary crash card rendered");
        return false;
      }
      return true;
    }

    // ---- 1) load + sign in ------------------------------------------------
    section("Load + sign in");
    await navigate(`${BASE}/dashboard/`);
    // Either the auth card or the shell appears; never a blank page.
    await waitFor(
      `document.querySelector('.auth-card') || document.querySelector('#app-sidebar')`,
      { timeout: 15000, desc: "auth card or shell" },
    );
    const onLogin = await evaluate(`!!document.querySelector('.auth-card')`);
    if (onLogin) {
      const isSetup = await evaluate(
        `(() => { const h = document.querySelector('.auth-card h2'); return !!h && /set up/i.test(h.textContent||''); })()`,
      );
      // Fill the controlled inputs the React way (native setter + input event),
      // then submit — this drives the real Login onSubmit, not a fetch shortcut.
      const fill = (val, sel) =>
        evaluate(
          `(() => {
             const el = document.querySelector(${JSON.stringify(sel)});
             if (!el) return false;
             const set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set;
             set.call(el, ${JSON.stringify(val)});
             el.dispatchEvent(new Event('input',{bubbles:true}));
             return true;
           })()`,
        );
      await fill(USER, ".auth-card .field:nth-of-type(1) input");
      await fill(PASS, '.auth-card input[type="password"]');
      if (isSetup) {
        await fill(PASS, ".auth-card .field:nth-of-type(3) input");
      }
      await evaluate(`document.querySelector('.auth-card button.btn').click()`);
      try {
        await waitFor(`document.querySelector('#app-sidebar')`, { timeout: 15000, desc: "shell after login" });
        record("sign in reaches the shell", true, isSetup ? "first-run setup" : "login");
      } catch (e) {
        const banner = await evaluate(
          `(() => { const b = document.querySelector('.auth-card .banner.err'); return b ? b.textContent.trim() : ''; })()`,
        );
        record("sign in reaches the shell", false, banner || e.message);
        throw new Error("cannot proceed without a session");
      }
    } else {
      record("sign in reaches the shell", true, "already authenticated (dev bypass)");
    }

    // ---- 2) click through every sidebar nav destination -------------------
    section("Sidebar navigation (all 8 routes)");
    const ROUTES = [
      { label: "Home", title: "Relux" },
      { label: "Prime", title: "Prime" },
      { label: "Inbox", title: "Inbox" },
      { label: "Work", title: "Work" },
      { label: "Crew", title: "Crew" },
      { label: "Plugins", title: "Plugins" },
      { label: "Approvals", title: "Approvals" },
      { label: "Health", title: "Health" },
    ];
    for (const route of ROUTES) {
      const before = snapshot();
      const clicked = await evaluate(
        `(() => {
           const items = Array.from(document.querySelectorAll('#app-sidebar a.nav-item'));
           const el = items.find(a => (a.textContent||'').trim().indexOf(${JSON.stringify(route.label)}) !== -1);
           if (!el) return false;
           el.click();
           return true;
         })()`,
      );
      if (!clicked) {
        record(`nav → ${route.label}`, false, "sidebar link not found");
        continue;
      }
      try {
        await waitFor(
          `(() => { const h = document.querySelector('.topbar h1'); return h && h.textContent.trim() === ${JSON.stringify(route.title)}; })()`,
          { timeout: 12000, desc: `topbar title ${route.title}` },
        );
        await waitForContent(route.label);
      } catch (e) {
        record(`nav → ${route.label}`, false, e.message);
        continue;
      }
      const okNotBlank = await assertNotBlank(`nav → ${route.label} renders content`);
      const probs = newProblems(before);
      if (probs.length) {
        record(`${route.label} clean (no console/page/5xx errors)`, false, probs.slice(0, 3).join(" | "));
      } else if (okNotBlank) {
        record(`nav → ${route.label}`, true, "routed + content + clean");
      }
    }

    // ---- 3) Prime: a greeting renders a reply, creates no work ------------
    section("Prime chat — greeting must not auto-create work");
    {
      const before = snapshot();
      await evaluate(
        `(() => {
           const items = Array.from(document.querySelectorAll('#app-sidebar a.nav-item'));
           const el = items.find(a => (a.textContent||'').trim().indexOf('Prime') !== -1);
           if (el) el.click();
         })()`,
      );
      await waitFor(`document.querySelector('.chat-input input')`, { timeout: 12000, desc: "prime input" });
      const before2 = `document.querySelectorAll('.chat-log .msg.assistant').length`;
      const startCount = await evaluate(before2);
      await evaluate(
        `(() => {
           const el = document.querySelector('.chat-input input');
           const set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set;
           set.call(el, 'hello there, just saying hi');
           el.dispatchEvent(new Event('input',{bubbles:true}));
         })()`,
      );
      // Click the Send button (the first .chat-input button).
      await evaluate(`document.querySelector('.chat-input button.btn').click()`);
      try {
        // A new assistant turn card (beyond the static greeting), or an error banner.
        await waitFor(
          `${before2} > ${startCount} || document.querySelector('.chat-log .banner.err')`,
          { timeout: 20000, desc: "prime reply" },
        );
      } catch (e) {
        record("Prime greeting returns a reply", false, e.message);
        throw e;
      }
      const errBanner = await evaluate(
        `(() => { const b = document.querySelector('.chat-log .banner.err'); return b ? b.textContent.trim() : ''; })()`,
      );
      if (errBanner) {
        record("Prime greeting returns a reply", false, "error banner: " + errBanner);
      } else {
        // Inspect the LAST assistant card: the rendered turn.
        const card = await evaluate(
          `(() => {
             const cards = Array.from(document.querySelectorAll('.chat-log .msg.assistant'));
             const last = cards[cards.length-1];
             if (!last) return null;
             const text = (last.innerText||'').trim();
             const anchors = Array.from(last.querySelectorAll('a')).map(a => a.getAttribute('href')||'');
             return {
               text,
               len: text.length,
               // raw transport envelope leaking instead of the shaped reply
               rawEnvelope: /"(disposition|ai_mode|intent_source|tool_output)"\\s*:/.test(text) || text.trim().startsWith('{"'),
               // a task/run link means work was created from a mere greeting
               createdWork: anchors.some(h => h.includes('work?task=') || h.includes('work?run=')) || /Created (task|tool-run)/i.test(text),
               executed: /\\bexecuted\\b/.test(text),
             };
           })()`,
        );
        if (!card || card.len < 1) {
          record("Prime greeting returns a reply", false, "assistant card empty");
        } else {
          record("Prime greeting returns a reply", true, `reply rendered (${card.len} chars)`);
          record("Prime reply is shaped, not a raw JSON envelope", !card.rawEnvelope, card.rawEnvelope ? "raw envelope leaked into chat" : "shaped");
          record("Prime greeting created no task/run", !card.createdWork, card.createdWork ? "a greeting auto-created work" : "no work created");
        }
      }
      const probs = newProblems(before);
      record("Prime clean (no console/page/5xx errors)", probs.length === 0, probs.slice(0, 3).join(" | "));
    }

    // ---- 4) Work: the Inspect button opens the task-detail panel ----------
    section("Work — Inspect button → task detail panel");
    {
      const before = snapshot();
      await evaluate(
        `(() => {
           const items = Array.from(document.querySelectorAll('#app-sidebar a.nav-item'));
           const el = items.find(a => (a.textContent||'').trim().indexOf('Work') !== -1);
           if (el) el.click();
         })()`,
      );
      await waitFor(
        `(() => { const h = document.querySelector('.topbar h1'); return h && h.textContent.trim()==='Work'; })()`,
        { timeout: 12000, desc: "work title" },
      );
      await waitForContent("Work");
      // Board Oversight v1: the composed oversight strip must load (not stay stuck
      // on "Loading oversight…") and render its count chips. This is the onClick →
      // GET /v1/relux/oversight → re-render binding a first-paint test cannot see.
      try {
        await waitFor(
          `(() => {
             const t = (document.querySelector('.workspace')?.innerText || '');
             return t.includes('Oversight') && t.includes('Active runs') && !t.includes('Loading oversight');
           })()`,
          { timeout: 12000, desc: "oversight strip loaded" },
        );
        record("Work oversight strip loads its composed summary", true, "counts rendered");
      } catch (e) {
        record("Work oversight strip loads its composed summary", false, e.message);
      }
      // The Blocked/Failed column is now part of the board (the previously invisible bucket).
      // The column heading uses CSS text-transform:uppercase, and innerText returns the
      // RENDERED (uppercased) text — so compare case-insensitively.
      const hasBlockedCol = await evaluate(
        `/blocked \\/ failed/i.test(document.querySelector('.workspace')?.innerText || '')`,
      );
      record("Work board shows the Blocked / Failed column", hasBlockedCol, hasBlockedCol ? "rendered" : "column missing");
      // Inline approval controls in the oversight strip: when a pending approval is
      // present the strip renders Approve & run / Allow always / Deny INLINE. We do
      // NOT click them here — every one of those decisions is a real mutation that
      // approves/executes/denies a governed action against the live kernel, and this
      // smoke deliberately clicks only non-destructive surfaces (see the Plugins/
      // Approvals section: "never a destructive action button"). The wiring of each
      // button → reluxApprovals route is covered by the action-model unit test
      // (test/approvalactions.test.ts), the component render test
      // (test/oversight-approvals-render.test.mjs), and the backend approval routes.
      // The smoke's seed (one task) creates no pending approval, so this only
      // asserts the controls render WHEN one exists; otherwise it records the honest
      // "no seeded approval" state rather than a false pass.
      {
        const approvalState = await evaluate(
          `(() => {
             const heads = Array.from(document.querySelectorAll('.workspace h5'));
             const head = heads.find(h => /pending approvals/i.test(h.textContent || ''));
             if (!head) return { seeded: false };
             // The approval rows are siblings under the same column container.
             const col = head.parentElement;
             const btns = Array.from(col ? col.querySelectorAll('button') : [])
               .map(b => (b.textContent || '').trim());
             return {
               seeded: true,
               hasApprove: btns.some(t => /^Approve/.test(t)),
               hasDeny: btns.includes('Deny'),
             };
           })()`,
        );
        if (!approvalState.seeded) {
          record("Work oversight inline approval controls (no approval seeded)", true, "no pending approval in this smoke seed; not clickable");
        } else {
          record(
            "Work oversight strip renders inline approval controls",
            approvalState.hasApprove && approvalState.hasDeny,
            approvalState.hasApprove && approvalState.hasDeny ? "Approve + Deny rendered (not clicked — destructive)" : "inline controls missing",
          );
        }
      }
      // The task list loads async (useAsync). Give it a bounded moment to settle
      // before deciding empty-vs-seeded, so a slow fetch is not misread as "no
      // tasks" (which would skip the Inspect→detail binding this step exists for).
      let hasInspect = false;
      try {
        await waitFor(
          `Array.from(document.querySelectorAll('.workspace button')).some(b => (b.textContent||'').trim()==='Inspect')`,
          { timeout: 8000, desc: "seeded task row" },
        );
        hasInspect = true;
      } catch {
        hasInspect = false;
      }
      if (!hasInspect) {
        // No seeded data — assert the empty state is a real view, not a blank page.
        const ok = await assertNotBlank("Work empty-state renders (no seeded tasks)");
        if (ok) record("Work empty-state renders (no seeded tasks)", true, "no Inspect button; empty columns shown");
      } else {
        await evaluate(
          `(() => {
             const b = Array.from(document.querySelectorAll('.workspace button')).find(b => (b.textContent||'').trim()==='Inspect');
             if (b) b.click();
           })()`,
        );
        try {
          // The detail panel is URL-driven (?task=...) and renders a Close button.
          await waitFor(
            `location.search.indexOf('task=')!==-1 && Array.from(document.querySelectorAll('.workspace button')).some(b => (b.textContent||'').trim()==='Close')`,
            { timeout: 12000, desc: "task detail panel" },
          );
          const ok = await assertNotBlank("Work Inspect opens the task detail panel");
          if (ok) record("Work Inspect opens the task detail panel", true, "panel rendered with Close");
          // Close it again and confirm the board returns (no blank after close).
          await evaluate(
            `(() => { const b = Array.from(document.querySelectorAll('.workspace button')).find(b => (b.textContent||'').trim()==='Close'); if (b) b.click(); })()`,
          );
          await waitFor(`location.search.indexOf('task=')===-1`, { timeout: 8000, desc: "panel closed" });
          await assertNotBlank("Work returns to the board after Close");
          record("Work returns to the board after Close", true, "board restored");
        } catch (e) {
          record("Work Inspect opens the task detail panel", false, e.message);
        }
      }
      // ---- 4b) Work status MOVE: a real onClick → network → re-render ----------
      // The seeded task is auto-assigned to Prime → "queued" (non-terminal), so its
      // card carries the compact Block / Cancel move select. This is a SAFE, in-scope
      // edit (relux-kernel set_task_status: the operator-settable allowlist, never a
      // risk-gated/governed action) on a throwaway DB, so unlike the oversight
      // approval controls it IS exercised: we pick "blocked" on the select and assert
      // the card re-buckets to a blocked status — the full move → reload binding the
      // static render test cannot see.
      {
        const captured = await evaluate(
          `(() => {
             const sel = document.querySelector('.workspace select[aria-label^="Move task status"]');
             if (!sel) return null;
             const card = sel.closest('.card');
             const idEl = card ? card.querySelector('.mono.muted') : null;
             return idEl ? (idEl.textContent || '').trim() : '';
           })()`,
        );
        if (captured == null || captured === "") {
          record("Work status move control present on a card", false, captured == null ? "no move select found" : "card task id not read");
        } else {
          // Drive the controlled <select> the React way (native setter + change), then
          // assert the card with that id now shows a "blocked" status badge.
          await evaluate(
            `(() => {
               const sel = document.querySelector('.workspace select[aria-label^="Move task status"]');
               if (!sel) return false;
               const set = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype,'value').set;
               set.call(sel, 'blocked');
               sel.dispatchEvent(new Event('change',{bubbles:true}));
               return true;
             })()`,
          );
          try {
            await waitFor(
              `(() => {
                 const cards = Array.from(document.querySelectorAll('.workspace .card'));
                 const card = cards.find(c => ((c.querySelector('.mono.muted')||{}).textContent||'').trim() === ${JSON.stringify(captured)});
                 return !!card && /blocked/i.test((card.innerText||''));
               })()`,
              { timeout: 12000, desc: "card re-buckets to blocked" },
            );
            record("Work status move (Block) updates the card", true, `${captured} → blocked via onChange → network → re-render`);
          } catch (e) {
            record("Work status move (Block) updates the card", false, e.message);
          }
        }
      }
      // ---- 4c) Work drag-to-column affordances (design §6) -------------------
      // Drag-to-column status movement is ADDITIVE over the select (4b). Native
      // HTML5 drag is not reliably synthesizable via CDP, so we assert the
      // deterministic affordances the real handlers hang off: a draggable card and
      // a labelled column drop region. The drop → setTaskStatus binding itself is
      // pinned by the pure helper test (taskmove columnDropTarget) + the backend
      // route tests; the select path (4b) already proves the live move→reload edge.
      {
        const drag = await evaluate(
          `(() => {
             const card = document.querySelector('.workspace .board-column .card[draggable="true"]');
             const col = document.querySelector('.workspace .board-column[data-bucket]');
             const label = col ? (col.getAttribute('aria-label') || '') : '';
             return {
               draggableCard: !!card,
               roleDesc: card ? (card.getAttribute('aria-roledescription') || '') : '',
               dropRegion: !!col,
               labelled: /drop a task here/i.test(label),
             };
           })()`,
        );
        const ok = !!(drag && drag.draggableCard && drag.dropRegion && drag.labelled);
        record(
          "Work cards are draggable and columns are labelled drop targets",
          ok,
          ok
            ? `card draggable (${drag.roleDesc}); column drop region labelled`
            : `draggableCard=${drag && drag.draggableCard} dropRegion=${drag && drag.dropRegion} labelled=${drag && drag.labelled}`,
        );
      }
      const probs = newProblems(before);
      record("Work clean (no console/page/5xx errors)", probs.length === 0, probs.slice(0, 3).join(" | "));
    }

    // ---- 5) Plugins / Approvals: disclosures expand without blanking ------
    // We click only safe disclosure <summary> toggles (pure UI reveal), never a
    // destructive action button (Disable/Revoke/Delete), then re-assert the
    // route still renders — exactly the "open a form, no blank page" check.
    for (const page of ["Plugins", "Approvals"]) {
      section(`${page} — disclosures expand, no blank`);
      const before = snapshot();
      await evaluate(
        `(() => {
           const items = Array.from(document.querySelectorAll('#app-sidebar a.nav-item'));
           const el = items.find(a => (a.textContent||'').trim().indexOf(${JSON.stringify(page)}) !== -1);
           if (el) el.click();
         })()`,
      );
      await waitFor(
        `(() => { const h = document.querySelector('.topbar h1'); return h && h.textContent.trim()===${JSON.stringify(page)}; })()`,
        { timeout: 12000, desc: `${page} title` },
      );
      await waitForContent(page);
      const okBefore = await assertNotBlank(`${page} renders`);
      // Toggle any <summary> disclosures (safe: native open/close, no mutation).
      const toggled = await evaluate(
        `(() => {
           const sums = Array.from(document.querySelectorAll('.workspace summary'));
           sums.slice(0, 6).forEach(s => s.click());
           return sums.length;
         })()`,
      );
      await sleep(250);
      const okAfter = await assertNotBlank(`${page} stays rendered after expanding disclosures`);
      const probs = newProblems(before);
      if (okBefore && okAfter && probs.length === 0) {
        record(`${page} interactions clean`, true, `${toggled} disclosure(s) toggled, no blank, no errors`);
      } else if (probs.length) {
        record(`${page} interactions clean`, false, probs.slice(0, 3).join(" | "));
      }
    }

    if (apiWarnings.length) {
      console.log(`\n  note: ${apiWarnings.length} /v1/relux 4xx response(s) observed (non-fatal): ${apiWarnings.slice(0, 4).join(", ")}`);
    }
  } finally {
    cleanup();
  }

  // ---- summary -----------------------------------------------------------
  section("Browser smoke summary");
  const pass = results.filter((r) => r.ok).length;
  const fail = results.filter((r) => !r.ok).length;
  console.log(`  ${pass} passed, ${fail} failed (of ${results.length} checks)`);
  if (fail === 0) {
    console.log("RESULT: PASS");
    process.exit(0);
  }
  console.log("RESULT: FAIL");
  process.exit(1);
}

main().catch((e) => {
  console.error("\nbrowser smoke aborted: " + (e?.stack || e?.message || e));
  process.exit(1);
});
