param(
  [Parameter(Mandatory=$true)][string]$Reader,
  [Parameter(Mandatory=$true)][string]$PathsFile
)
$ErrorActionPreference = 'Stop'
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Ledger = Join-Path $ScriptDir 'relix-file-line-coverage.jsonl'
$Progress = Join-Path $ScriptDir 'relix-file-line-coverage-progress.md'
$Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$Backup = Join-Path $ScriptDir "relix-file-line-coverage.$Stamp.bak.jsonl"
Copy-Item -LiteralPath $Ledger -Destination $Backup
$paths = Get-Content -LiteralPath $PathsFile | Where-Object { $_ -and $_.Trim() -ne '' } | ForEach-Object { $_.Trim() } | Sort-Object -Unique
$pathSet = @{}
foreach ($p in $paths) { $pathSet[$p] = $true }
$now = Get-Date -Format o
$updated = 0
$missing = New-Object System.Collections.Generic.List[string]
$seen = @{}
$rows = Get-Content -LiteralPath $Ledger | ForEach-Object {
  $row = $_ | ConvertFrom-Json
  if ($pathSet.ContainsKey($row.path)) {
    $seen[$row.path] = $true
    if ($row.kind -eq 'text') {
      $row.semantic_read_status = 'semantic_read_line_by_line'
      $row.semantic_reader = $Reader
      $row.semantic_read_at = $now
      $updated++
    }
  }
  $row
}
foreach ($p in $paths) {
  if (-not $seen.ContainsKey($p)) { $missing.Add($p) | Out-Null }
}
$rows | ForEach-Object { $_ | ConvertTo-Json -Compress -Depth 10 } | Set-Content -LiteralPath $Ledger -Encoding UTF8
$total = $rows.Count
$semantic = @($rows | Where-Object { $_.kind -eq 'text' -and $_.semantic_read_status -eq 'semantic_read_line_by_line' }).Count
$pending = @($rows | Where-Object { $_.kind -eq 'text' -and $_.semantic_read_status -ne 'semantic_read_line_by_line' }).Count
$generated = @($rows | Where-Object { $_.kind -eq 'generated_or_lock_text' }).Count
$binary = @($rows | Where-Object { $_.kind -eq 'binary_asset' }).Count
@(
  '# Relix Strict Coverage Progress'
  ''
  "Updated: $now"
  ''
  "- Total tracked rows: $total"
  "- Semantic line-read files: $semantic"
  "- Pending semantic text files: $pending"
  "- Generated/lock structural-summary rows: $generated"
  "- Binary inventoried rows: $binary"
  "- Last reader: $Reader"
  "- Last update marked: $updated"
  "- Backup before update: $Backup"
) | Set-Content -LiteralPath $Progress -Encoding UTF8
"updated=$updated"
"missing=$($missing.Count)"
if ($missing.Count -gt 0) { $missing | ForEach-Object { "missing=$_" } }
"progress=$Progress"
