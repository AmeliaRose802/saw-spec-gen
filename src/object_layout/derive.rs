//! Derive semantic fields only where independent Clang and LLVM facts agree.
//! Field/guard paths are relative to the object; configuration keys include region.

use super::clang::normalize_name;
use super::data_layout::DataLayout;
use super::model::{
    ByteRange, CompilerLayouts, FieldLayout, LayoutConfig, ObjectLayout, RecordLayout, RecordMember,
};
use crate::llvm_ir::{struct_defs, IrStructDef};
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(test)]
#[path = "bitfield_tests.rs"]
mod bitfield_tests;
#[path = "derive_bitfields.rs"]
mod bitfields;
#[path = "derive_fields.rs"]
mod fields;
#[path = "derive_llvm.rs"]
mod llvm;
#[cfg(test)]
#[path = "derive_tests.rs"]
mod tests;
#[path = "derive_variant.rs"]
mod variant;
#[cfg(test)]
#[path = "variant_tests.rs"]
mod variant_tests;
#[path = "derive_wrappers.rs"]
mod wrappers;

/// Fail closed on missing/ambiguous storage facts. Named integer bitfields use
/// exact LLVM backing words; unsupported mappings remain explicit unresolved facts.
pub fn derive(
    facts: &CompilerLayouts,
    ir: &str,
    source_name: &str,
    region: &str,
    config: &LayoutConfig,
) -> anyhow::Result<ObjectLayout> {
    derive_with_llvm(facts, ir, source_name, region, config, None)
}

/// A signature-carried sret/byval type is a precise compiler mapping even
/// when clang omits template arguments from its LLVM named struct spelling.
pub fn derive_with_llvm(
    facts: &CompilerLayouts,
    ir: &str,
    source_name: &str,
    region: &str,
    config: &LayoutConfig,
    llvm_hint: Option<&str>,
) -> Result<ObjectLayout> {
    ensure!(
        facts.schema_version == 1,
        "unsupported compiler layout schema {}",
        facts.schema_version
    );
    ensure!(!region.is_empty(), "object region must not be empty");
    let record = record_for(facts, source_name)?
        .with_context(|| format!("missing Clang record layout for {source_name}"))?;
    let analysis_ir = if facts.irgen_types.is_empty() {
        ir.into()
    } else {
        super::capture::with_irgen_types(facts, ir)?
    };
    let defs = struct_defs(&analysis_ir);
    check_snapshot(facts, ir, &defs)?;
    let name = match llvm_hint {
        Some(name) => {
            ensure!(
                defs.contains_key(name) && llvm::name_matches(name, &record.source_type),
                "signature LLVM type {name} disagrees with source {}",
                record.source_type
            );
            name.to_owned()
        }
        None => llvm::unique_name(&defs, &normalize_name(&record.source_type))?,
    };
    let dl = DataLayout::parse(&facts.data_layout)?;
    let ty = llvm::named_ref(&name);
    let allocated = dl.layout_of(&ty, &defs)?;
    ensure!(
        record.size == allocated.size,
        "size mismatch for {source_name}: Clang {}, LLVM {}",
        record.size,
        allocated.size
    );
    check_alignment(record, allocated.alignment)?;
    let storage = llvm::Storage::new(&dl, &defs, &ty)?;
    let object = ObjectLayout {
        allocation_type: if facts.llvm_types.contains_key(&name) {
            format!("llvm_alias \"{name}\"")
        } else { llvm::inline_saw_type(&ty, &defs, 0)? },
        source_type: record.source_type.clone(), llvm_type: name,
        size: record.size, alignment: record.alignment, llvm_alignment: allocated.alignment,
        fields: Vec::new(), padding: Vec::new(), bases: Vec::new(), unresolved: Vec::new(),
        validation: vec![format!(
            "Clang sizeof={} equals LLVM allocation size; source alignment {} >= LLVM ABI alignment {}; unique normalized type mapping",
            record.size, record.alignment, allocated.alignment
        )],
    };
    let root = RecordMember {
        name: String::new(),
        source_type: record.source_type.clone(),
        offset: 0,
        bit_offset: None,
        bit_width: None,
        is_base: false,
        is_empty: false,
        children: record.members.clone(),
    };
    let mut state = Deriver {
        facts,
        config,
        region,
        dl,
        defs,
        storage,
        object,
        visits: 0,
        paths: BTreeSet::new(),
        occupied: Vec::new(),
        inactive: Vec::new(),
    };
    state.walk(&root, "", None, 0)?;
    state.finish()?;
    Ok(state.object)
}

fn check_alignment(record: &RecordLayout, llvm_alignment: usize) -> Result<()> {
    ensure!(
        record.alignment.is_power_of_two(),
        "source alignment is not a power of two: {}",
        record.source_type
    );
    ensure!(
        record.alignment >= llvm_alignment,
        "source alignment below LLVM ABI alignment: {}",
        record.source_type
    );
    ensure!(
        record.size % record.alignment == 0,
        "source size is not a multiple of alignment: {}",
        record.source_type
    );
    Ok(())
}

fn record_for<'a>(facts: &'a CompilerLayouts, source: &str) -> Result<Option<&'a RecordLayout>> {
    let name = llvm::clean_source(source);
    let matches: Vec<_> = facts
        .records
        .values()
        .filter(|record| normalize_name(&record.source_type) == name)
        .collect();
    ensure!(
        matches.len() <= 1,
        "ambiguous Clang record layouts for {source}"
    );
    Ok(matches.first().copied())
}

fn check_snapshot(
    facts: &CompilerLayouts,
    ir: &str,
    defs: &HashMap<String, IrStructDef>,
) -> Result<()> {
    // Capture/load checks provenance too; direct callers must not silently use a
    // different target or replace an equal-sized recorded LLVM representation.
    for line in ir.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let expected = match key.trim() {
            "target triple" => &facts.target_triple,
            "target datalayout" => &facts.data_layout,
            _ => continue,
        };
        let actual = value
            .trim()
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .context("malformed LLVM target directive")?;
        ensure!(
            actual == expected,
            "compiler facts disagree with LLVM {}",
            key.trim()
        );
    }
    for (name, body) in &facts.llvm_types {
        if body == "opaque" {
            continue;
        } // Unreachable opaque types need no allocation layout.
        let recorded = struct_defs(&format!("{} = type {body}", llvm::named_ref(name)));
        let recorded = recorded
            .get(name)
            .with_context(|| format!("invalid recorded LLVM type {name}"))?;
        let current = defs
            .get(name)
            .with_context(|| format!("recorded LLVM type {name} missing from IR"))?;
        ensure!(
            recorded.fields == current.fields && recorded.is_packed == current.is_packed,
            "recorded LLVM type {name} differs from IR"
        );
    }
    Ok(())
}

struct Deriver<'a> {
    facts: &'a CompilerLayouts,
    config: &'a LayoutConfig,
    region: &'a str,
    dl: DataLayout,
    defs: HashMap<String, IrStructDef>,
    storage: llvm::Storage,
    object: ObjectLayout,
    paths: BTreeSet<String>,
    occupied: Vec<(usize, usize)>,
    inactive: Vec<ByteRange>,
    visits: usize,
}

impl Deriver<'_> {
    fn visit(&mut self, depth: usize) -> Result<()> {
        ensure!(
            depth < llvm::MAX_DEPTH,
            "Clang member nesting limit exceeded"
        );
        self.visits += 1;
        ensure!(
            self.visits <= llvm::MAX_ITEMS,
            "Clang member expansion limit exceeded"
        );
        Ok(())
    }

    fn bounds(&self, offset: usize, size: usize) -> Result<usize> {
        let end = offset
            .checked_add(size)
            .context("source field extent overflow")?;
        ensure!(
            end <= self.object.size,
            "source field outside object bounds: {offset}..{end} > {}",
            self.object.size
        );
        Ok(end)
    }

    fn record(&self, source: &str) -> Result<Option<&RecordLayout>> {
        record_for(self.facts, source)
    }

    fn named_extent(&self, member: &RecordMember) -> Result<Option<usize>> {
        let record = self.record(&member.source_type)?;
        let node = self
            .storage
            .named_at(&llvm::clean_source(&member.source_type), member.offset)?;
        if let Some(record) = record {
            check_alignment(record, node.map_or(1, |node| node.alignment))?;
            if let Some(node) = node {
                ensure!(
                    record.size == node.size,
                    "nested Clang/LLVM size mismatch: {}",
                    member.source_type
                );
            }
        }
        record
            .map(|r| r.size)
            .or_else(|| node.map(|n| n.size))
            .map(|size| self.bounds(member.offset, size))
            .transpose()
    }

    fn members(&mut self, member: &RecordMember) -> Result<Vec<RecordMember>> {
        let children = if !member.children.is_empty() {
            member.children.clone()
        } else if let Some(record) = self.record(&member.source_type)? {
            record
                .members
                .iter()
                .map(|child| llvm::shifted(child, 0, member.offset))
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        ensure!(
            children.iter().all(|child| child.offset >= member.offset),
            "Clang child precedes its parent: {}",
            member.source_type
        );
        self.remember_bases(&children);
        Ok(children)
    }

    fn remember_bases(&mut self, members: &[RecordMember]) {
        fn contains(tree: &RecordMember, target: &RecordMember) -> bool {
            (tree.is_base && tree.offset == target.offset && tree.source_type == target.source_type)
                || tree.children.iter().any(|child| contains(child, target))
        }
        for member in members {
            if member.is_base {
                if !self.object.bases.iter().any(|base| contains(base, member)) {
                    self.object.bases.push(member.clone());
                }
            } else {
                self.remember_bases(&member.children);
            }
        }
    }

    fn leaf_at(&self, offset: usize) -> Result<&llvm::Leaf> {
        self.storage
            .leaves
            .get(&offset)
            .with_context(|| format!("no exact LLVM storage leaf at source offset {offset}"))
    }

    fn scalar(&self, source: &str) -> Option<llvm::Scalar> {
        llvm::source_scalar(source, &self.facts.target_triple, self.dl.pointer_size)
    }

    fn insert(&mut self, field: FieldLayout) -> Result<()> {
        ensure!(!field.path.is_empty(), "semantic scalar has no source path");
        ensure!(
            self.paths.insert(field.path.clone()),
            "duplicate semantic field path {}",
            field.path
        );
        let end = self.bounds(field.offset, field.size)?;
        if field.size > 0 {
            self.occupied.push((field.offset, end));
        }
        self.object.fields.push(field);
        Ok(())
    }
}

fn joined(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.into()
    } else if name.is_empty() {
        parent.into()
    } else {
        format!("{parent}.{name}")
    }
}

fn record_spelling(source: &str) -> bool {
    matches!(source_head(source), Some("struct" | "class" | "union"))
}

fn source_head(source: &str) -> Option<&str> {
    source.split_whitespace().find(|word| {
        !matches!(
            *word,
            "const" | "volatile" | "restrict" | "__restrict" | "__restrict__" | "mutable"
        )
    })
}
