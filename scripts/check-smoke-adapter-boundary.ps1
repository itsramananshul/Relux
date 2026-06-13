# scripts/check-smoke-adapter-boundary.ps1
#
# Drift guard for the release smokes' LOCAL-vs-REAL-ADAPTER honesty contract
# (RELUX_MASTER_PLAN.md Sec 8.1 "Local Prime is deterministic - it fails closed
# on real external work"; relux_core::is_unfulfillable_local_request /
# KernelError::LocalAdapterUnsupported).
#
# The product LOCKED a correct behavior: the deterministic local Prime adapter
# does NO external work, so a free-form natural-language goal it was handed (a
# `prime_request` with no executable directive) must FAIL CLOSED honestly - the
# run reaches a terminal Failed state and the task is parked Blocked, never a
# silent hang and never a fabricated "done". Real external/agent work belongs to
# a configured Claude/Codex adapter, not to local Prime.
#
# It is tempting to "fix" a red release gate by making local Prime fake that work
# again (assert a free-form task reaches Completed locally). That would re-open
# the very bug the product closed. This static check reads the two smoke scripts
# and pins the contract so the smokes can never quietly drift back:
#
#   relux-first-release-check.ps1 (quick CLI gate):
#     - asserts the assigned run FAILS CLOSED honestly (the honest step exists);
#     - asserts the refused task is parked Blocked;
#     - does NOT carry the old success-expecting "assigned task runs" step.
#   relux-e2e-smoke.ps1 (full E2E):
#     - the autonomy tick refuses unfulfillable work honestly + parks it Blocked;
#     - does NOT assert the free-form goal "honestly moved to Completed" locally;
#     - DOES prove the positive path ("local fulfillable task completes");
#     - gates real external/agent work behind the -RunRealClaudeAdapter /
#       -RunRealCodexAdapter opt-ins (never local Prime).
#
# This is a static cross-source check (no build, no process). Companion to
# scripts/check-port-guidance.ps1 (port-busy wording parity).
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-smoke-adapter-boundary.ps1

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$Quick = Join-Path $PSScriptRoot "relux-first-release-check.ps1"
$E2E = Join-Path $PSScriptRoot "relux-e2e-smoke.ps1"

$Failures = 0
function Write-Step {
    param([string]$Name, [bool]$Ok, [string]$Detail = "")
    $tag = if ($Ok) { "PASS" } else { "FAIL" }
    $color = if ($Ok) { "Green" } else { "Red" }
    Write-Host ("  {0,-4} {1,-44} {2}" -f $tag, $Name, $Detail) -ForegroundColor $color
    if (-not $Ok) { $script:Failures += 1 }
}

Write-Host ""
Write-Host "== Relux smoke local-vs-real-adapter boundary contract ==" -ForegroundColor Cyan
Write-Host ("  quick gate: {0}" -f $Quick)
Write-Host ("  full e2e:   {0}" -f $E2E)
Write-Host ""

if (-not (Test-Path -LiteralPath $Quick) -or -not (Test-Path -LiteralPath $E2E)) {
    Write-Host "RESULT: FAIL (one or both smoke scripts are missing)" -ForegroundColor Red
    exit 1
}

$quickText = Get-Content -LiteralPath $Quick -Raw
$e2eText = Get-Content -LiteralPath $E2E -Raw

# -- 1) quick gate: honest fail-closed, not a faked success ----------------
Write-Step "quick: asserts assigned run fails closed honestly" `
    ($quickText -match 'assigned run fails closed honestly') "honest refusal step present"
Write-Step "quick: requires the local-adapter guidance" `
    (($quickText -match 'cannot fulfil') -and ($quickText -match 'no external work')) "guidance asserted, not a success"
Write-Step "quick: asserts the refused task is parked Blocked" `
    ($quickText -match 'parked Blocked') "Blocked (reopenable), not Running/Completed"
# NEGATIVE: the old step that ran `task run-assigned` and treated exit 0 as the
# pass criterion for the free-form goal must be gone.
Write-Step "quick: no old success-expecting 'assigned task runs' step" `
    (-not ($quickText -match "Invoke-NativeStep -Name `"assigned task runs`"")) "local Prime never expected to fake external work"

# -- 2) full e2e: autonomy fails closed (not a faked Completed) -------------
Write-Step "e2e: autonomy tick refuses unfulfillable work honestly" `
    ($e2eText -match 'autonomy tick refuses unfulfillable work honestly') "Tasks Run: 0 + guidance"
Write-Step "e2e: autonomy parks the free-form task Blocked" `
    ($e2eText -match 'task parked Blocked \(fail-closed\)') "Queued -> Blocked, no fabricated completion"
# NEGATIVE: the old assertion that the free-form goal "honestly moved to Completed"
# on the LOCAL adapter must be gone (that was the re-introduce-the-bug shape).
Write-Step "e2e: no 'free-form goal moved to Completed' local assertion" `
    (-not ($e2eText -match 'task honestly moved to Completed')) "local Prime never fakes external work"

# -- 3) full e2e: the POSITIVE local path is still proven -------------------
Write-Step "e2e: proves a fulfillable task completes locally" `
    ($e2eText -match 'local fulfillable task completes') "local Prime runs what it CAN fulfil, for real"

# -- 4) full e2e: real external/agent work is a real-adapter opt-in ---------
Write-Step "e2e: real agent work gated behind Claude/Codex adapter opt-in" `
    (($e2eText -match 'RunRealClaudeAdapter') -and ($e2eText -match 'RunRealCodexAdapter')) "never local Prime"

Write-Host ""
if ($Failures -eq 0) {
    Write-Host "RESULT: PASS" -ForegroundColor Green
    exit 0
}
Write-Host ("RESULT: FAIL ({0} failing check(s))" -f $Failures) -ForegroundColor Red
exit 1
