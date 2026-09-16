use super::*;

#[test]
fn fixture_plans_use_compiler_fields_on_both_64_bit_abis() {
    let Some(clang) = clang() else { return };
    for target in TARGETS {
        for (index, case) in CASES.into_iter().enumerate() {
            let module = Module::compile(&clang, &fixture(case, "verified.cpp"), target);
            let (plan, script) = module.valid(case, &fixture(case, "spec.toml"));
            assert_eq!(plan.schema_version, 1);
            assert_eq!(plan.target_triple, module.facts.target_triple);
            assert_eq!(plan.data_layout, module.facts.data_layout);
            assert_eq!(plan.command, module.facts.command);
            assert_eq!(plan.compiler, module.facts.compiler);
            assert!(!script.contains("llvm_unsafe_assume_spec"));
            let post = script.split_once("llvm_execute_func [").unwrap().1;
            assert!(
                !post.contains("padding"),
                "padding must not be post-asserted"
            );
            for object in plan.objects.values() {
                let record = &module.facts.records[&normalize_name(&object.layout.source_type)];
                assert_eq!(object.layout.size, record.size);
                assert_eq!(object.layout.alignment, record.alignment);
                assert!(object.layout.unresolved.is_empty());
                assert_eq!(object.projection, Projection::Fields);
                assert!(object.configured_shape.is_none());
                assert!(script.contains(&format!(
                    "llvm_alloc_aligned {} (llvm_alias \"{}\")",
                    record.alignment, object.layout.llvm_type
                )));
            }
            let p = &plan.objects["p"];
            assert_eq!(p.lowering, "pointer");
            match index {
                0 => {
                    let ret = &plan.objects["return"];
                    let expected = vec!["tag", "inner.x", "inner.enabled", "arr[0]", "arr[1]"];
                    assert_eq!(paths(&p.layout), expected);
                    assert_eq!(paths(&ret.layout), expected);
                    assert!(ret.layout.size > 16, "exercise sret on both ABIs");
                    assert_eq!(ret.lowering, "sret");
                    assert_eq!(ret.argument_index, 0);
                    assert_eq!(p.argument_index, 1);
                    assert!(
                        module.ir.contains("llvm.memcpy"),
                        "exercise aggregate copying"
                    );
                    assert!(ret.layout.padding.iter().any(|gap| gap.size > 0
                        && gap.offset > 0
                        && gap.offset + gap.size < ret.layout.size));
                    assert!(
                        script.contains("llvm_execute_func [result_ptr, p_ptr, llvm_term delta]")
                    );
                    assert!(post.contains("(advance_outer_contract p_pre delta).ret"));
                    assert!(post.contains("(advance_outer_contract p_pre delta).pPost"));
                    assert!(!post.contains("result_pre"));
                }
                1 => {
                    assert_eq!(paths(&p.layout), vec!["data", "value", "enabled"]);
                    assert!(field(&p.layout, "data").is_pointer);
                    assert_eq!(
                        p.framed.iter().map(String::as_str).collect::<Vec<_>>(),
                        vec!["data", "enabled"]
                    );
                    assert!(!p.asserted.iter().any(|path| path == "data"));
                    let record = script
                        .lines()
                        .find(|line| line.contains("let p_pre ="))
                        .unwrap();
                    assert!(!record.contains("data"));
                    assert!(record.contains("value =") && record.contains("enabled ="));
                    assert!(script.contains("layout_p_data <- llvm_fresh_pointer"));
                    assert!(post.contains("// Frame p.data:"));
                }
                2 => {
                    assert_eq!(paths(&p.layout), vec!["a", "b", "ok", "tail"]);
                    let bases: Vec<_> = p
                        .layout
                        .bases
                        .iter()
                        .map(|b| normalize_name(&b.source_type))
                        .collect();
                    assert_eq!(bases, vec!["A", "B"]);
                    assert!(!plan.objects.contains_key("return"));
                }
                3 => {
                    assert_eq!(paths(&p.layout), vec!["valid", "value.left", "tail"]);
                    let choice = &module.facts.records["Choice"];
                    assert!(choice.is_union);
                    assert_eq!(
                        member(&choice.members, "left").offset,
                        member(&choice.members, "right").offset
                    );
                    assert_eq!(field_key("value.left"), "value__left");
                    assert!(script.contains("value__left = layout_p_value__left"));
                }
                _ => unreachable!(),
            }
        }
    }
}

#[test]
fn cross_target_sidecars_and_wrong_data_layouts_are_rejected_without_saw() {
    let Some(clang) = clang() else { return };
    let case = CASES[0];
    let source = fixture(case, "verified.cpp");
    let windows = Module::compile(&clang, &source, TARGETS[0]);
    let linux = Module::compile(&clang, &source, TARGETS[1]);
    assert_ne!(windows.facts.target_triple, linux.facts.target_triple);
    assert_ne!(windows.facts.data_layout, linux.facts.data_layout);
    for (module, foreign) in [(&windows, &linux), (&linux, &windows)] {
        capture::write(&module.ll, &foreign.facts).unwrap();
        module.reject(
            case,
            &fixture(case, "spec.toml"),
            "compiler layout target triple mismatch",
        );
        let error = derive::derive(
            &foreign.facts,
            &module.ir,
            "Outer",
            "p",
            &LayoutConfig::default(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("compiler facts disagree with LLVM"));
        let mut facts = module.facts.clone();
        facts.data_layout.clone_from(&foreign.facts.data_layout);
        capture::write(&module.ll, &facts).unwrap();
        module.reject(
            case,
            &fixture(case, "spec.toml"),
            "compiler layout data layout mismatch",
        );
    }
}

#[test]
fn arrays_and_pointer_width_follow_compiler_alignment_including_32_bit_targets() {
    let Some(clang) = clang() else { return };
    let scratch = Scratch::new();
    // Compiler layout probes only; no function here is used as a verification target.
    let source = scratch.write(
        "abi.cpp",
        r#"
struct Word32 { unsigned int value; };
struct Word64 { unsigned long long value; };
struct ArrayProbe {
    unsigned char tag;
    Word32 words[3];
    Word64 wide[2];
    bool enabled;
};
unsigned long long array_probe(ArrayProbe *p) noexcept {
    return p->wide[1].value + p->words[2].value;
}
struct PointerProbe { void *data; };
void *pointer_probe(PointerProbe *p) noexcept { return p->data; }
"#,
    );
    for target in TARGETS
        .into_iter()
        .chain(["i686-pc-windows-msvc", "i686-unknown-linux-gnu"])
    {
        let module = Module::compile(&clang, &source, target);
        let config = LayoutConfig {
            projection: Projection::Fields,
            ..Default::default()
        };
        let layout = derive::derive(&module.facts, &module.ir, "ArrayProbe", "p", &config).unwrap();
        let record = &module.facts.records["ArrayProbe"];
        assert_eq!(
            (layout.size, layout.alignment),
            (record.size, record.alignment)
        );
        let dl = DataLayout::parse(&module.facts.data_layout).unwrap();
        let defs = saw_spec_gen::llvm_ir::struct_defs(&module.ir);
        for (name, element, count, scalar) in
            [("words", "Word32", 3, "i32"), ("wide", "Word64", 2, "i64")]
        {
            let element = &module.facts.records[element];
            let llvm = dl.layout_of(scalar, &defs).unwrap();
            assert_eq!(
                (llvm.size, llvm.alignment),
                (element.size, element.alignment)
            );
            let base = member(&record.members, name).offset;
            for index in 0..count {
                let leaf = field(&layout, &format!("{name}[{index}].value"));
                assert_eq!(
                    leaf.offset,
                    base + index * element.size + member(&element.members, "value").offset
                );
                assert_eq!(leaf.llvm_type, scalar);
            }
        }
        assert_eq!(
            layout.fields.len(),
            7,
            "tag, three words, two wide cells, bool"
        );
        let pointer = dl.pointer_layout(0).unwrap();
        let compiler_pointer = &module.facts.records["PointerProbe"];
        assert_eq!(
            (pointer.size, pointer.alignment),
            (compiler_pointer.size, compiler_pointer.alignment)
        );
        let mut setup = String::new();
        // The tested field-key spelling also covers leaves inside record arrays.
        assert_eq!(field_key("wide[1].value"), "wide_1__value");
        let mut object = ObjectPlan {
            region: "p".into(),
            layout,
            projection: Projection::Fields,
            mutable: true,
            argument_index: 0,
            lowering: "pointer".into(),
            configured_shape: None,
            inferred_shape: None,
            asserted: vec![],
            framed: vec![],
            selectors: BTreeMap::new(),
        };
        validate::validate_object(&mut object, &module.ir).unwrap();
        emit::setup(&mut setup, &object);
        assert!(setup.contains("wide_1__value = layout_p_wide_1__value"));
    }
}
