//! Frontend parsers: read clang AST JSON, LLVM IR text, and mir-json
//! and emit normalized [`crate::constraints::FunctionInfo`] values.

pub mod clang_ast;
pub mod llvm_ir;
pub mod mir_json;
