use super::*;

#[test]
fn fields_setup_inserts_named_values_once_and_keeps_padding_arbitrary() {
    let mut out = String::new();
    emission::setup(&mut out, &pair());
    assert_eq!(out.matches("llvm_points_to_at_type ").count(), 1);
    assert_eq!(out.matches("llvm_fresh_var").count(), 1); // Main already made field vars.
    assert!(!out.contains("llvm_alloc"));
    assert!(out.contains("layout_this_0_bitfield_0_padding && (24 : [8])"));
    assert!(out.contains("(zero # (this_pre.lo) : [8]) << (0 : [8])"));
    assert!(out.contains("(zero # (this_pre.hi) : [8]) << (5 : [8])"));
    assert!(!out.contains("llvm_precond")); // No signed-value restriction.
}

#[test]
fn bytes_setup_uses_the_real_backing_word_at_its_byte_offset() {
    let mut lead = member("lead", 0, 0);
    lead.source_type = "char".into();
    lead.bit_width = None;
    let mut object = object(
        "<{ i8, i16 }>",
        vec![lead, member("lo", 9, 3), member("hi", 16, 5)],
    );
    object.projection = Projection::Bytes;
    let mut out = String::new();
    emission::setup(&mut out, &object);
    assert_eq!(out.matches("llvm_points_to_at_type ").count(), 1);
    assert!(!out.contains("llvm_fresh_var"));
    assert!(out.contains(
        "(llvm_elem (llvm_cast_pointer this_ptr (llvm_array 3 (llvm_int 8))) 1) (llvm_int 16)"
    ));
    assert!(out.contains("(join (reverse (take`{2} (drop`{1} this_pre))))"));
    emission::postcondition(&mut out, &object, Some("next this_pre"));
    assert!(out.contains("drop`{13}"));
    assert!(out.contains("drop`{11}"));
}

#[test]
fn postconditions_bind_one_actual_word_then_assert_and_frame_only_named_bits() {
    let mut object = pair();
    object.asserted = vec!["lo".into()];
    object.framed = vec!["hi".into()];
    let mut out = String::new();
    emission::postcondition(&mut out, &object, Some("step this_pre"));
    assert_eq!(out.matches("llvm_points_to_at_type ").count(), 1);
    assert_eq!(out.matches("llvm_fresh_var").count(), 1);
    assert_eq!(out.matches("llvm_postcond").count(), 2);
    let word = "layout_this_0_bitfield_0_post";
    assert!(out.contains(&format!("(llvm_term {word})")));
    assert!(out.contains(&format!(
        "(drop`{{5}} ({word} >> (0 : [8]))) == (step this_pre).lo"
    )));
    assert!(out.contains(&format!(
        "(drop`{{5}} ({word} >> (5 : [8]))) == this_pre.hi"
    )));
    assert!(!out.contains("padding"));
    assert!(out.find("llvm_fresh_var").unwrap() < out.find("llvm_points_to_at_type").unwrap());
    assert!(out.find("llvm_points_to_at_type").unwrap() < out.find("llvm_postcond").unwrap());
    object.asserted.push("hi".into());
    out.clear();
    emission::postcondition(&mut out, &object, Some("step this_pre"));
    assert!(out.contains("(step this_pre).hi == this_pre.hi"));
    assert!(out.contains("// unchanged field"));
}

#[test]
fn frame_only_without_a_model_uses_prestate_and_no_obligations_emit_nothing() {
    let mut object = pair();
    let mut out = String::new();
    emission::postcondition(&mut out, &object, None);
    assert!(out.is_empty());
    object.framed.push("hi".into());
    emission::postcondition(&mut out, &object, None);
    assert_eq!(out.matches("llvm_postcond").count(), 1);
    assert!(out.contains("== this_pre.hi"));
    assert!(!out.contains(".lo"));
}

#[test]
fn guarded_storage_is_read_once_under_the_union_of_model_and_frame_guards() {
    let mut object = guarded_pair();
    object.asserted = vec!["lo".into()];
    object.framed = vec!["hi".into()];
    let mut out = String::new();
    emission::setup(&mut out, &object);
    assert_eq!(out.matches("llvm_conditional_points_to_at_type").count(), 1);
    assert!(out.contains("{{ (this_pre.has_value == 1) }}"));
    out.clear();
    emission::postcondition(&mut out, &object, Some("step this_pre"));
    assert_eq!(out.matches("llvm_conditional_points_to_at_type").count(), 1);
    assert!(out.contains("((step this_pre).has_value == 1)"));
    assert!(out.contains("(this_pre.has_value == 1)"));
    assert!(out.contains(" || "));
    assert!(out.contains("llvm_postcond {{ if ((step this_pre).has_value == 1) then"));
    assert!(out.contains("llvm_postcond {{ if (this_pre.has_value == 1) then"));
    object.region = "return".into();
    out.clear();
    emission::setup(&mut out, &object);
    assert!(!out.contains("conditional_points_to"));
    assert!(out.contains("llvm_cast_pointer result_ptr"));
}

#[test]
fn bool_bitfields_use_width_sized_terms_and_only_wider_bools_need_validity() {
    let mut small = member("small", 0, 1);
    small.source_type = "bool".into();
    let mut wide = member("wide", 2, 2);
    wide.source_type = "_Bool".into();
    let mut object = object("{ i32 }", vec![small, wide]);
    assert!(object
        .layout
        .fields
        .iter()
        .all(|f| f.validity.as_deref() == Some("bool")));
    let mut out = String::new();
    emission::setup(&mut out, &object);
    assert_eq!(out.matches("llvm_precond").count(), 1);
    assert!(out.contains("this_pre.wide <= (1 : [2])"));
    assert!(!out.contains("this_pre.small <= 1"));
    object.projection = Projection::Bytes;
    out.clear();
    emission::setup(&mut out, &object);
    assert_eq!(out.matches("llvm_precond").count(), 1);
    assert!(out.contains("drop`{30}"));
    assert!(!out.contains("drop`{31}"));
    object.region = "return".into();
    out.clear();
    emission::setup(&mut out, &object);
    assert!(!out.contains("llvm_precond"));
}

#[test]
fn nonbyte_integer_storage_and_full_128_bit_fields_avoid_width_overflow() {
    let mut object = object("{ i9 }", vec![member("value", 4, 5)]);
    object.projection = Projection::Bytes;
    let mut out = String::new();
    emission::setup(&mut out, &object);
    assert!(out.contains("(drop`{7} (join (reverse (take`{2} (drop`{0} this_pre)))))"));
    emission::postcondition(&mut out, &object, Some("step this_pre"));
    assert!(out.contains("drop`{4} (layout_this_0_bitfield_0_post >> (4 : [9]))"));
    let object = super::object("{ i128 }", vec![member("value", 0, 128)]);
    out.clear();
    emission::setup(&mut out, &object);
    assert!(out.contains("padding && (0 : [128])"));
    emission::postcondition(&mut out, &object, Some("step this_pre"));
    assert!(out.contains("drop`{0} (layout_this_0_bitfield_0_post >> (0 : [128]))"));
}

#[test]
fn distinct_storage_words_get_distinct_cells_without_extra_allocations() {
    let object = object("{ i8, i8 }", vec![member("lo", 0, 1), member("hi", 8, 1)]);
    let mut out = String::new();
    emission::setup(&mut out, &object);
    assert_eq!(out.matches("llvm_points_to_at_type ").count(), 2);
    assert_eq!(out.matches("llvm_fresh_var").count(), 2);
    assert!(!out.contains("llvm_alloc"));
    out.clear();
    emission::postcondition(&mut out, &object, Some("step this_pre"));
    assert_eq!(out.matches("llvm_points_to_at_type ").count(), 2);
    for offset in [0, 1] {
        assert!(out.contains(&format!("layout_this_0_bitfield_{offset}_post")));
    }
}
