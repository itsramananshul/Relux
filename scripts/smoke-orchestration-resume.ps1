#Requires -Version 5.1
# Real smoke for RESUME-AFTER-CANCEL of a non-blocking orchestration job
# (RELUX_MASTER_PLAN Sec 15). This builds on the mid-flight cancel smokes
# (3862b1b single-brief, 2c8a782 multi-brief): those prove a cancel leaves the
# in-flight round honest and downstream briefs pending. This proves the OTHER
# half of the contract - that the partially-done orchestration is genuinely
# RESUMABLE: a fresh job picks up exactly where the canceled one left off.
#
# What it proves end-to-end, over real HTTP, against two real spawned processes:
#   1. A 4-brief job runs at concurrency=2 against two deliberately SLOW local CLI
#      adapters. Round 1 has TWO independent ready briefs (research + operations);
#      we cancel while both are in flight. The round finishes honestly (both reach
#      `completed`), the job reaches terminal `canceled`, and the two DOWNSTREAM
#      briefs (implementation -> waits on research; documentation -> waits on
#      implementation) are left `pending`.
#   2. We snapshot the durable record: the two round-1 briefs are `completed` with
#      real run ids and round numbers; the two downstream briefs are `pending` with
#      NO run id.
#   3. A FRESH non-blocking job is started on the SAME orchestration. Because the
#      first job is terminal (canceled), this is ACCEPTED (not a 409 duplicate).
#   4. The resumed job runs to terminal `completed`, and we prove resume is honest:
#        - it ran ONLY the previously-pending downstream briefs (job.ran == 2),
#          never re-running either already-completed round-1 brief;
#        - each completed round-1 brief kept its ORIGINAL run id and round
#          (byte-identical before/after the resume);
#        - each downstream brief is now `completed` with a BRAND-NEW run id
#          (distinct from the round-1 run ids) and a round number;
#        - the final orchestration record is fully completed.
#
# The slow adapters are FAKE local commands, but each is spawned through the SAME
# real adapter spawn path the kernel uses for any CLI adapter (execute_cli_run ->
# run_adapter_command). The downstream briefs route to default Prime agents (local
# echo), so the resume completes deterministically without any paid model call.
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo "target\debug\relux-kernel.exe"
$smokeRoot = Join-Path $repo "dev-data\relux-smoke-resume"
$db = Join-Path $smokeRoot "local.db"
$addr = "127.0.0.1:19895"
$base = "http://$addr"

if (-not (Test-Path $bin)) { throw "kernel binary not found: $bin (run: cargo build -p relux-kernel --bin relux-kernel)" }
if (Test-Path $smokeRoot) { Remove-Item -Recurse -Force $smokeRoot }
New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null

# --- Two fake SLOW local CLI adapters --------------------------------------
# Each .cmd ignores its args/stdin, sleeps ~8s (ping is reliable under a piped
# stdin, unlike timeout.exe), prints a line, and exits 0.
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
$reachedCanceled = $false
$downstreamPendingAfterCancel = $false
$resumeAccepted = $false
$resumeCompleted = $false
$resumeRanOnlyPending = $false
$completedRunsPreserved = $false
$downstreamGotNewRuns = $false
$finalAllCompleted = $false

try {
  if (-not (Wait-Up)) { throw "server did not come up" }
  Write-Host "== server up =="

  Write-Host "== configuring SLOW fake CLI adapters (real spawn path) =="
  $rtR = Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-claude-cli/runtime" `
    -ContentType "application/json" `
    -Body (@{ enabled = $true; command = $slowResearch; timeout_seconds = 60 } | ConvertTo-Json)
  if (-not $rtR.resolved_path) { throw "slow research adapter did not resolve on PATH" }
  $rtO = Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-codex-cli/runtime" `
    -ContentType "application/json" `
    -Body (@{ enabled = $true; command = $slowOps; timeout_seconds = 60 } | ConvertTo-Json)
  if (-not $rtO.resolved_path) { throw "slow ops adapter did not resolve on PATH" }
  Write-Host "   research adapter resolved=$($rtR.resolved_path); ops adapter resolved=$($rtO.resolved_path)"

  Write-Host "== creating two specialist agents on the slow adapters =="
  $a1 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" -ContentType "application/json" `
    -Body (@{ id = "research-agent"; name = "Research Agent"; role = "investigates"; adapter_plugin = "relux-adapter-claude-cli" } | ConvertTo-Json)
  $a2 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" -ContentType "application/json" `
    -Body (@{ id = "ops-agent"; name = "Ops Agent"; role = "ships releases"; adapter_plugin = "relux-adapter-codex-cli" } | ConvertTo-Json)
  Write-Host "   $($a1.id) -> $($a1.adapter_plugin); $($a2.id) -> $($a2.adapter_plugin)"

  # research + deploy are independent (round 1); implement waits on research;
  # document waits on implement. So round 1 runs research + operations together,
  # and implementation + documentation are the downstream briefs that a cancel
  # leaves pending and a resume must finish.
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

  # ---- Phase A: start, cancel mid-round, reach terminal canceled ----------
  Write-Host "== [A] starting first NON-BLOCKING job (run-async, concurrency=2) =="
  $start1 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
    -ContentType "application/json" -Body (@{ concurrency = 2 } | ConvertTo-Json)
  $job1 = $start1.id
  Write-Host "   job1 $job1 state=$($start1.state) status_url=$($start1.status_url)"

  Write-Host "== [A] polling until BOTH round-1 briefs are genuinely running =="
  $maxRunningSeen = 0
  for ($i = 0; $i -lt 80; $i++) {
    Start-Sleep -Milliseconds 250
    $j = Invoke-RestMethod -Uri "$base$($start1.status_url)"
    $runningIds = @($j.steps | Where-Object { $_.outcome -eq "running" } | ForEach-Object { $_.task_id })
    if ($runningIds.Count -gt $maxRunningSeen) { $maxRunningSeen = $runningIds.Count }
    if ($runningIds.Count -ge 2) { Write-Host "   poll[$i] BOTH RUNNING: [$($runningIds -join ', ')]"; $sawBothRunning = $true; break }
    if ($j.state -ne "queued" -and $j.state -ne "running") { throw "job1 terminated ($($j.state)) before two briefs ran" }
  }
  if (-not $sawBothRunning) { throw "never observed two briefs running together (max seen=$maxRunningSeen)" }

  Write-Host "== [A] requesting cancel WHILE both briefs are in flight =="
  $cancelResp = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/orchestration-jobs/$job1/cancel" `
    -ContentType "application/json" -Body "{}"
  Write-Host "   cancel accepted: state=$($cancelResp.state) cancel_requested=$($cancelResp.cancel_requested)"

  Write-Host "== [A] polling job1 to terminal state =="
  $final1 = $null
  for ($i = 0; $i -lt 120; $i++) {
    Start-Sleep -Milliseconds 400
    $j = Invoke-RestMethod -Uri "$base$($start1.status_url)"
    if ($j.state -eq "canceled" -or $j.state -eq "completed" -or $j.state -eq "failed") { $final1 = $j; break }
  }
  if ($null -eq $final1) { throw "job1 did not reach a terminal state in time" }
  Write-Host "   job1 final state=$($final1.state)"
  if ($final1.state -eq "canceled") { $reachedCanceled = $true }

  Write-Host "== [A] durable record after cancel (round-1 done, downstream pending) =="
  $recA = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid"
  $runBefore = @{}
  $roundBefore = @{}
  foreach ($s in $recA.steps) {
    Write-Host "   $($s.task_id) [$($s.role)] outcome=$($s.outcome) round=$($s.round) run=$($s.run_id)"
    $runBefore[$s.task_id] = $s.run_id
    $roundBefore[$s.task_id] = $s.round
  }
  $round1Recs = $recA.steps | Where-Object { $round1Ids -contains $_.task_id }
  $round1Completed = ($round1Recs | Where-Object { $_.outcome -eq "completed" }).Count
  if ($round1Completed -ne 2) { throw "expected both round-1 briefs completed before resume, got $round1Completed/2" }

  $downstream = $recA.steps | Where-Object { $round1Ids -notcontains $_.task_id }
  $downstreamIds = @($downstream | ForEach-Object { $_.task_id })
  $downstreamCount = $downstream.Count
  $downstreamPendingCount = ($downstream | Where-Object { $_.outcome -eq "pending" }).Count
  if ($downstreamCount -gt 0 -and $downstreamPendingCount -eq $downstreamCount) { $downstreamPendingAfterCancel = $true }
  if (-not $downstreamPendingAfterCancel) { throw "downstream briefs were not all pending after cancel ($downstreamPendingCount/$downstreamCount)" }

  # ---- Phase B: fresh job resumes only the pending downstream briefs ------
  Write-Host "== [B] starting a FRESH job on the same orchestration (resume) =="
  $start2 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
    -ContentType "application/json" -Body (@{ concurrency = 2 } | ConvertTo-Json)
  $job2 = $start2.id
  $resumeAccepted = ($job2 -and $job2 -ne $job1)
  if (-not $resumeAccepted) { throw "resume job was not accepted as a distinct new job (job1=$job1 job2=$job2)" }
  Write-Host "   job2 $job2 state=$($start2.state) (distinct from job1 $job1)"

  Write-Host "== [B] polling resumed job to terminal state =="
  $final2 = $null
  for ($i = 0; $i -lt 120; $i++) {
    Start-Sleep -Milliseconds 400
    $j = Invoke-RestMethod -Uri "$base$($start2.status_url)"
    if ($j.state -eq "completed" -or $j.state -eq "failed" -or $j.state -eq "canceled") { $final2 = $j; break }
  }
  if ($null -eq $final2) { throw "resumed job did not reach a terminal state in time" }
  Write-Host "   job2 final state=$($final2.state) ran=$($final2.ran) completed=$($final2.completed)"
  if ($final2.state -eq "completed") { $resumeCompleted = $true }
  # The KEY honesty check: the resume ran ONLY the previously-pending briefs.
  if ($final2.ran -eq $downstreamCount) { $resumeRanOnlyPending = $true }

  Write-Host "== [B] durable record after resume (completed runs preserved, downstream fresh) =="
  $recB = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid"
  foreach ($s in $recB.steps) {
    Write-Host "   $($s.task_id) [$($s.role)] outcome=$($s.outcome) round=$($s.round) run=$($s.run_id)"
  }

  # Round-1 briefs: byte-identical run id and round before/after the resume.
  $preserved = $true
  foreach ($tid in $round1Ids) {
    $s = $recB.steps | Where-Object { $_.task_id -eq $tid } | Select-Object -First 1
    if ($s.outcome -ne "completed") { $preserved = $false; Write-Host "   ! $tid not completed after resume" }
    if ($s.run_id -ne $runBefore[$tid]) { $preserved = $false; Write-Host "   ! $tid run id changed: $($runBefore[$tid]) -> $($s.run_id)" }
    if ($s.round -ne $roundBefore[$tid]) { $preserved = $false; Write-Host "   ! $tid round changed: $($roundBefore[$tid]) -> $($s.round)" }
  }
  $completedRunsPreserved = $preserved

  # Downstream briefs: now completed, each with a NEW non-empty run id distinct
  # from every round-1 run id, and a recorded round.
  $round1Runs = @($round1Ids | ForEach-Object { $runBefore[$_] })
  $newRuns = $true
  foreach ($tid in $downstreamIds) {
    $s = $recB.steps | Where-Object { $_.task_id -eq $tid } | Select-Object -First 1
    if ($s.outcome -ne "completed") { $newRuns = $false; Write-Host "   ! downstream $tid not completed" }
    if (-not $s.run_id) { $newRuns = $false; Write-Host "   ! downstream $tid has no run id" }
    if ($round1Runs -contains $s.run_id) { $newRuns = $false; Write-Host "   ! downstream $tid reused a round-1 run id ($($s.run_id))" }
    if ($null -eq $s.round) { $newRuns = $false; Write-Host "   ! downstream $tid has no round" }
  }
  $downstreamGotNewRuns = $newRuns

  $finalAllCompleted = (($recB.steps | Where-Object { $_.outcome -eq "completed" }).Count -eq $recB.steps.Count)

  Write-Host ""
  Write-Host "SMOKE-RESULT both_running=$sawBothRunning reached_canceled=$reachedCanceled downstream_pending_after_cancel=$downstreamPendingAfterCancel ($downstreamPendingCount/$downstreamCount) resume_accepted=$resumeAccepted resume_completed=$resumeCompleted resume_ran_only_pending=$resumeRanOnlyPending (ran=$($final2.ran) expected=$downstreamCount) completed_runs_preserved=$completedRunsPreserved downstream_got_new_runs=$downstreamGotNewRuns final_all_completed=$finalAllCompleted"

  if (-not ($sawBothRunning -and $reachedCanceled -and $downstreamPendingAfterCancel -and $resumeAccepted -and $resumeCompleted -and $resumeRanOnlyPending -and $completedRunsPreserved -and $downstreamGotNewRuns -and $finalAllCompleted)) {
    throw "RESUME-AFTER-CANCEL SMOKE FAILED - one or more invariants did not hold (see flags above)"
  }
  Write-Host "RESUME-AFTER-CANCEL SMOKE PASSED"
}
finally {
  Write-Host "== stopping kernel =="
  if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
  Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue
  Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue
}
