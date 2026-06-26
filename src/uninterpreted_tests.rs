//! Tests for the uninterpreted-primitive declaration + emission paths.

use super::*;

#[test]
fn parse_block_annotation_without_symbol() {
    let src = "\
/** @uninterpreted */
hmacSha256 : [32][8] -> [n][8] -> [32][8]
";
    let got = parse_annotations(src);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].cryptol_fn, "hmacSha256");
    assert_eq!(got[0].symbol, None);
}

#[test]
fn parse_block_annotation_with_symbol() {
    let src = "\
/** @uninterpreted symbol=\"?HmacSha256@@YA_KXZ\" */
hmacSha256 : [32][8] -> [n][8] -> [32][8]
";
    let got = parse_annotations(src);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].symbol.as_deref(), Some("?HmacSha256@@YA_KXZ"));
}

#[test]
fn parse_line_comment_annotation() {
    let src = "\
// @uninterpreted
sha256 : [n][8] -> [32][8]
";
    let got = parse_annotations(src);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].cryptol_fn, "sha256");
}

#[test]
fn marker_without_following_signature_is_ignored() {
    let src = "\
/** @uninterpreted */

x = 3
";
    assert!(parse_annotations(src).is_empty());
}

#[test]
fn symbol_on_continuation_comment_line() {
    let src = "\
/** @uninterpreted
 *  symbol=\"hmac_impl\"
 */
hmacSha256 : [32][8] -> [n][8] -> [32][8]
";
    let got = parse_annotations(src);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].symbol.as_deref(), Some("hmac_impl"));
}

#[test]
fn gather_config_overrides_annotation_symbol() {
    let dir = std::env::temp_dir().join(format!("uninterp_gather_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cry = dir.join("spec.cry");
    std::fs::write(
        &cry,
        "/** @uninterpreted */\nhmacSha256 : [32][8] -> [n][8] -> [32][8]\n",
    )
    .unwrap();

    let cfg = vec![UninterpretedEntry {
        cryptol_fn: "hmacSha256".to_string(),
        symbol: Some("explicit_sym".to_string()),
    }];
    let merged = gather(&cry, &cfg);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].symbol.as_deref(), Some("explicit_sym"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn gather_appends_config_only_entry() {
    let dir = std::env::temp_dir().join(format!("uninterp_append_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cry = dir.join("spec.cry");
    std::fs::write(&cry, "foo : [8] -> [8]\n").unwrap();

    let cfg = vec![UninterpretedEntry {
        cryptol_fn: "aead".to_string(),
        symbol: None,
    }];
    let merged = gather(&cry, &cfg);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].cryptol_fn, "aead");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ct_compare_recognition() {
    assert!(is_ct_compare_symbol("subtle::ct_eq"));
    assert!(is_ct_compare_symbol("CRYPTO_memcmp"));
    assert!(is_ct_compare_symbol("foo_ConstantTimeEq_bar"));
    assert!(!is_ct_compare_symbol("hmacSha256"));
}

#[test]
fn emit_scalar_in_scalar_out() {
    let dir = std::env::temp_dir().join(format!("uninterp_scalar_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cry = dir.join("spec.cry");
    std::fs::write(&cry, "mix : [32] -> [32] -> [32]\n").unwrap();

    let entries = vec![UninterpretedEntry {
        cryptol_fn: "mix".to_string(),
        symbol: Some("mix_impl".to_string()),
    }];
    let block = emit_uninterpreted_block(&entries, &cry);
    assert_eq!(block.override_names, vec!["ov_uninterp_mix".to_string()]);
    let s = &block.snippet;
    assert!(s.contains("let mix_uninterp_spec = do {"));
    assert!(s.contains("a0 <- llvm_fresh_var \"a0\" (llvm_int 32);"));
    assert!(s.contains("a1 <- llvm_fresh_var \"a1\" (llvm_int 32);"));
    assert!(s.contains("llvm_execute_func [llvm_term a0, llvm_term a1];"));
    assert!(s.contains("llvm_return (llvm_term {{ mix a0 a1 }});"));
    assert!(
        s.contains("ov_uninterp_mix <- llvm_unsafe_assume_spec m \"mix_impl\" mix_uninterp_spec;")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn emit_byte_buffers_and_sret_return() {
    let dir = std::env::temp_dir().join(format!("uninterp_hmac_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cry = dir.join("spec.cry");
    std::fs::write(&cry, "hmacSha256 : [32][8] -> [16][8] -> [32][8]\n").unwrap();

    let entries = vec![UninterpretedEntry {
        cryptol_fn: "hmacSha256".to_string(),
        symbol: None,
    }];
    let block = emit_uninterpreted_block(&entries, &cry);
    let s = &block.snippet;
    // Aggregate return → sret pointer allocated and threaded first.
    assert!(s.contains("result_ptr <- llvm_alloc (llvm_array 32 (llvm_int 8));"));
    // Byte-array params passed by readonly pointer with points_to.
    assert!(s.contains("a0_ptr <- llvm_alloc_readonly (llvm_array 32 (llvm_int 8));"));
    assert!(s.contains("llvm_points_to a0_ptr (llvm_term a0);"));
    assert!(s.contains("a1_ptr <- llvm_alloc_readonly (llvm_array 16 (llvm_int 8));"));
    assert!(s.contains("llvm_execute_func [result_ptr, a0_ptr, a1_ptr];"));
    assert!(s.contains("llvm_points_to result_ptr (llvm_term {{ hmacSha256 a0 a1 }});"));
    // No explicit llvm_return for sret.
    assert!(!s.contains("llvm_return"));
    // Symbol defaults to the cryptol fn name.
    assert!(s.contains("llvm_unsafe_assume_spec m \"hmacSha256\""));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn emit_skips_unparseable_signature() {
    let dir = std::env::temp_dir().join(format!("uninterp_skip_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cry = dir.join("spec.cry");
    std::fs::write(&cry, "// nothing here\n").unwrap();

    let entries = vec![UninterpretedEntry {
        cryptol_fn: "missing".to_string(),
        symbol: None,
    }];
    let block = emit_uninterpreted_block(&entries, &cry);
    assert!(block.is_empty());
    assert!(block
        .snippet
        .contains("could not parse Cryptol signature for `missing`"));
    std::fs::remove_dir_all(&dir).ok();
}
