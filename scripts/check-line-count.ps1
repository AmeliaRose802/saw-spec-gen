<#
.SYNOPSIS
    Fails if any tracked source file exceeds MaxLines non-whitespace lines.

.DESCRIPTION
    PowerShell equivalent of scripts/check-line-count.sh for Windows users
    that prefer to run the check manually without bash.

.PARAMETER Files
    Optional list of files to check. If omitted, checks all tracked source files.

.PARAMETER MaxLines
    Threshold; default 500.

.EXAMPLE
    pwsh ./scripts/check-line-count.ps1
    pwsh ./scripts/check-line-count.ps1 src/main.rs
#>
[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Files,

    [int]$MaxLines = 500
)

$ErrorActionPreference = 'Stop'

$sourceExt = @(
    '.rs', '.py', '.sh', '.ps1', '.psm1', '.js', '.ts', '.tsx', '.jsx',
    '.c', '.cc', '.cpp', '.cxx', '.h', '.hh', '.hpp', '.hxx',
    '.saw', '.cry', '.java', '.go', '.rb'
)

$allowFile = Join-Path (Get-Location) '.linecount-allow'
$allowed = @()
if (Test-Path $allowFile) {
    $allowed = Get-Content $allowFile |
        Where-Object { $_ -and ($_ -notmatch '^\s*#') } |
        ForEach-Object { $_.Trim() -replace '\\', '/' }
}

if (-not $Files -or $Files.Count -eq 0) {
    $Files = git ls-files
}

$violations = 0
foreach ($f in $Files) {
    if (-not (Test-Path $f -PathType Leaf)) { continue }
    $ext = [System.IO.Path]::GetExtension($f).ToLowerInvariant()
    if ($sourceExt -notcontains $ext) { continue }
    $norm = ($f -replace '\\', '/')
    if ($allowed -contains $norm) { continue }
    $count = (Get-Content $f | Where-Object { $_ -match '\S' }).Count
    if ($count -gt $MaxLines) {
        Write-Host ("  {0}: {1} non-whitespace lines (limit {2})" -f $f, $count, $MaxLines) -ForegroundColor Red
        $violations++
    }
}

if ($violations -gt 0) {
    Write-Host ""
    Write-Host "ERROR: $violations file(s) exceed the $MaxLines non-whitespace line limit." -ForegroundColor Red
    Write-Host "Refactor the file(s) above into smaller modules. Do NOT add entries to .linecount-allow"
    Write-Host "without explicit reviewer approval."
    exit 1
}

exit 0
