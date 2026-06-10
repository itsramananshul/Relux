#Requires -Version 5.1
# Real smoke for COOPERATIVE cancellation of a MULTI-BRIEF in-flight round
# (RELUX_MASTER_PLAN Sec 15). This extends the single-in-flight-brief cancel smoke
# (3862b1b) to the case the worker's honesty contract really hinges on: a cancel
# that arrives while TWO independent briefs are running together in the SAME round.
#
# What it proves end-to-end, over real HTTP, against two real spawned processes:
#   1. A 4-brief job runs at concurrency=2. Round 1 has TWO independent ready
#      briefs - a research brief and an operations (deploy) brief - each routed to
#      its own deliberately SLOW local CLI adapter (a fake ping-based .cmd that
#      sleeps several seconds). With concurrency=2 both spawn on parallel OS
#      threads, so a single poll genuinely catches BOTH `running` at once.
#   2. While both briefs are in flight we POST .../cancel and observe the job report
#      cancel_requested=true while still `running` (the "canceling - finishing the
#      in-flight round" phase). The worker NEVER kills a brief mid-flight.
#   3. The in-flight round finishes HONESTLY: BOTH running briefs reach their real
#      `completed` outcome before the worker stops - cancel is honored only between
#      rounds, after the round's briefs persist.
#   4. The job then reaches terminal state `canceled`, and every DOWNSTREAM brief
#      (implementation, which waits on research; documentation, which waits on
#      implementation) is left `pending` (not faked, not run) for a human to resume.
#
# The slow adapters are FAKE local commands, but each is spawned through the SAME
# real adapter spawn path the kernel uses for any CLI adapter (execute_cli_run ->
# run_adapter_command) - no test-only internals, no paid model calls.
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo "target\debug\relux-kernel.exe"
$smokeRoot = Join-Path $repo "dev-data\relux-smoke-cancel-multi"
$db = Join-Path $smokeRoot "local.db"
$addr = "127.0.0.1:19896"
$base = "http://$addr"

if (-not (Test-Path $bin)) { throw "kernel binary not found: $bin (run: cargo build -p relux-kernel --bin relux-kernel)" }
if (Test-Path $smokeRoot) { Remove-Item -Recurse -Force $smokeRoot }
New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null

# --- Two fake SLOW local CLI adapters --------------------------------------
# Each .cmd ignores its args/stdin, sleeps ~8s (ping is reliable under a piped
# stdin, unlike timeout.exe), prints a line, and exits 0. Two separate scripts so
# the two round-1 briefs spawn two distinct, overlapping processes.
$slowResearch = Join-Path $smokeRoot "slow-research.cmd"
@"
@echo off
ping -n 9 127.0.0.1 >nul
echo SLOW_RESEARCH_DONE
exit /b 0
"@ | Set-Content -Path $slowResearch -Encoding ascii
$slowOps = Join-Path $smokeRoot "slow-ops.cmd"
@"
@echo off
ping -n 9 127.0.0.1 >nul
echo SLOW_OPS_DONE
exit /b 0
"@ | Set-Content -Path $slowOps -Encoding ascii

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
$sawBothRunning = $false
$maxRunningSeen = 0
$sawCancelRequestedWhileRunning = $false
$reachedCanceled = $false
$bothInflightCompletedHonestly = $false
$downstreamPending = $false
$downstreamPendingCount = 0
$downstreamCount = 0

try {
  if (-not (Wait-Up)) { throw "server did not come up" }
  Write-Host "== server up =="

  # Point two installed CLI adapter runtimes at our fake slow commands and enable
  # them. recognize_adapter_kind keeps each a real CLI adapter; only the override
  # command changes, so each brief runs through execute_cli_run -> run_adapter_command
  # exactly like a genuine CLI - just cheap and deterministic.
  Write-Host "== configuring SLOW fake CLI adapters (real spawn path) =="
  $rtR = Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-claude-cli/runtime" `
    -ContentType "application/json" `
    -Body (@{ enabled = $true; command = $slowResearch; timeout_seconds = 60 } | ConvertTo-Json)
  Write-Host "   research adapter state=$($rtR.state) resolved=$($rtR.resolved_path)"
  if (-not $rtR.resolved_path) { throw "slow research adapter did not resolve on PATH" }
  $rtO = Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-codex-cli/runtime" `
    -ContentType "application/json" `
    -Body (@{ enabled = $true; command = $slowOps; timeout_seconds = 60 } | ConvertTo-Json)
  Write-Host "   ops adapter state=$($rtO.state) resolved=$($rtO.resolved_path)"
  if (-not $rtO.resolved_path) { throw "slow ops adapter did not resolve on PATH" }

  Write-Host "== creating two specialist agents on the slow adapters =="
  # research-agent (id contains 'research') gets the research brief; ops-agent (id
  # contains 'ops') gets the operations/deploy brief. Both round-1 briefs are
  # independent, so they run together.
  $a1 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" -ContentType "application/json" `
    -Body (@{ id = "research-agent"; name = "Research Agent"; role = "investigates"; adapter_plugin = "relux-adapter-claude-cli" } | ConvertTo-Json)
  $a2 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" -ContentType "application/json" `
    -Body (@{ id = "ops-agent"; name = "Ops Agent"; role = "ships releases"; adapter_plugin = "relux-adapter-codex-cli" } | ConvertTo-Json)
  Write-Host "   $($a1.id) -> $($a1.adapter_plugin); $($a2.id) -> $($a2.adapter_plugin)"

  # research + deploy are independent (round 1); implement waits on research;
  # document waits on implement. So round 1 runs research + operations together,
  # and implementation + documentation are the downstream briefs that a cancel
  # must leave pending.
  $goal = "research the options and deploy the release, then implement a prototype and document it"
  Write-Host "== creating orchestration: $goal =="
  $orch = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations" `
    -ContentType "application/json" -Body (@{ goal = $goal } | ConvertTo-Json)
  $oid = $orch.id
  foreach ($s in $orch.steps) { Write-Host "   $($s.task_id) [$($s.role)] -> $($s.agent_id) deps=$($s.depends_on -join ',')" }
  $researchStep = $orch.steps | Where-Object { $_.agent_id -eq "research-agent" } | Select-Object -First 1
  $opsStep = $orch.steps | Where-Object { $_.agent_id -eq "ops-agent" } | Select-Object -First 1
  if (-not $researchStep) { throw "research brief did not route to research-agent" }
  if (-not $opsStep) { throw "operations brief did not route to ops-agent" }
  $round1Ids = @($researchStep.task_id, $opsStep.task_id)

  Write-Host "== starting NON-BLOCKING job (run-async, concurrency=2) =="
  $start = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
    -ContentType "application/json" -Body (@{ concurrency = 2 } | ConvertTo-Json)
  $jobId = $start.id
  Write-Host "   job $jobId state=$($start.state) concurrency=$($start.concurrency) status_url=$($start.status_url)"

  Write-Host "== polling until BOTH round-1 briefs are genuinely running together =="
  for ($i = 0; $i -lt 80; $i++) {
    Start-Sleep -Milliseconds 250
    $j = Invoke-RestMethod -Uri "$base$($start.status_url)"
    $runningIds = @($j.steps | Where-Object { $_.outcome -eq "running" } | ForEach-Object { $_.task_id })
    if ($runningIds.Count -gt $maxRunningSeen) { $maxRunningSeen = $runningIds.Count }
    if ($runningIds.Count -ge 2) {
      Write-Host "   poll[$i] BOTH RUNNING: [$($runningIds -join ', ')] round=$($j.current_round)"
      $sawBothRunning = $true
      break
    }
    if ($j.state -ne "queued" -and $j.state -ne "running") { throw "job terminated ($($j.state)) before two briefs ran" }
  }
  if (-not $sawBothRunning) { throw "never observed two briefs running together (max seen=$maxRunningSeen) - slow adapters may not have spawned in parallel" }

  Write-Host "== requesting cancel WHILE both briefs are in flight =="
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
    # response still proves the flag was set while we held both briefs running.
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

  Write-Host "== durable record: both in-flight briefs honest, downstream untouched =="
  $rec = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid"
  foreach ($s in $rec.steps) {
    Write-Host "   $($s.task_id) [$($s.role)] $($s.agent_id) outcome=$($s.outcome) round=$($s.round)"
  }
  $round1Recs = $rec.steps | Where-Object { $round1Ids -contains $_.task_id }
  $round1Completed = ($round1Recs | Where-Object { $_.outcome -eq "completed" }).Count
  if ($round1Completed -eq 2) { $bothInflightCompletedHonestly = $true }

  $downstream = $rec.steps | Where-Object { $round1Ids -notcontains $_.task_id }
  $downstreamCount = $downstream.Count
  $downstreamPendingCount = ($downstream | Where-Object { $_.outcome -eq "pending" }).Count
  if ($downstreamCount -gt 0 -and $downstreamPendingCount -eq $downstreamCount) { $downstreamPending = $true }

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
  Write-Host "SMOKE-RESULT both_running=$sawBothRunning max_running=$maxRunningSeen cancel_requested_seen=$sawCancelRequestedWhileRunning reached_canceled=$reachedCanceled both_inflight_completed_honestly=$bothInflightCompletedHonestly ($round1Completed/2) downstream_pending=$downstreamPending ($downstreamPendingCount/$downstreamCount) idempotent_recancel=$idempotent"

  if (-not ($sawBothRunning -and $sawCancelRequestedWhileRunning -and $reachedCanceled -and $bothInflightCompletedHonestly -and $downstreamPending -and $idempotent)) {
    throw "MULTI-BRIEF CANCELLATION SMOKE FAILED - one or more invariants did not hold (see flags above)"
  }
  Write-Host "MULTI-BRIEF CANCELLATION SMOKE PASSED"
}
finally {
  Write-Host "== stopping kernel =="
  if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
  Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue
  Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue
}
