<#
.SYNOPSIS
    Enforce the no-custom-runner policy on tests/e2e/cases.psd1.

.DESCRIPTION
    Scans the E2E manifest for Runner='custom' or Script= entries.
    Any such entry whose Script path is not listed in the allowlist
    (tests/e2e/custom-runner-allowlist.psd1) is a policy violation.
    Allowlisted entries whose Expires date has passed are also failures.

    Run locally before pushing:
        pwsh scripts/check-no-custom-runners.ps1

    Policy: use built-in runners (cpp, rust, equiv) only.  If a
    capability is missing from a built-in runner, extend the runner
    instead of wrapping a custom script.

.PARAMETER Manifest
    Path to the E2E manifest (default: tests/e2e/cases.psd1 relative
    to the repository root).

.PARAMETER Allowlist
    Path to the temporary-exception allowlist (default:
    tests/e2e/custom-runner-allowlist.psd1 relative to the repo root).
#>
[CmdletBinding()]
param(
    [string]$Manifest  = (Join-Path $PSScriptRoot '../tests/e2e/cases.psd1'),
    [string]$Allowlist = (Join-Path $PSScriptRoot '../tests/e2e/custom-runner-allowlist.psd1')
)

$ErrorActionPreference = 'Stop'

$today = [datetime]::Today

# Load allowlist: Script path -> entry hashtable
$allowedScripts = @{}
if (Test-Path $Allowlist) {
    $al = Import-PowerShellDataFile $Allowlist
    foreach ($entry in $al.Exceptions) {
        $allowedScripts[$entry.Script] = $entry
    }
}

# Load manifest and collect every case that uses a custom runner or Script=
$data        = Import-PowerShellDataFile $Manifest
$customCases = $data.Cases | Where-Object {
    $_.Runner -eq 'custom' -or $_.ContainsKey('Script')
}

$violations  = [System.Collections.Generic.List[string]]::new()
$expiredList = [System.Collections.Generic.List[string]]::new()

foreach ($c in $customCases) {
    $script = $c.Script
    if (-not $script) {
        $violations.Add("  (Runner='custom' without Script=)  tag=$($c.Tag)")
        continue
    }

    if ($allowedScripts.ContainsKey($script)) {
        $entry  = $allowedScripts[$script]
        $expiry = [datetime]::ParseExact($entry.Expires, 'yyyy-MM-dd', $null)
        if ($expiry -lt $today) {
            $expiredList.Add(
                "  EXPIRED  $script`n" +
                "           expired=$($entry.Expires)  owner=$($entry.Owner)"
            )
        }
        # Not yet expired: still within grace period — pass silently
    } else {
        $violations.Add("  $script  (tag=$($c.Tag))")
    }
}

$failed = $false

if ($violations.Count -gt 0) {
    Write-Host ''
    Write-Host 'ERROR: Disallowed custom-script E2E entries detected.' -ForegroundColor Red
    Write-Host "  Manifest : $Manifest" -ForegroundColor Red
    Write-Host '  Policy   : Runner=''custom'' and Script= are banned.'  -ForegroundColor Red
    Write-Host '  Fix      : use built-in runners (cpp, rust, equiv).'   -ForegroundColor Red
    Write-Host '             If a capability is missing, extend the runner rather than'  -ForegroundColor Red
    Write-Host '             adding script glue.'                         -ForegroundColor Red
    Write-Host '  Temporary: add an entry to the allowlist with Owner + Expires.'        -ForegroundColor Red
    Write-Host "             $Allowlist"                                  -ForegroundColor Red
    Write-Host ''
    Write-Host 'Violations:' -ForegroundColor Red
    $violations | ForEach-Object { Write-Host $_ -ForegroundColor Red }
    $failed = $true
}

if ($expiredList.Count -gt 0) {
    Write-Host ''
    Write-Host 'ERROR: Expired allowlist entries must be migrated or renewed.' -ForegroundColor Red
    Write-Host "  Allowlist: $Allowlist" -ForegroundColor Red
    Write-Host '  Action   : migrate to a built-in runner, or update Expires and file an issue.' -ForegroundColor Red
    Write-Host ''
    Write-Host 'Expired entries:' -ForegroundColor Red
    $expiredList | ForEach-Object { Write-Host $_ -ForegroundColor Red }
    $failed = $true
}

if (-not $failed) {
    $n = $allowedScripts.Count
    Write-Host "check-no-custom-runners: OK  (0 violations; $n temporarily allowlisted)" -ForegroundColor Green
}

exit ([int]$failed)
