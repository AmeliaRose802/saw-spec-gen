# Run all Rust havoc-spec demos and report VERIFIED/DISPROVED for each.
# Parallel to scripts/run_havoc_demos.ps1 for the C++ side.
$ErrorActionPreference = 'Continue'

$cases = @(
    @{ Dir = 'nothing_sketchy';           File = 'add_one_sat.rs';   Expected = 'VERIFIED'  }
    @{ Dir = 'nothing_sketchy';           File = 'add_one_unsat.rs'; Expected = 'DISPROVED' }
    @{ Dir = 'concrete_type_safe';        File = 'add_one_sat.rs';   Expected = 'VERIFIED'  }
    @{ Dir = 'concrete_type_safe';        File = 'add_one_unsat.rs'; Expected = 'DISPROVED' }
    @{ Dir = 'class_member_clobbered';    File = 'add_one_sat.rs';   Expected = 'VERIFIED'  }
    @{ Dir = 'class_member_clobbered';    File = 'add_one_unsat.rs'; Expected = 'DISPROVED' }
    @{ Dir = 'global_memory_clobbered';   File = 'add_one_sat.rs';   Expected = 'VERIFIED'  }
    @{ Dir = 'global_memory_clobbered';   File = 'add_one_unsat.rs'; Expected = 'DISPROVED' }
    @{ Dir = 'input_param_modified';      File = 'add_one_sat.rs';   Expected = 'VERIFIED'  }
    @{ Dir = 'input_param_modified';      File = 'add_one_unsat.rs'; Expected = 'DISPROVED' }
    @{ Dir = 'pointer_aliasing';          File = 'add_one_sat.rs';   Expected = 'VERIFIED'  }
    @{ Dir = 'trait_static_dispatch';     File = 'add_one_sat.rs';   Expected = 'VERIFIED'  }
    @{ Dir = 'trait_static_dispatch';     File = 'add_one_unsat.rs'; Expected = 'DISPROVED' }
    @{ Dir = 'trait_dynamic_dispatch';    File = 'add_one_sat.rs';   Expected = 'VERIFIED'  }
    @{ Dir = 'trait_dynamic_dispatch';    File = 'add_one_unsat.rs'; Expected = 'DISPROVED' }
    @{ Dir = 'trait_external_crate';      File = 'add_one_sat.rs';   Expected = 'VERIFIED'  }
    @{ Dir = 'trait_external_crate';      File = 'add_one_unsat.rs'; Expected = 'DISPROVED' }
)

foreach ($c in $cases) {
    $rs  = "demo/rust_equalivence_demo/$($c.Dir)/$($c.File)"
    $cry = "demo/rust_equalivence_demo/$($c.Dir)/add_one_spec.cry"
    $baseName = [System.IO.Path]::GetFileNameWithoutExtension($c.File)
    $out = "demo/rust_equalivence_demo/$($c.Dir)/out_rust_$baseName"
    if (Test-Path $out) { Remove-Item -Recurse -Force $out }

    $output = & .\verify-rust.ps1 -RustFile $rs -CryptolSpec $cry -CryptolFn add_one_spec -Function add_one *>&1 | Out-String
    $match  = [regex]::Match($output, 'RESULT:\s*(\S+)')
    $result = if ($match.Success) { $match.Groups[1].Value } else { 'UNKNOWN' }
    $tag    = if ($result -eq $c.Expected) { 'OK ' } else { 'BAD' }
    Write-Output "$tag  $($c.Dir)/$($c.File)  expected=$($c.Expected)  got=$result"
}

# ── Unknown-concrete-impl dyn-trait case ──────────────────────────────
# Uses its own runner (vtable-stub + llvm-link + assume_spec on the
# trait method).  Function name and Cryptol spec name differ from the
# add_one/add_one_spec template above.
$unknownCases = @(
    @{ File = 'add_step_sat.rs';   Expected = 'VERIFIED'  }
    @{ File = 'add_step_unsat.rs'; Expected = 'DISPROVED' }
)
foreach ($c in $unknownCases) {
    $rs = "demo/rust_equalivence_demo/trait_unknown_impl/$($c.File)"
    $output = & demo\rust_equalivence_demo\trait_unknown_impl\run_unknown_impl.ps1 `
        -RustFile $rs -ExpectedResult $c.Expected *>&1 | Out-String
    $match  = [regex]::Match($output, 'RESULT:\s*(\S+)')
    $result = if ($match.Success) { $match.Groups[1].Value } else { 'UNKNOWN' }
    $tag    = if ($result -eq $c.Expected) { 'OK ' } else { 'BAD' }
    Write-Output "$tag  trait_unknown_impl/$($c.File)  expected=$($c.Expected)  got=$result"
}
