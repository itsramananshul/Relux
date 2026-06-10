#Requires -Version 5.1
# Real smoke for TRUE bounded OS-parallel orchestration execution.
#
# Two INDEPENDENT briefs run in the SAME round at concurrency 2:
#   - a research brief on the REAL local Claude CLI (proves the real adapter path), and
#   - an operations brief on the Codex adapter whose binary is overridden with a
#     fake ~6s sleeping CLI (a cheap, deterministic second spawned process).
# Because they are independent and concurrency=2, the non-blocking job spawns both
# adapter processes on parallel OS threads. The poll loop then observes BOTH briefs
# reported `running` in the SAME snapshot — the live proof that they ran together,
# not one-after-another. Bounds cost: only the tiny "two plus two" prompt hits Claude.
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo "target\debug\relux-kernel.exe"
$smokeRoot = Join-Path $repo "dev-data\relux-smoke-parallel"
$db = Join-Path $smokeRoot "local.db"
$addr = "127.0.0.1:19899"
$base = "http://$addr"

if (Test-Path $smokeRoot) { Remove-Item -Recurse -Force $smokeRoot }
New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null

# A fake "slow" CLI for the second (Codex) brief: sleeps ~6s (so it overlaps the
# real Claude call), then prints a fixed line and exits 0. No external `find`/etc.
$fakeCli = Join-Path $smokeRoot "slow-agent.cmd"
@"
@echo off
ping -n 7 127.0.0.1 >nul
echo SLOW_AGENT_DONE
exit /b 0
"@ | Set-Content -Path $fakeCli -Encoding ASCII

$env:RELUX_DB = $db
$env:RELUX_HTTP_ADDR = $addr

Write-Host "== starting kernel ($addr) =="
$proc = Start-Process -FilePath $bin -ArgumentList "serve" -PassThru -WindowStyle Hidden `
  -RedirectStandardOutput (Join-Path $smokeRoot "server.out.log") `
  -RedirectStandardError (Join-Path $smokeRoot "server.err.log")

function Wait-Up {
  for ($i = 0; $i -lt 40; $i++) {
    try { Invoke-RestMethod -Uri "$base/v1/relux/health" -TimeoutSec 2 | Out-Null; return $true }
    catch { Start-Sleep -Milliseconds 250 }
  }
  return $false
}

try {
  if (-not (Wait-Up)) { throw "server did not come up" }
  Write-Host "== server up =="

  Write-Host "== enabling REAL Claude CLI adapter =="
  $rtc = Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-claude-cli/runtime" `
    -ContentType "application/json" -Body (@{ enabled = $true } | ConvertTo-Json)
  Write-Host "   claude state=$($rtc.state) resolved=$($rtc.resolved_path)"

  Write-Host "== enabling Codex adapter with a FAKE slow binary =="
  $rtx = Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-codex-cli/runtime" `
    -ContentType "application/json" `
    -Body (@{ enabled = $true; command = $fakeCli; timeout_seconds = 60 } | ConvertTo-Json)
  Write-Host "   codex state=$($rtx.state) resolved=$($rtx.resolved_path)"

  Write-Host "== creating the two agents =="
  $a1 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" -ContentType "application/json" `
    -Body (@{ id = "research-agent"; name = "Research Agent"; role = "investigates"; adapter_plugin = "relux-adapter-claude-cli" } | ConvertTo-Json)
  $a2 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" -ContentType "application/json" `
    -Body (@{ id = "ops-agent"; name = "Ops Agent"; role = "ships releases"; adapter_plugin = "relux-adapter-codex-cli" } | ConvertTo-Json)
  Write-Host "   $($a1.id) -> $($a1.adapter_plugin); $($a2.id) -> $($a2.adapter_plugin)"

  # research -> research-agent (real Claude); deploy/release -> ops-agent (fake Codex).
  # No dependency between research and operations => both ready in round 1.
  $goal = "research what two plus two equals and deploy the release"
  Write-Host "== creating orchestration: $goal =="
  $orch = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations" `
    -ContentType "application/json" -Body (@{ goal = $goal } | ConvertTo-Json)
  $oid = $orch.id
  foreach ($s in $orch.steps) { Write-Host "   $($s.task_id) [$($s.role)] -> $($s.agent_id) deps=$($s.depends_on -join ',')" }
  if ($orch.steps.Count -ne 2) { throw "expected 2 briefs, got $($orch.steps.Count)" }

  Write-Host "== starting NON-BLOCKING job (run-async, concurrency=2) =="
  $start = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
    -ContentType "application/json" -Body (@{ concurrency = 2 } | ConvertTo-Json)
  Write-Host "   job $($start.id) state=$($start.state) concurrency=$($start.concurrency)"

  Write-Host "== polling job (looking for BOTH briefs running at once) =="
  $maxRunningSeen = 0
  $bothRunningPolls = 0
  $final = $null
  for ($i = 0; $i -lt 300; $i++) {
    Start-Sleep -Milliseconds 400
    $j = Invoke-RestMethod -Uri "$base$($start.status_url)"
    $runningIds = @($j.steps | Where-Object { $_.outcome -eq "running" } | ForEach-Object { $_.task_id })
    if ($runningIds.Count -gt $maxRunningSeen) { $maxRunningSeen = $runningIds.Count }
    if ($runningIds.Count -ge 2) {
      $bothRunningPolls++
      Write-Host "   poll[$i] BOTH RUNNING: [$($runningIds -join ', ')] round=$($j.current_round)"
    }
    if ($j.state -eq "completed" -or $j.state -eq "failed") { $final = $j; break }
  }
  if ($null -eq $final) { throw "job did not terminate in time" }

  Write-Host "== final record =="
  $rec = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid"
  foreach ($s in $rec.steps) {
    Write-Host "   $($s.task_id) [$($s.role)] $($s.agent_id) outcome=$($s.outcome) round=$($s.round) run=$($s.run_id)"
    if ($s.note) { Write-Host "      note: $($s.note)" }
  }

  $claudeStep = $rec.steps | Where-Object { $_.agent_id -eq "research-agent" } | Select-Object -First 1
  $opsStep = $rec.steps | Where-Object { $_.agent_id -eq "ops-agent" } | Select-Object -First 1
  # Both ran in the same round, and a poll saw them both in flight at once.
  $sameRound = ($claudeStep.round -eq $opsStep.round)

  Write-Host ""
  Write-Host "SMOKE-RESULT job_state=$($final.state) record_status=$($rec.status) max_running_at_once=$maxRunningSeen both_running_polls=$bothRunningPolls same_round=$sameRound claude_outcome=$($claudeStep.outcome) ops_outcome=$($opsStep.outcome)"
  if ($maxRunningSeen -lt 2) { Write-Host "WARN: never observed two briefs running simultaneously (timing?)" }
}
finally {
  Write-Host "== stopping kernel =="
  if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
  Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue
  Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue
}
