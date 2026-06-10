#Requires -Version 5.1
# Real smoke for the non-blocking job path WITH a real Claude CLI brief.
# One brief runs through the local Claude CLI (slow enough to observe live
# polling/round progress); the rest run on the local Prime echo adapter.
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo "target\debug\relux-kernel.exe"
$smokeRoot = Join-Path $repo "dev-data\relux-smoke-claude"
$db = Join-Path $smokeRoot "local.db"
$addr = "127.0.0.1:19898"
$base = "http://$addr"

if (Test-Path $smokeRoot) { Remove-Item -Recurse -Force $smokeRoot }
New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null

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

  Write-Host "== enabling Claude CLI adapter runtime =="
  $rt = Invoke-RestMethod -Method Put -Uri "$base/v1/relux/adapters/relux-adapter-claude-cli/runtime" `
    -ContentType "application/json" -Body (@{ enabled = $true } | ConvertTo-Json)
  Write-Host "   adapter state=$($rt.state) resolved=$($rt.resolved_path)"

  Write-Host "== creating a Claude-backed research agent =="
  $agent = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/agents" `
    -ContentType "application/json" `
    -Body (@{ id = "research-agent"; name = "Research Agent"; role = "investigates"; adapter_plugin = "relux-adapter-claude-cli" } | ConvertTo-Json)
  Write-Host "   agent $($agent.id) adapter=$($agent.adapter_plugin)"

  # The research clause routes to research-agent (Claude); the document clause to
  # Prime (local echo). Keep the research brief tiny to bound token spend.
  $goal = "research what two plus two equals and document the answer"
  Write-Host "== creating orchestration: $goal =="
  $orch = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations" `
    -ContentType "application/json" -Body (@{ goal = $goal } | ConvertTo-Json)
  $oid = $orch.id
  foreach ($s in $orch.steps) { Write-Host "   $($s.task_id) [$($s.role)] -> $($s.agent_id)" }

  Write-Host "== starting NON-BLOCKING job (run-async) =="
  $start = Invoke-RestMethod -Method Post -Uri "$base/v1/relux/prime/orchestrations/$oid/run-async" `
    -ContentType "application/json" -Body (@{ concurrency = 1 } | ConvertTo-Json)
  Write-Host "   job $($start.id) state=$($start.state)"

  Write-Host "== polling job (watch live phase/round/running briefs) =="
  $seen = @()
  $final = $null
  for ($i = 0; $i -lt 200; $i++) {
    Start-Sleep -Milliseconds 500
    $j = Invoke-RestMethod -Uri "$base$($start.status_url)"
    $running = ($j.steps | Where-Object { $_.outcome -eq "running" } | ForEach-Object { $_.task_id }) -join ","
    $line = "state=$($j.state) round=$($j.current_round) ran=$($j.ran) completed=$($j.completed) failed=$($j.failed) blocked=$($j.blocked) running=[$running]"
    if ($seen.Count -eq 0 -or $seen[-1] -ne $line) { Write-Host "   poll[$i] $line"; $seen += $line }
    if ($j.state -eq "completed" -or $j.state -eq "failed") { $final = $j; break }
  }
  if ($null -eq $final) { throw "job did not terminate in time" }

  Write-Host "== final record =="
  $rec = Invoke-RestMethod -Uri "$base/v1/relux/prime/orchestrations/$oid"
  foreach ($s in $rec.steps) {
    Write-Host "   $($s.task_id) [$($s.role)] $($s.agent_id) outcome=$($s.outcome) round=$($s.round) run=$($s.run_id)"
    if ($s.note) { Write-Host "      note: $($s.note)" }
  }

  # Show the real Claude run transcript head, proving it was a real CLI call.
  $claudeStep = $rec.steps | Where-Object { $_.agent_id -eq "research-agent" } | Select-Object -First 1
  if ($claudeStep -and $claudeStep.run_id) {
    Write-Host "== Claude brief run $($claudeStep.run_id) events =="
    try {
      $events = Invoke-RestMethod -Uri "$base/v1/relux/runs/$($claudeStep.run_id)/events"
      foreach ($e in ($events | Select-Object -First 8)) {
        Write-Host "   event kind=$($e.kind) status=$($e.status)"
      }
    } catch { Write-Host "   (events unavailable: $_)" }
  }

  $pollCount = $seen.Count
  Write-Host ""
  Write-Host "SMOKE-RESULT job_state=$($final.state) record_status=$($rec.status) distinct_poll_states=$pollCount claude_brief_outcome=$($claudeStep.outcome)"
}
finally {
  Write-Host "== stopping kernel =="
  if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
  Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue
  Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue
}
