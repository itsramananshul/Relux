# scripts/smoke-prime-brain-envelope.ps1
#
# Smoke for the Prime CONVERSATIONAL BRAIN envelope contract (RELUX_MASTER_PLAN
# §15 + the AI "Conversational Shaping / Actionful Safety" section).
#
# The Prime chat/brain path is action-free by design: when a Claude/Codex CLI
# answers a chat turn and its result envelope ALSO declares `proposed_changes`,
# the kernel must (a) show ONLY the human reply in the chat bubble — never the raw
# JSON or the change payload — and (b) NOT capture the change into a run (that
# would manufacture hidden, mutable work from a casual message), but instead
# surface an honest advisory pointing at the documented assigned-run review/apply
# path. This smoke drives those guarantees with a FAKE CLI envelope string through
# the pure shaping/advisory functions — no network, no real Claude/Codex, no paid
# model quota. The behavior is fully deterministic and lives in the
# `--bin relux-kernel` test target.
#
# Prints a concise PASS/FAIL table and exits non-zero on any failure.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-prime-brain-envelope.ps1

[CmdletBinding()]
param()

# Native cargo writes progress to stderr; don't let that abort the script. Key off
# stdout ("test result: ok") + $LASTEXITCODE instead, and never merge stderr
# (2>&1) on a native exe (PS 5.1 wraps each stderr line in an ErrorRecord).
$ErrorActionPreference = 'Continue'
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$script:Results = @()
function Pass($name, $note) { $script:Results += [pscustomobject]@{ Result = 'PASS'; Name = $name; Note = $note }; Write-Host ("  PASS  {0}" -f $name) -ForegroundColor Green }
function Fail($name, $note) { $script:Results += [pscustomobject]@{ Result = 'FAIL'; Name = $name; Note = $note }; Write-Host ("  FAIL  {0}  ({1})" -f $name, $note) -ForegroundColor Red }
function Assert($name, $cond, $note) { if ($cond) { Pass $name $note } else { Fail $name $note } }

Write-Host '== Relux Prime brain envelope smoke ==' -ForegroundColor Cyan
Write-Host '-- fake-CLI-envelope brain contract (pure, no real Claude) --' -ForegroundColor Cyan

# Each test drives a fake `{ "type":"result", ... "proposed_changes":[...] }`
# envelope string through the conversational-brain shaping/advisory seam.
$tests = @(
    'brain_chat_envelope_with_proposed_changes_shows_only_reply_no_json',
    'brain_chat_envelope_with_proposed_changes_surfaces_honest_advisory',
    'brain_chat_greeting_envelope_has_no_advisory',
    'prime_response_wire_can_never_carry_proposed_changes',
    'claude_cli_brain_shows_only_human_text_not_raw_envelope'
)
foreach ($t in $tests) {
    $out = & cargo test -p relux-kernel --bin relux-kernel $t 2>$null | Out-String
    $ok = ($LASTEXITCODE -eq 0) -and ($out -match 'test result: ok')
    Assert ("brain test: $t") $ok 'fake envelope -> human reply + honest advisory, no hidden run'
}

# Core envelope parser keeps the JSON out of the human text and captures changes
# only from a recognized envelope (an arbitrary JSON blob never qualifies).
$coreOut = & cargo test -p relux-core adapter_result 2>$null | Out-String
Assert 'core adapter_result envelope parsing' (($LASTEXITCODE -eq 0) -and ($coreOut -match 'test result: ok')) 'envelope vs plain text, no fabrication'

Write-Host ''
$fails = @($script:Results | Where-Object { $_.Result -eq 'FAIL' }).Count
$script:Results | Format-Table -AutoSize | Out-String | Write-Host
if ($fails -gt 0) {
    Write-Host ("{0} check(s) FAILED." -f $fails) -ForegroundColor Red
    exit 1
}
Write-Host 'All Prime brain envelope checks passed.' -ForegroundColor Green
