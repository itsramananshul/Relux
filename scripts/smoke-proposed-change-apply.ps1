# scripts/smoke-proposed-change-apply.ps1
#
# Smoke for the first real Relux diff/apply model — reviewed, applyable PROPOSED
# CHANGES (RELUX_MASTER_PLAN §15 / §9.6). Two layers, both honest:
#
#   1. End-to-end model test (always): runs the canonical kernel integration
#      tests that drive a FAKE Claude CLI emitting a `proposed_changes` envelope —
#      a full-content replacement (with a baseline sha256) AND a new-file create
#      (no baseline) — captures them onto the durable run, approves them, and
#      APPLIES them into a temp workspace, asserting the files actually changed /
#      were created. This is the "fake CLI envelope producing a proposed change and
#      apply to a temp workspace" smoke, and it is cross-platform + deterministic
#      (no network, no real Claude).
#
#   2. HTTP wiring check (optional, default on): starts `relux-kernel serve` on a
#      free loopback port against a THROWAWAY RELUX_DB and confirms the new
#      review/apply routes exist and REFUSE HONESTLY on a bogus run/index
#      (404, never a fabricated success). Skip with -SkipServe.
#
# The throwaway DB never touches the operator's dev store. No real Claude/Codex is
# spawned. Prints a concise PASS/FAIL table and exits non-zero on any failure.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-proposed-change-apply.ps1
#   ... -SkipBuild   # reuse an existing target\release\relux-kernel.exe for serve
#   ... -SkipServe   # run only the end-to-end model test (no HTTP)
#   ... -KeepTemp    # keep the temp RELUX_DB for inspection

[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipServe,
    [switch]$KeepTemp
)

# Native cargo writes progress to stderr; don't let that abort the script. We key
# off stdout ("test result: ok") + $LASTEXITCODE instead, and never merge stderr
# (2>&1) on a native exe (PS 5.1 wraps each stderr line in an ErrorRecord).
$ErrorActionPreference = 'Continue'
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$script:Results = @()
function Pass($name, $note) { $script:Results += [pscustomobject]@{ Result = 'PASS'; Name = $name; Note = $note }; Write-Host ("  PASS  {0}" -f $name) -ForegroundColor Green }
function Fail($name, $note) { $script:Results += [pscustomobject]@{ Result = 'FAIL'; Name = $name; Note = $note }; Write-Host ("  FAIL  {0}  ({1})" -f $name, $note) -ForegroundColor Red }
function Skip($name, $note) { $script:Results += [pscustomobject]@{ Result = 'SKIP'; Name = $name; Note = $note }; Write-Host ("  SKIP  {0}  ({1})" -f $name, $note) -ForegroundColor Yellow }
function Assert($name, $cond, $note) { if ($cond) { Pass $name $note } else { Fail $name $note } }
function Get-FreePort { $l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0); $l.Start(); $p = $l.LocalEndpoint.Port; $l.Stop(); $p }

Write-Host '== Relux proposed-change apply smoke ==' -ForegroundColor Cyan

# -- 1) End-to-end model test (the fake-CLI-envelope -> apply-to-temp test) ----
Write-Host '-- end-to-end model test --' -ForegroundColor Cyan
$tests = @(
    'cli_run_captures_proposed_changes_and_apply_writes_end_to_end',
    'review_then_apply_writes_the_file',
    'apply_to_workspace_refuses_on_baseline_conflict_and_leaves_file',
    'apply_refuses_without_a_baseline_hash',
    'apply_refuses_without_a_workspace_root',
    # Transactional (multi-file) apply — the "two proposed changes -> apply set to
    # a temp workspace, all-or-nothing" slice (RELUX_MASTER_PLAN §15).
    'cli_run_captures_two_proposed_changes_and_set_apply_writes_both_end_to_end',
    'change_set_applies_multiple_files_atomically',
    'change_set_partial_conflict_leaves_all_files_untouched',
    'change_set_refuses_duplicate_target_paths',
    # Create action — new-file create within the same transaction safety model
    # (RELUX_MASTER_PLAN §15): create writes a new file (with parent dirs), an
    # existing target is a conflict, and a mixed create+replace set applies/rolls
    # back atomically. Includes a fake-CLI envelope with ONE create + ONE replace.
    'create_to_workspace_writes_new_file_and_makes_parent_dirs',
    'create_to_workspace_refuses_existing_file_as_conflict',
    'review_then_apply_create_writes_a_new_file',
    'apply_create_over_existing_file_refuses_as_conflict_and_leaves_it',
    'change_set_mixes_create_and_replace_atomically',
    'change_set_create_conflict_leaves_everything_untouched',
    'change_set_rolls_back_a_created_file_on_a_later_write_failure',
    'cli_run_captures_one_create_and_one_replace_and_set_apply_writes_both_end_to_end',
    # Rename/move action — moves an existing baseline file to a new destination
    # within the same transaction safety model (RELUX_MASTER_PLAN §15): the source
    # must match its baseline, the destination must not exist, and a mixed
    # rename+replace+create set applies/rolls back atomically. Includes a fake-CLI
    # envelope with ONE rename.
    'rename_to_workspace_moves_file_when_baseline_matches',
    'rename_to_workspace_refuses_when_dest_exists_as_conflict',
    'rename_to_workspace_refuses_on_baseline_conflict_and_leaves_source',
    'review_then_apply_rename_moves_the_file',
    'apply_rename_refuses_without_a_baseline_hash',
    'change_set_mixes_rename_replace_and_create_atomically',
    'change_set_rename_dest_conflict_leaves_everything_untouched',
    'change_set_refuses_overlapping_rename_and_create_targets',
    'change_set_rolls_back_a_rename_on_a_later_write_failure',
    'cli_run_captures_a_rename_and_apply_moves_the_file_end_to_end'
)
foreach ($t in $tests) {
    $out = & cargo test -p relux-kernel --lib $t 2>$null | Out-String
    $ok = ($LASTEXITCODE -eq 0) -and ($out -match "test result: ok")
    Assert ("kernel test: $t") $ok 'fake CLI envelope -> capture -> approve -> apply'
}

# Core capture + safety unit tests (path/baseline/size/content validation).
$coreOut = & cargo test -p relux-core proposed_change 2>$null | Out-String
Assert 'core proposed_change capture/safety tests' (($LASTEXITCODE -eq 0) -and ($coreOut -match 'test result: ok')) 'sanitize/baseline/size/content'

# -- 2) HTTP wiring: routes exist and refuse honestly -------------------------
$serveProc = $null
$oldDb = $env:RELUX_DB
$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("relux-pc-smoke-" + [System.Guid]::NewGuid().ToString('N').Substring(0, 8))
try {
    if ($SkipServe) {
        Write-Host '-- HTTP wiring --' -ForegroundColor Cyan
        Skip 'http review/apply route checks' '-SkipServe'
    }
    else {
        Write-Host '-- HTTP wiring --' -ForegroundColor Cyan
        $exe = Join-Path $RepoRoot 'target\release\relux-kernel.exe'
        if (-not $SkipBuild) {
            & cargo build --release -p relux-kernel 2>$null | Out-Null
        }
        if (-not (Test-Path $exe)) {
            Skip 'http review/apply route checks' 'release binary missing (build failed or -SkipBuild without a prior build)'
        }
        else {
            New-Item -ItemType Directory -Force -Path $TempRoot | Out-Null
            $env:RELUX_DB = Join-Path $TempRoot 'local.db'
            $port = Get-FreePort
            $env:RELUX_HTTP_ADDR = "127.0.0.1:$port"
            $base = "http://127.0.0.1:$port"
            $serveOut = Join-Path $TempRoot 'serve.out.log'
            $serveErr = Join-Path $TempRoot 'serve.err.log'
            $serveProc = Start-Process -FilePath $exe -ArgumentList 'serve' -PassThru -WindowStyle Hidden -RedirectStandardOutput $serveOut -RedirectStandardError $serveErr

            $ready = $false
            for ($i = 0; $i -lt 50; $i++) {
                if ($serveProc.HasExited) { break }
                try { $null = Invoke-WebRequest -UseBasicParsing -Uri "$base/v1/relux/state" -TimeoutSec 2; $ready = $true; break } catch { Start-Sleep -Milliseconds 200 }
            }
            Assert 'serve becomes ready' $ready "$base/v1/relux/state"

            if ($ready) {
                # Apply on an unknown run id must refuse honestly — the handler is
                # reached and returns the kernel's "unknown run" 400 (a 2xx here
                # would be a fabricated success; a missing route would 404). So a
                # 400 both proves the route is wired AND that it refused honestly.
                $status = 0
                try { $null = Invoke-WebRequest -UseBasicParsing -Method POST -Uri "$base/v1/relux/runs/run_nope/proposed-changes/0/apply" -TimeoutSec 5 }
                catch { $status = [int]$_.Exception.Response.StatusCode }
                Assert 'apply on unknown run refuses honestly (400)' ($status -eq 400) "got $status"

                # Review on an unknown run id likewise refuses honestly (400).
                $status2 = 0
                try { $null = Invoke-WebRequest -UseBasicParsing -Method POST -Uri "$base/v1/relux/runs/run_nope/proposed-changes/0/review" -Body '{"decision":"approve"}' -ContentType 'application/json' -TimeoutSec 5 }
                catch { $status2 = [int]$_.Exception.Response.StatusCode }
                Assert 'review on unknown run refuses honestly (400)' ($status2 -eq 400) "got $status2"

                # A bad decision is a 400, validated before any state change.
                $status3 = 0
                try { $null = Invoke-WebRequest -UseBasicParsing -Method POST -Uri "$base/v1/relux/runs/run_nope/proposed-changes/0/review" -Body '{"decision":"maybe"}' -ContentType 'application/json' -TimeoutSec 5 }
                catch { $status3 = [int]$_.Exception.Response.StatusCode }
                Assert 'review with bad decision -> 400' ($status3 -eq 400) "got $status3"

                # The transactional (multi-file) apply route exists and refuses
                # honestly: a VALID body on an unknown run reaches the kernel and
                # returns its "unknown run" 400 (a 2xx would be a fabricated apply;
                # a missing route would 404). Proves the new set route is wired.
                $status4 = 0
                try { $null = Invoke-WebRequest -UseBasicParsing -Method POST -Uri "$base/v1/relux/runs/run_nope/proposed-changes/apply" -Body '{"indices":[0,1]}' -ContentType 'application/json' -TimeoutSec 5 }
                catch { $status4 = [int]$_.Exception.Response.StatusCode }
                Assert 'set-apply on unknown run refuses honestly (400)' ($status4 -eq 400) "got $status4"

                # An empty selection is rejected up front (400) — never a no-op 2xx.
                $status5 = 0
                try { $null = Invoke-WebRequest -UseBasicParsing -Method POST -Uri "$base/v1/relux/runs/run_nope/proposed-changes/apply" -Body '{"indices":[]}' -ContentType 'application/json' -TimeoutSec 5 }
                catch { $status5 = [int]$_.Exception.Response.StatusCode }
                Assert 'set-apply with empty selection -> 400' ($status5 -eq 400) "got $status5"
            }
        }
    }
}
finally {
    if ($serveProc -and -not $serveProc.HasExited) { try { Stop-Process -Id $serveProc.Id -Force -ErrorAction SilentlyContinue } catch {} }
    if ($oldDb) { $env:RELUX_DB = $oldDb } else { Remove-Item Env:RELUX_DB -ErrorAction SilentlyContinue }
    Remove-Item Env:RELUX_HTTP_ADDR -ErrorAction SilentlyContinue
    if ((-not $KeepTemp) -and (Test-Path $TempRoot)) { Remove-Item -Recurse -Force $TempRoot -ErrorAction SilentlyContinue }
    elseif ($KeepTemp) { Write-Host ("  temp kept: {0}" -f $TempRoot) -ForegroundColor DarkGray }
}

# -- Summary ------------------------------------------------------------------
Write-Host ''
Write-Host '== Summary ==' -ForegroundColor Cyan
$script:Results | Format-Table -AutoSize Result, Name, Note | Out-String | Write-Host
$failed = @($script:Results | Where-Object { $_.Result -eq 'FAIL' }).Count
if ($failed -gt 0) { Write-Host ("$failed check(s) FAILED") -ForegroundColor Red; exit 1 }
Write-Host 'All proposed-change apply checks passed.' -ForegroundColor Green
exit 0
