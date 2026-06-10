#Requires -Version 5.1
# Real smoke for TRUE bounded OS-parallel execution on the SYNCHRONOUS run path.
#
# This proves the BLOCKING `POST /v1/relux/prime/orchestrations/:id/run` (the same
# engine the `prime orchestration run` CLI uses) now runs independent ready briefs
# as REAL concurrent OS processes — not one-at-a-time under the lock.
#
# Two INDEPENDENT briefs run in the SAME round at concurrency 2, each on an adapter
# whose binary is overridden with a fake CLI that sleeps ~4s then exits 0. No paid
# call. The synchronous /run blocks until the whole batch is done; we TIME it:
#   - parallel  => wall-clock ~= ONE sleep (~4-6s), because both spawn at once;
#   - sequential => wall-clock ~= TWO sleeps (~8s+).
# So an elapsed well under the two-sleep sum, with BOTH briefs completed in round 1,
# is a deterministic timing proof the sync path overlapped the two processes.
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo "target\debug\relux-kernel.exe"
$smokeRoot = Join-Path $repo "dev-data\relux-smoke-sync-parallel"
$db = Join-Path $smokeRoot "local.db"
$addr = "127.0.0.1:19898"
$base = "http://$addr"

if (Test-Path $smokeRoot) { Remove-Item -Recurse -Force $smokeRoot }
New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null

# Two fake "slow" CLIs that each sleep ~4s (ping -n 5 ~= 4s) then print + exit 0.
# Distinct files so neither contends on the other's I/O.
$slowA = Join-Path $smokeRoot "slow-a.cmd"
$slowB = Join-Path $smokeRoot "slow-b.cmd"
@"
@echo off
ping -n 5 127.0.0.1 >nul
echo SLOW_A_DONE
exit /b 0
"@ | Set-Content -Path $slowA -Encoding ASCII
@"
@echo off
ping -n 5 127.0.0.1 >nul
echo SLOW_B_DONE
exit /b 0
"@ | Set-Content -Path $slowB -Encoding ASCII

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

  Write-Host "== enabling Claude + Codex adapters with FAKE slow binaries =="
  Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-claude-cli/runtime" `
    -ContentType "application/json" `
    -Body (@{ enabled = $true; command = $slowA; timeout_seconds = 60 } | ConvertTo-Json) | Out-Null
  Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-codex-cli/runtime" `
    -ContentType "application/json" `
    -Body (@{ enabled = $true; command = $slowB; timeout_seconds = 60 } | ConvertTo-Json) | Out-Null

  Write-Host "== creating the two agents =="
  Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" -ContentType "application/json" `
    -Body (@{ id = "research-agent"; name = "Research Agent"; role = "investigates"; adapter_plugin = "relux-adapter-claude-cli" } | ConvertTo-Json) | Out-Null
  Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" -ContentType "application/json" `
    -Body (@{ id = "ops-agent"; name = "Ops Agent"; role = "ships releases"; adapter_plugin = "relux-adapter-codex-cli" } | ConvertTo-Json) | Out-Null

  # research -> research-agent; deploy/release -> ops-agent. No dependency between
  # them => both ready in round 1.
  $goal = "research the options and deploy the release"
  Write-Host "== creating orchestration: $goal =="
  $orch = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations" `
    -ContentType "application/json" -Body (@{ goal = $goal } | ConvertTo-Json)
  $oid = $orch.id
  foreach ($s in $orch.steps) { Write-Host "   $($s.task_id) [$($s.role)] -> $($s.agent_id) deps=$($s.depends_on -join ',')" }
  if ($orch.steps.Count -ne 2) { throw "expected 2 briefs, got $($orch.steps.Count)" }

  Write-Host "== calling SYNCHRONOUS /run (blocks until done, concurrency=2) =="
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $result = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run" `
    -ContentType "application/json" -Body (@{ concurrency = 2 } | ConvertTo-Json)
  $sw.Stop()
  $elapsed = [math]::Round($sw.Elapsed.TotalSeconds, 2)

  Write-Host "== blocking /run returned after ${elapsed}s =="
  Write-Host "   status=$($result.status) ran=$($result.ran) completed=$($result.completed) failed=$($result.failed) blocked=$($result.blocked) rounds=$($result.rounds) concurrency=$($result.concurrency)"

  $rec = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid"
  foreach ($s in $rec.steps) {
    Write-Host "   $($s.task_id) [$($s.role)] $($s.agent_id) outcome=$($s.outcome) round=$($s.round)"
  }
  $claudeStep = $rec.steps | Where-Object { $_.agent_id -eq "research-agent" } | Select-Object -First 1
  $opsStep = $rec.steps | Where-Object { $_.agent_id -eq "ops-agent" } | Select-Object -First 1
  $sameRound = ($claudeStep.round -eq $opsStep.round)

  # Parallel proof: both briefs each sleep ~4s. Run together (one round) the blocking
  # call returns in ~one sleep; run sequentially it would take ~two (8s+). A threshold
  # of 7s sits safely between the two regimes.
  $parallelProven = ($result.completed -eq 2) -and ($result.rounds -eq 1) -and $sameRound -and ($elapsed -lt 7.0)

  Write-Host ""
  Write-Host "SMOKE-RESULT sync_status=$($result.status) completed=$($result.completed) rounds=$($result.rounds) same_round=$sameRound elapsed_s=$elapsed parallel_proven=$parallelProven"
  if (-not $parallelProven) {
    throw "sync /run did NOT prove parallel execution (completed=$($result.completed) rounds=$($result.rounds) same_round=$sameRound elapsed=${elapsed}s; expected 2 completed in 1 round under 7s)"
  }
  Write-Host "PASS: synchronous /run executed two independent briefs in parallel (~one sleep, not two)."
}
finally {
  Write-Host "== stopping kernel =="
  if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
  Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue
  Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue
}
