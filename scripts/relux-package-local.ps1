param(
    [switch]$SkipChecks,
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$KernelToml = Join-Path $Root "crates\relux-kernel\Cargo.toml"

if (-not $Version) {
    $versionLine = Select-String -Path $KernelToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $Version = if ($versionLine -and $versionLine.Matches.Count -gt 0) {
        $versionLine.Matches[0].Groups[1].Value
    } else {
        (git -C $Root rev-parse --short HEAD).Trim()
    }
}

if (-not $SkipChecks) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "relux-first-release-check.ps1")
    if ($LASTEXITCODE -ne 0) {
        throw "Readiness check failed; package was not created."
    }
} else {
    Write-Host "SKIP readiness check (--SkipChecks)" -ForegroundColor Yellow
}

$ReleaseExe = Join-Path $Root "target\release\relux-kernel.exe"
if (-not (Test-Path -LiteralPath $ReleaseExe)) {
    & cargo build -p relux-kernel --release
    if ($LASTEXITCODE -ne 0) {
        throw "Release build failed."
    }
}

$DistRoot = Join-Path $Root "dist"
$BundleName = "relux-local-$Version-windows-x64"
$BundleRoot = Join-Path $DistRoot $BundleName
$ZipPath = Join-Path $DistRoot "$BundleName.zip"

if (Test-Path -LiteralPath $BundleRoot) {
    Remove-Item -LiteralPath $BundleRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $BundleRoot | Out-Null

Copy-Item -LiteralPath $ReleaseExe -Destination (Join-Path $BundleRoot "relux-kernel.exe")

$DashboardDist = Join-Path $Root "crates\relix-web-bridge\dashboard-dist"
if (-not (Test-Path -LiteralPath (Join-Path $DashboardDist "index.html"))) {
    throw "Dashboard dist is missing. Run npm run build in apps/dashboard."
}
Copy-Item -LiteralPath $DashboardDist -Destination (Join-Path $BundleRoot "dashboard-dist") -Recurse

$Examples = Join-Path $Root "examples\relux-plugins"
if (Test-Path -LiteralPath $Examples) {
    New-Item -ItemType Directory -Path (Join-Path $BundleRoot "examples") | Out-Null
    Copy-Item -LiteralPath $Examples -Destination (Join-Path $BundleRoot "examples\relux-plugins") -Recurse
}

Copy-Item -LiteralPath (Join-Path $Root "README.md") -Destination (Join-Path $BundleRoot "README.md")
New-Item -ItemType Directory -Path (Join-Path $BundleRoot "docs") | Out-Null
Copy-Item -LiteralPath (Join-Path $Root "docs\RELUX_MASTER_PLAN.md") -Destination (Join-Path $BundleRoot "docs\RELUX_MASTER_PLAN.md")

@'
param(
    [int]$Port = 19891
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot
$env:RELUX_HTTP_ADDR = "127.0.0.1:$Port"
$env:RELUX_DB = Join-Path $Root "data\local.db"
$env:RELUX_DASHBOARD_DIST = Join-Path $Root "dashboard-dist"
New-Item -ItemType Directory -Path (Join-Path $Root "data") -Force | Out-Null

Write-Host "Starting Relux on http://127.0.0.1:$Port/dashboard"
& (Join-Path $Root "relux-kernel.exe") serve
'@ | Set-Content -LiteralPath (Join-Path $BundleRoot "Start-Relux.ps1") -Encoding UTF8

@"
Relux local first-release bundle
================================

Start:
  powershell -NoProfile -ExecutionPolicy Bypass -File .\Start-Relux.ps1

Dashboard:
  http://127.0.0.1:19891/dashboard

Notes:
- Data is stored under .\data\local.db inside this bundle.
- The dashboard is served from .\dashboard-dist.
- Bundled example plugins live under .\examples\relux-plugins.
"@ | Set-Content -LiteralPath (Join-Path $BundleRoot "RELEASE-NOTES.txt") -Encoding UTF8

if (Test-Path -LiteralPath $ZipPath) {
    Remove-Item -LiteralPath $ZipPath -Force
}
if (Get-Command Compress-Archive -ErrorAction SilentlyContinue) {
    Compress-Archive -LiteralPath $BundleRoot -DestinationPath $ZipPath -Force
}

$bundleSize = (Get-ChildItem -LiteralPath $BundleRoot -Recurse -Force | Measure-Object -Property Length -Sum).Sum
Write-Host ("Bundle: {0}" -f $BundleRoot) -ForegroundColor Green
Write-Host ("Bundle size: {0:N2} MB" -f ($bundleSize / 1MB))
if (Test-Path -LiteralPath $ZipPath) {
    $zipSize = (Get-Item -LiteralPath $ZipPath).Length
    Write-Host ("Zip: {0}" -f $ZipPath) -ForegroundColor Green
    Write-Host ("Zip size: {0:N2} MB" -f ($zipSize / 1MB))
}
