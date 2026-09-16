use super::*;

#[path = "derive_tests/abi.rs"]
mod abi;
#[path = "derive_tests/storage.rs"]
mod storage;
#[path = "derive_tests/wrappers.rs"]
mod wrappers;

const DL: &str = "e-p:64:64-i64:64-i128:128";

fn member(ty: &str, name: &str, offset: usize) -> RecordMember {
    RecordMember {
        name: name.into(),
        source_type: ty.into(),
        offset,
        bit_offset: None,
        bit_width: None,
        is_base: false,
        is_empty: false,
        children: Vec::new(),
    }
}

fn children(mut member: RecordMember, children: Vec<RecordMember>) -> RecordMember {
    member.children = children;
    member
}

fn base(mut member: RecordMember) -> RecordMember {
    member.is_base = true;
    member
}

fn record(ty: &str, size: usize, alignment: usize, members: Vec<RecordMember>) -> RecordLayout {
    RecordLayout {
        source_type: ty.into(),
        size,
        alignment,
        is_union: ty.starts_with("union "),
        members,
    }
}

fn facts(ir: &str, records: Vec<RecordLayout>) -> CompilerLayouts {
    CompilerLayouts {
        irgen_types: Default::default(),
        schema_version: 1,
        target_triple: "x86_64-pc-windows-msvc".into(),
        data_layout: DL.into(),
        compiler: "synthesized compiler facts for unit tests".into(),
        command: Vec::new(),
        llvm_types: struct_defs(ir)
            .into_iter()
            .map(|(name, def)| {
                let fields = def.fields.join(", ");
                let body = if def.is_packed {
                    format!("<{{ {fields} }}>")
                } else {
                    format!("{{ {fields} }}")
                };
                (name, body)
            })
            .collect(),
        records: records
            .into_iter()
            .map(|r| (normalize_name(&r.source_type), r))
            .collect(),
    }
}

fn checked(ir: &str, records: Vec<RecordLayout>, source: &str) -> Result<ObjectLayout> {
    derive(
        &facts(ir, records),
        ir,
        source,
        "this",
        &LayoutConfig::default(),
    )
}

fn field<'a>(layout: &'a ObjectLayout, path: &str) -> &'a FieldLayout {
    layout
        .fields
        .iter()
        .find(|field| field.path == path)
        .unwrap_or_else(|| panic!("missing {path}: {:?}", layout.fields))
}

fn optional_fixture(flag: &str, value: &str) -> (String, Vec<RecordLayout>) {
    let ir = "%struct.Value = type { i32, i8 }\n%union.Storage = type { %struct.Value }\n%struct.OptBase = type { %union.Storage, i8 }\n%\"class.std::optional<Value>\" = type { %struct.OptBase }\n%struct.Host = type { i8, %\"class.std::optional<Value>\", ptr }".to_owned();
    let payload = children(
        member("remove_cv_t<struct Value>", value, 0),
        vec![member("int", "id", 0), member("bool", "live", 4)],
    );
    let mut dummy = member("struct Empty", "_Dummy", 0);
    dummy.is_empty = true;
    let union = children(member("union Storage", "", 0), vec![dummy, payload]);
    let wrapper = base(children(
        member("struct std::_Optional_base<Value>", "", 0),
        vec![union.clone(), member("_Bool", flag, 8)],
    ));
    let records = vec![
        record(
            "struct Value",
            8,
            4,
            vec![member("int", "id", 0), member("bool", "live", 4)],
        ),
        record("union Storage", 8, 4, union.children),
        record("class std::optional<struct Value>", 12, 4, vec![wrapper]),
        record(
            "struct Host",
            24,
            8,
            vec![
                member("char", "lead", 0),
                member("class std::optional<Value>", "opt", 4),
                member("void *", "next", 16),
            ],
        ),
    ];
    (ir, records)
}
