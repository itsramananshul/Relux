#Requires -Version 5.1
# Real smoke for RESTART-HONEST orchestration job status (RELUX_MASTER_PLAN Sec 15).
# The job registry is in-memory, so a server restart loses every live job. This
# proves the user-facing contract that survives a restart end-to-end, over real
# HTTP, against a kernel process that is genuinely STOPPED and RESTARTED over the
# same SQLite store:
#
#   1. A first NON-BLOCKING job is run with max=1, so it deterministically runs
#      exactly ONE brief and stops at terminal `completed`, leaving the rest
#      pending (the same partial shape a mid-flight cancel leaves, but with no
#      racy timing). We capture the (process-local) job id.
#   2. The kernel is STOPPED and RESTARTED over the same db.
#   3. Polling BY ORCHESTRATION ID (GET …/orchestrations/:id/job) is restart-honest:
#      it reconstructs an `interrupted` status from the durable record (no live
#      worker, briefs still pending), with a clearly-synthetic `durable:` id and a
#      ran count that matches what really ran. It does NOT misleadingly 404.
#   4. Polling BY (process-local) JOB ID (GET …/orchestration-jobs/:job_id) 404s
#      for the lost job — its id cannot be mapped to an orchestration after a
#      restart — and the message points at the durable by-orchestration poll.
#   5. A FRESH job resumes the pending briefs to terminal `completed` (the
#      reconstruction never blocks a resume).
#   6. After ANOTHER restart, the by-orchestration poll reconstructs `completed`
#      (every brief terminal, nothing pending), proving the honest distinction
#      between an interrupted run and a finished one.
#
# All briefs route to default Prime agents (local echo), so every round completes
# deterministically with NO paid model call and NO external CLI.
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo "target\debug\relux-kernel.exe"
$smokeRoot = Join-Path $repo "dev-data\relux-smoke-restart"
$db = Join-Path $smokeRoot "local.db"
$addr = "127.0.0.1:19896"
$base = "http://$addr"

if (-not (Test-Path $bin)) { throw "kernel binary not found: $bin (run: cargo build -p relux-kernel --bin relux-kernel)" }
if (Test-Path $smokeRoot) { Remove-Item -Recurse -Force $smokeRoot }
New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null

$env:RELUX_DB = $db
$env:RELUX_HTTP_ADDR = $addr

$script:proc = $null
$script:bootCount = 0

function Wait-Up {
  for ($i = 0; $i -lt 40; $i++) {
    try { Invoke-RestMethod -Uri "$base/v1/relux/health" -TimeoutSec 2 | Out-Null; return $true }
    catch { Start-Sleep -Milliseconds 250 }
  }
  return $false
}

function Start-Kernel {
  $script:bootCount += 1
  $tag = $script:bootCount
  Write-Host "== starting kernel #$tag ($addr), db=$db =="
  $script:proc = Start-Process -FilePath $bin -ArgumentList "serve" -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $smokeRoot "server.$tag.out.log") `
    -RedirectStandardError (Join-Path $smokeRoot "server.$tag.err.log")
  if (-not (Wait-Up)) { throw "server #$tag did not come up" }
  Write-Host "== server #$tag up =="
}

function Stop-Kernel {
  if ($script:proc -and -not $script:proc.HasExited) {
    Write-Host "== stopping kernel =="
    Stop-Process -Id $script:proc.Id -Force
    $script:proc.WaitForExit(5000) | Out-Null
  }
  $script:proc = $null
}

# Result flags (proved as we go); summarized at the end.
$firstJobCompletedOneBrief = $false
$reconstructedInterrupted = $false
$reconstructedIdSynthetic = $false
$reconstructedRanMatches = $false
$jobIdPoll404 = $false
$resumeCompleted = $false
$reconstructedCompleted = $false

try {
  Start-Kernel

  $goal = "research the rust options and document the findings"
  Write-Host "== creating orchestration: $goal =="
  $orch = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations" `
    -ContentType "application/json" -Body (@{ goal = $goal } | ConvertTo-Json)
  $oid = $orch.id
  $briefCount = $orch.steps.Count
  if ($briefCount -lt 2) { throw "needs a multi-brief plan, got $briefCount" }
  Write-Host "   $oid with $briefCount briefs"

  # ---- Phase A: run exactly one brief, then restart -----------------------
  Write-Host "== [A] first job, max=1 (runs exactly one brief, leaves the rest pending) =="
  $start1 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
    -ContentType "application/json" -Body (@{ max = 1; concurrency = 1 } | ConvertTo-Json)
  $job1 = $start1.id
  $final1 = $null
  for ($i = 0; $i -lt 120; $i++) {
    Start-Sleep -Milliseconds 250
    $j = Invoke-RestMethod -Uri "$base$($start1.status_url)"
    if ($j.state -eq "completed" -or $j.state -eq "failed" -or $j.state -eq "canceled") { $final1 = $j; break }
  }
  if ($null -eq $final1) { throw "job1 did not reach a terminal state in time" }
  Write-Host "   job1 $job1 final state=$($final1.state) ran=$($final1.ran)"
  if ($final1.state -eq "completed" -and $final1.ran -eq 1) { $firstJobCompletedOneBrief = $true }
  if (-not $firstJobCompletedOneBrief) { throw "expected job1 completed with ran=1, got state=$($final1.state) ran=$($final1.ran)" }

  $recA = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid"
  $ranBefore = @($recA.steps | Where-Object { $_.run_id }).Count
  $pendingBefore = @($recA.steps | Where-Object { $_.outcome -eq "pending" }).Count
  Write-Host "   durable record: $ranBefore ran, $pendingBefore pending"
  if ($pendingBefore -lt 1) { throw "expected pending briefs after a max=1 run" }

  Write-Host "== RESTART #1 (the in-memory job registry is lost) =="
  Stop-Kernel
  Start-Kernel

  # ---- Phase B: by-orchestration poll is restart-honest (reconstructs) ----
  Write-Host "== [B] poll BY ORCHESTRATION ID after restart (expect reconstructed 'interrupted') =="
  $recon = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid/job"
  Write-Host "   reconstructed: id=$($recon.id) state=$($recon.state) ran=$($recon.ran)"
  if ($recon.state -eq "interrupted") { $reconstructedInterrupted = $true }
  if ($recon.id -like "durable:*") { $reconstructedIdSynthetic = $true }
  if ($recon.ran -eq $ranBefore) { $reconstructedRanMatches = $true }
  if (-not $reconstructedInterrupted) { throw "expected reconstructed state 'interrupted', got '$($recon.state)'" }
  if (-not $reconstructedIdSynthetic) { throw "expected a synthetic 'durable:' id, got '$($recon.id)'" }
  if (-not $reconstructedRanMatches) { throw "reconstructed ran=$($recon.ran) != durable ran=$ranBefore" }

  Write-Host "== [B] poll BY (process-local) JOB ID after restart (expect 404) =="
  try {
    Invoke-RestMethod -Uri "$base/v1/relux/orchestration-jobs/$job1" | Out-Null
    throw "by-job-id poll unexpectedly succeeded for a lost job"
  }
  catch {
    $code = $null
    if ($_.Exception.Response) { $code = [int]$_.Exception.Response.StatusCode }
    if ($code -eq 404) { $jobIdPoll404 = $true; Write-Host "   got 404 as expected for lost job id $job1" }
    else { throw "expected 404 for the lost job id, got '$code'" }
  }

  # ---- Phase C: a fresh job still resumes the pending briefs ---------------
  Write-Host "== [C] fresh job resumes the pending briefs to completion =="
  $start2 = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
    -ContentType "application/json" -Body (@{ concurrency = 2 } | ConvertTo-Json)
  $final2 = $null
  for ($i = 0; $i -lt 120; $i++) {
    Start-Sleep -Milliseconds 250
    $j = Invoke-RestMethod -Uri "$base$($start2.status_url)"
    if ($j.state -eq "completed" -or $j.state -eq "failed" -or $j.state -eq "canceled") { $final2 = $j; break }
  }
  if ($null -eq $final2) { throw "resumed job did not reach a terminal state in time" }
  Write-Host "   job2 $($start2.id) final state=$($final2.state) ran=$($final2.ran)"
  if ($final2.state -eq "completed") { $resumeCompleted = $true }
  if (-not $resumeCompleted) { throw "expected resumed job completed, got $($final2.state)" }

  Write-Host "== RESTART #2 (after the orchestration finished) =="
  Stop-Kernel
  Start-Kernel

  # ---- Phase D: a finished orchestration reconstructs as 'completed' -------
  Write-Host "== [D] poll BY ORCHESTRATION ID after restart (expect reconstructed 'completed') =="
  $recon2 = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid/job"
  Write-Host "   reconstructed: id=$($recon2.id) state=$($recon2.state) ran=$($recon2.ran)"
  if ($recon2.state -eq "completed") { $reconstructedCompleted = $true }
  if (-not $reconstructedCompleted) { throw "expected reconstructed state 'completed', got '$($recon2.state)'" }

  Write-Host ""
  Write-Host "SMOKE-RESULT first_job_completed_one_brief=$firstJobCompletedOneBrief reconstructed_interrupted=$reconstructedInterrupted reconstructed_id_synthetic=$reconstructedIdSynthetic reconstructed_ran_matches=$reconstructedRanMatches (ran=$($recon.ran) durable=$ranBefore) job_id_poll_404=$jobIdPoll404 resume_completed=$resumeCompleted reconstructed_completed=$reconstructedCompleted"

  if (-not ($firstJobCompletedOneBrief -and $reconstructedInterrupted -and $reconstructedIdSynthetic -and $reconstructedRanMatches -and $jobIdPoll404 -and $resumeCompleted -and $reconstructedCompleted)) {
    throw "RESTART-HONEST SMOKE FAILED - one or more invariants did not hold (see flags above)"
  }
  Write-Host "RESTART-HONEST SMOKE PASSED"
}
finally {
  Stop-Kernel
  Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue
  Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue
}
