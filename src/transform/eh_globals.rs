//! Stripping of C++ exception-handling metadata globals from LLVM IR.
//!
//! Two sub-passes cover both major ABIs:
//!
//! * **MSVC** (`strip_msvc_eh_globals`) — replaces `_TI*` / `_CTA*` /
//!   `_CT??_R0*` globals in `section ".xdata"` with `external constant`
//!   declarations. Their initializers use `ptrtoint` differences
//!   against `@__ImageBase` that Crucible cannot constant-fold.
//!
//! * **Itanium** (`strip_itanium_typeinfo_globals`) — replaces `_ZTI*`
//!   typeinfo globals whose initializers `getelementptr` into the
//!   external `@_ZTVN10__cxxabiv1*` vtable declarations. SAW aborts
//!   with "Global was not a compile-time constant" on those GEPs.
//!
//! Both categories of metadata are only consumed by the OS/C++
//! runtime unwinder, never by user code, so dropping the
//! initializer is sound for SAW verification.

// ── MSVC EH globals ────────────────────────────────────────────

/// Replace MSVC EH-metadata global *definitions* with
/// `external constant` *declarations*.
///
/// We identify a global as MSVC EH metadata when both:
///
/// 1. its name matches the MSVC scheme for throw-info / catchable-type
///    chains (`_TI*`, `_CTA*`, `_CT??_R0*`), and
/// 2. the line places it in `section ".xdata"` (the MSVC exception
///    data section).
///
/// Requiring (2) means a stray user global called `_TIstats` (vanishingly
/// unlikely but legal) is not affected.
pub(crate) fn strip_msvc_eh_globals(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut buf = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if is_msvc_eh_global(line) {
            if let Some(decl) = rewrite_msvc_eh_global(line) {
                buf.push_str(&decl);
                count += 1;
                continue;
            }
        }
        buf.push_str(line);
    }
    (buf, count)
}

fn is_msvc_eh_global(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('@') {
        return false;
    }
    if !trimmed.contains("section \".xdata\"") {
        return false;
    }
    eh_global_name(trimmed).is_some()
}

fn eh_global_name(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('@')?;
    let (name_with_at, _) = if let Some(rest) = rest.strip_prefix('"') {
        let end = rest.find('"')?;
        let full = &line[..1 + 1 + end + 1];
        (full, &line[full.len()..])
    } else {
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        let full = &line[..1 + end];
        (full, &line[full.len()..])
    };
    let inner = name_with_at.trim_start_matches('@').trim_matches('"');
    if inner.starts_with("_CT??_R0") || inner.starts_with("_TI") || inner.starts_with("_CTA") {
        Some(name_with_at)
    } else {
        None
    }
}

fn rewrite_msvc_eh_global(line: &str) -> Option<String> {
    let name = eh_global_name(line.trim_start())?;
    let const_idx = line
        .find(" constant ")
        .map(|i| i + " constant ".len())
        .or_else(|| line.find(" global ").map(|i| i + " global ".len()))?;
    let brace_idx = line[const_idx..].find('{')?;
    let ty = line[const_idx..const_idx + brace_idx].trim();
    if ty.is_empty() {
        return None;
    }
    let lead_ws_end = line.len() - line.trim_start().len();
    let leading = &line[..lead_ws_end];
    Some(format!("{leading}{name} = external constant {ty}\n"))
}

// ── Itanium typeinfo globals ───────────────────────────────────

/// Strip Itanium-ABI C++ typeinfo globals whose initializers
/// reference `@_ZTVN10__cxxabiv1*` vtable externals.
///
/// On Linux/Itanium, `throw Foo{}` emits a typeinfo global like:
///
///     @_ZTI3Foo = linkonce_odr constant { ptr, ptr }
///         { ptr getelementptr inbounds (ptr,
///             ptr @_ZTVN10__cxxabiv117__class_type_infoE, i64 2),
///           ptr @_ZTS3Foo }, comdat, align 8
///
/// The `getelementptr` into the external vtable declaration cannot
/// be evaluated by SAW's constant folder — it aborts with
/// "Global was not a compile-time constant". The typeinfo is only
/// read by the C++ runtime for RTTI / catch-clause matching, so
/// dropping the initializer is sound for SAW verification.
pub(crate) fn strip_itanium_typeinfo_globals(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut buf = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("@_ZTI")
            && trimmed.contains("@_ZTVN10__cxxabiv1")
            && (trimmed.contains(" constant ") || trimmed.contains(" global "))
        {
            if let Some(decl) = rewrite_itanium_typeinfo(line) {
                buf.push_str(&decl);
                count += 1;
                continue;
            }
        }
        buf.push_str(line);
    }
    (buf, count)
}

fn rewrite_itanium_typeinfo(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let name_end = trimmed.find(" = ")?;
    let name = &trimmed[..name_end];
    let const_idx = line
        .find(" constant ")
        .map(|i| i + " constant ".len())
        .or_else(|| line.find(" global ").map(|i| i + " global ".len()))?;
    let rest = &line[const_idx..];
    let ty = if rest.starts_with('{') {
        let end = balanced_brace(rest, '{', '}')?;
        &rest[..end]
    } else if rest.starts_with('[') {
        let end = balanced_brace(rest, '[', ']')?;
        &rest[..end]
    } else {
        let end = rest.find(['{', '[']).unwrap_or(rest.len());
        rest[..end].trim()
    };
    if ty.is_empty() {
        return None;
    }
    let lead_ws_end = line.len() - trimmed.len();
    let leading = &line[..lead_ws_end];
    Some(format!("{leading}{name} = external constant {ty}\n"))
}

fn balanced_brace(s: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
    }
    None
}

// ── Exception-lower globals injection ──────────────────────────

/// Scan the lowered LLVM IR for exception-lower globals and add them to
/// the target spec so SAW can allocate them. The exception-lower pass
/// synthesises `@__exclow_error_flag`, `@__exclow_error_typeinfo`, and
/// `@__exclow_error_value` — none of which appear in the clang AST.
pub fn inject_exclow_globals(
    spec: &mut crate::constraints::SpecConstraint,
    ir_path: &std::path::Path,
) {
    use crate::constraints::types::{GlobalVarInfo, TypeInfo};

    let text = match std::fs::read_to_string(ir_path) {
        Ok(t) => t,
        Err(_) => return,
    };

    // Quick check: if the flag global isn't present, there's nothing to do.
    if !text.contains("@__exclow_error_flag") {
        return;
    }

    let exclow_globals = [
        GlobalVarInfo {
            name: "__exclow_error_flag".into(),
            mangled_name: "__exclow_error_flag".into(),
            ty: TypeInfo::Bool,
            init_value: Some("0".into()),
        },
        GlobalVarInfo {
            name: "__exclow_error_typeinfo".into(),
            mangled_name: "__exclow_error_typeinfo".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
            init_value: None,
        },
        GlobalVarInfo {
            name: "__exclow_error_value".into(),
            mangled_name: "__exclow_error_value".into(),
            ty: TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
            init_value: None,
        },
    ];

    for g in exclow_globals {
        if !spec
            .referenced_globals
            .iter()
            .any(|existing| existing.mangled_name == g.mangled_name)
        {
            spec.referenced_globals.push(g);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_throwinfo_global() {
        let input = "\
@_TI1H = linkonce_odr unnamed_addr constant %eh.ThrowInfo { i32 0, i32 0, i32 0, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @_CTA1H to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32) }, section \".xdata\", comdat
";
        let (out, n) = strip_msvc_eh_globals(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("@_TI1H = external constant %eh.ThrowInfo"),
            "got: {out}"
        );
        assert!(!out.contains("ptrtoint"));
    }

    #[test]
    fn strips_quoted_catchabletype_global() {
        let input = "@\"_CT??_R0H@84\" = linkonce_odr unnamed_addr constant %eh.CatchableType { i32 1, i32 trunc (i64 sub nuw nsw (i64 ptrtoint (ptr @\"??_R0H@8\" to i64), i64 ptrtoint (ptr @__ImageBase to i64)) to i32), i32 0, i32 -1, i32 0, i32 4, i32 0 }, section \".xdata\", comdat\n";
        let (out, n) = strip_msvc_eh_globals(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("@\"_CT??_R0H@84\" = external constant %eh.CatchableType"),
            "got: {out}"
        );
    }

    #[test]
    fn leaves_non_xdata_global_alone() {
        let input = "@\"??_R0H@8\" = linkonce_odr global %rtti.TypeDescriptor2 { ptr @\"??_7type_info@@6B@\", ptr null, [3 x i8] c\".H\\00\" }, comdat\n";
        let (out, n) = strip_msvc_eh_globals(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }

    #[test]
    fn leaves_user_globals_alone() {
        let input = "@my_global = linkonce_odr constant i32 42\n";
        let (out, n) = strip_msvc_eh_globals(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }

    #[test]
    fn strips_itanium_typeinfo_with_class_type_info() {
        let input = "@_ZTI11HarmlessTag = linkonce_odr dso_local constant { ptr, ptr } { ptr getelementptr inbounds (ptr, ptr @_ZTVN10__cxxabiv117__class_type_infoE, i64 2), ptr @_ZTS11HarmlessTag }, comdat, align 8\n";
        let (out, n) = strip_itanium_typeinfo_globals(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("@_ZTI11HarmlessTag = external constant { ptr, ptr }"),
            "got: {out}"
        );
        assert!(!out.contains("getelementptr"));
    }

    #[test]
    fn strips_itanium_typeinfo_with_si_class_type_info() {
        let input = "@_ZTI7Derived = linkonce_odr constant { ptr, ptr, ptr } { ptr getelementptr inbounds (ptr, ptr @_ZTVN10__cxxabiv120__si_class_type_infoE, i64 2), ptr @_ZTS7Derived, ptr @_ZTI4Base }, comdat, align 8\n";
        let (out, n) = strip_itanium_typeinfo_globals(input);
        assert_eq!(n, 1);
        assert!(
            out.contains("@_ZTI7Derived = external constant { ptr, ptr, ptr }"),
            "got: {out}"
        );
    }

    #[test]
    fn leaves_zti_without_vtable_ref_alone() {
        let input = "@_ZTI3Foo = linkonce_odr constant { ptr, ptr } { ptr null, ptr @_ZTS3Foo }\n";
        let (out, n) = strip_itanium_typeinfo_globals(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }

    #[test]
    fn leaves_zts_name_strings_alone() {
        let input = "@_ZTS11HarmlessTag = linkonce_odr dso_local constant [14 x i8] c\"11HarmlessTag\\00\", comdat, align 1\n";
        let (out, n) = strip_itanium_typeinfo_globals(input);
        assert_eq!(n, 0);
        assert_eq!(out, input);
    }
}
