# Run all the existing havoc-spec demos and report SAT/UNSAT for each.
$ErrorActionPreference = 'Continue'
$cases = @(
    @{ Dir = 'nothing_sketchy'; File = 'add_one_sat.cpp'; Expected = 'SAT' }
    @{ Dir = 'nothing_sketchy'; File = 'add_one_unsat.cpp'; Expected = 'UNSAT' }
    # Compositional gen-verify havocs all direct callees, so OkLog::log is
    # overridden adversarially and may clobber super_important — hence UNSAT.
    @{ Dir = 'concrete_type_safe'; File = 'add_one_sat.cpp'; Expected = 'UNSAT' }
    @{ Dir = 'concrete_type_safe'; File = 'add_one_unsat.cpp'; Expected = 'UNSAT' }
    @{ Dir = 'class_member_clobbered'; File = 'add_one_sat.cpp'; Expected = 'SAT' }
    @{ Dir = 'class_member_clobbered'; File = 'add_one_unsat.cpp'; Expected = 'UNSAT' }
    @{ Dir = 'global_memory_clobbered'; File = 'add_one_sat.cpp'; Expected = 'SAT' }
    @{ Dir = 'global_memory_clobbered'; File = 'add_one_unsat.cpp'; Expected = 'UNSAT' }
    @{ Dir = 'input_param_modified'; File = 'add_one_sat.cpp'; Expected = 'SAT' }
    @{ Dir = 'input_param_modified'; File = 'add_one_unsat.cpp'; Expected = 'UNSAT' }
    @{ Dir = 'output_param_uninitialized'; File = 'add_one_sat.cpp'; Expected = 'SAT' }
    @{ Dir = 'output_param_uninitialized'; File = 'add_one_unsat.cpp'; Expected = 'UNSAT' }
    @{ Dir = 'pointer_aliasing'; File = 'add_one_sat.cpp'; Expected = 'SAT' }
    @{ Dir = 'pointer_aliasing'; File = 'add_one_unsat.cpp'; Expected = 'UNSAT' }
    @{ Dir = 'adversarial_holes'; File = 'add_one_false_unsat_ctor_stub.cpp'; Expected = 'SAT' }
    @{ Dir = 'adversarial_holes'; File = 'add_one_false_sat_mutable.cpp'; Expected = 'UNSAT' }
    @{ Dir = 'multi_method_ordering'; File = 'call_validate_sat.cpp'; Expected = 'SAT' }
    @{ Dir = 'multi_method_ordering'; File = 'call_audit_sat.cpp'; Expected = 'SAT' }
    @{ Dir = 'multi_method_ordering'; File = 'call_prepare_unsat.cpp'; Expected = 'UNSAT' }
    @{ Dir = 'multi_method_ordering'; File = 'call_report_sat.cpp'; Expected = 'SAT' }
    @{ Dir = 'multi_method_ordering'; File = 'call_extra_unsat.cpp'; Expected = 'UNSAT' }
)
foreach ($c in $cases) {
    $cpp = "demo/vtable_havoc_spec_demos/$($c.Dir)/$($c.File)"
    $cry = "demo/vtable_havoc_spec_demos/$($c.Dir)/add_one_spec.cry"
    $baseName = [System.IO.Path]::GetFileNameWithoutExtension($c.File)
    $out = "demo/vtable_havoc_spec_demos/$($c.Dir)/out_$baseName"
    if (Test-Path $out) { Remove-Item -Recurse -Force $out }
    $output = & .\verify.ps1 -CppFile $cpp -CryptolSpec $cry -CryptolFn add_one_spec -Function add_one *>&1 | Out-String
    $match = [regex]::Match($output, 'RESULT:\s*(\S+)')
    $result = if ($match.Success) { $match.Groups[1].Value } else { 'UNKNOWN' }
    $tag = if ($result -eq $c.Expected) { 'OK ' } else { 'BAD' }
    Write-Output "$tag  $($c.Dir)/$($c.File)  expected=$($c.Expected)  got=$result"
}
