//! Unit tests for [`super`] ([`crate::project_config`]). Extracted via
//! `#[path]` include to keep `project_config.rs` under the 500-line limit.

use super::*;

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn per_function_table_parses_from_toml() {
    let cfg: ProjectConfig = toml::from_str(
        r#"
            in_buffer_size = ["g=1"]

            [functions.canonicalize_lp]
            in_buffer_size   = ["m=4", "b=4"]
            out_buffer_param = ["out=10"]
            cryptol_fn_out   = ["out=canonicalize_lp_post"]
            max_len_precond  = ["nm=4", "nb=4"]
            "#,
    )
    .expect("config parses");

    let f = cfg.functions.get("canonicalize_lp").expect("table present");
    assert_eq!(f.in_buffer_size, v(&["m=4", "b=4"]));
    assert_eq!(f.out_buffer_param, v(&["out=10"]));
    assert_eq!(f.cryptol_fn_out, v(&["out=canonicalize_lp_post"]));
    assert_eq!(f.max_len_precond, v(&["nm=4", "nb=4"]));
}

#[test]
fn apply_concatenates_per_function_then_global() {
    let mut cfg = ProjectConfig {
        in_buffer_size: v(&["global=2"]),
        ..Default::default()
    };
    cfg.functions.insert(
        "canonicalize_lp".to_string(),
        FunctionConfig {
            in_buffer_size: v(&["m=4", "b=4"]),
            out_buffer_param: v(&["out=10"]),
            cryptol_fn_out: v(&["out=canonicalize_lp_post"]),
            ..Default::default()
        },
    );

    let merged = cfg.apply("canonicalize_lp");

    // per-function, then global.
    assert_eq!(merged.in_buffer_size, v(&["m=4", "b=4", "global=2"]));
    assert_eq!(merged.out_buffer_param, v(&["out=10"]));
    assert_eq!(merged.cryptol_fn_out, v(&["out=canonicalize_lp_post"]));
}

#[test]
fn apply_falls_back_to_global_when_function_absent() {
    let cfg = ProjectConfig {
        out_buffer_param: v(&["g=1"]),
        ..Default::default()
    };
    let merged = cfg.apply("no_such_fn");
    assert_eq!(merged.out_buffer_param, v(&["g=1"]));
}

#[test]
fn sret_assert_bytes_parses_and_per_function_wins() {
    let cfg: ProjectConfig = toml::from_str(
        r#"
            sret_assert_bytes = 8

            [functions.provision_ret]
            sret_assert_bytes = 65
            "#,
    )
    .expect("config parses");
    // Per-function value wins over the global default.
    assert_eq!(cfg.apply("provision_ret").sret_assert_bytes, Some(65));
    // A function with no table falls back to the global value.
    assert_eq!(cfg.apply("other").sret_assert_bytes, Some(8));
}

#[test]
fn preconditions_parse_and_merge_per_function_then_global() {
    let cfg: ProjectConfig = toml::from_str(
        r#"
            preconditions = ["global_pred"]

            [functions.activate_ret]
            preconditions = ["(this_pre @ 128) <= 1", "(this_pre @ 144) <= 1"]
            "#,
    )
    .expect("config parses");
    let merged = cfg.apply("activate_ret");
    assert_eq!(
        merged.preconditions,
        v(&[
            "(this_pre @ 128) <= 1",
            "(this_pre @ 144) <= 1",
            "global_pred"
        ]),
    );
    // A function with no table still inherits the global precondition.
    assert_eq!(cfg.apply("other").preconditions, v(&["global_pred"]));
}

#[test]
fn compose_parses_and_lowers_to_uninterpreted() {
    let cfg: ProjectConfig = toml::from_str(
        r#"
            [functions.double_plus_one_spec]
            compose = [{ cryptol_fn = "double_it_spec", symbol = "double_it" }]
            combine_scope = "callgraph"
            "#,
    )
    .expect("config parses");
    let merged = cfg.apply("double_plus_one_spec");
    assert_eq!(merged.combine_scope.as_deref(), Some("callgraph"));
    assert_eq!(merged.compose.len(), 1);
    let ov = merged.compose[0].to_uninterpreted();
    assert_eq!(ov.cryptol_fn, "double_it_spec");
    assert_eq!(ov.resolved_symbol(), "double_it");
    // A function with no table gets no compose entries.
    assert!(cfg.apply("other").compose.is_empty());
}

#[test]
fn compose_symbol_defaults_to_function_then_cryptol_fn() {
    let by_function = ComposeEntry {
        cryptol_fn: "f_spec".into(),
        function: Some("f".into()),
        ..Default::default()
    };
    assert_eq!(by_function.resolved_symbol(), "f");
    let by_name = ComposeEntry {
        cryptol_fn: "g_spec".into(),
        ..Default::default()
    };
    assert_eq!(by_name.resolved_symbol(), "g_spec");
}

#[test]
fn contract_ensures_desugars_to_projected_cryptol_fn_out() {
    let cfg: ProjectConfig = toml::from_str(
        r#"
            [functions.bump]
            out_buffer_param = ["out=i32"]
            contract_return  = "ret"
            contract_ensures = ["out=outPost"]
            "#,
    )
    .expect("config parses");
    let merged = cfg.apply("bump");
    assert_eq!(merged.contract_return.as_deref(), Some("ret"));
    let outs = merged.cryptol_fn_out_with_contract("bump").unwrap();
    // `bump` is the contract; the ensures entry projects `.outPost`.
    assert_eq!(outs, v(&["out=bump.outPost"]));
}

#[test]
fn boolean_true_from_any_layer_wins() {
    let mut cfg = ProjectConfig::default();
    cfg.functions.insert(
        "f".to_string(),
        FunctionConfig {
            spec_only_on_missing: Some(true),
            ..Default::default()
        },
    );
    // per-function true, no global.
    let merged = cfg.apply("f");
    assert!(merged.spec_only_on_missing);

    // global true reaches an unrelated function.
    let cfg2 = ProjectConfig {
        use_llvm_combine_modules: Some(true),
        ..Default::default()
    };
    let merged2 = cfg2.apply("other");
    assert!(merged2.use_llvm_combine_modules);
}
