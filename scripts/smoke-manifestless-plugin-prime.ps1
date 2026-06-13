# scripts/smoke-manifestless-plugin-prime.ps1
#
# Durable release smoke for the manifestless-plugin -> Prime flow.
#
# Proves the product promise we hand-verified for v0.1.42/v0.1.43 against a FRESH
# bundle, turned into a reusable gate so a future release cannot silently regress it
# (`docs/plugins.md` "Plugin Lens" + "Manifestless ZIP root inference";
# `docs/RELUX_MASTER_PLAN.md` §11.1 redaction parity, §10.5/§17.1 Prime tool use):
#
#   1. A REAL relux-kernel serve boots on an isolated loopback port + throwaway
#      RELUX_DB and local operator login works (first-run setup mints a session).
#   2. A tiny LOCAL manifestless plugin (no relux-plugin.json) whose content lives
#      under a single GitHub-style nested root installs through the real install-dir
#      route, lands as an honest metadata-only wrapper, and its generated id / name /
#      description are inferred from the NESTED repo root (v0.1.43 behavior) — not
#      from the artificial archive wrapper folder.
#   3. GET /v1/relux/prime/tools lists the four read-only Plugin Lens tools
#      (plugin.summary / inspect / search / read_file) for that plugin, sourced from
#      the plugin and directly runnable (not gated).
#   4. Prime chat (the DETERMINISTIC local brain — no LLM, no network) actually USES
#      those tools: a natural "summarize / search / read" message invokes the real
#      kernel-executed source tool, returns a natural answer (never a raw JSON
#      envelope), creates NO task, and the planted FAKE secret never leaks into the
#      reply or the structured / raw tool detail (redaction parity).
#   5. The dashboard SPA shell is served for /dashboard and the client routes
#      /dashboard/plugins, /dashboard/prime, /dashboard/work, /dashboard/crew
#      (history fallback to index.html), non-blank — or an honest SKIP when the
#      bundle is not built (503).
#
# No external services, no network, no GitHub clone, no real Claude/Codex, no bypass
# flags, NO REAL SECRETS (the fake credentials are assembled at runtime so no
# contiguous key-shaped literal lives in this file). Everything temporary (DB,
# plugins, uploads, the serve process, the fixture) is always cleaned up. Prints a
# concise PASS/FAIL/SKIP table and exits non-zero on any failure.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-manifestless-plugin-prime.ps1
#   ... -BundlePath C:\path\to\relux-local-0.1.x-windows-x64   # test an EXTRACTED release bundle
#   ... -KeepTemp                                              # keep the temp data for inspection
#
# Default (no -BundlePath): the in-repo build is used — target\release\relux-kernel.exe
# plus crates\relix-web-bridge\dashboard-dist. This is the form the FullE2E release
# gate (scripts\relux-first-release-check.ps1 -FullE2E) calls after the release binary
# and dashboard have been built.

[CmdletBinding()]
param(
    [string]$BundlePath = "",
    [switch]$KeepTemp
)

$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

# Resolve the binary + dashboard bundle: an extracted release bundle when -BundlePath
# is given, else the in-repo build outputs.
if ($BundlePath) {
    $BundlePath = (Resolve-Path $BundlePath).Path
    $ReleaseExe = Join-Path $BundlePath 'relux-kernel.exe'
    $DashboardDist = Join-Path $BundlePath 'dashboard-dist'
} else {
    $ReleaseExe = Join-Path $Root 'target\release\relux-kernel.exe'
    $DashboardDist = Join-Path $Root 'crates\relix-web-bridge\dashboard-dist'
}

# System.Net.Http is not auto-loaded in Windows PowerShell 5.1.
Add-Type -AssemblyName System.Net.Http -ErrorAction SilentlyContinue

# -- tiny PASS/FAIL/SKIP reporting harness ---------------------------------
$script:Results = New-Object System.Collections.ArrayList
function Record {
    param([string]$Name, [string]$Status, [string]$Detail = '')
    [void]$script:Results.Add([pscustomobject]@{ Name = $Name; Status = $Status; Detail = $Detail })
    $color = 'Red'
    if ($Status -eq 'PASS') { $color = 'Green' } elseif ($Status -eq 'SKIP') { $color = 'Yellow' }
    Write-Host ("  {0,-4} {1,-44} {2}" -f $Status, $Name, $Detail) -ForegroundColor $color
}
function Pass([string]$n, [string]$d = '') { Record $n 'PASS' $d }
function Fail([string]$n, [string]$d = '') { Record $n 'FAIL' $d }
function Skip([string]$n, [string]$d = '') { Record $n 'SKIP' $d }
function Assert([string]$n, [bool]$ok, [string]$d = '') { if ($ok) { Pass $n $d } else { Fail $n $d } }
function Section([string]$t) { Write-Host ''; Write-Host ">> $t" -ForegroundColor DarkCyan }

function Get-FreePort {
    $p = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $p.Start(); $port = $p.LocalEndpoint.Port; $p.Stop(); return $port
}

# -- state captured for the finally cleanup --------------------------------
$TempRoot = $null
$oldDb = $env:RELUX_DB
$oldAddr = $env:RELUX_HTTP_ADDR
$oldDash = $env:RELUX_DASHBOARD_DIST
$serveProc = $null
$client = $null

Write-Host ''
Write-Host '== Relux manifestless-plugin -> Prime release smoke ==' -ForegroundColor Cyan
Write-Host ("workspace: {0}" -f $Root)
Write-Host ("binary:    {0}" -f $ReleaseExe)
Write-Host ("dashboard: {0}" -f $DashboardDist)

try {
    Section 'Release binary'
    if (-not (Test-Path -LiteralPath $ReleaseExe)) {
        Fail 'release binary present' $ReleaseExe
        throw "release binary missing: $ReleaseExe (build it first, or pass -BundlePath)"
    }
    Pass 'release binary present' $ReleaseExe

    # -- isolate: throwaway RELUX_DB (plugins/uploads land next to it) ------
    $TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('relux-mf-smoke-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
    New-Item -ItemType Directory -Path $TempRoot | Out-Null
    $env:RELUX_DB = Join-Path $TempRoot 'local.db'
    $env:RELUX_DASHBOARD_DIST = $DashboardDist
    Write-Host ("  temp RELUX_DB: {0}" -f $env:RELUX_DB) -ForegroundColor DarkGray

    # -- build the manifestless fixture (single nested GitHub-style root) ---
    # Layout (the "extracted archive" we hand to install-dir):
    #   <extractRoot>/                       <- only one entry: the wrapper dir
    #     widgetron-main/                    <- the single wrapper dir (GitHub `repo-branch/`)
    #       README.md                        <- makes it "look like a source root"
    #       config.env                       <- holds the FAKE secret (redaction target)
    #       src/app.py                       <- holds the searchable MARKER
    # The wrapper folder name is distinct from the extract dir name, so an id derived
    # from the NESTED root ("relux-plugin-widgetron-main") proves repo-root inference.
    Section 'Manifestless fixture'
    $extractRoot = Join-Path $TempRoot ('mf-archive-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $repoDir = Join-Path $extractRoot 'widgetron-main'
    New-Item -ItemType Directory -Path (Join-Path $repoDir 'src') -Force | Out-Null

    # A unique, NON-secret marker so plugin.search has a deterministic hit in a source file.
    $marker = 'RELUX_SMOKE_MARKER_' + [guid]::NewGuid().ToString('N').Substring(0, 8).ToUpper()
    # FAKE credentials assembled at runtime (no contiguous key-shaped literal in this file).
    # First is masked by its known prefix; second by its secret-named key.
    $fakeSecretPrefix = 'sk-ant-' + '00smoke00fixture00not00real00'
    $fakeSecretOpaque = 'Zq' + '83smokeFixtureOpaqueTokenNotReal'

    # Write WITHOUT a BOM (Set-Content -Encoding UTF8 prepends one, which would leak
    # into the README excerpt the Plugin Lens reads). Plain LF text the kernel reads
    # byte-for-byte.
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    function Write-Fixture([string]$RelPath, [string]$Text) {
        [System.IO.File]::WriteAllText((Join-Path $repoDir $RelPath), $Text, $utf8NoBom)
    }
    Write-Fixture 'README.md' "# Widgetron`n`nMakes widgets from briefs. A tiny manifestless fixture for the Relux release smoke.`n"
    Write-Fixture 'config.env' "# fixture config - FAKE credentials, never real, never committed`nOPENAI_API_KEY=$fakeSecretPrefix`nSERVICE_TOKEN=$fakeSecretOpaque`n"
    Write-Fixture 'src\app.py' "# widgetron entrypoint`nMARKER = `"$marker`"`n`n`ndef main():`n    print(`"widgetron`", MARKER)`n"
    Pass 'fixture written' ("nested root widgetron-main, marker " + $marker)

    # -- start the kernel HTTP server on a free loopback port --------------
    Section 'HTTP serve + login'
    $srvPort = Get-FreePort
    $base = "http://127.0.0.1:$srvPort"
    $env:RELUX_HTTP_ADDR = "127.0.0.1:$srvPort"
    $serveOut = Join-Path $TempRoot 'serve.out.log'
    $serveErr = Join-Path $TempRoot 'serve.err.log'
    $serveProc = Start-Process -FilePath $ReleaseExe -ArgumentList 'serve' -PassThru -WindowStyle Hidden -RedirectStandardOutput $serveOut -RedirectStandardError $serveErr

    # A CookieContainer captures the relux_session cookie minted by /v1/auth/setup and
    # re-sends it on every later request - exactly what the browser dashboard does.
    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.CookieContainer = New-Object System.Net.CookieContainer
    $handler.UseCookies = $true
    $client = New-Object System.Net.Http.HttpClient($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(60)
    function Invoke-Api {
        param([string]$Method, [string]$Path, [string]$Json)
        try {
            if ($Method -eq 'GET') {
                $r = $client.GetAsync("$base$Path").GetAwaiter().GetResult()
            } else {
                $payload = '{}'; if ($Json) { $payload = $Json }
                $c = New-Object System.Net.Http.StringContent($payload, [System.Text.Encoding]::UTF8, 'application/json')
                $r = $client.PostAsync("$base$Path", $c).GetAwaiter().GetResult()
            }
            return [pscustomobject]@{ Status = [int]$r.StatusCode; Body = $r.Content.ReadAsStringAsync().GetAwaiter().GetResult() }
        } catch {
            return [pscustomobject]@{ Status = 0; Body = $_.Exception.Message }
        }
    }
    # Raw GET that returns the body even for non-API (HTML) routes.
    function Get-Raw {
        param([string]$Path)
        try {
            $r = $client.GetAsync("$base$Path").GetAwaiter().GetResult()
            return [pscustomobject]@{ Status = [int]$r.StatusCode; Body = $r.Content.ReadAsStringAsync().GetAwaiter().GetResult() }
        } catch {
            return [pscustomobject]@{ Status = 0; Body = $_.Exception.Message }
        }
    }

    # Wait (bounded) for the server to answer the PUBLIC health route.
    $deadline = (Get-Date).AddSeconds(25)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        if ($serveProc.HasExited) { break }
        $h = Invoke-Api 'GET' '/v1/relux/health' $null
        if ($h.Status -eq 200) { $ready = $true; break }
        Start-Sleep -Milliseconds 400
    }
    Assert 'serve becomes ready' $ready ("$base/v1/relux/health")
    if (-not $ready) {
        if (Test-Path $serveErr) {
            Write-Host '  serve.err.log tail:' -ForegroundColor DarkGray
            Get-Content $serveErr -Tail 15 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
        }
        throw 'serve did not become ready'
    }

    # First-run setup mints the relux_session cookie into the client's jar.
    $setupBody = @{ username = 'mf-admin'; password = 'mf-pass-12345' } | ConvertTo-Json -Compress
    $setup = Invoke-Api 'POST' '/v1/auth/setup' $setupBody
    Assert 'auth setup creates admin + session' ($setup.Status -eq 200) ("status " + $setup.Status)
    $me = Invoke-Api 'GET' '/v1/auth/me' $null
    Assert 'auth me returns the session user' (($me.Status -eq 200) -and ($me.Body -match 'mf-admin')) ("status " + $me.Status)

    # -- install the manifestless plugin -----------------------------------
    Section 'Manifestless install (nested-root inference)'
    $insBody = @{ path = $extractRoot } | ConvertTo-Json -Compress
    $ins = Invoke-Api 'POST' '/v1/relux/plugins/install-dir' $insBody
    Assert 'install-dir succeeds' ($ins.Status -eq 200) ("status " + $ins.Status + " " + $ins.Body)
    $rec = $null; try { $rec = $ins.Body | ConvertFrom-Json } catch {}
    $pluginId = if ($rec) { $rec.id } else { $null }
    # The id/name/description are inferred from the NESTED repo root (widgetron-main),
    # NOT the artificial archive wrapper dir (mf-archive-xxxx).
    $idFromNested = $pluginId -and ($pluginId -eq 'relux-plugin-widgetron-main')
    Assert 'id inferred from nested repo root' $idFromNested ("id=" + $pluginId)
    Assert 'install is honest metadata-only wrapper' ($rec -and ($rec.generated -eq $true)) ("generated=" + ($rec.generated))
    Assert 'name comes from nested root' ($rec -and ($rec.name -match 'widgetron')) ("name=" + ($rec.name))
    Assert 'description comes from nested README' ($rec -and ($rec.description -match 'Widgetron')) ("desc carries the nested README summary")
    # The archive wrapper name must NOT have become the seed.
    Assert 'archive wrapper name not used as id' ($pluginId -notmatch 'mf-archive') ("id=" + $pluginId)

    # -- Plugin Lens tools visible to Prime --------------------------------
    Section 'Plugin Lens tools in Prime catalog'
    $pt = Invoke-Api 'GET' '/v1/relux/prime/tools' $null
    Assert 'prime tools route ok' ($pt.Status -eq 200) ("status " + $pt.Status)
    $tools = @(); try { $tools = @($pt.Body | ConvertFrom-Json) } catch {}
    function Find-Tool([string]$name) {
        @($tools | Where-Object { $_.tool_name -eq $name -and $_.plugin_id -eq $pluginId })
    }
    foreach ($lens in @('plugin.summary', 'plugin.inspect', 'plugin.search', 'plugin.read_file')) {
        $hit = Find-Tool $lens
        $ok = ($hit.Count -ge 1) -and ($hit[0].source -eq 'plugin')
        Assert ("lens tool listed: $lens") $ok ("source=" + ($(if ($hit.Count) { $hit[0].source } else { 'missing' })))
    }

    # -- Prime chat USES the lens tools (deterministic, natural, no task) ---
    # A helper that runs one Prime turn and asserts the shared invariants: a real
    # kernel-executed source tool, a natural (non-raw-JSON) reply, NO task created,
    # and NO fake secret leaking into the reply OR the structured/raw tool detail.
    Section 'Prime chat uses lens tools'
    function Test-PrimeLensTurn {
        param([string]$Label, [string]$Message, [string]$ExpectTool)
        $r = Invoke-Api 'POST' '/v1/relux/prime' (@{ message = $Message } | ConvertTo-Json -Compress)
        if ($r.Status -ne 200) { Fail "$Label : turn ok" ("status " + $r.Status); return $null }
        $turn = $null; try { $turn = $r.Body | ConvertFrom-Json } catch {}
        if (-not $turn) { Fail "$Label : turn parses" 'no JSON'; return $null }
        $expectedLabel = "$pluginId/$ExpectTool"
        Assert "$Label : invoked the lens tool" ($turn.invoked_tool -eq $expectedLabel) ("invoked_tool=" + $turn.invoked_tool)
        $reply = [string]$turn.reply
        Assert "$Label : natural answer (not raw JSON)" (($reply.Length -gt 0) -and (-not $reply.TrimStart().StartsWith('{'))) ("reply len=" + $reply.Length)
        Assert "$Label : created no task" (-not $turn.created_task) ("created_task=" + $turn.created_task)
        # Redaction parity: the FAKE secret must not appear ANYWHERE in the response
        # body (covers the visible reply AND the structured/raw tool_output detail).
        $leak = ($r.Body.Contains($fakeSecretPrefix)) -or ($r.Body.Contains($fakeSecretOpaque))
        Assert "$Label : no secret leak in reply/detail" (-not $leak) ("fake secret absent from the whole turn body")
        return $turn
    }

    # summary: "summarize the widgetron plugin" -> plugin.summary
    $sum = Test-PrimeLensTurn 'summary' 'summarize the widgetron plugin' 'plugin.summary'
    if ($sum) { Assert 'summary : answer names the plugin' (([string]$sum.reply) -match 'widgetron') 'reply mentions widgetron' }

    # search: "search the widgetron plugin for <MARKER>" -> plugin.search (hits app.py)
    $srch = Test-PrimeLensTurn 'search' ("search the widgetron plugin for " + $marker) 'plugin.search'
    if ($srch) {
        $mc = 0; try { $mc = [int]$srch.tool_output.structuredContent.match_count } catch {}
        Assert 'search : found the marker in source' ($mc -ge 1) ("match_count=" + $mc)
    }

    # read: "read config.env in the widgetron plugin" -> plugin.read_file (secret redacted)
    $rd = Test-PrimeLensTurn 'read' 'read config.env in the widgetron plugin' 'plugin.read_file'
    if ($rd) {
        Assert 'read : redaction placeholder present' (([string]$rd.tool_output.structuredContent.content) -match '\*\*\*REDACTED\*\*\*') 'fake secret was masked, not dropped'
    }

    # -- dashboard SPA shell for the key client routes ---------------------
    Section 'Dashboard SPA routes'
    $rootMarker = 'id="root"'
    foreach ($path in @('/dashboard', '/dashboard/plugins', '/dashboard/prime', '/dashboard/work', '/dashboard/crew')) {
        $d = Get-Raw $path
        if ($d.Status -eq 503) {
            Skip ("GET $path") '503 (dashboard bundle not built)'
        } elseif ($d.Status -eq 200) {
            $shell = ($d.Body.Length -gt 0) -and (($d.Body -match $rootMarker) -or ($d.Body -match '<script'))
            Assert ("GET $path") $shell ("served the SPA shell (" + $d.Body.Length + " bytes)")
        } else {
            Fail ("GET $path") ("status " + $d.Status)
        }
    }

} finally {
    Section 'cleanup'
    if ($client) { try { $client.Dispose() } catch {} }
    if ($serveProc) {
        try { if (-not $serveProc.HasExited) { Stop-Process -Id $serveProc.Id -Force -ErrorAction SilentlyContinue } } catch {}
    }
    if ($null -eq $oldDb) { Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue } else { $env:RELUX_DB = $oldDb }
    if ($null -eq $oldAddr) { Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue } else { $env:RELUX_HTTP_ADDR = $oldAddr }
    if ($null -eq $oldDash) { Remove-Item Env:\RELUX_DASHBOARD_DIST -ErrorAction SilentlyContinue } else { $env:RELUX_DASHBOARD_DIST = $oldDash }
    if ($TempRoot -and (Test-Path -LiteralPath $TempRoot)) {
        if ($KeepTemp) {
            Write-Host ("  temp data kept at {0}" -f $TempRoot) -ForegroundColor Yellow
        } else {
            Remove-Item -LiteralPath $TempRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

# -- summary ---------------------------------------------------------------
Write-Host ''
Write-Host '== manifestless-plugin -> Prime smoke summary ==' -ForegroundColor Cyan
$pass = @($script:Results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($script:Results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($script:Results | Where-Object { $_.Status -eq 'SKIP' }).Count
Write-Host ("  {0} passed, {1} failed, {2} skipped (of {3} checks)" -f $pass, $fail, $skip, $script:Results.Count)

if ($fail -eq 0) {
    Write-Host 'RESULT: PASS' -ForegroundColor Green
    exit 0
}
Write-Host ("RESULT: FAIL ({0} failing check(s))" -f $fail) -ForegroundColor Red
exit 1
