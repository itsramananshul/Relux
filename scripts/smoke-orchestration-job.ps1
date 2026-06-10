#Requires -Version 5.1
# Real smoke for the non-blocking orchestration job + polling path.
# Starts a sandboxed kernel, creates a multi-step orchestration, starts a
# background job, and polls it to completion — proving real recorded progress.
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo "target\debug\relux-kernel.exe"
$smokeRoot = Join-Path $repo "dev-data\relux-smoke"
$db = Join-Path $smokeRoot "local.db"
$addr = "127.0.0.1:19899"
$base = "http://$addr"

if (Test-Path $smokeRoot) { Remove-Item -Recurse -Force $smokeRoot }
New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null

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

try {
  if (-not (Wait-Up)) { throw "server did not come up" }
  Write-Host "== server up =="

  $goal = "research the options, implement a prototype, test it, and document it"
  Write-Host "== creating orchestration: $goal =="
  $orch = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations" `
    -ContentType "application/json" -Body (@{ goal = $goal } | ConvertTo-Json)
  $oid = $orch.id
  Write-Host "   orchestration $oid with $($orch.steps.Count) briefs, status=$($orch.status)"

  Write-Host "== starting NON-BLOCKING job (run-async) =="
  $start = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
    -ContentType "application/json" -Body (@{ concurrency = 2 } | ConvertTo-Json)
  $jobId = $start.id
  Write-Host "   job $jobId state=$($start.state) status_url=$($start.status_url)"
  if ($start.state -ne "queued" -and $start.state -ne "running") { throw "expected queued/running, got $($start.state)" }

  # Duplicate guard: a second start while active must be rejected with 409.
  Write-Host "== verifying duplicate-job guard (expect 409) =="
  $dupRejected = $false
  try {
    Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
      -ContentType "application/json" -Body "{}" | Out-Null
  } catch {
    if ($_.Exception.Response.StatusCode.value__ -eq 409) { $dupRejected = $true }
  }
  Write-Host "   duplicate rejected with 409: $dupRejected"

  Write-Host "== polling job until terminal =="
  $seen = @()
  $final = $null
  for ($i = 0; $i -lt 120; $i++) {
    Start-Sleep -Milliseconds 400
    $j = Invoke-RestMethod -Uri "$base$($start.status_url)"
    $line = "state=$($j.state) round=$($j.current_round) ran=$($j.ran) completed=$($j.completed) failed=$($j.failed) blocked=$($j.blocked) :: $($j.last_event)"
    if ($seen.Count -eq 0 -or $seen[-1] -ne $line) { Write-Host "   poll[$i] $line"; $seen += $line }
    if ($j.state -eq "completed" -or $j.state -eq "failed") { $final = $j; break }
  }
  if ($null -eq $final) { throw "job did not terminate in time" }

  Write-Host "== final job =="
  Write-Host "   state=$($final.state)"
  Write-Host "   result.summary: $($final.result.summary)"
  Write-Host "   result.status:  $($final.result.status)"
  Write-Host "   result.next_action: $($final.result.next_action)"

  Write-Host "== durable record after job (incremental progress persisted) =="
  $rec = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid"
  foreach ($s in $rec.steps) {
    Write-Host "   $($s.task_id) [$($s.role)] outcome=$($s.outcome) round=$($s.round) run=$($s.run_id)"
  }

  $completedCount = ($rec.steps | Where-Object { $_.outcome -eq "completed" }).Count
  Write-Host ""
  Write-Host "SMOKE-RESULT job_state=$($final.state) record_status=$($rec.status) completed_briefs=$completedCount/$($rec.steps.Count) dup_guard_409=$dupRejected"
}
finally {
  Write-Host "== stopping kernel =="
  if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
  Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue
  Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue
}
