# scripts/check-launcher-preflight.ps1
#
# Validate the port preflight baked into the generated bundle launcher
# (Start-Relux.ps1). dist/ is gitignored and not committed, so this checks the
# SOURCE of truth - the here-string inside scripts/relux-package-local.ps1 - by
# extracting that launcher and exercising it for real:
#
#   1. Pull the Start-Relux.ps1 body out of the packaging script.
#   2. Static-check it still contains the preflight markers.
#   3. Behavioral check: hold a loopback port, run the launcher with -Port set to
#      that busy port, and assert it exits non-zero WITHOUT starting the kernel,
#      printing the actionable "already in use / -Port" message + dashboard URL.
#   4. Behavioral control: with a known-FREE port the launcher must get PAST the
#      preflight (it then fails on the dummy exe, proving the preflight passed).
#
# Robust on Windows PowerShell 5.1; never leaves a process or temp dir behind.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-launcher-preflight.ps1

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$Pkg = Join-Path $PSScriptRoot "relux-package-local.ps1"

$Failures = 0
function Write-Step {
    param([string]$Name, [bool]$Ok, [string]$Detail = "")
    $tag = if ($Ok) { "PASS" } else { "FAIL" }
    $color = if ($Ok) { "Green" } else { "Red" }
    Write-Host ("  {0,-4} {1,-34} {2}" -f $tag, $Name, $Detail) -ForegroundColor $color
    if (-not $Ok) { $script:Failures += 1 }
}

Write-Host ""
Write-Host "== Relux bundle launcher preflight check ==" -ForegroundColor Cyan
Write-Host ("  source: {0}" -f $Pkg)
Write-Host ""

# -- 1) extract the launcher here-string from the packaging script ----------
$pkgText = Get-Content -LiteralPath $Pkg -Raw
$launcherPattern = '(?s)@''\r?\n(.*?)\r?\n''@\s*\|\s*Set-Content -LiteralPath \(Join-Path \$BundleRoot "Start-Relux\.ps1"\)'
$m = [regex]::Match($pkgText, $launcherPattern)
Write-Step "extract launcher body" $m.Success "matched the Start-Relux.ps1 here-string"
if (-not $m.Success) {
    Write-Host ""
    Write-Host "RESULT: FAIL (could not locate the launcher here-string)" -ForegroundColor Red
    exit 1
}
$launcher = $m.Groups[1].Value

# -- 2) static markers ------------------------------------------------------
$hasFn      = $launcher -match "Test-PortListening"
$hasInUse   = $launcher -match "already in use"
$hasPortArg = $launcher -match "Start-Relux\.ps1 -Port"
$hasDash    = $launcher -match "/dashboard"
Write-Step "preflight function present" $hasFn      "Test-PortListening"
Write-Step "actionable in-use message"  $hasInUse   "'already in use'"
Write-Step "alt -Port command shown"    $hasPortArg "Start-Relux.ps1 -Port <free>"
Write-Step "dashboard URL mentioned"    $hasDash    "/dashboard"

# -- behavioral harness: materialize the launcher into a temp bundle --------
$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("relux-launcher-check-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $TempRoot -Force | Out-Null
$listener = $null
try {
    $launcherPath = Join-Path $TempRoot "Start-Relux.ps1"
    Set-Content -LiteralPath $launcherPath -Value $launcher -Encoding UTF8
    # The launcher checks for relux-kernel.exe before the preflight; a dummy file
    # satisfies that Test-Path. It is never executed: the busy-port case exits at
    # the preflight, and the free-port case fails trying to run this non-exe.
    Set-Content -LiteralPath (Join-Path $TempRoot "relux-kernel.exe") -Value "" -Encoding ASCII

    # -- 3) busy-port case: hold a loopback port, then launch against it ------
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $busyPort = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port

    $out = & powershell -NoProfile -ExecutionPolicy Bypass -File $launcherPath -Port $busyPort 2>&1
    $code = $LASTEXITCODE
    $text = ($out | Out-String)

    $exitedNonZero = ($code -ne 0)
    $saysInUse     = ($text -match "already in use")
    $offersAlt     = ($text -match "Start-Relux\.ps1 -Port")
    $showsDash     = ($text -match "http://127\.0\.0\.1:$busyPort/dashboard")
    Write-Step "busy port -> non-zero exit" $exitedNonZero ("exit code {0}" -f $code)
    Write-Step "busy port -> 'in use' msg"  $saysInUse     "printed the actionable in-use line"
    Write-Step "busy port -> alt -Port cmd" $offersAlt     "printed Start-Relux.ps1 -Port <free>"
    Write-Step "busy port -> dashboard URL" $showsDash     "pointed at the running instance"

    $listener.Stop(); $listener = $null

    # -- 4) free-port control: must get PAST the preflight -------------------
    # Pick a port that is now free (the listener above is stopped). The launcher
    # should clear the preflight and then fail invoking the dummy non-exe - which
    # proves the preflight did NOT block a free port.
    $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $probe.Start()
    $freePort = ([System.Net.IPEndPoint]$probe.LocalEndpoint).Port
    $probe.Stop()

    # The launcher clears the preflight then fails invoking the dummy non-exe,
    # which surfaces as a NativeCommandError on stderr. Relax Stop locally so that
    # expected failure does not abort this check; we only care about the output.
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $out2 = & powershell -NoProfile -ExecutionPolicy Bypass -File $launcherPath -Port $freePort 2>&1
    } finally {
        $ErrorActionPreference = $savedEap
    }
    $text2 = ($out2 | Out-String)
    $passedPreflight = ($text2 -notmatch "already in use")
    Write-Step "free port -> preflight passes" $passedPreflight "no false 'in use' on a free port"
} finally {
    if ($listener) { try { $listener.Stop() } catch {} }
    Remove-Item -LiteralPath $TempRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ""
if ($Failures -eq 0) {
    Write-Host "RESULT: PASS" -ForegroundColor Green
    exit 0
}
Write-Host ("RESULT: FAIL ({0} failing check(s))" -f $Failures) -ForegroundColor Red
exit 1
