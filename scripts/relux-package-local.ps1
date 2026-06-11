# scripts/relux-package-local.ps1
#
# Build a portable, local-first Windows release bundle for Relux.
#
# Modes:
#   (default)    Run the QUICK first-release gate, then package.
#   -FullE2E     Run the FULL gate (quick checks PLUS the standalone end-to-end
#                smoke) via relux-first-release-check.ps1 -FullE2E, then package.
#   -SkipChecks  Skip the readiness gate entirely (still builds the release
#                binary if missing). Use only for a fast local repackage.
#
# The bundle is self-describing: VERSION.txt + RELEASE-NOTES.txt record the
# version, git commit, build timestamp, the check mode that produced it, and the
# supported core loops. The script never leaves a temp DB or process behind, and
# dist/ stays gitignored (never committed).
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1 -FullE2E
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1 -SkipChecks

param(
    [switch]$SkipChecks,
    [switch]$FullE2E,
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$KernelToml = Join-Path $Root "crates\relux-kernel\Cargo.toml"

# -- version + git metadata ------------------------------------------------
if (-not $Version) {
    $versionLine = Select-String -Path $KernelToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $Version = if ($versionLine -and $versionLine.Matches.Count -gt 0) {
        $versionLine.Matches[0].Groups[1].Value
    } else {
        (git -C $Root rev-parse --short HEAD).Trim()
    }
}

function Get-GitValue {
    param([string[]]$GitArgs, [string]$Fallback = "unknown")
    try {
        $out = (& git -C $Root @GitArgs 2>$null | Out-String).Trim()
        if ($LASTEXITCODE -eq 0 -and $out) { return $out }
    } catch {}
    return $Fallback
}

$CommitFull = Get-GitValue @("rev-parse", "HEAD")
$CommitShort = Get-GitValue @("rev-parse", "--short", "HEAD")
$Branch = Get-GitValue @("rev-parse", "--abbrev-ref", "HEAD")
$gitDirty = Get-GitValue @("status", "--porcelain") ""
$WorkingTree = if ($gitDirty) { "dirty (uncommitted local changes)" } else { "clean" }
$BuildTimestamp = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

# -- readiness gate --------------------------------------------------------
# CheckMode is recorded in the bundle metadata so a consumer knows exactly how
# verified the artifact is.
$CheckMode = "skipped"
if ($SkipChecks) {
    Write-Host "SKIP readiness check (-SkipChecks)" -ForegroundColor Yellow
    $CheckMode = "skipped"
} else {
    $checkScript = Join-Path $PSScriptRoot "relux-first-release-check.ps1"
    $checkArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $checkScript)
    if ($FullE2E) {
        $checkArgs += "-FullE2E"
        $CheckMode = "full-e2e"
        Write-Host "Running FULL first-release gate (-FullE2E: quick checks + end-to-end smoke)..." -ForegroundColor Cyan
    } else {
        $CheckMode = "quick"
        Write-Host "Running QUICK first-release gate..." -ForegroundColor Cyan
    }
    & powershell @checkArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Readiness check failed (mode: $CheckMode); package was not created."
    }
}

# -- release binary --------------------------------------------------------
$ReleaseExe = Join-Path $Root "target\release\relux-kernel.exe"
if (-not (Test-Path -LiteralPath $ReleaseExe)) {
    Write-Host "Release binary missing; building target\release\relux-kernel.exe ..." -ForegroundColor DarkGray
    & cargo build -p relux-kernel --release
    if ($LASTEXITCODE -ne 0) {
        throw "Release build failed."
    }
}

# -- assemble the bundle ---------------------------------------------------
$DistRoot = Join-Path $Root "dist"
$BundleName = "relux-local-$Version-windows-x64"
$BundleRoot = Join-Path $DistRoot $BundleName
$ZipPath = Join-Path $DistRoot "$BundleName.zip"

if (Test-Path -LiteralPath $BundleRoot) {
    Remove-Item -LiteralPath $BundleRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $BundleRoot -Force | Out-Null

Copy-Item -LiteralPath $ReleaseExe -Destination (Join-Path $BundleRoot "relux-kernel.exe")

$DashboardDist = Join-Path $Root "crates\relix-web-bridge\dashboard-dist"
if (-not (Test-Path -LiteralPath (Join-Path $DashboardDist "index.html"))) {
    throw "Dashboard dist is missing. Run npm run build in apps/dashboard."
}
Copy-Item -LiteralPath $DashboardDist -Destination (Join-Path $BundleRoot "dashboard-dist") -Recurse

$Examples = Join-Path $Root "examples\relux-plugins"
if (Test-Path -LiteralPath $Examples) {
    New-Item -ItemType Directory -Path (Join-Path $BundleRoot "examples") -Force | Out-Null
    Copy-Item -LiteralPath $Examples -Destination (Join-Path $BundleRoot "examples\relux-plugins") -Recurse
}

Copy-Item -LiteralPath (Join-Path $Root "README.md") -Destination (Join-Path $BundleRoot "README.md")
New-Item -ItemType Directory -Path (Join-Path $BundleRoot "docs") -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $Root "docs\RELUX_MASTER_PLAN.md") -Destination (Join-Path $BundleRoot "docs\RELUX_MASTER_PLAN.md")

# -- Start-Relux.ps1 (robust launcher) -------------------------------------
@'
# Start-Relux.ps1 - launch the local Relux control plane from this bundle.
#
# Boots relux-kernel.exe with a self-contained data dir + dashboard bundle and
# prints the dashboard URL. Override the port with -Port; data persists under
# .\data\local.db inside this bundle.

param(
    [int]$Port = 19891
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot

$Exe = Join-Path $Root "relux-kernel.exe"
if (-not (Test-Path -LiteralPath $Exe)) {
    Write-Host "ERROR: relux-kernel.exe was not found next to this script." -ForegroundColor Red
    Write-Host ("Expected at: {0}" -f $Exe) -ForegroundColor Red
    Write-Host "This bundle looks incomplete. Re-extract the release zip and run Start-Relux.ps1 from inside it." -ForegroundColor Yellow
    exit 1
}

# -- port preflight --------------------------------------------------------
# A second Relux (or any other process) already holding the loopback port is the
# most common first-run failure. Without this check the kernel would still fail
# to bind and exit, but only AFTER this script printed a dashboard URL that
# actually points at the OTHER process - a misleading "it started" message.
# Probe 127.0.0.1:$Port up front and stop with an actionable message instead.
function Test-PortListening {
    param([int]$ProbePort)
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $async = $client.BeginConnect("127.0.0.1", $ProbePort, $null, $null)
        if (-not $async.AsyncWaitHandle.WaitOne(400)) { return $false }
        try { $client.EndConnect($async) } catch { return $false }
        return $client.Connected
    } catch {
        return $false
    } finally {
        $client.Close()
    }
}

if (Test-PortListening -ProbePort $Port) {
    $alt = if ($Port -eq 20000) { 20001 } else { 20000 }
    Write-Host ""
    Write-Host ("ERROR: port {0} on 127.0.0.1 is already in use; Relux did not start." -f $Port) -ForegroundColor Red
    Write-Host "The most likely cause is that Relux is already running on this port." -ForegroundColor Yellow
    Write-Host ("  If so, open the running instance: http://127.0.0.1:{0}/dashboard" -f $Port) -ForegroundColor Cyan
    Write-Host "Otherwise another program holds that port. Start Relux on a free port, e.g.:" -ForegroundColor Yellow
    Write-Host ("  powershell -NoProfile -ExecutionPolicy Bypass -File .\Start-Relux.ps1 -Port {0}" -f $alt) -ForegroundColor Green
    Write-Host ""
    exit 1
}

$DashboardDist = Join-Path $Root "dashboard-dist"
$DataDir = Join-Path $Root "data"
New-Item -ItemType Directory -Path $DataDir -Force | Out-Null

$env:RELUX_HTTP_ADDR = "127.0.0.1:$Port"
$env:RELUX_DB = Join-Path $DataDir "local.db"
$env:RELUX_DASHBOARD_DIST = $DashboardDist

if (-not (Test-Path -LiteralPath (Join-Path $DashboardDist "index.html"))) {
    Write-Host "WARNING: dashboard bundle not found at .\dashboard-dist; the dashboard route will return an honest 'not built' notice." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "Starting Relux ..." -ForegroundColor Cyan
Write-Host ("  Dashboard: http://127.0.0.1:{0}/dashboard" -f $Port) -ForegroundColor Green
Write-Host ("  API:       http://127.0.0.1:{0}/v1/relux/state" -f $Port)
Write-Host ("  Data:      {0}" -f $env:RELUX_DB) -ForegroundColor DarkGray
Write-Host "  Press Ctrl+C to stop." -ForegroundColor DarkGray
Write-Host ""

& $Exe serve
'@ | Set-Content -LiteralPath (Join-Path $BundleRoot "Start-Relux.ps1") -Encoding UTF8

# -- VERSION.txt (machine-friendly metadata) -------------------------------
@"
Relux local release
version=$Version
git_commit=$CommitFull
git_commit_short=$CommitShort
git_branch=$Branch
working_tree=$WorkingTree
build_timestamp_utc=$BuildTimestamp
check_mode=$CheckMode
platform=windows-x64
"@ | Set-Content -LiteralPath (Join-Path $BundleRoot "VERSION.txt") -Encoding UTF8

# -- RELEASE-NOTES.txt (human-friendly) ------------------------------------
$checkModeNote = switch ($CheckMode) {
    "full-e2e" { "FULL - quick gate plus the standalone end-to-end smoke (relux-first-release-check.ps1 -FullE2E)" }
    "quick"    { "QUICK - build dashboard, test + lint core/kernel, build release binary, doctor, Prime task smoke (relux-first-release-check.ps1)" }
    default    { "SKIPPED - packaged without the readiness gate (-SkipChecks); verify before sharing" }
}

@"
Relux - local first-release bundle
==================================

Version:        $Version
Git commit:     $CommitShort ($CommitFull)
Git branch:     $Branch
Working tree:   $WorkingTree
Built (UTC):    $BuildTimestamp
Verification:   $checkModeNote
Platform:       Windows x64

Start Relux
-----------
  powershell -NoProfile -ExecutionPolicy Bypass -File .\Start-Relux.ps1

Then open the dashboard:
  http://127.0.0.1:19891/dashboard

Use a different port:
  powershell -NoProfile -ExecutionPolicy Bypass -File .\Start-Relux.ps1 -Port 20000

Supported core loops (this release)
-----------------------------------
  - Prime chat               : talk to the local operator (deterministic; optional
                               OpenRouter-backed conversational replies).
  - Work / task run          : create tasks, assign them, and run assigned tasks
                               through the governed local adapter path.
  - Plugins                  : install/list ToolSet and Adapter plugins; bundled
                               plugins refresh idempotently on every load.
  - Loopback tool runtime    : point an installed ToolSet plugin at an operator-run
                               http://127.0.0.1:<port> server (Plugin Runtime v1).
  - Adapter runtime controls : enable/disable local coding-agent CLI adapters
                               (Claude CLI / Codex CLI / generic command). Disabled
                               by default; no CLI is ever spawned without opt-in.
  - Autonomy                 : the safe Prime autonomy loop runs ready assigned
                               tasks through the same governed path; disabled by
                               default.

What's in this bundle
---------------------
  - relux-kernel.exe         : the Relux control-plane binary.
  - dashboard-dist\          : the built dashboard served at /dashboard.
  - examples\relux-plugins\  : bundled example plugins/adapters.
  - docs\RELUX_MASTER_PLAN.md: the canonical product/design plan.
  - README.md                : full feature + CLI/API reference.
  - Start-Relux.ps1          : the launcher (sets RELUX_HTTP_ADDR / RELUX_DB /
                               RELUX_DASHBOARD_DIST and prints the dashboard URL).

Notes
-----
  - Local-first by design: the API binds loopback and is gated by a single-admin
    local operator login (set the admin password on first launch; recover with
    "relux-kernel.exe reset-admin"). It is not a multi-user or production surface,
    and the loopback transport has no TLS. Set RELUX_AUTH_DISABLED=1 only for a
    throwaway local dev box.
  - Data is stored under .\data\local.db inside this bundle.
  - Relux never auto-runs downloaded plugin code and never passes any CLI
    permission-bypass flag.
"@ | Set-Content -LiteralPath (Join-Path $BundleRoot "RELEASE-NOTES.txt") -Encoding UTF8

# -- zip the bundle --------------------------------------------------------
if (Test-Path -LiteralPath $ZipPath) {
    Remove-Item -LiteralPath $ZipPath -Force
}
if (Get-Command Compress-Archive -ErrorAction SilentlyContinue) {
    Compress-Archive -LiteralPath $BundleRoot -DestinationPath $ZipPath -Force
}

# -- report ----------------------------------------------------------------
$bundleSize = (Get-ChildItem -LiteralPath $BundleRoot -Recurse -Force | Measure-Object -Property Length -Sum).Sum
Write-Host ""
Write-Host ("Bundle:      {0}" -f $BundleRoot) -ForegroundColor Green
Write-Host ("Version:     {0} ({1}, {2})" -f $Version, $CommitShort, $WorkingTree)
Write-Host ("Check mode:  {0}" -f $CheckMode)
Write-Host ("Bundle size: {0:N2} MB" -f ($bundleSize / 1MB))
if (Test-Path -LiteralPath $ZipPath) {
    $zipSize = (Get-Item -LiteralPath $ZipPath).Length
    Write-Host ("Zip:         {0}" -f $ZipPath) -ForegroundColor Green
    Write-Host ("Zip size:    {0:N2} MB" -f ($zipSize / 1MB))
}
