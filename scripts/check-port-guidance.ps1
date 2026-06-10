# scripts/check-port-guidance.ps1
#
# Drift guard for the port-busy guidance CONTRACT (RELUX_MASTER_PLAN.md Sec 22,
# "Configuration" / RELUX_HTTP_ADDR). Two independent places tell an operator what
# to do when the loopback port is already taken, and they must not diverge:
#
#   1. Rust  - relux-kernel `serve` bind failure (bind_failure_message, AddrInUse
#              arm in crates/relux-kernel/src/server.rs). The source-checkout /
#              raw-binary path.
#   2. PS    - the generated bundle launcher's port preflight (the Start-Relux.ps1
#              here-string inside scripts/relux-package-local.ps1). The bundle path.
#
# PowerShell and Rust cannot share a string literal, so this script reads BOTH
# sources of truth and asserts the contract holds in each. The contract:
#
#   SHARED (must appear in BOTH messages):
#     - "already in use"            - names the conflict in the same words.
#     - "/dashboard"                - points at the running instance to check.
#     - "Start-Relux.ps1 -Port"     - the bundle's explicit alt-port command.
#   RUST-ONLY (the source-checkout override path):
#     - "RELUX_HTTP_ADDR"           - the env override `serve` documents.
#   NEITHER (negative - never auto-pick):
#     - no "automatic" / "auto-pick" / "for you" language. Relux NEVER silently
#       picks a free port; the operator always chooses one explicitly. If either
#       message starts promising auto-selection, that is a contract break.
#
# This is a static cross-source check (no build, no process). The companion
# scripts/check-launcher-preflight.ps1 behaviorally exercises the launcher; this
# one pins the wording parity between the two surfaces.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-port-guidance.ps1

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$ServerRs = Join-Path $Root "crates\relux-kernel\src\server.rs"
$Pkg = Join-Path $PSScriptRoot "relux-package-local.ps1"

$Failures = 0
function Write-Step {
    param([string]$Name, [bool]$Ok, [string]$Detail = "")
    $tag = if ($Ok) { "PASS" } else { "FAIL" }
    $color = if ($Ok) { "Green" } else { "Red" }
    Write-Host ("  {0,-4} {1,-38} {2}" -f $tag, $Name, $Detail) -ForegroundColor $color
    if (-not $Ok) { $script:Failures += 1 }
}

Write-Host ""
Write-Host "== Relux port-busy guidance contract check ==" -ForegroundColor Cyan
Write-Host ("  rust:     {0}" -f $ServerRs)
Write-Host ("  launcher: {0}" -f $Pkg)
Write-Host ""

# -- 1) extract the Rust AddrInUse message ---------------------------------
$rsText = Get-Content -LiteralPath $ServerRs -Raw
$rsMatch = [regex]::Match(
    $rsText,
    '(?s)ErrorKind::AddrInUse\s*=>\s*format!\((.*?)\),\s*\r?\n\s*ErrorKind::PermissionDenied')
Write-Step "extract rust AddrInUse message" $rsMatch.Success "bind_failure_message arm"
$rustMsg = if ($rsMatch.Success) { $rsMatch.Groups[1].Value } else { "" }

# -- 2) extract the launcher preflight message -----------------------------
$pkgText = Get-Content -LiteralPath $Pkg -Raw
$pfMatch = [regex]::Match(
    $pkgText,
    '(?s)if \(Test-PortListening -ProbePort \$Port\) \{(.*?)exit 1')
Write-Step "extract launcher preflight message" $pfMatch.Success "Start-Relux.ps1 here-string"
$launcherMsg = if ($pfMatch.Success) { $pfMatch.Groups[1].Value } else { "" }

if (-not $rsMatch.Success -or -not $pfMatch.Success) {
    Write-Host ""
    Write-Host "RESULT: FAIL (could not locate one or both messages)" -ForegroundColor Red
    exit 1
}

# -- 3) SHARED markers: present in BOTH ------------------------------------
$shared = @(
    @{ Label = "names conflict 'already in use'"; Pattern = "already in use" },
    @{ Label = "points at /dashboard";            Pattern = "/dashboard" },
    @{ Label = "shows Start-Relux.ps1 -Port";     Pattern = "Start-Relux\.ps1 -Port" }
)
foreach ($m in $shared) {
    $inRust = $rustMsg -match $m.Pattern
    $inLauncher = $launcherMsg -match $m.Pattern
    Write-Step ("shared: " + $m.Label) ($inRust -and $inLauncher) `
        ("rust={0} launcher={1}" -f $inRust, $inLauncher)
}

# -- 4) RUST-ONLY: the env override path -----------------------------------
Write-Step "rust documents RELUX_HTTP_ADDR" ($rustMsg -match "RELUX_HTTP_ADDR") `
    "source-checkout override"

# -- 5) NEGATIVE: never promise auto-pick ----------------------------------
$autoPattern = '(?i)automatic|auto-?pick|auto-?select|\bfor you\b'
$rustAuto = $rustMsg -match $autoPattern
$launcherAuto = $launcherMsg -match $autoPattern
Write-Step "rust: no auto-pick language"     (-not $rustAuto)     "explicit port only"
Write-Step "launcher: no auto-pick language" (-not $launcherAuto) "explicit port only"

Write-Host ""
if ($Failures -eq 0) {
    Write-Host "RESULT: PASS" -ForegroundColor Green
    exit 0
}
Write-Host ("RESULT: FAIL ({0} failing check(s))" -f $Failures) -ForegroundColor Red
exit 1
