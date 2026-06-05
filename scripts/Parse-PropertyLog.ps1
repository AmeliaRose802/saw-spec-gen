<#
.SYNOPSIS
    Turn a SAW log (with BEGIN_PROOF / PROVED markers) into one
    schema-1 result.json per property.

.DESCRIPTION
    Reads a captured SAW stdout/stderr log and walks the
    `BEGIN_PROOF <name>` / `PROVED <name>` marker pairs documented in
    `docs/proof-markers.md`.  For each marker pair it writes:

        <OutputDir>/properties/<name>/result.json

    using the shared `Write-VerifyResult` helper, so the on-disk shape
    is byte-identical to what verify.ps1 produces from a single-proof
    run.  `saw-spec-gen collect-results` aggregates the per-property
    files transparently.

    A `BEGIN_PROOF` with no matching `PROVED` is treated as a failure:
    the log lines between the BEGIN_PROOF and the next BEGIN_PROOF (or
    EOF) are preserved verbatim as a `{ name: "_evidence", value: ... }`
    counterexample entry so the failure context survives aggregation.

.PARAMETER LogPath
    Path to the SAW log file.  Pass `-` (or omit and pipe) to read
    from stdin.

.PARAMETER OutputDir
    Root directory under which `properties/<name>/result.json` files
    are written.  Created if missing.

.PARAMETER Side
    Value to record in the `side` field (default 'cpp').

.PARAMETER CryptolFn
    Cryptol function the property was checked against.  Defaults to
    the marker name when omitted (callers with a real mapping should
    pass `-CryptolFnMap @{ foo = 'foo_spec' }`).

.PARAMETER CryptolFnMap
    Hashtable of `<property name> -> <cryptol fn>` overrides.  Wins
    over `-CryptolFn` when the property name appears as a key.

.PARAMETER Solver
    Solver to record (default 'z3').

.PARAMETER ImplFile
    Implementation source file basename to record (optional).

.PARAMETER PassThru
    Emit a PSCustomObject summary per parsed property to the pipeline
    in addition to writing files.  Useful for tests.

.EXAMPLE
    pwsh scripts/Parse-PropertyLog.ps1 -LogPath out/saw.log -OutputDir out

    Writes out/properties/<name>/result.json for every BEGIN_PROOF in
    the log.
#>
[CmdletBinding()]
param(
    [Parameter(ValueFromPipeline = $true, Position = 0)]
    [string]$LogPath = '-',

    [Parameter(Mandatory)]
    [string]$OutputDir,

    [ValidateSet('cpp', 'rust', 'equiv')]
    [string]$Side = 'cpp',

    [string]$CryptolFn,

    [hashtable]$CryptolFnMap,

    [string]$Solver = 'z3',

    [string]$ImplFile,

    [switch]$PassThru
)

$ErrorActionPreference = 'Stop'

# Reuse the shared writer so the on-disk shape stays in lock-step
# with verify.ps1 / verify-rust.ps1.  Resolving relative to *this*
# script lets the parser be invoked from any cwd.
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $here 'Write-ResultJson.ps1')

# Markers are emitted by SAW's `print` command on their own lines.
# We tolerate trailing CR (Windows-captured logs) and optional
# surrounding whitespace.
$beginRegex  = '^[\s]*BEGIN_PROOF\s+(\S+)\s*$'
$provedRegex = '^[\s]*PROVED\s+(\S+)\s*$'

function Read-LogLines {
    param([string]$Path)
    if ($Path -eq '-' -or [string]::IsNullOrEmpty($Path)) {
        return @($input)
    }
    if (-not (Test-Path -LiteralPath $Path)) {
        throw "log not found: $Path"
    }
    return Get-Content -LiteralPath $Path
}

function Resolve-CryptolFn {
    param([string]$Name)
    if ($CryptolFnMap -and $CryptolFnMap.ContainsKey($Name)) {
        return [string]$CryptolFnMap[$Name]
    }
    if ($CryptolFn) { return $CryptolFn }
    return $Name
}

function Emit-Property {
    param(
        [string]$Name,
        [string]$Verdict,
        [string[]]$Evidence
    )

    $propDir = Join-Path $OutputDir (Join-Path 'properties' $Name)
    if (-not (Test-Path -LiteralPath $propDir)) {
        New-Item -ItemType Directory -Force -Path $propDir | Out-Null
    }

    $cex = @()
    if ($Verdict -eq 'DISPROVED' -and $Evidence -and $Evidence.Count -gt 0) {
        $cex = @([PSCustomObject]@{
            Name  = '_evidence'
            Value = ($Evidence -join "`n")
        })
    }

    $cryFn = Resolve-CryptolFn -Name $Name

    $params = @{
        OutputDir      = $propDir
        Side           = $Side
        Function       = $Name
        CryptolFn      = $cryFn
        Verdict        = $Verdict
        Counterexample = $cex
        Solver         = $Solver
    }
    if ($ImplFile) { $params['ImplFile'] = $ImplFile }

    Write-VerifyResult @params

    if ($PassThru) {
        [PSCustomObject]@{
            Name       = $Name
            Verdict    = $Verdict
            ResultPath = Join-Path $propDir 'result.json'
            Evidence   = $Evidence
        }
    }
}

if (-not (Test-Path -LiteralPath $OutputDir)) {
    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
}

$lines = Read-LogLines -Path $LogPath

# Walk the log once, threading state across lines:
#   $openName     = property whose BEGIN_PROOF we last saw and which
#                   has not yet been closed by a PROVED marker.
#   $openEvidence = lines accumulated since BEGIN_PROOF (used as the
#                   counterexample evidence on failure).
#   $emitted      = set of names we've already written a result.json
#                   for, so a duplicate BEGIN_PROOF (rare, but
#                   defensive) doesn't silently overwrite.
$openName     = $null
$openEvidence = New-Object System.Collections.Generic.List[string]
$emitted      = New-Object System.Collections.Generic.HashSet[string]

foreach ($rawLine in $lines) {
    $line = if ($null -eq $rawLine) { '' } else { [string]$rawLine }

    if ($line -match $beginRegex) {
        $name = $Matches[1]
        # If we had an open property, the new BEGIN_PROOF means the
        # previous one never completed → failure.
        if ($openName) {
            if (-not $emitted.Contains($openName)) {
                Emit-Property -Name $openName -Verdict 'DISPROVED' -Evidence $openEvidence.ToArray()
                [void]$emitted.Add($openName)
            }
        }
        $openName = $name
        $openEvidence.Clear()
        continue
    }

    if ($line -match $provedRegex) {
        $name = $Matches[1]
        if ($openName -ne $name) {
            Write-Warning "PROVED $name has no matching BEGIN_PROOF (open=$openName)"
        }
        if (-not $emitted.Contains($name)) {
            Emit-Property -Name $name -Verdict 'VERIFIED' -Evidence @()
            [void]$emitted.Add($name)
        }
        $openName = $null
        $openEvidence.Clear()
        continue
    }

    if ($openName) {
        # Bound evidence so a runaway log doesn't blow up memory.
        if ($openEvidence.Count -lt 512) {
            $openEvidence.Add($line)
        }
    }
}

# EOF with a still-open BEGIN_PROOF → that property failed.
if ($openName -and -not $emitted.Contains($openName)) {
    Emit-Property -Name $openName -Verdict 'DISPROVED' -Evidence $openEvidence.ToArray()
    [void]$emitted.Add($openName)
}
