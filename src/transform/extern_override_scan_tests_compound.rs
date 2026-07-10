//! Tests for compound LLVM return-type parsing extracted from the parent
//! test module to keep `extern_override_scan_tests.rs` under the 500
//! NWS-line limit.

use super::*;

#[test]
fn scan_parses_compound_array_return_type() {
    // `[16 x i8]` contains spaces and must be kept as a single token.
    // If `extract_return_type_token` uses plain `split_whitespace` it would
    // return the last fragment `"i8]"` and the override emitter would fall
    // back to an opaque pointer return instead of the correct llvm_array.
    let ir = r#"
declare [16 x i8] @"?_Get_buf@str@@QEBA?AU_Bxty@@XZ"(ptr noundef %0)

define i32 @target(ptr %s) {
  %1 = call [16 x i8] @"?_Get_buf@str@@QEBA?AU_Bxty@@XZ"(ptr %s)
  ret i32 0
}
"#;
    let targets = scan(ir, "target");
    let buf = targets
        .iter()
        .find(|t| t.symbol == "?_Get_buf@str@@QEBA?AU_Bxty@@XZ")
        .expect("_Get_buf must be in override set");
    assert_eq!(
        buf.return_ir_type, "[16 x i8]",
        "compound array return type must be parsed as a single token",
    );
}
