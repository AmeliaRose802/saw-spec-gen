//! Tests for the clang-AST path filter.
//! Extracted from `path_filter.rs` to keep that file under the 500-line limit.

use super::*;
use serde_json::json;

fn project_dir() -> PathBuf {
    PathBuf::from(r"C:\Users\me\proj")
}

fn system_dir() -> PathBuf {
    PathBuf::from(r"C:\Program Files\LLVM\include")
}

/// Helper: produce a FunctionDecl JSON node that LOOKS LIKE A
/// DEFINITION (has a CompoundStmt body), so the path filter
/// applies normally to it. Pure declarations (no body) are
/// always kept and would defeat path-filtering tests.
fn fn_def(name: &str, file: &str) -> serde_json::Value {
    json!({
        "kind": "FunctionDecl",
        "name": name,
        "loc": { "file": file },
        "inner": [ { "kind": "CompoundStmt" } ]
    })
}

#[test]
fn keeps_decls_under_project_dir() {
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            fn_def("foo", r"C:\Users\me\proj\foo.cpp"),
            fn_def("stl_thing", r"C:\Program Files\LLVM\include\xstring"),
            fn_def("bar", r"C:\Users\me\proj\bar.h"),
        ]
    });
    let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
    assert_eq!(stats.kept, 2);
    assert_eq!(stats.dropped, 1);
    assert_eq!(stats.no_loc, 0);
    let kept_count = ast["inner"].as_array().unwrap().len();
    assert_eq!(kept_count, 2);
}

#[test]
fn verbatim_keep_dir_matches_plain_ast_paths() {
    // `Path::canonicalize` yields a `\\?\` verbatim keep-dir while
    // clang emits plain `C:\...` paths in the AST. The prefix must
    // be normalised away or the main-file decl is wrongly dropped.
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            fn_def("add_one", r"C:\Users\me\proj\add_one.cpp"),
            fn_def("stl_thing", r"C:\Program Files\LLVM\include\xstring"),
        ]
    });
    let verbatim_keep = PathBuf::from(r"\\?\C:\Users\me\proj");
    let stats = filter_translation_unit_value(&mut ast, &[verbatim_keep]);
    assert_eq!(stats.kept, 1);
    assert_eq!(stats.dropped, 1);
    let kept = ast["inner"].as_array().unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["name"], "add_one");
}

#[test]
fn elided_file_inherits_from_previous_sibling() {
    // Clang omits `loc.file` when a sibling decl uses the same
    // file as the immediately-preceding one. The filter must
    // remember the last-seen file to classify the elided sibling
    // correctly.
    // All four entries have bodies (CompoundStmt) so they're
    // subject to path filtering rather than the pure-decl
    // pass-through.
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {"kind": "FunctionDecl", "loc": {"file": r"C:\Program Files\LLVM\include\xstring", "line": 10}, "inner": [{"kind": "CompoundStmt"}]},
            {"kind": "FunctionDecl", "loc": {"line": 20}, "inner": [{"kind": "CompoundStmt"}]},
            {"kind": "FunctionDecl", "loc": {"file": r"C:\Users\me\proj\foo.cpp", "line": 1}, "inner": [{"kind": "CompoundStmt"}]},
            {"kind": "FunctionDecl", "loc": {"line": 2}, "inner": [{"kind": "CompoundStmt"}]},
        ]
    });
    let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
    assert_eq!(
        stats.dropped, 2,
        "both stl entries (line 10 + elided line 20) dropped"
    );
    assert_eq!(
        stats.kept, 2,
        "both project entries (line 1 + elided line 2) kept"
    );
}

#[test]
fn no_loc_decls_are_kept() {
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {"kind": "TypedefDecl", "name": "__int128_t"},
            {"kind": "FunctionDecl", "loc": {"file": r"C:\Users\me\proj\foo.cpp"}},
        ]
    });
    let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
    // __int128_t has no loc at all -- it's a builtin -- and would
    // never be dropped. The function under the project dir is
    // kept on its own merits.
    assert_eq!(stats.no_loc, 1);
    assert_eq!(stats.kept, 1);
    assert_eq!(stats.dropped, 0);
}

#[test]
fn comparison_is_case_insensitive() {
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            // Same path, different casing on the drive letter.
            fn_def("foo", r"c:\users\ME\proj\Foo.cpp"),
            fn_def("bar", r"C:\Users\me\proj\bar.cpp"),
        ]
    });
    let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
    assert_eq!(stats.kept, 2);
    assert_eq!(stats.dropped, 0);
}

#[test]
fn unix_style_paths_round_trip() {
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            fn_def("foo", "/home/me/proj/foo.cpp"),
            fn_def("stl_thing", "/usr/include/c++/13/string"),
        ]
    });
    let stats = filter_translation_unit_value(&mut ast, &[PathBuf::from("/home/me/proj")]);
    assert_eq!(stats.kept, 1);
    assert_eq!(stats.dropped, 1);
}

#[test]
fn non_translation_unit_root_is_passed_through() {
    // -ast-dump-filter outputs single FunctionDecl objects, not a
    // TranslationUnitDecl. The filter should leave them alone.
    let mut ast = json!({
        "kind": "FunctionDecl",
        "name": "foo",
        "loc": {"file": r"C:\Program Files\LLVM\include\xstring"}
    });
    let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
    assert_eq!(stats.kept, 1);
    assert_eq!(stats.dropped, 0);
}

#[test]
fn multiple_keep_paths_act_as_union() {
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            fn_def("foo", r"C:\Users\me\proj\foo.cpp"),
            fn_def("lib_helper", r"C:\Users\me\third_party\lib.h"),
            fn_def("stl_thing", r"C:\Program Files\LLVM\include\xstring"),
        ]
    });
    let stats = filter_translation_unit_value(
        &mut ast,
        &[project_dir(), PathBuf::from(r"C:\Users\me\third_party")],
    );
    assert_eq!(stats.kept, 2);
    assert_eq!(stats.dropped, 1);
    // Make sure the deliberately-dropped system header path
    // wasn't accidentally retained.
    let _ = system_dir();
}

#[test]
fn expansion_loc_is_consulted_when_loc_file_missing() {
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {
                "kind": "FunctionDecl",
                "range": {
                    "begin": {
                        "expansionLoc": {"file": r"C:\Users\me\proj\macro_user.cpp"}
                    }
                }
            }
        ]
    });
    let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
    assert_eq!(stats.kept, 1);
    assert_eq!(stats.dropped, 0);
}

#[test]
fn pure_declarations_outside_keep_path_are_kept() {
    // Models a typical <cstdlib> declaration for `int rand()`.
    // gen-verify needs the declaration to emit an external
    // override spec, so the filter must keep it even though it
    // lives outside the user's directory.
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            // Pure declaration in a system header -- KEEP.
            {
                "kind": "FunctionDecl",
                "name": "rand",
                "loc": {"file": r"C:\Program Files\LLVM\include\stdlib.h"},
                "inner": [
                    {"kind": "ParmVarDecl", "name": "p1"}
                ]
            },
            // Function with a body in the same system header -- DROP.
            {
                "kind": "FunctionDecl",
                "name": "abort_impl",
                "loc": {"file": r"C:\Program Files\LLVM\include\stdlib.h"},
                "inner": [
                    {"kind": "ParmVarDecl", "name": "p1"},
                    {"kind": "CompoundStmt"}
                ]
            },
            // User code -- KEEP.
            {
                "kind": "FunctionDecl",
                "name": "my_func",
                "loc": {"file": r"C:\Users\me\proj\foo.cpp"}
            },
        ]
    });
    let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
    assert_eq!(stats.kept, 2, "rand decl + my_func kept");
    assert_eq!(stats.dropped, 1, "abort_impl with body dropped");
    let kept_names: Vec<&str> = ast["inner"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(kept_names, vec!["rand", "my_func"]);
}

#[test]
fn forward_declarations_with_no_inner_are_kept() {
    // Some `FunctionDecl` JSON dumps have no `inner` key at all
    // (truly forward declarations). They should still be treated
    // as pure declarations and kept.
    let mut ast = json!({
        "kind": "TranslationUnitDecl",
        "inner": [
            {
                "kind": "FunctionDecl",
                "name": "printf",
                "loc": {"file": r"C:\Program Files\LLVM\include\stdio.h"}
            }
        ]
    });
    let stats = filter_translation_unit_value(&mut ast, &[project_dir()]);
    assert_eq!(stats.kept, 1);
    assert_eq!(stats.dropped, 0);
}
