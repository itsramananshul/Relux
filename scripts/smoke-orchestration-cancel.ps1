#Requires -Version 5.1
# Real smoke for COOPERATIVE cancellation of a non-blocking orchestration job
# (RELUX_MASTER_PLAN Sec 15). This closes the live-cancel gap left by 52a290a:
# the Rust tests pin the state machine deterministically, but no HTTP smoke had
# ever cancelled a job MID-FLIGHT against a real, slowly-running adapter process.
#
# What it proves end-to-end, over real HTTP, against a real spawned process:
#   1. A multi-brief job starts; the first brief routes to a deliberately SLOW
#      local CLI adapter (a fake `ping`-based .cmd that sleeps several seconds),
#      so a poll genuinely catches that brief `running`.
#   2. While that brief is in flight we POST .../cancel and observe the job report
#      cancel_requested=true while still `running` (the "canceling - finishing the
#      in-flight round" phase).
#   3. The in-flight round finishes HONESTLY: the running brief reaches its real
#      `completed` outcome (the worker never kills a brief mid-flight).
#   4. The job then reaches terminal state `canceled`, and every downstream brief
#      is left `pending` (not faked, not run) for a human to resume.
#
# The slow adapter is a FAKE local command, but it is spawned through the SAME
# real adapter spawn path the kernel uses for any CLI adapter (execute_cli_run ->
# run_adapter_command) - no test-only internals, no paid model calls.
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo "target\debug\relux-kernel.exe"
$smokeRoot = Join-Path $repo "dev-data\relux-smoke-cancel"
$db = Join-Path $smokeRoot "local.db"
$addr = "127.0.0.1:19897"
$base = "http://$addr"

if (-not (Test-Path $bin)) { throw "kernel binary not found: $bin (run: cargo build -p relux-kernel --bin relux-kernel)" }
if (Test-Path $smokeRoot) { Remove-Item -Recurse -Force $smokeRoot }
New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null

# --- Fake SLOW local CLI adapter -------------------------------------------
# A .cmd that ignores its args/stdin, sleeps ~8s (ping is reliable under a piped
# stdin, unlike timeout.exe), prints a line, and exits 0. Spawned via the real
# adapter path, so the running brief stays in flight long enough to cancel.
$slowCli = Join-Path $smokeRoot "slow-adapter.cmd"
@"
@echo off
ping -n 9 127.0.0.1 >nul
echo SLOW_ADAPTER_DONE
exit /b 0
"@ | Set-Content -Path $slowCli -Encoding ascii

$env:RELUX_DB = $db
$env:RELUX_HTTP_ADDR = $addr

Write-Host "== starting kernel ($addr), db=$db =="
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

# Result flags (proved as we go); summarized at the end.
$sawRunning = $false
$sawCancelRequestedWhileRunning = $false
$reachedCanceled = $false
$inflightCompletedHonestly = $false
$downstreamPending = $false

try {
  if (-not (Wait-Up)) { throw "server did not come up" }
  Write-Host "== server up =="

  Write-Host "== configuring the SLOW fake CLI adapter (real spawn path) =="
  # Point the (installed) Codex CLI adapter runtime at our fake slow command and
  # enable it. recognize_adapter_kind keeps this a real CLI adapter; the override
  # command is all that changes, so the brief runs through execute_cli_run ->
  # run_adapter_command exactly like a genuine CLI.
  $rt = Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-codex-cli/runtime" `
    -ContentType "application/json" `
    -Body (@{ enabled = $true; command = $slowCli; timeout_seconds = 60 } | ConvertTo-Json)
  Write-Host "   adapter state=$($rt.state) resolved=$($rt.resolved_path)"
  if (-not $rt.resolved_path) { throw "slow adapter did not resolve on PATH" }

  Write-Host "== creating a research agent bound to the slow adapter =="
  # Id contains 'research' so the planner routes the first (research) brief here.
  $agent = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" `
    -ContentType "application/json" `
    -Body (@{ id = "research-agent"; name = "Research Agent"; role = "investigates"; adapter_plugin = "relux-adapter-codex-cli" } | ConvertTo-Json)
  Write-Host "   agent $($agent.id) adapter=$($agent.adapter_plugin)"

  $goal = "research the options, implement a prototype, test it, and document it"
  Write-Host "== creating orchestration: $goal =="
  $orch = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations" `
    -ContentType "application/json" -Body (@{ goal = $goal } | ConvertTo-Json)
  $oid = $orch.id
  foreach ($s in $orch.steps) { Write-Host "   $($s.task_id) [$($s.role)] -> $($s.agent_id)" }
  $researchStep = $orch.steps | Where-Object { $_.agent_id -eq "research-agent" } | Select-Object -First 1
  if (-not $researchStep) { throw "research brief did not route to research-agent" }

  # concurrency=1 => one brief per round. Round 1 runs ONLY the slow research
  # brief, giving a clean window to cancel mid-flight; the cancel is honored
  # before round 2 (implement) ever starts.
  Write-Host "== starting NON-BLOCKING job (run-async, concurrency=1) =="
  $start = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
    -ContentType "application/json" -Body (@{ concurrency = 1 } | ConvertTo-Json)
  $jobId = $start.id
  Write-Host "   job $jobId state=$($start.state) status_url=$($start.status_url)"

  Write-Host "== polling until the research brief is genuinely running =="
  for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Milliseconds 300
    $j = Invoke-RestMethod -Uri "$base$($start.status_url)"
    $running = ($j.steps | Where-Object { $_.outcome -eq "running" } | ForEach-Object { $_.task_id }) -join ","
    if ($running) {
      Write-Host "   poll[$i] state=$($j.state) running=[$running]"
      $sawRunning = $true
      break
    }
    if ($j.state -ne "queued" -and $j.state -ne "running") { throw "job terminated ($($j.state)) before a brief ran" }
  }
  if (-not $sawRunning) { throw "never observed a running brief - slow adapter may not have spawned" }

  Write-Host "== requesting cancel WHILE the brief is in flight =="
  $cancelResp = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/orchestration-jobs/$jobId/cancel" `
    -ContentType "application/json" -Body "{}"
  Write-Host "   cancel accepted: state=$($cancelResp.state) cancel_requested=$($cancelResp.cancel_requested)"

  # Immediately re-read: the worker should still be finishing the in-flight round,
  # so the job is still 'running' with cancel_requested=true (the canceling phase).
  $mid = Invoke-RestMethod -Uri "$base$($start.status_url)"
  Write-Host "   mid-cancel: state=$($mid.state) cancel_requested=$($mid.cancel_requested) last_event=$($mid.last_event)"
  if ($mid.cancel_requested -eq $true -and $mid.state -eq "running") {
    $sawCancelRequestedWhileRunning = $true
  } elseif ($cancelResp.cancel_requested -eq $true) {
    # Acceptable race: the round finished between cancel and re-read. The accept
    # response still proves the flag was set while we held the brief running.
    $sawCancelRequestedWhileRunning = $true
    Write-Host "   (round finished between cancel and re-read; cancel flag was set on accept)"
  }

  Write-Host "== polling to terminal state =="
  $final = $null
  for ($i = 0; $i -lt 120; $i++) {
    Start-Sleep -Milliseconds 400
    $j = Invoke-RestMethod -Uri "$base$($start.status_url)"
    if ($j.state -eq "canceled" -or $j.state -eq "completed" -or $j.state -eq "failed") { $final = $j; break }
  }
  if ($null -eq $final) { throw "job did not reach a terminal state in time" }
  Write-Host "   final job state=$($final.state)"
  Write-Host "   last_event: $($final.last_event)"
  if ($final.state -eq "canceled") { $reachedCanceled = $true }

  Write-Host "== durable record: in-flight honest, downstream untouched =="
  $rec = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid"
  foreach ($s in $rec.steps) {
    Write-Host "   $($s.task_id) [$($s.role)] $($s.agent_id) outcome=$($s.outcome) round=$($s.round)"
  }
  $researchRec = $rec.steps | Where-Object { $_.task_id -eq $researchStep.task_id } | Select-Object -First 1
  if ($researchRec.outcome -eq "completed") { $inflightCompletedHonestly = $true }

  $downstream = $rec.steps | Where-Object { $_.task_id -ne $researchStep.task_id }
  $downstreamPendingCount = ($downstream | Where-Object { $_.outcome -eq "pending" }).Count
  if ($downstream.Count -gt 0 -and $downstreamPendingCount -eq $downstream.Count) { $downstreamPending = $true }

  # Cancelling an already-canceled job is idempotent (200), never a faked re-run.
  Write-Host "== verifying cancel is idempotent on an already-canceled job (expect 200) =="
  $idempotent = $false
  try {
    $again = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/orchestration-jobs/$jobId/cancel" `
      -ContentType "application/json" -Body "{}"
    if ($again.state -eq "canceled") { $idempotent = $true }
  } catch { }
  Write-Host "   idempotent re-cancel ok: $idempotent"

  Write-Host ""
  Write-Host "SMOKE-RESULT saw_running=$sawRunning cancel_requested_seen=$sawCancelRequestedWhileRunning reached_canceled=$reachedCanceled inflight_completed_honestly=$inflightCompletedHonestly downstream_pending=$downstreamPending ($downstreamPendingCount/$($downstream.Count)) idempotent_recancel=$idempotent"

  if (-not ($sawRunning -and $sawCancelRequestedWhileRunning -and $reachedCanceled -and $inflightCompletedHonestly -and $downstreamPending -and $idempotent)) {
    throw "CANCELLATION SMOKE FAILED - one or more invariants did not hold (see flags above)"
  }
  Write-Host "CANCELLATION SMOKE PASSED"
}
finally {
  Write-Host "== stopping kernel =="
  if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
  Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue
  Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue
}
