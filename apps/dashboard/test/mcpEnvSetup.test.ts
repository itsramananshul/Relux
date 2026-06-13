// Pure-logic tests for the guided MCP secret/env setup helpers (src/mcpEnvSetup.ts):
// turning the value-free requirement view into form rows, and the filled rows back into a
// POST body — including that an inline value rides as `value` and an existing secret rides
// as `secret_name`, that an untouched satisfied row is skipped, and that nothing here ever
// fabricates a value.
//
// Run: `npm test` (auto-discovered) or
// `node --test --experimental-strip-types test/mcpEnvSetup.test.ts`.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  rowsFromSetup,
  envSetupMappings,
  envSetupBody,
  rowHasInput,
  setupNeedsWork,
  requirementStatusLabel,
  type EnvSetupRow,
} from "../src/mcpEnvSetup.ts";
import type { ReluxMcpServerSetup } from "../src/api.ts";

function setup(over: Partial<ReluxMcpServerSetup> = {}): ReluxMcpServerSetup {
  return {
    server_id: "gh",
    requirements: [
      {
        env_var: "OPENAI_API_KEY",
        required: true,
        description: "Expected by the imported MCP server — map it to a stored secret.",
        secret_mapped: false,
        secret_present: false,
      },
    ],
    ready: false,
    missing: ["OPENAI_API_KEY"],
    ...over,
  };
}

test("setupNeedsWork is true only with unsatisfied requirements", () => {
  assert.equal(setupNeedsWork(setup()), true);
  assert.equal(setupNeedsWork(setup({ ready: true, missing: [] })), false);
  assert.equal(setupNeedsWork({ server_id: "x", requirements: [], ready: true, missing: [] }), false);
  assert.equal(setupNeedsWork(undefined), false);
  assert.equal(setupNeedsWork(null), false);
});

test("rowsFromSetup builds one row per requirement, defaulting unmapped vars to value mode", () => {
  const rows = rowsFromSetup(setup());
  assert.equal(rows.length, 1);
  assert.equal(rows[0].envVar, "OPENAI_API_KEY");
  assert.equal(rows[0].mode, "value");
  assert.equal(rows[0].satisfied, false);
  assert.equal(rows[0].value, "");
});

test("a mapped requirement prefills existing-secret mode with the secret name", () => {
  const rows = rowsFromSetup(
    setup({
      requirements: [
        {
          env_var: "TOKEN",
          required: true,
          description: "Mapped on this server's configuration.",
          secret_mapped: true,
          secret_name: "my_token",
          secret_present: true,
        },
      ],
      ready: true,
      missing: [],
    }),
  );
  assert.equal(rows[0].mode, "existing");
  assert.equal(rows[0].secretName, "my_token");
  assert.equal(rows[0].satisfied, true);
});

test("envSetupMappings: a value rides as value, an existing secret as secret_name", () => {
  const rows: EnvSetupRow[] = [
    { envVar: "OPENAI_API_KEY", satisfied: false, mode: "value", value: " sk-abc ", secretName: "" },
    { envVar: "GH_TOKEN", satisfied: false, mode: "existing", value: "", secretName: " my_token " },
  ];
  const mappings = envSetupMappings(rows);
  assert.deepEqual(mappings, [
    { env_var: "OPENAI_API_KEY", value: "sk-abc" },
    { env_var: "GH_TOKEN", secret_name: "my_token" },
  ]);
});

test("envSetupMappings skips a row with no input (an untouched satisfied requirement)", () => {
  const rows: EnvSetupRow[] = [
    { envVar: "TOKEN", satisfied: true, mode: "existing", value: "", secretName: "" },
    { envVar: "OTHER", satisfied: false, mode: "value", value: "x", secretName: "" },
  ];
  assert.equal(rowHasInput(rows[0]), false);
  assert.equal(rowHasInput(rows[1]), true);
  assert.deepEqual(envSetupMappings(rows), [{ env_var: "OTHER", value: "x" }]);
});

test("envSetupBody carries the full declared expected set and the rediscover flag", () => {
  const rows: EnvSetupRow[] = [
    { envVar: "A", satisfied: false, mode: "value", value: "v", secretName: "" },
    { envVar: "B", satisfied: true, mode: "existing", value: "", secretName: "" },
  ];
  const body = envSetupBody(rows, true);
  assert.deepEqual(body.expected_env, ["A", "B"], "all declared vars, even untouched ones");
  assert.deepEqual(body.mappings, [{ env_var: "A", value: "v" }]);
  assert.equal(body.rediscover, true);
});

test("requirementStatusLabel reflects mapped/present without leaking a value", () => {
  assert.equal(
    requirementStatusLabel({ env_var: "X", required: true, description: "", secret_mapped: false, secret_present: false }),
    "needs a secret",
  );
  assert.equal(
    requirementStatusLabel({ env_var: "X", required: true, description: "", secret_mapped: true, secret_name: "s", secret_present: false }),
    "mapped, secret missing",
  );
  assert.equal(
    requirementStatusLabel({ env_var: "X", required: true, description: "", secret_mapped: true, secret_name: "s", secret_present: true }),
    "secret mapped",
  );
});
