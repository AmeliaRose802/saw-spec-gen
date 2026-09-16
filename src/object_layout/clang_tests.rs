use super::*;

// Clang 20.1.8 x86_64-pc-windows-msvc, -fdump-record-layouts-complete.
const MSVC: &str = r"*** Dumping AST Record Layout
         0 | struct Empty (empty)
           | [sizeof=1, align=1,
           |  nvsize=0, nvalign=1]

*** Dumping AST Record Layout
         0 | struct Holder
         0 |   char lead
         8 |   struct Derived d
         8 |     struct Base (primary base)
         8 |       (Base vftable pointer)
        16 |       int b
        32 |     struct Empty (base) (empty)
        24 |     (Derived vbtable pointer)
        32 |     unsigned short[2] a
    36:0-2 |     unsigned int x
            40:- |     unsigned int
        40:0-4 |     unsigned int
                44 |     union Derived::(anonymous at <stdin>:4:122)
        44 |       int i
        44 |       char c
        48 |     _Bool isActive
        56 |     struct Virtual (virtual base)
        56 |       int v
           | [sizeof=64, align=8,
           |  nvsize=64, nvalign=8]
";

// Same Derived definition, Clang 20.1.8 --target=x86_64-unknown-linux-gnu.
const ITANIUM: &str = r"*** Dumping AST Record Layout
         0 | struct Derived
         0 |   struct Base (primary base)
         0 |     (Base vtable pointer)
         8 |     int b
         0 |   struct Empty (base) (empty)
        12 |   unsigned short[2] a
    16:0-2 |   unsigned int x
            20:- |   unsigned int
        20:0-4 |   unsigned int
                24 |   union Derived::(anonymous at <stdin>:4:122)
        24 |     int i
        24 |     char c
        28 |   _Bool isActive
        32 |   struct Virtual (virtual base)
        32 |     int v
           | [sizeof=40, dsize=36, align=8,
           |  nvsize=29, nvalign=8]

*** Dumping AST Record Layout
         0 | struct D
         0 |   struct V (primary virtual base)
         0 |     (V vtable pointer)
           | [sizeof=8, dsize=8, align=8,
           |  nvsize=8, nvalign=8]

*** Dumping AST Record Layout
         0 | struct Functions
         0 |   void (*)(int) f
         8 |   int V::* p
           | [sizeof=16, dsize=16, align=8,
           |  nvsize=16, nvalign=8]
";

// Real MSVC <optional> dump: retain the typedef spelling and anonymous location.
const OPTIONAL: &str = r"*** Dumping AST Record Layout
         0 | union std::_Optional_destruct_base<unsigned int>::(anonymous at C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\include\optional:71:5)
         0 |   struct std::_Nontrivial_dummy_type _Dummy (empty)
         0 |   remove_cv_t<unsigned int> _Value
           | [sizeof=4, align=4,
           |  nvsize=4, nvalign=4]

*** Dumping AST Record Layout
         0 | class std::optional<unsigned int>
         0 |   struct std::_Optional_construct_base<unsigned int> (base)
         0 |     struct std::_Optional_destruct_base<unsigned int> (base)
         0 |       union std::_Optional_destruct_base<unsigned int>::(anonymous at C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\include\optional:71:5)
         0 |         struct std::_Nontrivial_dummy_type _Dummy (empty)
         0 |         remove_cv_t<unsigned int> _Value
         4 |       _Bool _Has_value
           | [sizeof=8, align=4,
           |  nvsize=8, nvalign=4]

*** Dumping AST Record Layout
         0 | struct OptionalHolder
         0 |   char prefix
         4 |   class std::optional<unsigned int> value
         4 |     struct std::_Optional_construct_base<unsigned int> (base)
         4 |       struct std::_Optional_destruct_base<unsigned int> (base)
         4 |         union std::_Optional_destruct_base<unsigned int>::(anonymous at C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\include\optional:71:5)
         4 |           struct std::_Nontrivial_dummy_type _Dummy (empty)
         4 |           remove_cv_t<unsigned int> _Value
         8 |         _Bool _Has_value
           | [sizeof=12, align=4,
           |  nvsize=12, nvalign=4]
";

// Real MSVC <mutex> layout, including both union alternatives and their children.
const MUTEX: &str = r"*** Dumping AST Record Layout
         0 | class KeyStore
         0 |   class std::mutex mu_
         0 |     class std::_Mutex_base (base)
         0 |       struct _Mtx_internal_imp_t _Mtx_storage
         0 |         int _Type
         8 |         union _Mtx_internal_imp_t::(anonymous at C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\include\__msvc_threads_core.hpp:44:5)
         8 |           struct _Stl_critical_section _Critical_section
         8 |             void * _Unused
        16 |             _Smtx_t _M_srw_lock
         8 |           union std::_Align_type<double, 64> _Cs_storage
         8 |             double _Val
         8 |             char[64] _Pad
        72 |         long _Thread_id
        76 |         int _Count
        80 |   _Bool isActive
           | [sizeof=88, align=8,
           |  nvsize=88, nvalign=8]
";

fn field<'a>(members: &'a [RecordMember], name: &str) -> &'a RecordMember {
    members.iter().find(|member| member.name == name).unwrap()
}

#[test]
fn msvc_preserves_absolute_offsets_bases_and_synthetic_entries() {
    let records = parse_record_layouts(MSVC).unwrap();
    assert_eq!(records.len(), 2);
    assert!(records["Empty"].members.is_empty());
    assert_eq!(records["Empty"].size, 1);
    let holder = &records["Holder"];
    assert_eq!((holder.size, holder.alignment), (64, 8));
    assert_eq!(holder.source_type, "struct Holder");
    let derived = field(&holder.members, "d");
    assert_eq!(derived.offset, 8);
    assert_eq!(derived.children.len(), 10);
    let base = &derived.children[0];
    assert!(base.is_base);
    assert!(base.name.is_empty());
    assert_eq!(base.source_type, "struct Base");
    assert_eq!(field(&base.children, "b").offset, 16);
    assert_eq!(base.children[0].source_type, "(Base vftable pointer)");
    assert_eq!(base.children[0].offset, 8);
    assert!(derived.children[1].is_base && derived.children[1].is_empty);
    assert_eq!(derived.children[2].source_type, "(Derived vbtable pointer)");
    let virtual_base = &derived.children[9];
    assert!(virtual_base.is_base);
    assert_eq!(virtual_base.offset, 56);
    assert_eq!(field(&virtual_base.children, "v").offset, 56);
}

#[test]
fn arrays_bool_bitfields_and_unnamed_union_are_not_flattened() {
    let records = parse_record_layouts(MSVC).unwrap();
    let children = &field(&records["Holder"].members, "d").children;
    assert_eq!(field(children, "a").source_type, "unsigned short[2]");
    assert_eq!(field(children, "isActive").source_type, "_Bool");
    let bits = field(children, "x");
    assert_eq!(
        (bits.offset, bits.bit_offset, bits.bit_width),
        (36, Some(0), Some(3))
    );
    assert_eq!(
        (children[5].bit_offset, children[5].bit_width),
        (None, Some(0))
    );
    assert!(children[5].name.is_empty());
    assert_eq!(children[5].source_type, "unsigned int");
    assert_eq!(children[6].bit_width, Some(5));
    assert!(children[6].name.is_empty());
    let union = &children[7];
    assert!(union.name.is_empty());
    assert_eq!(
        union.source_type,
        "union Derived::(anonymous at <stdin>:4:122)"
    );
    assert_eq!(union.children.len(), 2);
    assert!(union.children.iter().all(|member| member.offset == 44));
}

#[test]
fn itanium_supports_tail_padding_reuse_and_primary_virtual_bases() {
    let records = parse_record_layouts(ITANIUM).unwrap();
    let derived = &records["Derived"];
    assert_eq!((derived.size, derived.alignment), (40, 8));
    assert_eq!(derived.members[1].offset, 0); // Empty base overlaps the primary base.
    assert!(derived.members[1].is_empty);
    assert_eq!(field(&derived.members, "a").offset, 12); // Primary base tail padding.
    let base = &records["D"].members[0];
    assert!(base.is_base);
    assert_eq!(base.source_type, "struct V");
    assert_eq!(base.children[0].source_type, "(V vtable pointer)");
    let functions = &records["Functions"].members;
    assert_eq!(field(functions, "f").source_type, "void (*)(int)");
    assert_eq!(field(functions, "p").source_type, "int V::*");
}

#[test]
fn real_optional_keeps_nested_bases_empty_alternative_and_absolute_payload() {
    let records = parse_record_layouts(OPTIONAL).unwrap();
    assert_eq!(records["std::optional<unsigned int>"].size, 8);
    let holder = &records["OptionalHolder"];
    assert_eq!(holder.size, 12);
    let value = field(&holder.members, "value");
    let construct = &value.children[0];
    let destruct = &construct.children[0];
    assert!(construct.is_base && destruct.is_base);
    let union = &destruct.children[0];
    assert!(union.name.is_empty());
    assert!(union
        .source_type
        .contains(r"C:\Program Files (x86)\Microsoft Visual Studio"));
    assert!(field(&union.children, "_Dummy").is_empty);
    let payload = field(&union.children, "_Value");
    assert_eq!(payload.source_type, "remove_cv_t<unsigned int>");
    assert_eq!(payload.offset, 4);
    assert_eq!(field(&destruct.children, "_Has_value").offset, 8);
    let standalone_union = &records[&normalize_name(&union.source_type)];
    assert!(standalone_union.is_union);
    assert_eq!(standalone_union.size, 4);
    assert_eq!(standalone_union.members.len(), 2);
}

#[test]
fn real_mutex_retains_all_union_alternatives_and_multilevel_members() {
    let records = parse_record_layouts(MUTEX).unwrap();
    let store = &records["KeyStore"];
    assert_eq!((store.size, store.alignment), (88, 8));
    let mu = field(&store.members, "mu_");
    assert_eq!(mu.source_type, "class std::mutex");
    let base = &mu.children[0];
    assert!(base.is_base);
    let storage = field(&base.children, "_Mtx_storage");
    assert_eq!(field(&storage.children, "_Count").offset, 76);
    let union = &storage.children[1];
    assert!(union.name.is_empty());
    assert_eq!(union.children.len(), 2);
    let cs = field(&union.children, "_Cs_storage");
    assert_eq!(cs.source_type, "union std::_Align_type<double, 64>");
    assert_eq!(field(&cs.children, "_Pad").source_type, "char[64]");
    assert_eq!(field(&store.members, "isActive").offset, 80);
}

#[test]
fn normalization_retains_qualified_template_names_and_numeric_suffixes() {
    assert_eq!(
        normalize_name(" class ns::Box<struct ns::Entry, union ns::Choice>.17 "),
        "ns::Box<ns::Entry, ns::Choice>.17"
    );
    assert_eq!(
        normalize_name("class ns::Box<unsigned   long>"),
        "ns::Box<unsigned long>"
    );
    assert_eq!(
        normalize_name("structural::class_name.42"),
        "structural::class_name.42"
    );
    assert_eq!(
        normalize_name(r#"struct ns::Box<"class  union", struct T>"#),
        r#"ns::Box<"class  union", T>"#
    );
    let name = r"Scope::(anonymous at C:\path with spaces\class folder\file.cpp:4:2)";
    assert_eq!(normalize_name(&format!("union {name}")), name);
    let dump = "*** Dumping AST Record Layout\n         0 | class ns::Box<struct ns::Entry, 17> (empty)\n           | [sizeof=1, align=1, nvsize=0, nvalign=1]";
    assert!(parse_record_layouts(dump)
        .unwrap()
        .contains_key("ns::Box<ns::Entry, 17>"));
}

#[test]
fn ignores_irgen_blocks_without_interpreting_their_llvm_record() {
    let irgen = "*** Dumping IRgen Record Layout\nRecord: CXXRecordDecl 0x123\nLayout: <CGRecordLayout\n  LLVMType:%struct.NotAnAstRecord = type { i64 }\n  BitFields:[\n]>\n";
    let text = format!("unrelated compiler banner\n{irgen}{MSVC}{irgen}{OPTIONAL}");
    let records = parse_record_layouts(&text).unwrap();
    assert_eq!(records.len(), 5);
    assert!(parse_record_layouts(irgen).unwrap().is_empty());
    assert!(parse_record_layouts("").unwrap().is_empty());
}

#[test]
fn retains_unknown_member_spellings_instead_of_guessing_a_scalar() {
    let dump = "*** Dumping AST Record Layout\n         0 | struct Unknown\n         0 |   (Unknown vendor ABI entry)\n         8 |   vendor::opaque_type storage\n           | [sizeof=16, align=8]\n";
    let records = parse_record_layouts(dump).unwrap();
    let members = &records["Unknown"].members;
    assert_eq!(members[0].source_type, "(Unknown vendor ABI entry)");
    assert!(members[0].name.is_empty());
    assert_eq!(members[1].source_type, "vendor::opaque_type");
    assert_eq!(members[1].name, "storage");
}

#[test]
fn rejects_malformed_or_truncated_ast_blocks() {
    let header = "*** Dumping AST Record Layout\n         0 | struct Broken\n";
    for body in [
        "",
        "unrecognized record row\n           | [sizeof=8, align=4]",
        "         ? |   int x\n           | [sizeof=8, align=4]",
        "         0 |       int x\n           | [sizeof=8, align=4]",
        "         0 |  int x\n           | [sizeof=8, align=4]",
        "     0:2-1 |   unsigned int x\n           | [sizeof=8, align=4]",
        "       0:2 |   unsigned int x\n           | [sizeof=8, align=4]",
        "         9 |   int x\n           | [sizeof=8, align=4]",
        "           | [sizeof=8, align=4,\n*** Dumping IRgen Record Layout",
        "           | [sizeof=8, align=0]",
        "           | [sizeof=8]",
        "           | [sizeof=8, align=4, nvsize=unknown]",
        "           | [sizeof=8, align=4, bogus=0]",
        "           | [sizeof=8, align=4, align=4]",
        "           | [sizeof=8, align=4] garbage",
    ] {
        assert!(
            parse_record_layouts(&format!("{header}{body}")).is_err(),
            "accepted {body:?}"
        );
    }
}

#[test]
fn repeated_identical_layouts_are_allowed_but_conflicts_are_not() {
    assert_eq!(
        parse_record_layouts(&format!("{MSVC}{MSVC}"))
            .unwrap()
            .len(),
        2
    );
    let changed = MSVC.replace("sizeof=64", "sizeof=72");
    assert!(parse_record_layouts(&format!("{MSVC}{changed}")).is_err());
}

#[test]
fn tolerates_crlf_and_trimmed_trailing_blanks_without_losing_anonymous_members() {
    let crlf = OPTIONAL.replace('\n', "\r\n");
    assert_eq!(
        parse_record_layouts(&crlf).unwrap(),
        parse_record_layouts(OPTIONAL).unwrap()
    );
    let trimmed = OPTIONAL
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        parse_record_layouts(&trimmed).unwrap(),
        parse_record_layouts(OPTIONAL).unwrap()
    );
    let trimmed = MSVC
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        parse_record_layouts(&trimmed).unwrap(),
        parse_record_layouts(MSVC).unwrap()
    );
}

#[test]
fn zero_sized_unions_and_unnamed_qualified_types_stay_explicit() {
    let dump = "*** Dumping AST Record Layout\n         0 | union EmptyUnion\n           | [sizeof=0, align=1]\n";
    let records = parse_record_layouts(dump).unwrap();
    assert!(records["EmptyUnion"].is_union);
    assert_eq!(records["EmptyUnion"].size, 0);
    assert!(records["EmptyUnion"].members.is_empty());
    for spelling in ["unsigned int", "const volatile E", "enum E", "enum class E"] {
        assert_eq!(
            split_declaration(spelling),
            (spelling.into(), String::new())
        );
    }
}
