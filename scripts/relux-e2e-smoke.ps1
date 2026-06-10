# scripts/relux-e2e-smoke.ps1
#
# Standalone Relux first-release END-TO-END smoke.
#
# Proves the first version of the standalone Relux product is actually usable
# after every big chunk - not just unit-tested. It drives the real release
# binary (target/release/relux-kernel.exe) through every critical local flow
# against a THROWAWAY temporary RELUX_DB, and NEVER touches the operator's real
# dev store (dev-data/relux/local.db) or any real serve instance.
#
# What it covers (each step records PASS / FAIL / SKIP):
#
#   1. doctor          - the release binary reports healthy and the bundled
#                        plugin/adapter count includes all shipped bundles
#                        (relux-tools-echo, relux-tools-status,
#                        relux-adapter-local-prime, relux-adapter-claude-cli,
#                        relux-adapter-codex-cli).
#   2. Prime chat      - greeting does NOT create work or call a tool;
#                        "what tools can you use?" lists the REAL built-in tools;
#                        a status question invokes the status tool; an echo
#                        request invokes the echo tool and returns the input.
#   3. Tool CLI        - `tools` lists the built-ins as ready; `tool invoke
#                        relux-tools-echo echo.say {json}` returns the same JSON.
#   4. Loopback (opt)  - installs a TEMP non-bundled ToolSet plugin, points it at
#                        an in-script loopback HTTP server this script runs
#                        itself, grants Prime its permission, invokes it through
#                        the kernel, and confirms the loopback server's output
#                        flowed back (Plugin Runtime v1). Needs the local server;
#                        skip with -SkipLoopback. (Default: run on Windows.)
#   5. Adapter runtime - `adapters` shows the claude/codex/local-prime records
#                        after the bundled refresh; enabling an adapter with a
#                        deliberately FAKE command persists+reports the runtime
#                        config; disabling clears it. No real Claude/Codex is
#                        ever spawned (no task is run through a CLI adapter).
#   6. Autonomy        - creates a ready task through Prime, enables autonomy with
#                        safe settings, runs ONE manual tick, and verifies the
#                        task honestly moved Queued -> Completed with a run.
#   7. HTTP serve (opt)- starts `relux-kernel serve` on a free loopback port and
#                        hits /dashboard, /v1/relux/state, /v1/relux/prime/autonomy,
#                        and /v1/relux/tools; stops the server at the end. The
#                        loopback test (step 4) rides on this same server because
#                        granting Prime a new tool permission is an API operation.
#                        Skip with -SkipServe.
#
# Everything temporary (DB, plugins, uploads, server, jobs, processes) is always
# cleaned up. Prints a concise PASS/FAIL table and exits non-zero on any failure.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-e2e-smoke.ps1
#   ... -SkipBuild       # reuse an existing target\release\relux-kernel.exe
#   ... -SkipServe       # skip the HTTP serve checks AND the loopback test
#   ... -SkipLoopback    # skip only the loopback runtime test
#   ... -KeepTemp        # keep the temp RELUX_DB for inspection
#   ... -RunRealClaudeAdapter  # opt-in: run ONE tiny non-mutating assigned task
#                              # through the REAL Claude CLI adapter (needs `claude`
#                              # on PATH + logged in). DISABLED by default. Never
#                              # uses --dangerously-skip-permissions.
#   ... -RunRealCodexAdapter   # same, for the real Codex CLI (`codex`).
#
# The real-adapter smokes need the HTTP serve (do not combine with -SkipServe).
# When the CLI is not on PATH they SKIP; otherwise they record an honest PASS/FAIL
# from the actual run. No bypass/danger flags are ever passed to the CLI.

[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipServe,
    [switch]$SkipLoopback,
    [switch]$KeepTemp,
    [switch]$RunRealClaudeAdapter,
    [switch]$RunRealCodexAdapter
)

$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$ReleaseExe = Join-Path $Root 'target\release\relux-kernel.exe'

# System.Net.Http is not auto-loaded in Windows PowerShell 5.1.
Add-Type -AssemblyName System.Net.Http -ErrorAction SilentlyContinue

# -- tiny PASS/FAIL/SKIP reporting harness ---------------------------------
$script:Results = New-Object System.Collections.ArrayList
function Record {
    param([string]$Name, [string]$Status, [string]$Detail = '')
    [void]$script:Results.Add([pscustomobject]@{ Name = $Name; Status = $Status; Detail = $Detail })
    $color = 'Red'
    if ($Status -eq 'PASS') { $color = 'Green' } elseif ($Status -eq 'SKIP') { $color = 'Yellow' }
    Write-Host ("  {0,-4} {1,-30} {2}" -f $Status, $Name, $Detail) -ForegroundColor $color
}
function Pass([string]$n, [string]$d = '') { Record $n 'PASS' $d }
function Fail([string]$n, [string]$d = '') { Record $n 'FAIL' $d }
function Skip([string]$n, [string]$d = '') { Record $n 'SKIP' $d }
function Assert([string]$n, [bool]$ok, [string]$d = '') { if ($ok) { Pass $n $d } else { Fail $n $d } }
function Section([string]$t) { Write-Host ''; Write-Host ">> $t" -ForegroundColor DarkCyan }

# Run the release binary, capturing combined stdout+stderr as one string. A
# non-zero exit / stderr is captured as text (not terminating) so callers can
# assert on the output of an intentionally-failing command.
function Exe {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$ArgList)
    $ErrorActionPreference = 'Continue'
    $out = & $ReleaseExe @ArgList 2>&1 | Out-String
    return $out
}

function Get-FreePort {
    $p = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $p.Start(); $port = $p.LocalEndpoint.Port; $p.Stop(); return $port
}

# -- state captured for the finally cleanup --------------------------------
$TempRoot = $null
$oldDb = $env:RELUX_DB
$oldAddr = $env:RELUX_HTTP_ADDR
$serveProc = $null
$lbJob = $null

Write-Host ''
Write-Host '== Relux standalone first-release E2E smoke ==' -ForegroundColor Cyan
Write-Host ("workspace: {0}" -f $Root)

try {
    # -- 0) build / locate the release binary ------------------------------
    Section 'Release binary'
    if (-not $SkipBuild) {
        Write-Host '  building target\release\relux-kernel.exe ...' -ForegroundColor DarkGray
        & cargo build -p relux-kernel --release
        if ($LASTEXITCODE -ne 0) { Fail 'release build' 'cargo build failed'; throw 'release build failed' }
        Pass 'release build' 'cargo build -p relux-kernel --release'
    } else {
        Skip 'release build' '-SkipBuild'
    }
    if (-not (Test-Path -LiteralPath $ReleaseExe)) {
        Fail 'release binary present' $ReleaseExe
        throw 'release binary missing'
    }
    Pass 'release binary present' $ReleaseExe

    # -- isolate: throwaway RELUX_DB (plugins/uploads land next to it) -----
    $TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('relux-e2e-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
    New-Item -ItemType Directory -Path $TempRoot | Out-Null
    $env:RELUX_DB = Join-Path $TempRoot 'local.db'
    Write-Host ("  temp RELUX_DB: {0}" -f $env:RELUX_DB) -ForegroundColor DarkGray

    # -- 1) doctor + bundled plugin/adapter coverage -----------------------
    Section 'doctor + bundled coverage'
    $doctor = Exe doctor
    Assert 'doctor status PASS' ($doctor -match 'Status:\s*PASS') 'relux-kernel doctor'
    $bundledCount = 0
    if ($doctor -match 'Installed plugins:\s*(\d+)') { $bundledCount = [int]$Matches[1] }
    Assert 'bundled plugin count >= 5' ($bundledCount -ge 5) ("installed plugins = {0}" -f $bundledCount)

    $adaptersOut = Exe adapters
    $expectedAdapters = @('relux-adapter-local-prime', 'relux-adapter-claude-cli', 'relux-adapter-codex-cli')
    $missingAdapters = @($expectedAdapters | Where-Object { $adaptersOut -notmatch [regex]::Escape($_) })
    Assert 'bundled adapters present' ($missingAdapters.Count -eq 0) ("expected: " + ($expectedAdapters -join ', '))

    $toolsOut = Exe tools
    $expectedTools = @('relux-tools-echo', 'relux-tools-status')
    $missingTools = @($expectedTools | Where-Object { $toolsOut -notmatch [regex]::Escape($_) })
    Assert 'built-in tools present' ($missingTools.Count -eq 0) ("expected: " + ($expectedTools -join ', '))

    # -- 2) Prime chat flows ----------------------------------------------
    Section 'Prime chat'
    $greet = Exe prime 'hey'
    $greetNoTool = ($greet -notmatch 'tool\s+>')
    $greetIsGreeting = ($greet -match '\[Greeting/')
    Assert 'greeting does not call a tool' ($greetNoTool -and $greetIsGreeting) 'no tool/work from "hey"'

    $disc = Exe prime 'what tools can you use?'
    # echo is an internal dev/test fixture: it must be HIDDEN from Prime's
    # user-facing tool catalogue; the genuine status tool is shown.
    $discOk = ($disc -match 'ToolDiscovery') -and ($disc -notmatch 'echo\.say') -and ($disc -match 'relux-tools-status/status\.summary')
    Assert 'tool discovery hides echo, lists real tools' $discOk 'status.summary listed; echo hidden'

    $status = Exe prime 'what is going on?'
    $statusOk = ($status -match 'tool\s+>\s+relux-tools-status/status\.summary')
    Assert 'status request invokes status tool' $statusOk 'relux-tools-status/status.summary'

    $echoTok = 'e2e-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
    $echoTurn = Exe prime ("echo {0}" -f $echoTok)
    $echoTurnOk = ($echoTurn -match 'tool\s+>\s+relux-tools-echo/echo\.say') -and ($echoTurn -match [regex]::Escape($echoTok))
    Assert 'echo request invokes echo tool' $echoTurnOk 'relux-tools-echo/echo.say echoes input'

    # -- 3) Tool CLI flows ------------------------------------------------
    Section 'Tool CLI'
    $toolsListOk = ($toolsOut -match 'relux-tools-echo\s+echo\.say\s+\S+\s+ready') -and ($toolsOut -match 'relux-tools-status\s+status\.summary\s+\S+\s+ready')
    Assert 'tools lists built-ins as ready' $toolsListOk 'echo + status ready'

    $invTok = 'inv-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
    # Escape the inner quotes so the native exe receives valid JSON: Windows
    # PowerShell strips bare double quotes when forwarding a native argument.
    $invJson = '{\"token\":\"' + $invTok + '\"}'
    $invOut = Exe tool invoke relux-tools-echo echo.say $invJson
    $invOk = ($invOut -match 'invoked relux-tools-echo/echo\.say') -and ($invOut -match [regex]::Escape($invTok))
    Assert 'tool invoke echo returns same JSON' $invOk ('echoed token ' + $invTok)

    # -- 5) Adapter runtime controls (no real Claude/Codex spawned) --------
    # NOTE: ordered before autonomy so the autonomy run is the last state change.
    Section 'Adapter runtime controls'
    $fakeCmd = 'relux-e2e-fake-cli-binary-does-not-exist'
    $adEnable = Exe adapter runtime enable relux-adapter-claude-cli --command $fakeCmd --timeout-seconds 30 --max-output-bytes 4096
    Assert 'adapter enable persists config' ($adEnable -match 'enabled claude_cli adapter runtime') 'fake command, not spawned'
    $adShow = Exe adapter runtime relux-adapter-claude-cli
    $adShowOk = ($adShow -match 'enabled:\s*true') -and ($adShow -match [regex]::Escape($fakeCmd)) -and ($adShow -match 'on PATH:\s*false')
    Assert 'adapter status reflects enable' $adShowOk 'enabled=true, fake binary off PATH'
    $adDisable = Exe adapter runtime disable relux-adapter-claude-cli
    $adShow2 = Exe adapter runtime relux-adapter-claude-cli
    Assert 'adapter disable clears runtime' (($adDisable -match 'disabled adapter runtime') -and ($adShow2 -match 'enabled:\s*false')) 'enabled=false after disable'

    # -- 6) Autonomy: create a ready task, tick once, verify honest move ---
    Section 'Prime autonomy'
    $createTurn = Exe prime 'create a task to inspect this repo'
    Assert 'prime creates a task' ($createTurn -match 'Created task_\d+') 'task queued + assigned to prime'
    $stateBefore = Exe state
    $queuedBefore = ($stateBefore -match 'task_0001\s+\[Queued\]')
    Assert 'task starts Queued' $queuedBefore 'task_0001 Queued before tick'
    [void](Exe prime autonomy configure --interval 5 --max-tasks 1 --auto-assign false)
    [void](Exe prime autonomy enable)
    $tick = Exe prime autonomy tick
    $tickRan = ($tick -match 'Tasks Run:\s*1')
    Assert 'autonomy tick runs one task' $tickRan 'one ready task executed'
    $stateAfter = Exe state
    $completedAfter = ($stateAfter -match 'task_0001\s+\[Completed\]') -and ($stateAfter -match 'runs=1')
    Assert 'task honestly moved to Completed' $completedAfter 'Queued -> Completed with a run'
    [void](Exe prime autonomy disable)

    # -- 4 + 7) HTTP serve endpoints + loopback ToolSet runtime ------------
    if ($SkipServe) {
        Section 'HTTP serve + loopback'
        Skip 'http serve checks' '-SkipServe'
        Skip 'loopback runtime' '-SkipServe (needs the API to grant Prime the tool permission)'
        if ($RunRealClaudeAdapter) { Skip 'real claude adapter run' '-SkipServe (needs the serve API)' }
        if ($RunRealCodexAdapter) { Skip 'real codex adapter run' '-SkipServe (needs the serve API)' }
    } else {
        Section 'HTTP serve + loopback'

        # Prepare a temp non-bundled ToolSet plugin for the loopback test.
        $plugDir = Join-Path $TempRoot 'relux-tools-smoke'
        New-Item -ItemType Directory -Path $plugDir | Out-Null
        $smokeManifest = @'
{
  "id": "relux-tools-smoke",
  "name": "Relux Smoke Loopback Tool",
  "version": "0.1.0",
  "kind": "ToolSet",
  "description": "Temporary ToolSet plugin used by the e2e smoke to exercise the HTTP loopback runtime.",
  "author": "Relux Smoke",
  "trust_level": "private",
  "capabilities": {
    "tools": [
      { "name": "smoke.ping", "description": "Loopback ping.", "risk": "low", "permission": "tool:relux-tools-smoke:ping", "approval": "never", "timeout_secs": 5 }
    ],
    "permissions": ["tool:relux-tools-smoke:ping"]
  },
  "health": "unknown"
}
'@
        # Write WITHOUT a BOM (Windows PowerShell 5.1 Set-Content -Encoding UTF8
        # prepends a BOM that the manifest JSON parser rejects).
        [System.IO.File]::WriteAllText((Join-Path $plugDir 'relux-plugin.json'), $smokeManifest, (New-Object System.Text.UTF8Encoding($false)))

        $srvPort = Get-FreePort
        $lbPort = Get-FreePort
        $lbToken = 'lb-' + [guid]::NewGuid().ToString('N').Substring(0, 12)
        $base = "http://127.0.0.1:$srvPort"

        # In-script loopback HTTP server (raw TcpListener so it needs no URL ACL
        # / admin, unlike HttpListener). It answers POST /invoke with a fixed
        # { "output": { token } } envelope; seeing that token flow back through
        # the kernel proves the loopback runtime executed (not a built-in, not
        # fabricated). Runs in a background job for the life of the serve check.
        if (-not $SkipLoopback) {
            $lbJob = Start-Job -ScriptBlock {
                param($port, $token)
                $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $port)
                $listener.Start()
                $bodyJson = '{"output":{"runtime":"loopback","token":"' + $token + '"}}'
                $bodyBytes = [System.Text.Encoding]::UTF8.GetBytes($bodyJson)
                for ($i = 0; $i -lt 8; $i++) {
                    try {
                        $client = $listener.AcceptTcpClient()
                        $stream = $client.GetStream()
                        $stream.ReadTimeout = 2000
                        Start-Sleep -Milliseconds 60
                        $buf = New-Object byte[] 8192
                        try { while ($stream.DataAvailable) { [void]$stream.Read($buf, 0, $buf.Length) } } catch {}
                        $head = "HTTP/1.1 200 OK`r`nContent-Type: application/json`r`nContent-Length: $($bodyBytes.Length)`r`nConnection: close`r`n`r`n"
                        $headBytes = [System.Text.Encoding]::ASCII.GetBytes($head)
                        $stream.Write($headBytes, 0, $headBytes.Length)
                        $stream.Write($bodyBytes, 0, $bodyBytes.Length)
                        $stream.Flush()
                        $client.Close()
                    } catch {}
                }
                $listener.Stop()
            } -ArgumentList $lbPort, $lbToken
        }

        # Start the kernel HTTP server on a free loopback port. Start-Process
        # -PassThru gives us the exact PID to stop later (we never touch a real
        # serve instance the operator may be running on the default port).
        $env:RELUX_HTTP_ADDR = "127.0.0.1:$srvPort"
        $serveOut = Join-Path $TempRoot 'serve.out.log'
        $serveErr = Join-Path $TempRoot 'serve.err.log'
        $serveProc = Start-Process -FilePath $ReleaseExe -ArgumentList 'serve' -PassThru -WindowStyle Hidden -RedirectStandardOutput $serveOut -RedirectStandardError $serveErr

        # A CookieContainer captures the relux_session cookie minted by
        # /v1/auth/setup and re-sends it on every later request - exactly what the
        # browser dashboard does. Local operator login now guards /v1/relux/*.
        $handler = New-Object System.Net.Http.HttpClientHandler
        $handler.CookieContainer = New-Object System.Net.CookieContainer
        $handler.UseCookies = $true
        $client = New-Object System.Net.Http.HttpClient($handler)
        # Generous timeout: execute-assigned is synchronous, and a REAL Claude/Codex
        # adapter run (opt-in) blocks this call until the CLI finishes. Fast calls
        # still return immediately, so a high ceiling is harmless.
        $client.Timeout = [TimeSpan]::FromSeconds(180)
        function Invoke-Api {
            param([string]$Method, [string]$Path, [string]$Json)
            try {
                if ($Method -eq 'GET') {
                    $r = $client.GetAsync("$base$Path").GetAwaiter().GetResult()
                } elseif ($Method -eq 'DELETE') {
                    $r = $client.DeleteAsync("$base$Path").GetAwaiter().GetResult()
                } elseif ($Method -eq 'PUT') {
                    $c = New-Object System.Net.Http.StringContent($Json, [System.Text.Encoding]::UTF8, 'application/json')
                    $r = $client.PutAsync("$base$Path", $c).GetAwaiter().GetResult()
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

        # Wait (bounded) for the server to answer. Probe the PUBLIC health route
        # (no session required) so readiness does not depend on login.
        $deadline = (Get-Date).AddSeconds(25)
        $ready = $false
        while ((Get-Date) -lt $deadline) {
            if ($serveProc.HasExited) { break }
            $h = Invoke-Api 'GET' '/v1/relux/health' $null
            if ($h.Status -eq 200) { $ready = $true; break }
            Start-Sleep -Milliseconds 400
        }
        Assert 'serve becomes ready' $ready ("$base/v1/relux/health")

        if ($ready) {
            # -- Local operator login -----------------------------------------
            # NEGATIVE control: a protected route with NO session must be 401
            # (proves auth is genuinely enforced, not wide open). Use a fresh
            # cookieless client so the main client's jar stays empty until setup.
            $bare = New-Object System.Net.Http.HttpClient
            $bare.Timeout = [TimeSpan]::FromSeconds(20)
            try {
                $noAuth = $bare.GetAsync("$base/v1/relux/state").GetAwaiter().GetResult()
                Assert 'protected route 401 without session' ([int]$noAuth.StatusCode -eq 401) ("status " + [int]$noAuth.StatusCode)
            } catch {
                Fail 'protected route 401 without session' $_.Exception.Message
            } finally {
                $bare.Dispose()
            }
            # First-run setup mints the relux_session cookie into the main client's
            # jar, so every subsequent /v1/relux/* call authenticates automatically.
            $setupBody = @{ username = 'e2e-admin'; password = 'e2e-pass-123' } | ConvertTo-Json -Compress
            $setup = Invoke-Api 'POST' '/v1/auth/setup' $setupBody
            Assert 'auth setup creates admin + session' ($setup.Status -eq 200) ("status " + $setup.Status)
            $me = Invoke-Api 'GET' '/v1/auth/me' $null
            Assert 'auth me returns the session user' (($me.Status -eq 200) -and ($me.Body -match 'e2e-admin')) ("status " + $me.Status)

            # -- Authenticated password change ---------------------------------
            # A SECOND signed-in session (its own cookie jar) proves the documented
            # invalidation: a change keeps the caller's own session but boots every
            # other live session.
            $otherHandler = New-Object System.Net.Http.HttpClientHandler
            $otherHandler.CookieContainer = New-Object System.Net.CookieContainer
            $otherHandler.UseCookies = $true
            $other = New-Object System.Net.Http.HttpClient($otherHandler)
            $other.Timeout = [TimeSpan]::FromSeconds(20)
            $otherLogin = $other.PostAsync("$base/v1/auth/login", (New-Object System.Net.Http.StringContent((@{ username = 'e2e-admin'; password = 'e2e-pass-123' } | ConvertTo-Json -Compress), [System.Text.Encoding]::UTF8, 'application/json'))).GetAwaiter().GetResult()
            $otherStateBefore = $other.GetAsync("$base/v1/relux/state").GetAwaiter().GetResult()
            Assert 'second session is live before change' (([int]$otherLogin.StatusCode -eq 200) -and ([int]$otherStateBefore.StatusCode -eq 200)) ("login=" + [int]$otherLogin.StatusCode + " state=" + [int]$otherStateBefore.StatusCode)

            # Wrong current password is refused (401) and changes nothing.
            $cpWrong = Invoke-Api 'POST' '/v1/auth/change-password' (@{ current_password = 'not-the-password'; new_password = 'e2e-pass-456' } | ConvertTo-Json -Compress)
            Assert 'change-password rejects wrong current' ($cpWrong.Status -eq 401) ("status " + $cpWrong.Status)
            # Too-short new password is refused (400).
            $cpShort = Invoke-Api 'POST' '/v1/auth/change-password' (@{ current_password = 'e2e-pass-123'; new_password = 'short' } | ConvertTo-Json -Compress)
            Assert 'change-password rejects too-short new' ($cpShort.Status -eq 400) ("status " + $cpShort.Status)
            # Successful change (200) — never echoes the password/hash.
            $cpOk = Invoke-Api 'POST' '/v1/auth/change-password' (@{ current_password = 'e2e-pass-123'; new_password = 'e2e-pass-456' } | ConvertTo-Json -Compress)
            $cpClean = ($cpOk.Status -eq 200) -and (-not ($cpOk.Body -match 'e2e-pass-456')) -and (-not ($cpOk.Body -match 'argon2'))
            Assert 'change-password succeeds without leaking the secret' $cpClean ("status " + $cpOk.Status)
            # The caller's own session SURVIVES the change.
            Assert 'current session survives the change' ((Invoke-Api 'GET' '/v1/relux/state' $null).Status -eq 200) '200'
            # Every OTHER session is invalidated (now 401).
            $otherStateAfter = $other.GetAsync("$base/v1/relux/state").GetAwaiter().GetResult()
            Assert 'other session invalidated by the change' ([int]$otherStateAfter.StatusCode -eq 401) ("status " + [int]$otherStateAfter.StatusCode)
            $other.Dispose()
            # The old password no longer logs in; the new one does.
            $loginOld = Invoke-Api 'POST' '/v1/auth/login' (@{ username = 'e2e-admin'; password = 'e2e-pass-123' } | ConvertTo-Json -Compress)
            Assert 'old password no longer works' ($loginOld.Status -eq 401) ("status " + $loginOld.Status)
            $loginNew = Invoke-Api 'POST' '/v1/auth/login' (@{ username = 'e2e-admin'; password = 'e2e-pass-456' } | ConvertTo-Json -Compress)
            Assert 'new password works' ($loginNew.Status -eq 200) ("status " + $loginNew.Status)

            $dash = Invoke-Api 'GET' '/dashboard' $null
            if ($dash.Status -eq 200) {
                Pass 'GET /dashboard' '200'
            } elseif ($dash.Status -eq 503) {
                Skip 'GET /dashboard' '503 (dashboard bundle not built; run npm run build in apps/dashboard)'
            } else {
                Fail 'GET /dashboard' ("status " + $dash.Status)
            }
            Assert 'GET /v1/relux/state' ((Invoke-Api 'GET' '/v1/relux/state' $null).Status -eq 200) '200'
            Assert 'GET /v1/relux/prime/autonomy' ((Invoke-Api 'GET' '/v1/relux/prime/autonomy' $null).Status -eq 200) '200'
            Assert 'GET /v1/relux/tools' ((Invoke-Api 'GET' '/v1/relux/tools' $null).Status -eq 200) '200'

            if ($SkipLoopback) {
                Skip 'loopback runtime' '-SkipLoopback'
            } else {
                # Install the temp plugin, point it at the in-script loopback
                # server, grant Prime its permission, and invoke through the
                # kernel. The grant is an API op (no CLI exists for it), which is
                # why the loopback test runs against the live server.
                $ins = Invoke-Api 'POST' '/v1/relux/plugins/install-dir' (@{ path = $plugDir } | ConvertTo-Json -Compress)
                $rt = Invoke-Api 'PUT' '/v1/relux/plugins/relux-tools-smoke/runtime' (@{ base_url = "http://127.0.0.1:$lbPort"; enabled = $true; timeout_ms = 4000 } | ConvertTo-Json -Compress)
                $gr = Invoke-Api 'POST' '/v1/relux/agents/prime/permissions' (@{ permission = 'tool:relux-tools-smoke:ping' } | ConvertTo-Json -Compress)
                $setupOk = ($ins.Status -eq 200) -and ($rt.Status -eq 200) -and ($gr.Status -eq 200)
                Assert 'loopback plugin configured' $setupOk ("install=$($ins.Status) runtime=$($rt.Status) grant=$($gr.Status)")

                Start-Sleep -Milliseconds 300
                $inv = Invoke-Api 'POST' '/v1/relux/tools/invoke' (@{ plugin_id = 'relux-tools-smoke'; tool_name = 'smoke.ping'; input = @{ hello = 'world' } } | ConvertTo-Json -Compress)
                $loopOk = ($inv.Status -eq 200) -and ($inv.Body.Contains($lbToken))
                Assert 'loopback runtime returns its output' $loopOk ("invoke=$($inv.Status), token flowed back=$($inv.Body.Contains($lbToken))")
            }

            # -- Real CLI adapter smoke (OPT-IN; never bypass flags) -----------
            # Drives ONE tiny, non-mutating assigned task through the REAL Claude /
            # Codex CLI: enable the adapter (auto-detected binary, no --command, no
            # danger flags), create an agent that uses it, assign a trivial task,
            # execute it, and record the honest run outcome. Skips cleanly when the
            # CLI is not on PATH. Disabled by default.
            $realAdapters = @(
                @{ On = $RunRealClaudeAdapter; Bin = 'claude'; Adapter = 'relux-adapter-claude-cli'; Tag = 'claude' },
                @{ On = $RunRealCodexAdapter;  Bin = 'codex';  Adapter = 'relux-adapter-codex-cli';  Tag = 'codex' }
            )
            foreach ($ra in $realAdapters) {
                if (-not $ra.On) { continue }
                $label = "real $($ra.Tag) adapter run"
                $onPath = [bool](Get-Command $ra.Bin -ErrorAction SilentlyContinue)
                if (-not $onPath) { Skip $label ("$($ra.Bin) not on PATH"); continue }

                $agName = "smoke-$($ra.Tag)"
                # Reset to a clean runtime first: an earlier step may have left a
                # fake command on this adapter. Clear it, then enable with the real
                # binary explicitly so detection cannot inherit stale config.
                [void](Invoke-Api 'DELETE' "/v1/relux/adapters/$($ra.Adapter)/runtime" $null)
                $en = Invoke-Api 'PUT' "/v1/relux/adapters/$($ra.Adapter)/runtime" (@{ enabled = $true; command = $ra.Bin } | ConvertTo-Json -Compress)
                $ag = Invoke-Api 'POST' '/v1/relux/agents' (@{ id = $agName; name = $agName; adapter_plugin = $ra.Adapter } | ConvertTo-Json -Compress)
                $tk = Invoke-Api 'POST' '/v1/relux/tasks' (@{ title = 'Reply with the single word OK and do nothing else. Do not create or modify any files.' } | ConvertTo-Json -Compress)
                $taskId = $null; try { $taskId = ($tk.Body | ConvertFrom-Json).id } catch {}
                if (-not $taskId) { Fail $label ("could not create task: enable=$($en.Status) agent=$($ag.Status) task=$($tk.Status)"); continue }
                [void](Invoke-Api 'POST' "/v1/relux/tasks/$taskId/assign" (@{ agent_id = $agName } | ConvertTo-Json -Compress))

                $ex = Invoke-Api 'POST' "/v1/relux/tasks/$taskId/execute-assigned" '{}'
                $runId = $null; try { $runId = ($ex.Body | ConvertFrom-Json).run_id } catch {}
                $runObj = $null
                if ($runId) { try { $runObj = (Invoke-Api 'GET' "/v1/relux/runs/$runId" $null).Body | ConvertFrom-Json } catch {} }
                $completed = ($ex.Status -eq 200) -and $runObj -and ($runObj.status -eq 'completed')
                if ($completed) {
                    Pass $label ("run $runId completed via real $($ra.Bin)")
                } else {
                    $why = if ($runObj) { "status=$($runObj.status) error=$($runObj.error)" } else { "execute=$($ex.Status)" }
                    Fail $label $why
                }

                # -- Prime Brain chat through the real CLI -----------------------
                # Select this CLI as Prime's brain and ask a conversational
                # question; the reply must come back tagged with the CLI's ai_mode
                # (not deterministic), proving Prime talks THROUGH the CLI.
                $brainLabel = "real $($ra.Tag) Prime brain chat"
                $brainVal = "$($ra.Tag)_cli"
                [void](Invoke-Api 'PUT' '/v1/relux/ai/config' (@{ brain = $brainVal } | ConvertTo-Json -Compress))
                $brainTok = ($ra.Tag.ToUpper() + 'OK')
                $primeBody = @{ message = ("Reply with exactly one short sentence. Begin it with the token {0}." -f $brainTok) } | ConvertTo-Json -Compress
                $pr = Invoke-Api 'POST' '/v1/relux/prime' $primeBody
                $prTurn = $null; try { $prTurn = $pr.Body | ConvertFrom-Json } catch {}
                $brainOk = ($pr.Status -eq 200) -and $prTurn -and ($prTurn.ai_mode -eq $brainVal) -and ($prTurn.reply)
                if ($brainOk) {
                    Pass $brainLabel ("ai_mode=$($prTurn.ai_mode); reply len=$($prTurn.reply.Length)")
                } else {
                    $why = if ($prTurn) { "ai_mode=$($prTurn.ai_mode) note=$($prTurn.ai_note)" } else { "prime=$($pr.Status)" }
                    Fail $brainLabel $why
                }
                [void](Invoke-Api 'DELETE' '/v1/relux/ai/config' $null)

                # Always disable the adapter again so no later step can spawn it.
                [void](Invoke-Api 'DELETE' "/v1/relux/adapters/$($ra.Adapter)/runtime" $null)
            }
        } else {
            if (Test-Path $serveErr) {
                Write-Host '  serve.err.log tail:' -ForegroundColor DarkGray
                Get-Content $serveErr -Tail 15 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
            }
            Skip 'loopback runtime' 'serve did not become ready'
        }
        if ($client) { $client.Dispose() }
    }

} finally {
    Section 'cleanup'
    # Stop the kernel serve process we started (only this PID).
    if ($serveProc) {
        try { if (-not $serveProc.HasExited) { Stop-Process -Id $serveProc.Id -Force -ErrorAction SilentlyContinue } } catch {}
    }
    # Stop the loopback server job.
    if ($lbJob) {
        Stop-Job $lbJob -ErrorAction SilentlyContinue
        Remove-Job $lbJob -Force -ErrorAction SilentlyContinue
    }
    # Restore env.
    if ($null -eq $oldDb) { Remove-Item Env:\RELUX_DB -ErrorAction SilentlyContinue } else { $env:RELUX_DB = $oldDb }
    if ($null -eq $oldAddr) { Remove-Item Env:\RELUX_HTTP_ADDR -ErrorAction SilentlyContinue } else { $env:RELUX_HTTP_ADDR = $oldAddr }
    # Clean the temp store.
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
Write-Host '== E2E smoke summary ==' -ForegroundColor Cyan
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
