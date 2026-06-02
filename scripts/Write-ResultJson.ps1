<#
.SYNOPSIS
    Shared writer for the per-run result.json files emitted by
    verify.ps1 / verify-rust.ps1 / verify-equiv.ps1.

.DESCRIPTION
    A single source of truth for the result.json shape so all three
    wrappers stay in sync.  The on-disk schema is versioned via the
    `schema_version` field (currently `"1"`).  See `docs/result-json.md`
    for the full specification.

    Dot-source from a verify script then call:

        . (Join-Path $ScriptRoot 'scripts/Write-ResultJson.ps1')
        Write-VerifyResult `
            -OutputDir $OutputDir `
            -Side      'cpp' `
            -Function  $Function `
            -CryptolFn $CryptolFn `
            -Verdict   'VERIFIED'

    Optional parameters (`-Counterexample`, `-Expected`, `-Actual`,
    `-Solver`, `-TimeSecs`, `-ImplFile`) populate the matching JSON
    fields.  Omitted optional parameters are written as `null` (or as
    an empty array for `counterexample`) so downstream consumers always
    see the full keyset.

.PARAMETER OutputDir
    Directory the verify script writes its artifacts into.  The file
    name is always `result.json`.

.PARAMETER Side
    Which wrapper produced the file: 'cpp', 'rust', or 'equiv'.

.PARAMETER Verdict
    One of: VERIFIED, DISPROVED, UNKNOWN, EQUIVALENT, NOT EQUIVALENT.

.PARAMETER Counterexample
    Array of @{ Name=...; Value=... } pairs (or PSCustomObjects with
    those properties).  Optional Bits property is preserved when
    present.  Empty when verdict is VERIFIED.
#>

# Bumped whenever the on-disk shape changes in a way collect-results /
# pretty-specs adapters must learn about.  Kept as a string to keep
# JSON.parse semantics stable across JS / Python / Rust consumers.
$Script:SawSpecGenResultSchemaVersion = '1'

function Write-VerifyResult {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$OutputDir,
        [Parameter(Mandatory)]
        [ValidateSet('cpp', 'rust', 'equiv')]
        [string]$Side,
        [Parameter(Mandatory)][string]$Function,
        [Parameter(Mandatory)][string]$CryptolFn,
        [Parameter(Mandatory)]
        [ValidateSet('VERIFIED', 'DISPROVED', 'UNKNOWN', 'EQUIVALENT', 'NOT EQUIVALENT')]
        [string]$Verdict,
        [object[]]$Counterexample = @(),
        [string]$Expected,
        [string]$Actual,
        [string]$Solver,
        [double]$TimeSecs,
        [string]$ImplFile
    )

    # Normalise the counterexample so every entry has Name + Value (as
    # strings) and an optional Bits field.  Accepts plain hashtables
    # OR PSCustomObjects.  Downstream JSON contract is documented in
    # docs/result-json.md.
    $cexNormalised = @()
    if ($Counterexample) {
        foreach ($entry in $Counterexample) {
            if (-not $entry) { continue }
            $name = $null; $value = $null; $bits = $null
            if ($entry -is [hashtable]) {
                $name  = $entry['Name']
                $value = $entry['Value']
                $bits  = $entry['Bits']
            } else {
                $name  = $entry.Name
                $value = $entry.Value
                if ($entry.PSObject.Properties.Match('Bits').Count -gt 0) {
                    $bits = $entry.Bits
                }
            }
            if ($null -eq $name) { continue }
            $obj = [ordered]@{
                name  = [string]$name
                value = if ($null -ne $value) { [string]$value } else { $null }
            }
            if ($null -ne $bits) { $obj['bits'] = [int]$bits }
            $cexNormalised += [PSCustomObject]$obj
        }
    }

    $payload = [ordered]@{
        schema_version = $Script:SawSpecGenResultSchemaVersion
        side           = $Side
        function       = $Function
        cryptol_fn     = $CryptolFn
        verdict        = $Verdict
        counterexample = $cexNormalised
        expected       = if ($PSBoundParameters.ContainsKey('Expected')) { $Expected } else { $null }
        actual         = if ($PSBoundParameters.ContainsKey('Actual'))   { $Actual }   else { $null }
        solver         = if ($PSBoundParameters.ContainsKey('Solver'))   { $Solver }   else { $null }
        time_secs      = if ($PSBoundParameters.ContainsKey('TimeSecs')) { $TimeSecs } else { $null }
        impl_file      = if ($PSBoundParameters.ContainsKey('ImplFile')) { $ImplFile } else { $null }
    }

    $resultPath = Join-Path $OutputDir 'result.json'
    [PSCustomObject]$payload |
        ConvertTo-Json -Depth 6 |
        Set-Content -Path $resultPath -Encoding utf8
}
