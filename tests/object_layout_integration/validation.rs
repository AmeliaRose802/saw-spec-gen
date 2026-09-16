use super::*;

#[test]
fn invalid_extents_offsets_and_padding_prefixes_fail_before_emission() {
    let Some(clang) = clang() else { return };
    let case = CASES[0];
    for target in TARGETS {
        let module = Module::compile(&clang, &fixture(case, "verified.cpp"), target);
        let (plan, _) = module.valid(case, &fixture(case, "spec.toml"));
        let record = &module.facts.records["Outer"];
        let inner = member(&record.members, "inner");
        let x = member(&inner.children, "x").offset;
        let ret = &plan.objects["return"].layout;
        let semantic_end = ret.fields.iter().map(|f| f.offset + f.size).max().unwrap();
        let gap = ret
            .padding
            .iter()
            .find(|g| g.size > 0 && g.offset + g.size < semantic_end)
            .unwrap();
        // Valid controls: explicit extent/assertions must not force byte projection.
        let exact = module.config(case, |v| {
            put(v, "out_buffer_param", vec![format!("p={}", record.size)]);
            put(v, "alias_size", vec![format!("Outer={}", record.size)]);
            put(v, "sret_assert_bytes", semantic_end);
            put(
                &mut v["layout"],
                "offsets",
                BTreeMap::from([("p.inner.x", x)]),
            );
            put(
                &mut v["layout"],
                "alignments",
                BTreeMap::from([("p", record.alignment)]),
            );
        });
        let (exact_plan, _) = module.valid(case, &exact);
        assert_eq!(exact_plan.objects["p"].projection, Projection::Fields);
        for size in [record.size - 1, record.size + 1] {
            let config = module.config(case, |v| {
                put(v, "out_buffer_param", vec![format!("p={size}")])
            });
            module.reject(case, &config, "neither undersize nor oversize is allowed");
            let config = module.config(case, |v| {
                put(v, "alias_size", vec![format!("Outer={size}")])
            });
            module.reject(case, &config, "disagrees with exact compiler size");
        }
        let config = module.config(case, |v| {
            put(
                &mut v["layout"],
                "offsets",
                BTreeMap::from([("p.inner.x", x + 1)]),
            );
        });
        module.reject(case, &config, "offset assertion p.inner.x");
        let config = module.config(case, |v| {
            put(
                &mut v["layout"],
                "alignments",
                BTreeMap::from([("p", record.alignment * 2)]),
            );
        });
        module.reject(case, &config, "alignment assertion p");
        for prefix in [semantic_end - 1, gap.offset + gap.size] {
            let config = module.config(case, |v| put(v, "sret_assert_bytes", prefix));
            module.reject(case, &config, "omits semantic field return.");
        }
        let config = module.config(case, |v| put(v, "sret_assert_bytes", ret.size + 1));
        module.reject(case, &config, "exceeds compiler return size");
        let config = module.config(case, |v| {
            put(&mut v["layout"], "projection", "bytes");
            put(v, "preconditions", vec![format!("p @ {} <= 1", gap.offset)]);
        });
        module.reject(case, &config, "selects padding/inactive storage");
    }
}

#[test]
fn equal_union_storage_still_requires_an_actual_selected_member() {
    let Some(clang) = clang() else { return };
    let case = CASES[3];
    for target in TARGETS {
        let module = Module::compile(&clang, &fixture(case, "verified.cpp"), target);
        module.reject(
            case,
            &fixture(case, "missing_active.toml"),
            "union p.value requires an explicit active_members selection",
        );
        module.reject(
            case,
            &fixture(case, "unknown_active.toml"),
            "does not identify one real member",
        );
        // Selection changes the semantic name despite identical scalar storage.
        // This does not assert general std::variant support or prove right-use code.
        let config = LayoutConfig {
            projection: Projection::Fields,
            active_members: BTreeMap::from([("p.value".into(), "right".into())]),
            ..Default::default()
        };
        let layout = derive::derive(&module.facts, &module.ir, "Tagged", "p", &config).unwrap();
        assert_eq!(paths(&layout), vec!["valid", "value.right", "tail"]);
        assert_eq!(field_key("value.right"), "value__right");
    }
}

#[test]
fn scalar_bool_validity_follows_source_type_with_the_same_config() {
    let Some(clang) = clang() else { return };
    let case = CASES[1];
    let source = fs::read_to_string(fixture(case, "verified.cpp")).unwrap();
    assert_eq!(source.matches("bool enabled;").count(), 1);
    let scratch = Scratch::new();
    for target in TARGETS {
        for (spelling, validity) in [("bool", Some("bool")), ("unsigned char", None)] {
            let changed = source.replace("bool enabled;", &format!("{spelling} enabled;"));
            let cpp = scratch.write("node.cpp", &changed);
            let module = Module::compile(&clang, &cpp, target);
            let (plan, script) = module.valid(case, &fixture(case, "spec.toml"));
            let flag = field(&plan.objects["p"].layout, "enabled");
            assert_eq!(
                flag.llvm_type, "i8",
                "same storage, different source validity"
            );
            assert_eq!(flag.validity.as_deref(), validity);
            assert_eq!(
                plan.validity_constraints
                    .contains(&"valid(p.enabled)".into()),
                validity.is_some()
            );
            assert_eq!(
                script.contains("// C++ valid representation: p.enabled"),
                validity.is_some()
            );
        }
    }
}
