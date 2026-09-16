//! Serializable compiler facts and the checked memory setup plan.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    #[default]
    Bytes,
    Fields,
    /// Validated legacy integer/tuple shapes (selected by buffer configuration).
    Llvm,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutConfig {
    /// Bytes retain existing Cryptol contracts; fields use a named scalar record.
    #[serde(default)]
    pub projection: Projection,
    /// Named union path = selected member. Never guessed from equal sizes.
    #[serde(default)]
    pub active_members: BTreeMap<String, String>,
    /// Optional mutation boundary. All other semantic fields are framed.
    #[serde(default)]
    pub modifies: Option<Vec<String>>,
    /// Optional assertions about compiler-derived facts, never replacements.
    #[serde(default)]
    pub offsets: BTreeMap<String, usize>,
    #[serde(default)]
    pub alignments: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecordMember {
    pub name: String,
    pub source_type: String,
    pub offset: usize,
    pub bit_offset: Option<usize>,
    pub bit_width: Option<usize>,
    pub is_base: bool,
    pub is_empty: bool,
    pub children: Vec<RecordMember>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecordLayout {
    pub source_type: String,
    pub size: usize,
    pub alignment: usize,
    pub is_union: bool,
    pub members: Vec<RecordMember>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompilerLayouts {
    pub schema_version: u32,
    pub target_triple: String,
    pub data_layout: String,
    pub compiler: String,
    pub command: Vec<String>,
    /// Exact named LLVM definitions from the same compiler invocation.
    pub llvm_types: BTreeMap<String, String>,
    /// CGRecordLayout types can be omitted from the module with opaque pointers.
    #[serde(default)]
    pub irgen_types: BTreeMap<String, String>,
    pub records: BTreeMap<String, RecordLayout>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ByteRange {
    pub offset: usize,
    pub size: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FieldLayout {
    pub path: String,
    pub source_type: String,
    pub llvm_type: String,
    pub offset: usize,
    pub size: usize,
    pub bit_offset: Option<usize>,
    pub bit_width: Option<usize>,
    pub array_count: Option<usize>,
    pub array_stride: Option<usize>,
    /// Relative path of an optional engaged flag guarding this field.
    pub guard: Option<String>,
    /// Only bool and justified enum representation bounds belong here.
    pub validity: Option<String>,
    pub is_pointer: bool,
    /// ABI-private storage owned by a narrow runtime abstraction (e.g. mutex).
    #[serde(default)]
    pub runtime: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ObjectLayout {
    pub source_type: String,
    pub llvm_type: String,
    /// A compiler-named alias or an exact inline CGRecordLayout type.
    #[serde(default)]
    pub allocation_type: String,
    pub size: usize,
    pub alignment: usize,
    pub llvm_alignment: usize,
    pub fields: Vec<FieldLayout>,
    pub padding: Vec<ByteRange>,
    pub bases: Vec<RecordMember>,
    pub validation: Vec<String>,
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ObjectPlan {
    pub region: String,
    pub layout: ObjectLayout,
    pub projection: Projection,
    pub mutable: bool,
    pub argument_index: usize,
    pub lowering: String,
    pub configured_shape: Option<String>,
    #[serde(default)]
    pub inferred_shape: Option<String>,
    pub asserted: Vec<String>,
    pub framed: Vec<String>,
    /// Field path -> Cryptol selector for an explicitly configured LLVM shape.
    #[serde(default)]
    pub selectors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LayoutPlan {
    pub schema_version: u32,
    pub target_triple: String,
    pub data_layout: String,
    pub compiler: String,
    pub abi: String,
    pub command: Vec<String>,
    pub objects: BTreeMap<String, ObjectPlan>,
    /// Abstract interface subobjects use the explicit vtable contract backend,
    /// not an invented complete-object size for an unknown implementation.
    #[serde(default)]
    pub abstract_objects: BTreeMap<String, ObjectLayout>,
    pub validity_constraints: Vec<String>,
    pub semantic_preconditions: Vec<String>,
    pub warnings: Vec<String>,
    pub abstraction_boundaries: Vec<String>,
    #[serde(default)]
    pub records: BTreeMap<String, RecordLayout>,
    #[serde(default)]
    pub llvm_types: BTreeMap<String, String>,
}

impl LayoutPlan {
    pub fn object(&self, name: &str) -> Option<&ObjectPlan> {
        self.objects.get(name)
    }
}

/// Reversible path spelling for flat Cryptol records. Length-prefixing would
/// be less readable; instead callers check for collisions before emission.
pub fn field_key(path: &str) -> String {
    path.replace('.', "__").replace('[', "_").replace(']', "")
}
