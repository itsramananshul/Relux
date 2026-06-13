param(
    [switch]$SkipSmoke,
    [switch]$FullE2E,
    [switch]$KeepTemp
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$ReleaseExe = Join-Path $Root "target\release\relux-kernel.exe"
$Failures = 0

# Windows-local build-parallelism cap (-j N) for the heavy cargo gates below.
# relux-kernel pulls reqwest/rustls/axum, so a cold build/test/clippy is a big
# link storm that can hit the commit-limit OOM (LNK1102) and emit bogus
# rlib/metadata errors. See scripts/cargo-jobs.ps1; override with
# $env:RELUX_CARGO_JOBS (0 = no cap).
. (Join-Path $PSScriptRoot "cargo-jobs.ps1")
$JobsArgs = Get-CargoJobsArgs

function Write-Step {
    param([string]$Name, [string]$Status, [string]$Detail = "")
    $color = if ($Status -eq "PASS") { "Green" } elseif ($Status -eq "SKIP") { "Yellow" } else { "Red" }
    Write-Host ("{0,-4} {1,-34} {2}" -f $Status, $Name, $Detail) -ForegroundColor $color
    if ($Status -eq "FAIL") {
        $script:Failures += 1
    }
}

function Invoke-NativeStep {
    param(
        [string]$Name,
        [string]$Exe,
        [string[]]$Arguments,
        [string]$WorkingDirectory = $Root
    )
    Write-Host ""
    Write-Host (">> {0}" -f $Name) -ForegroundColor DarkCyan
    Push-Location $WorkingDirectory
    try {
        & $Exe @Arguments
        $code = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
    } finally {
        Pop-Location
    }
    if ($code -eq 0) {
        Write-Step $Name "PASS"
        return $true
    }
    Write-Step $Name "FAIL" "exit code $code"
    return $false
}

function Assert-Command {
    param([string]$Name, [string]$Command)
    $cmd = Get-Command $Command -ErrorAction SilentlyContinue
    if ($cmd) {
        Write-Step $Name "PASS" $cmd.Source
        return $cmd.Source
    }
    Write-Step $Name "FAIL" "$Command not found on PATH"
    return $null
}

Write-Host "== Relux First Local Release Check ==" -ForegroundColor Cyan
Write-Host ("workspace: {0}" -f $Root)

$Cargo = Assert-Command "cargo available" "cargo"
$Npm = Assert-Command "npm available" "npm"

if ($Cargo) {
    Invoke-NativeStep -Name "cargo test core/kernel" -Exe $Cargo -Arguments (@("test", "-p", "relux-core", "-p", "relux-kernel") + $JobsArgs)
    Invoke-NativeStep -Name "cargo clippy core/kernel" -Exe $Cargo -Arguments (@("clippy", "-p", "relux-core", "-p", "relux-kernel", "--all-targets") + $JobsArgs + @("--", "-D", "warnings"))
    Invoke-NativeStep -Name "kernel release build" -Exe $Cargo -Arguments (@("build", "-p", "relux-kernel", "--release") + $JobsArgs)
}

if ($Npm) {
    Invoke-NativeStep -Name "dashboard build" -Exe $Npm -Arguments @("run", "build") -WorkingDirectory (Join-Path $Root "apps\dashboard")
}

# The bundle launcher's port preflight must hold the actionable behavior (busy
# port -> non-zero exit + dashboard URL + alt -Port command). This validates the
# SOURCE here-string in relux-package-local.ps1, so a regression is caught before
# a package is cut. No build needed.
$launcherCheck = Join-Path $PSScriptRoot "check-launcher-preflight.ps1"
[void](Invoke-NativeStep -Name "bundle launcher preflight" -Exe "powershell" `
    -Arguments @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $launcherCheck))

# The kernel `serve` bind-failure message (Rust) and the launcher preflight
# message (PowerShell) are two independent surfaces for the same port-busy
# guidance (RELUX_MASTER_PLAN Sec 22). This static cross-source check pins their
# wording parity so they cannot drift - both name "already in use", point at
# /dashboard, and show `Start-Relux.ps1 -Port`, and neither promises auto-pick.
$portGuidanceCheck = Join-Path $PSScriptRoot "check-port-guidance.ps1"
[void](Invoke-NativeStep -Name "port guidance contract" -Exe "powershell" `
    -Arguments @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $portGuidanceCheck))

# The release smokes must prove HONEST local-vs-real-adapter behavior: local
# Prime fails closed on free-form external work (never a hang, never a fake
# "done"), runs only what it CAN fulfil, and defers real agent work to a
# configured Claude/Codex adapter (RELUX_MASTER_PLAN Sec 8.1). This static check
# pins that contract so the smokes cannot drift back to faking external work to
# turn the gate green. No build/process needed.
$smokeBoundaryCheck = Join-Path $PSScriptRoot "check-smoke-adapter-boundary.ps1"
[void](Invoke-NativeStep -Name "smoke adapter boundary contract" -Exe "powershell" `
    -Arguments @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $smokeBoundaryCheck))

if (Test-Path -LiteralPath $ReleaseExe) {
    Invoke-NativeStep -Name "release doctor" -Exe $ReleaseExe -Arguments @("doctor")
} elseif ($Cargo) {
    Invoke-NativeStep -Name "doctor via cargo" -Exe $Cargo -Arguments (@("run", "-p", "relux-kernel") + $JobsArgs + @("--", "doctor"))
}

if ($SkipSmoke) {
    Write-Step "prime assigned-run smoke" "SKIP" "requested"
} elseif (Test-Path -LiteralPath $ReleaseExe) {
    $TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("relux-release-smoke-" + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $TempRoot | Out-Null
    $oldDb = $env:RELUX_DB
    $env:RELUX_DB = Join-Path $TempRoot "local.db"
    try {
        # Prime CAN create work locally: a free-form "inspect this repo" goal
        # becomes a queued, Prime-assigned task. This is the honest first half of
        # the local flow.
        Invoke-NativeStep -Name "prime creates task" -Exe $ReleaseExe -Arguments @("prime", "create a task to inspect this repo")

        # The local Prime adapter is DETERMINISTIC and does NO external work
        # (clone/filesystem/network/import). Running this free-form goal must
        # therefore FAIL CLOSED honestly: a non-zero exit + actionable guidance
        # naming the real-adapter remedy - NOT a silent hang and NOT a fabricated
        # "done" (RELUX_MASTER_PLAN Sec 8.1 "Local Prime is deterministic";
        # relux_core::is_unfulfillable_local_request / LocalAdapterUnsupported).
        # We assert the refusal, not a success. To run this as REAL agent work,
        # configure a Claude/Codex adapter - see scripts\relux-e2e-smoke.ps1
        # -RunRealClaudeAdapter / -RunRealCodexAdapter.
        Write-Host ""
        Write-Host ">> assigned run fails closed honestly" -ForegroundColor DarkCyan
        $eapOld = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $assignedOut = & $ReleaseExe task run-assigned task_0001 2>&1 | Out-String
        $assignedCode = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
        $stateOut = & $ReleaseExe state 2>&1 | Out-String
        $ErrorActionPreference = $eapOld
        $refusedHonestly = ($assignedCode -ne 0) `
            -and ($assignedOut -match 'cannot fulfil') `
            -and ($assignedOut -match 'no external work') `
            -and ($assignedOut -match 'Claude or Codex')
        if ($refusedHonestly) {
            Write-Step "assigned run fails closed honestly" "PASS" "local Prime refused free-form external work (exit $assignedCode)"
        } else {
            Write-Step "assigned run fails closed honestly" "FAIL" "expected non-zero exit + guidance, got exit $assignedCode"
        }
        # The refused task is parked Blocked (operator-actionable + reopenable once
        # a real adapter is assigned) - never stuck Running (a hang) and never
        # Completed (a fake success).
        $parkedBlocked = ($stateOut -match 'task_0001\s+\[Blocked\]')
        if ($parkedBlocked) {
            Write-Step "refused task parked Blocked" "PASS" "task_0001 Blocked (reopenable), not Running/Completed"
        } else {
            Write-Step "refused task parked Blocked" "FAIL" "task_0001 not parked Blocked after honest refusal"
        }
    } finally {
        if ($null -eq $oldDb) {
            Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue
        } else {
            $env:RELUX_DB = $oldDb
        }
        if ($KeepTemp) {
            Write-Host ("Temp smoke data kept at {0}" -f $TempRoot) -ForegroundColor Yellow
        } else {
            Remove-Item -LiteralPath $TempRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
} else {
    Write-Step "prime assigned-run smoke" "FAIL" "release exe missing"
}

# Full product end-to-end smoke (opt-in). The quick checks above keep the normal
# gate fast; pass -FullE2E to also run scripts\relux-e2e-smoke.ps1, which drives
# the release binary through doctor, Prime chat, the tool CLI, the HTTP loopback
# ToolSet runtime, adapter runtime controls, autonomy, and the `serve` HTTP
# endpoints against a throwaway RELUX_DB. Run this before cutting a release.
if ($FullE2E) {
    $e2e = Join-Path $PSScriptRoot "relux-e2e-smoke.ps1"
    $e2eArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $e2e, "-SkipBuild")
    if ($KeepTemp) { $e2eArgs += "-KeepTemp" }
    # Invoke-NativeStep records PASS/FAIL (and increments $Failures on non-zero exit).
    [void](Invoke-NativeStep -Name "full e2e smoke" -Exe "powershell" -Arguments $e2eArgs)

    # The manifestless-plugin -> Prime flow we hand-verified for v0.1.42/v0.1.43
    # (manifestless install with nested-root inference, Plugin Lens tools, Prime
    # using them with natural answers + no task + secret redaction) is now a durable
    # gate too, so a future release cannot silently regress it (docs/plugins.md
    # "Plugin Lens"/"Manifestless ZIP root inference"; RELUX_MASTER_PLAN §11.1). It
    # drives the SAME release binary + built dashboard against its own isolated DB /
    # port; HTTP/API only, no browser, no network. Reuses the dashboard-dist the
    # `dashboard build` step above produced.
    $mfSmoke = Join-Path $PSScriptRoot "smoke-manifestless-plugin-prime.ps1"
    $mfArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $mfSmoke)
    if ($KeepTemp) { $mfArgs += "-KeepTemp" }
    [void](Invoke-NativeStep -Name "manifestless plugin -> Prime smoke" -Exe "powershell" -Arguments $mfArgs)
} else {
    Write-Step "full e2e smoke" "SKIP" "pass -FullE2E to run scripts\relux-e2e-smoke.ps1"
    Write-Step "manifestless plugin -> Prime smoke" "SKIP" "pass -FullE2E to run scripts\smoke-manifestless-plugin-prime.ps1"
}

Write-Host ""
if ($Failures -eq 0) {
    Write-Host "RESULT: PASS" -ForegroundColor Green
    exit 0
}

Write-Host ("RESULT: FAIL ({0} failing step(s))" -f $Failures) -ForegroundColor Red
exit 1
