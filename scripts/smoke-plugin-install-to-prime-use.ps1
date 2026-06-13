# scripts/smoke-plugin-install-to-prime-use.ps1
#
# Smoke for the product promise "install a plugin -> Prime can actually use it"
# (docs/prime-tool-use.md; RELUX_MASTER_PLAN §8 Plugin Model, §8.2 Command Tools,
# §10.2 Action Layer, §10.3 Approval Rules).
#
# It runs the canonical kernel integration tests that drive the SAME product routes
# the dashboard/Prime drive, against a LOCAL fixture plugin only:
#
#   1. A realistic, non-echo source plugin with NO relux-plugin.json (a tiny npm CLI
#      that declares a `bin`) is installed through the real install-dir route, lands
#      as an honest metadata-only wrapper, and detects a `command_tool` candidate.
#   2. Prime's governed backend action configures that candidate (no code runs on
#      configure), the new tool appears in the SAME catalog Prime sees (gated), and
#      it is invocable ONLY through the approval gate — refused without a grant, run
#      argv-only and audited with one.
#
# Everything is deterministic and lives in the `--bin relux-kernel` test target. No
# network, no GitHub clone, no arbitrary remote repo code, no real Claude/Codex, no
# bypass flags. The fixture is created inside the test's temp dir and never becomes a
# product default. This is an OPTIONAL, manual smoke — it is not required by CI.
#
# Prints a concise PASS/FAIL table and exits non-zero on any failure.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-plugin-install-to-prime-use.ps1

[CmdletBinding()]
param()

# Native cargo writes progress to stderr; don't let that abort the script. Key off
# stdout ("test result: ok") + $LASTEXITCODE instead, and never merge stderr (2>&1)
# on a native exe (PS 5.1 wraps each stderr line in an ErrorRecord).
$ErrorActionPreference = 'Continue'
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$script:Results = @()
function Pass($name, $note) { $script:Results += [pscustomobject]@{ Result = 'PASS'; Name = $name; Note = $note }; Write-Host ("  PASS  {0}" -f $name) -ForegroundColor Green }
function Fail($name, $note) { $script:Results += [pscustomobject]@{ Result = 'FAIL'; Name = $name; Note = $note }; Write-Host ("  FAIL  {0}  ({1})" -f $name, $note) -ForegroundColor Red }
function Assert($name, $cond, $note) { if ($cond) { Pass $name $note } else { Fail $name $note } }

Write-Host '== Relux plugin install -> Prime use smoke ==' -ForegroundColor Cyan
Write-Host '-- local fixture only: no clone, no remote code, no real brain --' -ForegroundColor Cyan

# `node` makes the final governed run a REAL argv-only run with captured stdout; the
# test asserts the install->configure->catalog->gate wiring regardless, and only the
# real-run stdout/audit assertions need node. Surface its presence for transparency.
$node = (& node --version 2>$null | Out-String).Trim()
if ($node) {
    Write-Host ("   node detected ({0}) -> the governed run is exercised for real" -f $node) -ForegroundColor DarkGray
} else {
    Write-Host '   node not on PATH -> gate+grant path asserted, real-run stdout skipped (still honest)' -ForegroundColor Yellow
}

# Each test drives a real product-route sequence in the server test harness.
$tests = @(
    'install_configure_then_prime_can_use_the_governed_command_tool',
    'prime_configure_candidate_activates_a_command_tool',
    'prime_configure_candidate_registers_an_mcp_server',
    'command_tool_configures_gates_runs_persists_and_removes'
)
foreach ($t in $tests) {
    $out = & cargo test -p relux-kernel --bin relux-kernel $t 2>$null | Out-String
    $ok = ($LASTEXITCODE -eq 0) -and ($out -match 'test result: ok')
    Assert ("route test: $t") $ok 'install -> configure -> catalog -> gated invoke, audited'
}

Write-Host ''
$fails = @($script:Results | Where-Object { $_.Result -eq 'FAIL' }).Count
$script:Results | Format-Table -AutoSize | Out-String | Write-Host
if ($fails -gt 0) {
    Write-Host ("{0} check(s) FAILED." -f $fails) -ForegroundColor Red
    exit 1
}
Write-Host 'All plugin install -> Prime use checks passed.' -ForegroundColor Green
