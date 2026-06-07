<#
.SYNOPSIS
    Shared-artifact cache for verify.ps1 Steps 1-2.5 (compile + AST dump + filter).

.DESCRIPTION
    Steps 1-2.5 (bitcode + the multi-hundred-MB clang AST dump + filter) depend
    only on the C++ source, its headers, and the clang flags — NOT on which
    Cryptol function we verify. When pretty-specs/pipeline.ps1 drives verify.ps1
    once per top-level function (e.g. 21x for SDEP), re-dumping the identical AST
    each time dominates wall-clock. These helpers cache the post-patch .bc/.ll and
    the filtered AST keyed on (source+header mtimes, flags, target) and reuse them
    for every function after the first. Cache miss => identical behaviour to
    before, so this is fully backward compatible.
#>

# Compute the cache key/paths and, on a hit, copy cached artifacts into place.
# Returns a context object consumed by Save-AstCache. `.Hit` is $true when the
# caller can skip Steps 1-2.5; `.LlFile` reflects the (possibly nulled) .ll path.
function Get-AstCacheContext {
    param(
        [string]$CppFile,
        [string[]]$IncludeDirs,
        [string[]]$UserClangFlags,
        [string]$LlvmTarget,
        [string]$OutputDir,
        [string]$BaseName,
        [string]$BcFile,
        [string]$LlFile,
        [string]$AstFile
    )
    $ctx = [ordered]@{ Hit = $false; Dir = $null; Bc = $null; Ll = $null; Ast = $null; LlFile = $LlFile }
    try {
        $newest = (Get-Item $CppFile).LastWriteTimeUtc
        foreach ($d in $IncludeDirs) {
            $rp = Resolve-Path $d -ErrorAction SilentlyContinue
            if ($rp) {
                Get-ChildItem $rp.Path -Recurse -File -ErrorAction SilentlyContinue | ForEach-Object {
                    if ($_.LastWriteTimeUtc -gt $newest) { $newest = $_.LastWriteTimeUtc }
                }
            }
        }
        $keyString = @("$CppFile", ($UserClangFlags -join ' '), $LlvmTarget, $newest.Ticks) -join '|'
        $md5  = [System.Security.Cryptography.MD5]::Create()
        $hash = ([System.BitConverter]::ToString($md5.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($keyString)))).Replace('-', '').Substring(0, 16)
        $ctx.Dir = Join-Path (Join-Path (Split-Path $OutputDir -Parent) '.astcache') $hash
        $ctx.Bc  = Join-Path $ctx.Dir "$BaseName.bc"
        $ctx.Ll  = Join-Path $ctx.Dir "$BaseName.ll"
        $ctx.Ast = Join-Path $ctx.Dir "${BaseName}_ast.json"
        if ((Test-Path $ctx.Bc) -and (Test-Path $ctx.Ast)) {
            Copy-Item $ctx.Bc $BcFile -Force
            if (Test-Path $ctx.Ll) { Copy-Item $ctx.Ll $LlFile -Force } else { $ctx.LlFile = $null }
            Copy-Item $ctx.Ast $AstFile -Force
            $ctx.Hit = $true
            Write-Host ""
            Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
            Write-Host " Steps 1-2.5: reusing cached bitcode + AST ($hash)" -ForegroundColor Green
            Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
            Write-Host "  → $AstFile (from cache)" -ForegroundColor Green
        }
    } catch {
        Write-Host "  (AST cache disabled: $($_.Exception.Message))" -ForegroundColor DarkYellow
    }
    return $ctx
}

# Populate the shared cache so the next function reuses these artifacts. Failures
# are non-fatal: a cache that can't be written just means the next run recomputes.
function Save-AstCache {
    param(
        $Ctx,
        [string]$BcFile,
        [string]$LlFile,
        [string]$AstFile
    )
    if (-not $Ctx -or -not $Ctx.Dir) { return }
    try {
        New-Item -ItemType Directory -Path $Ctx.Dir -Force | Out-Null
        Copy-Item $BcFile $Ctx.Bc -Force
        if ($LlFile) { Copy-Item $LlFile $Ctx.Ll -Force }
        Copy-Item $AstFile $Ctx.Ast -Force
    } catch {
        Write-Host "  (failed to populate AST cache: $($_.Exception.Message))" -ForegroundColor DarkYellow
    }
}
