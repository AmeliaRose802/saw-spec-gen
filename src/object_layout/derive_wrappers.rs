//! Compiler-defined optional wrappers and narrowly scoped opaque mutex storage.

use super::fields::from_leaf;
use super::*;

const FLAGS: &[&str] = &["_Has_value", "_M_engaged", "__engaged_"];
const VALUES: &[&str] = &["_Value", "_M_value", "__val_"];

impl Deriver<'_> {
    pub(super) fn optional(
        &mut self,
        member: &RecordMember,
        path: &str,
        guard: Option<&str>,
        depth: usize,
    ) -> Result<()> {
        let source = llvm::clean_source(&member.source_type);
        let end = self.named_extent(member)?;
        let first = self.object.fields.len();
        let mut parts = self.optional_parts(member, depth + 1, true)?;
        ensure!(parts.flags.len() == 1 && parts.values.len() == 1 && parts.extra.is_empty(),
            "optional {path} requires one unique compiler bool flag and payload, without unknown storage (flags={}, payloads={}, extra={:?})",
            parts.flags.len(), parts.values.len(), parts.extra);
        let flag = parts.flags.pop().context("missing optional flag")?;
        let original = parts.values.pop().context("missing optional payload")?;
        let mut payload = original.clone();
        let inner = source
            .split_once('<')
            .and_then(|(_, inner)| inner.strip_suffix('>'))
            .context("malformed optional source type")?;
        // This is std::optional's specified value_type alias, not a general
        // typedef guess. Keep the original spelling in scalar field metadata.
        if [
            format!("remove_cv_t<{inner}>"),
            format!("std::remove_cv_t<{inner}>"),
        ]
        .contains(&llvm::clean_source(&payload.source_type))
        {
            payload.source_type = inner.into();
            self.object.validation.push(format!(
                "{path}: compiler optional payload alias {} resolves to {inner}",
                original.source_type
            ));
        }
        let flag_path = joined(path, "has_value");
        let condition = guard
            .map(|outer| format!("{outer} && {flag_path}"))
            .unwrap_or_else(|| flag_path.clone());
        self.scalar_field(&flag, &flag_path, guard)?;
        for (union, selected) in parts.unions {
            let selected = if selected == original {
                &payload
            } else {
                &selected
            };
            self.mark_inactive(&union, selected, path, Some(&condition))?;
        }
        let payload_path = joined(path, "value");
        self.walk(&payload, &payload_path, Some(&condition), depth + 1)?;
        for field in &mut self.object.fields[first..] {
            if field.path == payload_path {
                field.source_type = original.source_type.clone();
            }
            if let Some(end) = end {
                ensure!(
                    field.offset >= member.offset && field.offset + field.size <= end,
                    "optional field outside compiler subobject: {path}"
                );
            }
        }
        self.object.validation.push(format!(
            "{}: optional flag {} at {} and payload {} at {} identified from compiler members; payload guard {condition}",
            joined(self.region, path), flag.name, flag.offset, payload.name, payload.offset
        ));
        Ok(())
    }

    fn optional_parts(
        &mut self,
        member: &RecordMember,
        depth: usize,
        root: bool,
    ) -> Result<OptionalParts> {
        self.visit(depth)?;
        self.bounds(member.offset, 0)?;
        let mut result = OptionalParts::default();
        if !root && FLAGS.contains(&member.name.as_str()) {
            ensure!(
                self.scalar(&member.source_type).is_some_and(|s| s.boolean)
                    && member.bit_width.is_none()
                    && member.children.is_empty(),
                "optional engagement member must be a compiler bool, not {}",
                member.source_type
            );
            result.flags.push(member.clone());
            return Ok(result);
        }
        if !root && VALUES.contains(&member.name.as_str()) {
            result.values.push(member.clone()); // Do not discover flags inside a nested payload.
            return Ok(result);
        }
        if member.is_empty {
            return Ok(result);
        }
        let children = self.members(member)?;
        if children.is_empty() && self.record(&member.source_type)?.is_none() {
            result
                .extra
                .push(format!("{}: {}", member.name, member.source_type));
        }
        let mut branches = children
            .iter()
            .map(|child| self.optional_parts(child, depth + 1, false))
            .collect::<Result<Vec<_>>>()?;
        if self.is_union(member)? {
            ensure!(
                branches.iter().all(|branch| branch.flags.is_empty()),
                "optional engagement flag inside a union is unsupported"
            );
            if branches
                .iter()
                .map(|branch| branch.values.len())
                .sum::<usize>()
                == 1
            {
                let index = branches
                    .iter()
                    .position(|branch| !branch.values.is_empty())
                    .context("missing optional payload branch")?;
                let mut selected = branches.remove(index);
                selected
                    .unions
                    .push((member.clone(), children[index].clone()));
                return Ok(selected); // Other union alternatives are not active payload storage.
            }
        }
        for branch in branches {
            result.flags.extend(branch.flags);
            result.values.extend(branch.values);
            result.extra.extend(branch.extra);
            result.unions.extend(branch.unions);
        }
        Ok(result)
    }

    pub(super) fn mutex(
        &mut self,
        member: &RecordMember,
        path: &str,
        guard: Option<&str>,
        depth: usize,
    ) -> Result<()> {
        // Only the exact standard-library type gets this exception, not classes
        // named Mutex or user unions that happen to have the same byte size.
        let node = self
            .storage
            .named_at("std::mutex", member.offset)?
            .context("std::mutex requires its exact nested LLVM named subobject")?
            .clone();
        let end = self.bounds(node.offset, node.size)?;
        if let Some(record) = self.record(&member.source_type)? {
            ensure!(
                record.size == node.size,
                "std::mutex Clang/LLVM size mismatch"
            );
            check_alignment(record, node.alignment)?;
        }
        let mut names = Vec::new();
        self.mutex_names(member, depth + 1, &mut names)?;
        let leaves: Vec<_> = self
            .storage
            .leaves
            .range(node.offset..end)
            .map(|(_, leaf)| leaf.clone())
            .collect();
        for leaf in leaves {
            ensure!(
                leaf.offset + leaf.size <= end,
                "LLVM mutex leaf exceeds its typed subobject"
            );
            let candidates: Vec<_> = names.iter().filter(|m| m.offset == leaf.offset).collect();
            let named = candidates
                .first()
                .filter(|_| candidates.len() == 1)
                .copied()
                .filter(|m| names.iter().filter(|other| other.name == m.name).count() == 1);
            let (name, source_type) = if let Some(source) = named {
                if self.scalar(&source.source_type).is_none() {
                    self.object.validation.push(format!(
                        "{}: opaque runtime typedef {:?} uses exact LLVM leaf {} at {}; no source width guessed",
                        joined(path, &source.name), source.source_type, leaf.ty, leaf.offset
                    ));
                }
                (source.name.clone(), source.source_type.clone())
            } else {
                (
                    format!("storage_{}", leaf.offset - node.offset),
                    "opaque std::mutex runtime storage".into(),
                )
            };
            let mut field = from_leaf(member, joined(path, &name), &leaf, guard);
            field.source_type = source_type;
            field.runtime = true;
            self.insert(field)?;
        }
        self.object.validation.push(format!(
            "{}: std::mutex runtime representation is opaque; frame all compiler-typed LLVM storage, including union tail arrays; preserve pointers; no synchronization execution is assumed",
            joined(self.region, path)
        ));
        Ok(())
    }

    fn mutex_names(
        &mut self,
        member: &RecordMember,
        depth: usize,
        names: &mut Vec<RecordMember>,
    ) -> Result<()> {
        self.visit(depth)?;
        if member.is_empty || member.bit_width.is_some() {
            return Ok(());
        }
        if llvm::source_array(&member.source_type)?.is_some() {
            let (count, element, stride) = match self.array_parts(member) {
                Ok(parts) => parts,
                Err(error) => {
                    // Source names are hints in this opaque runtime region;
                    // unsupported inactive aliases must not invent a stride.
                    self.object.validation.push(format!(
                        "opaque mutex member {} has no checked source array stride ({error}); retain exact LLVM storage names",
                        member.name
                    ));
                    return Ok(());
                }
            };
            for index in 0..count {
                let offset = index
                    .checked_mul(stride)
                    .and_then(|n| n.checked_add(member.offset))
                    .context("mutex array offset overflow")?;
                let mut value = llvm::shifted(member, member.offset, offset)?;
                value.source_type = element.clone();
                value.name = format!("{}[{index}]", member.name);
                self.mutex_names(&value, depth + 1, names)?;
            }
            return Ok(());
        }
        let children = self.members(member)?;
        if !children.is_empty() {
            for child in &children {
                self.mutex_names(child, depth + 1, names)?;
            }
        } else if !member.name.is_empty() {
            if let Some(leaf) = self.storage.leaves.get(&member.offset) {
                let scalar = self.scalar(&member.source_type);
                if scalar.as_ref().is_none_or(|scalar| scalar.matches(leaf))
                    && (scalar.is_some() || !record_spelling(&member.source_type))
                {
                    names.push(member.clone());
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct OptionalParts {
    flags: Vec<RecordMember>,
    values: Vec<RecordMember>,
    extra: Vec<String>,
    unions: Vec<(RecordMember, RecordMember)>,
}
