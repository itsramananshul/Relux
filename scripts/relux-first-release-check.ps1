param(
    [switch]$SkipSmoke,
    [switch]$KeepTemp
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$ReleaseExe = Join-Path $Root "target\release\relux-kernel.exe"
$Failures = 0

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
    Invoke-NativeStep -Name "cargo test core/kernel" -Exe $Cargo -Arguments @("test", "-p", "relux-core", "-p", "relux-kernel")
    Invoke-NativeStep -Name "cargo clippy core/kernel" -Exe $Cargo -Arguments @("clippy", "-p", "relux-core", "-p", "relux-kernel", "--all-targets", "--", "-D", "warnings")
    Invoke-NativeStep -Name "kernel release build" -Exe $Cargo -Arguments @("build", "-p", "relux-kernel", "--release")
}

if ($Npm) {
    Invoke-NativeStep -Name "dashboard build" -Exe $Npm -Arguments @("run", "build") -WorkingDirectory (Join-Path $Root "apps\dashboard")
}

if (Test-Path -LiteralPath $ReleaseExe) {
    Invoke-NativeStep -Name "release doctor" -Exe $ReleaseExe -Arguments @("doctor")
} elseif ($Cargo) {
    Invoke-NativeStep -Name "doctor via cargo" -Exe $Cargo -Arguments @("run", "-p", "relux-kernel", "--", "doctor")
}

if ($SkipSmoke) {
    Write-Step "prime assigned-run smoke" "SKIP" "requested"
} elseif (Test-Path -LiteralPath $ReleaseExe) {
    $TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("relux-release-smoke-" + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $TempRoot | Out-Null
    $oldDb = $env:RELUX_DB
    $env:RELUX_DB = Join-Path $TempRoot "local.db"
    try {
        Invoke-NativeStep -Name "prime creates task" -Exe $ReleaseExe -Arguments @("prime", "create a task to inspect this repo")
        Invoke-NativeStep -Name "assigned task runs" -Exe $ReleaseExe -Arguments @("task", "run-assigned", "task_0001")
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

Write-Host ""
if ($Failures -eq 0) {
    Write-Host "RESULT: PASS" -ForegroundColor Green
    exit 0
}

Write-Host ("RESULT: FAIL ({0} failing step(s))" -f $Failures) -ForegroundColor Red
exit 1
