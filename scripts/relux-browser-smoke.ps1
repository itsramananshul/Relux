# scripts/relux-browser-smoke.ps1
#
# Live-browser CLICK smoke for the Relux dashboard (one command).
#
# The render harness (apps/dashboard/test/*-render.test.mjs) proves every route's
# FIRST PAINT, but a StaticRouter render never fires an effect or a click, so it
# cannot catch the regression the operator actually hit: a page that goes BLANK
# after clicking a nav link / View / Inspect / Send, a raw JSON envelope leaking
# into the Prime chat, or a 5xx behind a button. This wrapper closes that gap end
# to end: it boots the REAL release kernel (which serves /dashboard + the live
# /v1/relux/* control plane on ONE origin, exactly like production) against a
# THROWAWAY RELUX_DB, seeds one task so the Work board has something to inspect,
# and drives the operator's already-installed Chrome/Edge through the dashboard
# over the Chrome DevTools Protocol (apps/dashboard/scripts/browser-smoke.mjs).
#
# It adds NO npm dependency and commits NO browser binary — it reuses an engine
# that is already on the machine, which is the bar apps/dashboard/README.md set
# for a live-DOM smoke. It NEVER touches the operator's real dev store
# (dev-data/relux/local.db) or a real serve instance.
#
# Usage:
#   pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\relux-browser-smoke.ps1
#   ... -Rebuild      # rebuild target\release\relux-kernel.exe first
#   ... -Headful      # watch the browser run (debugging)
#   ... -KeepTemp     # keep the throwaway RELUX_DB for inspection
#
# Exits non-zero if the kernel fails to serve, the dashboard bundle is missing
# (run `npm run build` in apps/dashboard), or any browser check fails.

[CmdletBinding()]
param(
    [switch]$Rebuild,
    [switch]$Headful,
    [switch]$KeepTemp
)

$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$ReleaseExe = Join-Path $Root 'target\release\relux-kernel.exe'
$SmokeScript = Join-Path $Root 'apps\dashboard\scripts\browser-smoke.mjs'

. (Join-Path $PSScriptRoot 'cargo-jobs.ps1')
$JobsArgs = Get-CargoJobsArgs

Add-Type -AssemblyName System.Net.Http -ErrorAction SilentlyContinue

function Get-FreePort {
    $p = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $p.Start(); $port = $p.LocalEndpoint.Port; $p.Stop(); return $port
}

$TempRoot = $null
$oldDb = $env:RELUX_DB
$oldAddr = $env:RELUX_HTTP_ADDR
$serveProc = $null
$exitCode = 1

Write-Host ''
Write-Host '== Relux dashboard live-browser click smoke ==' -ForegroundColor Cyan
Write-Host ("workspace: {0}" -f $Root)

try {
    # -- 0) node + release binary -----------------------------------------
    $node = Get-Command node -ErrorAction SilentlyContinue
    if (-not $node) { throw 'node is not on PATH (needed to drive the browser over CDP).' }
    if (-not (Test-Path -LiteralPath $SmokeScript)) { throw "missing $SmokeScript" }

    if ($Rebuild -or -not (Test-Path -LiteralPath $ReleaseExe)) {
        Write-Host '  building target\release\relux-kernel.exe ...' -ForegroundColor DarkGray
        & cargo build -p relux-kernel --release @JobsArgs
        if ($LASTEXITCODE -ne 0) { throw 'cargo build -p relux-kernel --release failed' }
    }
    if (-not (Test-Path -LiteralPath $ReleaseExe)) { throw "release binary missing: $ReleaseExe" }
    Write-Host ("  release binary: {0}" -f $ReleaseExe) -ForegroundColor DarkGray

    # -- isolate: throwaway RELUX_DB --------------------------------------
    $TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('relux-browser-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
    New-Item -ItemType Directory -Path $TempRoot | Out-Null
    $env:RELUX_DB = Join-Path $TempRoot 'local.db'
    Write-Host ("  temp RELUX_DB: {0}" -f $env:RELUX_DB) -ForegroundColor DarkGray

    # -- 1) start the kernel serve on a free loopback port ----------------
    $srvPort = Get-FreePort
    $base = "http://127.0.0.1:$srvPort"
    $env:RELUX_HTTP_ADDR = "127.0.0.1:$srvPort"
    $serveOut = Join-Path $TempRoot 'serve.out.log'
    $serveErr = Join-Path $TempRoot 'serve.err.log'
    $serveProc = Start-Process -FilePath $ReleaseExe -ArgumentList 'serve' -PassThru -WindowStyle Hidden -RedirectStandardOutput $serveOut -RedirectStandardError $serveErr
    Write-Host ("  kernel serving at {0}" -f $base) -ForegroundColor DarkGray

    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.CookieContainer = New-Object System.Net.CookieContainer
    $handler.UseCookies = $true
    $http = New-Object System.Net.Http.HttpClient($handler)
    $http.Timeout = [TimeSpan]::FromSeconds(30)
    function Invoke-Api {
        param([string]$Method, [string]$Path, [string]$Json)
        try {
            if ($Method -eq 'GET') {
                $r = $http.GetAsync("$base$Path").GetAwaiter().GetResult()
            } else {
                $payload = '{}'; if ($Json) { $payload = $Json }
                $c = New-Object System.Net.Http.StringContent($payload, [System.Text.Encoding]::UTF8, 'application/json')
                $r = $http.PostAsync("$base$Path", $c).GetAwaiter().GetResult()
            }
            return [pscustomobject]@{ Status = [int]$r.StatusCode; Body = $r.Content.ReadAsStringAsync().GetAwaiter().GetResult() }
        } catch {
            return [pscustomobject]@{ Status = 0; Body = $_.Exception.Message }
        }
    }

    # Wait (bounded) for readiness on the PUBLIC health route.
    $deadline = (Get-Date).AddSeconds(30)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        if ($serveProc.HasExited) { break }
        if ((Invoke-Api 'GET' '/v1/relux/health' $null).Status -eq 200) { $ready = $true; break }
        Start-Sleep -Milliseconds 400
    }
    if (-not $ready) {
        if (Test-Path $serveErr) { Get-Content $serveErr -Tail 15 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray } }
        throw "kernel serve did not become ready at $base"
    }

    # -- 2) first-run setup mints the operator admin (+ session cookie) ---
    $user = 'browser-smoke'
    $pass = 'browser-smoke-pass-123'
    $setup = Invoke-Api 'POST' '/v1/auth/setup' (@{ username = $user; password = $pass } | ConvertTo-Json -Compress)
    if ($setup.Status -ne 200) { throw "auth setup failed (status $($setup.Status)): $($setup.Body)" }

    # -- 3) confirm the dashboard bundle is actually served ---------------
    $dash = Invoke-Api 'GET' '/dashboard' $null
    if ($dash.Status -eq 503) { throw 'dashboard bundle not built — run `npm run build` in apps/dashboard, then retry.' }
    if ($dash.Status -ne 200) { throw "GET /dashboard returned $($dash.Status)" }

    # -- 4) seed one task so the Work board has something to Inspect ------
    $seed = Invoke-Api 'POST' '/v1/relux/tasks' (@{ title = 'Browser smoke seed task — inspect this repo' } | ConvertTo-Json -Compress)
    if ($seed.Status -eq 200) {
        Write-Host '  seeded one task for the Work board' -ForegroundColor DarkGray
    } else {
        Write-Host ("  warning: could not seed a task (status {0}); Work will be tested in its empty state" -f $seed.Status) -ForegroundColor Yellow
    }
    $http.Dispose()

    # -- 4b) seed a throwaway MANIFESTLESS plugin fixture for the live import smoke
    # A folder with NO relux-plugin.json — exactly the common case (any GitHub repo /
    # local folder). The browser smoke drives Plugins → + Install → Local folder →
    # this HOST path → Install and asserts the metadata-only result card with working
    # next-action buttons (the import path must never dead-end). The path is read on
    # THIS host (the kernel process host), which is the same machine driving Chrome,
    # and lives under the throwaway $TempRoot that cleanup removes.
    $fixtureDir = Join-Path $TempRoot 'manifestless-plugin'
    New-Item -ItemType Directory -Path $fixtureDir | Out-Null
    Set-Content -LiteralPath (Join-Path $fixtureDir 'README.md') `
        -Value "# smoke-manifestless`nA throwaway source with no relux-plugin.json." -Encoding utf8
    Set-Content -LiteralPath (Join-Path $fixtureDir 'package.json') `
        -Value '{ "name": "smoke-manifestless", "version": "0.0.0", "bin": { "smoke": "index.js" } }' -Encoding utf8
    $env:RELUX_SMOKE_PLUGIN_DIR = $fixtureDir
    Write-Host ("  manifestless fixture: {0}" -f $fixtureDir) -ForegroundColor DarkGray

    # -- 5) drive the browser over CDP ------------------------------------
    Write-Host ''
    $env:RELUX_SMOKE_BASE = $base
    $env:RELUX_SMOKE_USER = $user
    $env:RELUX_SMOKE_PASS = $pass
    if ($Headful) { $env:RELUX_SMOKE_HEADFUL = '1' } else { Remove-Item Env:\RELUX_SMOKE_HEADFUL -ErrorAction SilentlyContinue }

    & node $SmokeScript
    $exitCode = $LASTEXITCODE
}
finally {
    Write-Host ''
    Write-Host '>> cleanup' -ForegroundColor DarkCyan
    if ($serveProc) {
        try { if (-not $serveProc.HasExited) { Stop-Process -Id $serveProc.Id -Force -ErrorAction SilentlyContinue } } catch {}
    }
    foreach ($v in 'RELUX_SMOKE_BASE', 'RELUX_SMOKE_USER', 'RELUX_SMOKE_PASS', 'RELUX_SMOKE_HEADFUL', 'RELUX_SMOKE_PLUGIN_DIR') {
        Remove-Item "Env:\$v" -ErrorAction SilentlyContinue
    }
    if ($null -eq $oldDb) { Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue } else { $env:RELUX_DB = $oldDb }
    if ($null -eq $oldAddr) { Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue } else { $env:RELUX_HTTP_ADDR = $oldAddr }
    if ($TempRoot -and (Test-Path -LiteralPath $TempRoot)) {
        if ($KeepTemp) { Write-Host ("  temp data kept at {0}" -f $TempRoot) -ForegroundColor Yellow }
        else { Remove-Item -LiteralPath $TempRoot -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

if ($exitCode -eq 0) {
    Write-Host ''
    Write-Host 'RESULT: PASS' -ForegroundColor Green
} else {
    Write-Host ''
    Write-Host ("RESULT: FAIL (browser smoke exit {0})" -f $exitCode) -ForegroundColor Red
}
exit $exitCode
