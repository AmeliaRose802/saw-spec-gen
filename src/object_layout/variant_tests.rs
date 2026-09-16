//! Synthesized full compiler facts, not compiler/SAW integration proof.
//! The scalar fixture models #include <variant> and State { variant<A,B> choice;
//! unsigned long long tail; }, including real library head/tail storage shapes.

use super::*;

const A: &str = "unsigned long long";
const B: &str = "long long";
const NS: &str = "std::__detail::__variant";

struct Fixture {
    ir: String,
    facts: CompilerLayouts,
    gnu: bool,
    storage: String,
    owner: String,
    variant: String,
}

impl Fixture {
    fn own(&mut self) -> &mut Vec<RecordMember> {
        &mut self.facts.records.get_mut(&self.owner).unwrap().members
    }

    fn members(&mut self, source: &str) -> &mut Vec<RecordMember> {
        &mut self.facts.records.get_mut(source).unwrap().members
    }
}

fn member(source: &str, name: &str, offset: usize) -> RecordMember {
    RecordMember {
        source_type: source.into(),
        name: name.into(),
        offset,
        bit_offset: None,
        bit_width: None,
        is_base: false,
        is_empty: false,
        children: Vec::new(),
    }
}

fn base(source: &str) -> RecordMember {
    let mut result = member(source, "", 0);
    result.is_base = true;
    result
}

fn record(source: &str, size: usize, alignment: usize, members: Vec<RecordMember>) -> RecordLayout {
    RecordLayout {
        source_type: source.into(),
        size,
        alignment,
        members,
        is_union: source.starts_with("union "),
    }
}

fn llvm_types(ir: &str) -> BTreeMap<String, String> {
    struct_defs(ir)
        .into_iter()
        .map(|(name, def)| (name, format!("{{ {} }}", def.fields.join(", "))))
        .collect()
}

fn fixture(gnu: bool, types: [(&str, usize); 2], backing: &str, capacity: usize) -> Fixture {
    let mut records = Vec::new();
    let terminal = if gnu {
        format!("union {NS}::_Variadic_union<>")
    } else {
        "class std::_Variant_storage_<true>".into()
    };
    records.push(record(&terminal, 1, 1, Vec::new()));
    let tail_name = if gnu { "_M_rest" } else { "_Tail" };
    let mut tail = member(&terminal, tail_name, 0);
    tail.is_empty = true;
    for index in (0..2).rev() {
        let args = types[index..]
            .iter()
            .map(|t| t.0)
            .collect::<Vec<_>>()
            .join(", ");
        let source = if gnu {
            format!("union {NS}::_Variadic_union<{args}>")
        } else {
            format!("class std::_Variant_storage_<true, {args}>")
        };
        let head = if gnu {
            let wrapper = format!("struct {NS}::_Uninitialized<{}, true>", types[index].0);
            let fields = vec![member(types[index].0, "_M_storage", 0)];
            records.push(record(
                &wrapper,
                types[index].1,
                types[index].1.min(8),
                fields,
            ));
            member(&wrapper, "_M_first", 0)
        } else {
            member(&format!("std::remove_cv_t<{}>", types[index].0), "_Head", 0)
        };
        let mut children = vec![head, tail];
        if !gnu {
            let name = normalize_name(&source);
            let anonymous = format!("union {name}::(anonymous at variant:100:5)");
            let mut union = member(&anonymous, "", 0);
            union.children = children;
            children = vec![union];
        }
        let size = if index == 0 { capacity } else { types[index].1 };
        records.push(record(&source, size, 8, children));
        tail = member(&source, tail_name, 0);
    }
    let storage = normalize_name(&tail.source_type);
    let args = types.iter().map(|t| t.0).collect::<Vec<_>>().join(", ");
    let mut ir = if gnu {
        format!(
            "%\"struct.{NS}::_Uninitialized\" = type {{ {backing} }}\n\
            %\"union.{NS}::_Variadic_union\" = type {{ %\"struct.{NS}::_Uninitialized\" }}\n"
        )
    } else {
        format!(
            "%union.anon = type {{ {backing} }}\n\
            %\"class.std::_Variant_storage_\" = type {{ %union.anon }}\n"
        )
    };
    let (owner, mut llvm_owner) = if gnu {
        tail.name = "_M_u".into();
        (
            format!("struct {NS}::_Variant_storage<true, {args}>"),
            format!("struct.{NS}::_Variant_storage"),
        )
    } else {
        tail.name.clear();
        tail.is_base = true;
        (
            format!("class std::_Variant_base<{args}>"),
            "class.std::_Variant_base".into(),
        )
    };
    let storage_ir = if gnu {
        format!("union.{NS}::_Variadic_union")
    } else {
        "class.std::_Variant_storage_".into()
    };
    ir.push_str(&format!(
        "%\"{llvm_owner}\" = type {{ %\"{storage_ir}\", i8 }}\n"
    ));
    let (tag_type, tag_name) = if gnu {
        ("__index_type", "_M_index")
    } else {
        ("_Index_t", "_Which")
    };
    let fields = vec![tail, member(tag_type, tag_name, capacity)];
    records.push(record(&owner, capacity + 8, 8, fields));
    let mut source_owner = owner.clone();
    if gnu {
        for stem in [
            "_Copy_ctor_base",
            "_Move_ctor_base",
            "_Copy_assign_base",
            "_Move_assign_base",
            "_Variant_base",
        ] {
            let args = if stem == "_Variant_base" {
                args.clone()
            } else {
                format!("true, {args}")
            };
            let source = format!("struct {NS}::{stem}<{args}>");
            records.push(record(&source, capacity + 8, 8, vec![base(&source_owner)]));
            let next = format!("struct.{NS}::{stem}");
            ir.push_str(&format!("%\"{next}\" = type {{ %\"{llvm_owner}\" }}\n"));
            (source_owner, llvm_owner) = (source, next);
        }
    }
    let variant = format!("class std::variant<{args}>");
    records.push(record(&variant, capacity + 8, 8, vec![base(&source_owner)]));
    let fields = vec![
        member(&variant, "choice", 0),
        member(A, "tail", capacity + 8),
    ];
    records.push(record("struct State", capacity + 16, 8, fields));
    ir.push_str(&format!(
        "%\"class.std::variant\" = type {{ %\"{llvm_owner}\" }}\n\
        %struct.State = type {{ %\"class.std::variant\", i64 }}\n"
    ));
    let target = if gnu {
        "x86_64-unknown-linux-gnu"
    } else {
        "x86_64-pc-windows-msvc"
    };
    let facts = CompilerLayouts {
        schema_version: 1,
        compiler: "synthesized MSVC/GNU variant facts".into(),
        target_triple: target.into(),
        data_layout: "e-p:64:64-i64:64-i128:128".into(),
        command: Vec::new(),
        llvm_types: llvm_types(&ir),
        irgen_types: BTreeMap::new(),
        records: records
            .into_iter()
            .map(|r| (normalize_name(&r.source_type), r))
            .collect(),
    };
    Fixture {
        ir,
        facts,
        gnu,
        storage,
        owner: normalize_name(&owner),
        variant,
    }
}

fn scalar(gnu: bool) -> Fixture {
    fixture(gnu, [(A, 8), (B, 8)], "i64", 8)
}

fn config(key: &str, selected: &str) -> LayoutConfig {
    LayoutConfig {
        active_members: BTreeMap::from([(key.into(), selected.into())]),
        ..LayoutConfig::default()
    }
}

fn selected(f: &Fixture, index: &str) -> Result<ObjectLayout> {
    derive(&f.facts, &f.ir, "State", "p", &config("p.choice", index))
}

fn field<'a>(layout: &'a ObjectLayout, path: &str) -> &'a FieldLayout {
    layout.fields.iter().find(|f| f.path == path).unwrap()
}

fn gap(layout: &ObjectLayout, offset: usize, size: usize) -> &ByteRange {
    layout
        .padding
        .iter()
        .find(|r| r.offset == offset && r.size == size)
        .unwrap()
}

fn branches(f: &mut Fixture) -> &mut Vec<RecordMember> {
    let children = &mut f.facts.records.get_mut(&f.storage).unwrap().members;
    if f.gnu {
        children
    } else {
        &mut children[0].children
    }
}

fn rejected(f: &Fixture, index: &str, message: &str) {
    let error = selected(f, index).unwrap_err();
    assert!(error.to_string().contains(message), "{error:#}");
}

#[test]
fn scalar_indices_use_compiler_offsets_and_explicit_selection_metadata() {
    for gnu in [false, true] {
        let mut f = scalar(gnu);
        for index in ["0", "1"] {
            let layout = selected(&f, index).unwrap();
            let tag = field(&layout, "choice.index");
            assert_eq!((tag.offset, tag.size, tag.llvm_type.as_str()), (8, 1, "i8"));
            assert_eq!(tag.validity, Some(format!("active_variant:{index}")));
            assert!(tag.guard.is_none());
            let value = field(&layout, "choice.value");
            assert_eq!(
                (value.offset, value.size, value.llvm_type.as_str()),
                (0, 8, "i64")
            );
            let source = if index == "0" { A } else { B };
            assert!(value.source_type.contains(source));
            assert!(value.validity.is_none() && value.guard.is_none());
            assert_eq!(field(&layout, "tail").offset, 16);
            assert_eq!(layout.fields.len(), 3);
            assert!(layout.unresolved.is_empty());
            let note = format!("p.choice: selected actual compiler union member {index}");
            assert!(layout.validation.contains(&note));
            assert_eq!(gap(&layout, 9, 7).reason, "compiler padding");
        }
        let root = derive(&f.facts, &f.ir, &f.variant, "p", &config("p", "1")).unwrap();
        assert_eq!(
            field(&root, "index").validity.as_deref(),
            Some("active_variant:1")
        );
        let state = f.facts.records.get_mut("State").unwrap();
        state.size = 32;
        state.members[0].offset = 8;
        state.members[1].offset = 24;
        state.members.insert(0, member(A, "lead", 0));
        f.ir = f.ir.replace(
            "%struct.State = type { %\"class.std::variant\", i64 }",
            "%struct.State = type { i64, %\"class.std::variant\", i64 }",
        );
        f.facts.llvm_types = llvm_types(&f.ir);
        let layout = selected(&f, "1").unwrap();
        assert_eq!(field(&layout, "choice.value").offset, 8);
        assert_eq!(field(&layout, "choice.index").offset, 16);
    }
}

#[test]
fn missing_non_numeric_out_of_range_and_wrong_path_selections_fail_closed() {
    for gnu in [false, true] {
        let f = scalar(gnu);
        assert!(derive(&f.facts, &f.ir, "State", "p", &LayoutConfig::default()).is_err());
        for index in [
            "",
            "_Head",
            "-1",
            "+1",
            " 0",
            "2",
            "999999999999999999999999",
        ] {
            assert!(selected(&f, index).is_err(), "accepted {index:?}");
        }
        assert!(derive(&f.facts, &f.ir, "State", "p", &config("choice", "0")).is_err());
    }
}

#[test]
fn tag_must_be_unique_independent_non_bool_integer_storage() {
    for gnu in [false, true] {
        let mut f = scalar(gnu);
        f.own().pop();
        rejected(&f, "0", "unique tag");
        branches(&mut f)[0]
            .children
            .push(member("_Index_t", "_Which", 0));
        rejected(&f, "0", "unique tag"); // Never search inside an alternative.
        let mut f = scalar(gnu);
        f.own().push(member("int", "__index", 12));
        rejected(&f, "0", "unique tag");
        for source in ["bool", "void *", "double", "UnknownTag", "unsigned int"] {
            let mut f = scalar(gnu);
            f.own()[1].source_type = source.into();
            rejected(&f, "1", "variant tag source");
        }
        for offset in [0, 9, 16] {
            let mut f = scalar(gnu);
            f.own()[1].offset = offset;
            assert!(selected(&f, "0").is_err());
        }
        let mut f = scalar(gnu);
        f.own()[1].name = "__index".into();
        assert!(selected(&f, "0").is_ok());
        f.own()[1].bit_width = Some(1);
        rejected(&f, "0", "bitfield");
    }
}

#[test]
fn unknown_wrappers_extra_semantic_members_and_bad_chains_are_rejected() {
    for gnu in [false, true] {
        let mut f = scalar(gnu);
        f.own().push(member("UnknownPointerAlias", "extra", 0));
        rejected(&f, "0", "unknown semantic variant");
        let mut f = scalar(gnu);
        branches(&mut f).swap(0, 1);
        rejected(&f, "0", "ordered");
        let mut f = scalar(gnu);
        branches(&mut f).pop();
        rejected(&f, "0", "ordered");
        let mut f = scalar(gnu);
        branches(&mut f)[1].source_type = if gnu {
            format!("union {NS}::_Variadic_union<{A}>")
        } else {
            format!("class std::_Variant_storage_<true, {A}>")
        };
        rejected(&f, "0", "alternative order");
        let mut f = scalar(gnu);
        branches(&mut f)[0].source_type = if gnu {
            format!("struct {NS}::_Uninitialized<{B}, true>")
        } else {
            format!("std::remove_cv_t<{B}>")
        };
        assert!(selected(&f, "0").is_err()); // Equal i64 width must not associate wrong types.
        let mut f = scalar(gnu);
        f.facts.records.get_mut(&f.storage).unwrap().size += 8;
        assert!(selected(&f, "0").is_err());
    }
}

#[test]
fn gnu_storage_alias_is_tied_to_its_checked_template_argument() {
    let mut f = scalar(true);
    let wrapper = normalize_name(&branches(&mut f)[0].source_type);
    f.members(&wrapper)[0].source_type = "_Type".into();
    assert_eq!(
        field(&selected(&f, "0").unwrap(), "choice.value").source_type,
        A
    );
    f.members(&wrapper)[0].source_type = "Mystery".into();
    rejected(&f, "0", "disagrees with source argument");
    let children = f.members(&wrapper);
    children[0].source_type = A.into();
    children.push(children[0].clone());
    rejected(&f, "0", "unique _M_storage");
    let children = f.members(&wrapper);
    children.pop();
    children.push(member("void *", "extra", 0));
    rejected(&f, "0", "extra GNU");
}

#[test]
fn nested_aggregate_alternatives_keep_source_fields_and_nested_commas() {
    let packet = "Packet<unsigned int, long long>";
    for gnu in [false, true] {
        let mut f = fixture(gnu, [(packet, 16), (packet, 16)], "%struct.Packet", 16);
        f.ir.push_str(
            "%struct.Inner = type { i32, i8 }\n%struct.Packet = type { %struct.Inner, i64 }\n",
        );
        let details = vec![
            member("unsigned int", "count", 0),
            member("bool", "live", 4),
        ];
        let payload = vec![
            member("struct Inner", "details", 0),
            member(B, "_M_index", 8),
        ];
        for r in [
            record("struct Inner", 8, 4, details),
            record(&format!("struct {packet}"), 16, 8, payload),
        ] {
            f.facts.records.insert(normalize_name(&r.source_type), r);
        }
        f.facts.llvm_types = llvm_types(&f.ir);
        let layout = selected(&f, "1").unwrap();
        assert_eq!(
            field(&layout, "choice.value.details.count").llvm_type,
            "i32"
        );
        let live = field(&layout, "choice.value.details.live");
        assert_eq!(live.validity.as_deref(), Some("bool"));
        assert_eq!(field(&layout, "choice.value._M_index").offset, 8);
        assert_eq!(field(&layout, "choice.index").offset, 16);
        f.members("Inner")[0].source_type = "UnknownAlias".into();
        rejected(&f, "1", "unrecognized variant payload source");
    }
}

#[test]
fn only_exact_selected_leaves_are_supported_and_inactive_capacity_is_not_padding() {
    for gnu in [false, true] {
        let f = fixture(gnu, [("unsigned int", 4), (B, 8)], "i64", 8);
        rejected(&f, "0", "disagrees with LLVM leaf");
        let mut f = fixture(gnu, [("Big", 16), ("Small", 8)], "%struct.Big", 16);
        f.ir.push_str("%struct.Big = type { i64, i64 }\n");
        let big = record(
            "struct Big",
            16,
            8,
            vec![member(A, "x", 0), member(A, "y", 8)],
        );
        let small = record("struct Small", 8, 8, vec![member(A, "x", 0)]);
        for r in [big, small] {
            f.facts.records.insert(normalize_name(&r.source_type), r);
        }
        f.facts.llvm_types = llvm_types(&f.ir);
        let layout = selected(&f, "1").unwrap();
        assert_eq!(field(&layout, "choice.index").offset, 16);
        assert!(gap(&layout, 8, 8)
            .reason
            .starts_with("inactive variant storage"));
        assert_eq!(gap(&layout, 17, 7).reason, "compiler padding");
    }
}
