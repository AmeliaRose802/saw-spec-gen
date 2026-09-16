//! Source hierarchy walking, scalar leaves, arrays and selected unions.

use super::*;

impl Deriver<'_> {
    pub(super) fn walk(
        &mut self,
        member: &RecordMember,
        path: &str,
        guard: Option<&str>,
        depth: usize,
    ) -> Result<()> {
        self.visit(depth)?;
        self.bounds(member.offset, 0)?;
        let source = llvm::clean_source(&member.source_type);
        if ["std::variant<", "std::__1::variant<", "std::__2::variant<"]
            .iter()
            .any(|prefix| source.starts_with(*prefix))
            && source.ends_with('>')
        {
            return self.variant(member, path, guard, depth);
        }
        if member.bit_width.is_some() {
            return self.bitfield(member, path, guard);
        }
        if member.is_empty {
            return Ok(());
        }
        if llvm::source_array(&member.source_type)?.is_some() {
            return self.array(member, path, guard, depth);
        }
        if source == "std::mutex" {
            return self.mutex(member, path, guard, depth);
        }
        if [
            "std::optional<",
            "std::__1::optional<",
            "std::__2::optional<",
        ]
        .iter()
        .any(|prefix| source.starts_with(*prefix))
            && source.ends_with('>')
        {
            return self.optional(member, path, guard, depth);
        }
        if self.scalar(&member.source_type).is_some() {
            ensure!(
                member.children.is_empty(),
                "scalar source member has record children: {path}"
            );
            return self.scalar_field(member, path, guard);
        }
        let children = self.members(member)?;
        if self.is_union(member)? {
            self.named_extent(member)?;
            let key = joined(self.region, path);
            let selected = self.config.active_members.get(&key).with_context(|| {
                format!("union {key} requires an explicit active_members selection")
            })?;
            let matches: Vec<_> = children
                .iter()
                .filter(|child| !child.is_base && child.name == *selected && !child.name.is_empty())
                .collect();
            ensure!(
                matches.len() == 1,
                "union {key} selection {selected:?} does not identify one real member"
            );
            let selected = matches[0];
            self.mark_inactive(member, selected, path, guard)?;
            self.object.validation.push(format!(
                "{key}: selected actual compiler union member {}",
                selected.name
            ));
            return self.walk(selected, &joined(path, &selected.name), guard, depth + 1);
        }
        if !children.is_empty() || self.record(&member.source_type)?.is_some() {
            // A base's complete-object sizeof may include reused tail padding;
            // never substitute it for an LLVM .base subobject's allocation size.
            let extent = if member.is_base {
                self.record(&member.source_type)?
                    .map(|r| {
                        member
                            .offset
                            .checked_add(r.size)
                            .context("base extent overflow")
                    })
                    .transpose()?
            } else {
                self.named_extent(member)?
            };
            let first = self.object.fields.len();
            for child in &children {
                let child_path = if child.is_base {
                    path.to_owned()
                } else {
                    joined(path, &child.name)
                };
                self.walk(child, &child_path, guard, depth + 1)?;
            }
            if let Some(end) = extent {
                ensure!(
                    self.object.fields[first..]
                        .iter()
                        .all(|f| f.offset >= member.offset && f.offset + f.size <= end),
                    "semantic field outside its source record: {path}"
                );
            }
            return Ok(());
        }
        ensure!(
            !record_spelling(&member.source_type),
            "missing Clang member facts for {}",
            member.source_type
        );
        ensure!(
            self.storage.named_at(&source, member.offset)?.is_none(),
            "missing Clang fields for LLVM aggregate {} at {}",
            member.source_type,
            member.offset
        );
        self.scalar_field(member, path, guard)
    }

    pub(super) fn is_union(&self, member: &RecordMember) -> Result<bool> {
        Ok(source_head(&member.source_type) == Some("union")
            || self
                .record(&member.source_type)?
                .is_some_and(|record| record.is_union))
    }

    fn known_size(&self, source: &str, depth: usize) -> Result<Option<usize>> {
        ensure!(
            depth < llvm::MAX_DEPTH,
            "source type nesting limit exceeded"
        );
        if let Some((count, element)) = llvm::source_array(source)? {
            return self
                .known_size(&element, depth + 1)?
                .map(|size| {
                    size.checked_mul(count)
                        .context("source array extent overflow")
                })
                .transpose();
        }
        if let Some(scalar) = self.scalar(source) {
            return Ok(Some(scalar.size));
        }
        Ok(self.record(source)?.map(|record| record.size))
    }

    fn member_size(&self, member: &RecordMember) -> Result<usize> {
        if let Some(size) = self.known_size(&member.source_type, 0)? {
            return Ok(size);
        }
        if llvm::source_array(&member.source_type)?.is_some() {
            let (count, _, stride) = self.array_parts(member)?;
            return count
                .checked_mul(stride)
                .context("source array extent overflow");
        }
        if let Some(node) = self
            .storage
            .named_at(&llvm::clean_source(&member.source_type), member.offset)?
        {
            return Ok(node.size);
        }
        ensure!(
            member.children.is_empty() && !record_spelling(&member.source_type),
            "missing compiler subobject extent for {}",
            member.source_type
        );
        Ok(self.leaf_at(member.offset)?.size)
    }

    pub(super) fn array_parts(&self, member: &RecordMember) -> Result<(usize, String, usize)> {
        let (count, element) =
            llvm::source_array(&member.source_type)?.context("expected source array")?;
        ensure!(
            count <= llvm::MAX_ITEMS,
            "source array expansion limit exceeded"
        );
        let stride = if let Some(size) = self.known_size(&element, 0)? {
            size
        } else {
            let candidates: Vec<_> = self
                .storage
                .nodes
                .iter()
                .filter_map(|node| {
                    let (n, ty) = llvm::llvm_array(&node.ty)?;
                    (node.offset == member.offset && n == count).then_some(ty)
                })
                .collect();
            ensure!(
                candidates.len() == 1,
                "missing/ambiguous LLVM array stride for {} at {}",
                member.source_type,
                member.offset
            );
            self.dl.layout_of(candidates[0], &self.defs)?.size
        };
        ensure!(stride > 0 || count == 0, "zero source array stride");
        self.bounds(
            member.offset,
            stride
                .checked_mul(count)
                .context("source array extent overflow")?,
        )?;
        Ok((count, element, stride))
    }

    fn array(
        &mut self,
        member: &RecordMember,
        path: &str,
        guard: Option<&str>,
        depth: usize,
    ) -> Result<()> {
        let (count, element, stride) = self.array_parts(member)?;
        self.object.validation.push(format!("{path}: {count} array elements, stride {stride}; each scalar checked against LLVM storage"));
        for index in 0..count {
            let offset = index
                .checked_mul(stride)
                .and_then(|n| n.checked_add(member.offset))
                .context("array element offset overflow")?;
            let mut value = llvm::shifted(member, member.offset, offset)?;
            value.source_type = element.clone();
            let first = self.object.fields.len();
            self.walk(&value, &format!("{path}[{index}]"), guard, depth + 1)?;
            for field in &mut self.object.fields[first..] {
                field.array_count.get_or_insert(count);
                field.array_stride.get_or_insert(stride);
            }
        }
        Ok(())
    }

    pub(super) fn scalar_field(
        &mut self,
        member: &RecordMember,
        path: &str,
        guard: Option<&str>,
    ) -> Result<()> {
        let leaf = self.leaf_at(member.offset)?.clone();
        let source = llvm::clean_source(&member.source_type);
        ensure!(
            !source.contains("::*") && !source.contains("(*[") && !source.contains("(&["),
            "unsupported source pointer/member-pointer declarator: {source}"
        );
        if source.contains('[') && !source.contains('<') {
            ensure!(
                source.ends_with(']') && self.scalar(&source).is_some_and(|s| s.pointer),
                "unsupported source array declarator: {source}"
            );
        }
        let mut field_path = path.to_owned();
        let synthetic_pointer = member.name.is_empty()
            && source.starts_with('(')
            && [" vtable pointer)", " vftable pointer)", " vbtable pointer)"]
                .iter()
                .any(|suffix| source.ends_with(*suffix));
        if synthetic_pointer {
            ensure!(
                leaf.pointer && leaf.size == self.dl.pointer_size,
                "compiler table pointer is not an LLVM pointer: {source}"
            );
            field_path = joined(path, &format!("vptr_{}", member.offset));
        } else {
            ensure!(
                !member.name.is_empty(),
                "unnamed scalar source member cannot be resolved: {source}"
            );
        }
        let mut field = from_leaf(member, field_path, &leaf, guard);
        if let Some(expected) = self.scalar(&source) {
            ensure!(expected.matches(&leaf),
                "source field {path} ({source}, {} bytes) disagrees with LLVM leaf {} ({} bytes) at offset {}",
                expected.size, leaf.ty, leaf.size, leaf.offset);
            if expected.boolean {
                field.validity = Some("bool".into());
            }
        } else if !synthetic_pointer {
            if source.starts_with("enum ") {
                ensure!(
                    leaf.ty.starts_with('i') && !leaf.pointer,
                    "enum {path} does not have integer LLVM storage"
                );
            }
            self.object.validation.push(format!(
                "{path}: unresolved source scalar spelling {source:?}; exact LLVM leaf {} at {} supplies representation ({} bytes); no guessed typedef width or enumerator-only restriction",
                leaf.ty, leaf.offset, leaf.size
            ));
        }
        self.insert(field)
    }

    pub(super) fn mark_inactive(
        &mut self,
        union: &RecordMember,
        selected: &RecordMember,
        path: &str,
        guard: Option<&str>,
    ) -> Result<()> {
        let end = self.bounds(union.offset, self.member_size(union)?)?;
        let selected_end = self.bounds(selected.offset, self.member_size(selected)?)?;
        ensure!(
            selected.offset >= union.offset && selected_end <= end,
            "selected union member outside compiler storage: {path}"
        );
        let selected_path = joined(path, &selected.name);
        let condition = guard.map(|g| format!("; guard {g}")).unwrap_or_default();
        let reason = format!("inactive union storage (selected {selected_path}{condition})");
        for (start, end) in [(union.offset, selected.offset), (selected_end, end)] {
            if start < end {
                self.inactive.push(ByteRange {
                    offset: start,
                    size: end - start,
                    reason: reason.clone(),
                });
            }
        }
        Ok(())
    }
}

pub(super) fn from_leaf(
    member: &RecordMember,
    path: String,
    leaf: &llvm::Leaf,
    guard: Option<&str>,
) -> FieldLayout {
    FieldLayout {
        path,
        source_type: member.source_type.clone(),
        llvm_type: leaf.ty.clone(),
        offset: leaf.offset,
        size: leaf.size,
        bit_offset: None,
        bit_width: None,
        array_count: None,
        array_stride: None,
        guard: guard.map(str::to_owned),
        validity: None,
        is_pointer: leaf.pointer,
        runtime: false,
    }
}
