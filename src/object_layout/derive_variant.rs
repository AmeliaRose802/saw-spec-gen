//! Explicit initial alternatives, not automatic C++ validity or dynamic switching.
//! Only compiler MSVC _Variant_storage_ / GNU _Variadic_union chains are known.
//! Payloads must map to exact LLVM leaves: no byte splitting, typedef-width
//! guesses, aligned-buffer reinterpretation, or synthesized variant layout.
//! Empty alternatives and unrecognized scalar aliases remain unsupported.
//! The parent must enforce active_variant:N on both pre- and post-state tags.

use super::fields::from_leaf;
use super::*;

const FLAGS: &[&str] = &["_Which", "_M_index", "__index"];
const MSVC: &str = "std::_Variant_storage_";
const GNU: &str = "std::__detail::__variant::_Variadic_union";
const GNU_VALUE: &str = "std::__detail::__variant::_Uninitialized";

#[derive(Default)]
struct Parts {
    flags: Vec<RecordMember>,
    storage: Vec<(bool, RecordMember)>, // true = GNU, false = MSVC
}

impl Deriver<'_> {
    pub(super) fn variant(
        &mut self,
        member: &RecordMember,
        path: &str,
        guard: Option<&str>,
        depth: usize,
    ) -> Result<()> {
        let (_, arguments) = template(&member.source_type)?;
        ensure!(!arguments.is_empty(), "variant {path} has no alternatives");
        ensure!(!member.is_empty, "compiler variant cannot be empty");
        let key = joined(self.region, path);
        let selection = self.config.active_members.get(&key).with_context(|| {
            format!("variant {key} requires an explicit numeric active_members selection")
        })?;
        ensure!(
            !selection.is_empty() && selection.bytes().all(|b| b.is_ascii_digit()),
            "variant {key} selection must be a numeric alternative index"
        );
        let index: usize = selection.parse().context("variant index overflow")?;
        ensure!(
            index < arguments.len(),
            "variant index {index} out of range"
        );
        let end = self
            .named_extent(member)?
            .context("missing compiler variant extent")?;
        let mut parts = Parts::default();
        self.variant_parts(member, depth + 1, true, &mut parts)?;
        ensure!(
            parts.flags.len() == 1 && parts.storage.len() == 1,
            "variant {key} requires one unique tag outside one compiler storage chain"
        );
        let flag = parts.flags.pop().context("missing variant tag")?;
        let (gnu, storage) = parts.storage.pop().context("missing variant storage")?;
        // The MSVC container has only its anonymous union; GNU names the union.
        // Neither the entire variant nor a guessed largest alternative is its extent.
        let storage_end = self
            .named_extent(&storage)?
            .context("missing compiler variant storage extent")?;
        ensure!(
            storage.offset >= member.offset && storage_end <= end,
            "variant storage outside compiler subobject: {key}"
        );
        let mut values = Vec::new();
        self.variant_chain(&storage, &arguments, gnu, depth + 1, &mut values)?;
        let mut payloads = Vec::new();
        for (value, expected) in values.iter().zip(&arguments) {
            let actual = value_type(&value.source_type)?;
            ensure!(
                actual == llvm::clean_source(expected) || (gnu && actual == "_Type"),
                "variant alternative type {} disagrees with source argument {expected}",
                value.source_type
            );
            let mut payload = value.clone();
            // _Type is accepted only inside the checked GNU _Uninitialized<T>.
            payload.source_type = expected.clone();
            let payload_end = self.variant_value_end(&payload)?;
            ensure!(
                payload.offset >= storage.offset && payload_end <= storage_end,
                "variant alternative outside compiler union storage: {expected}"
            );
            payloads.push((payload, payload_end));
        }
        let leaf = self.leaf_at(flag.offset)?.clone();
        let bits = leaf
            .ty
            .strip_prefix('i')
            .and_then(|n| n.parse::<u32>().ok());
        ensure!(
            bits.is_some_and(|n| matches!(n, 8 | 16 | 32 | 64)) && !leaf.pointer,
            "variant tag requires an exact integer LLVM backing leaf"
        );
        let source = llvm::clean_source(&flag.source_type);
        ensure!(
            self.scalar(&source).map_or_else(
                || index_alias(&source),
                |s| !s.boolean && !s.pointer && s.matches(&leaf)
            ),
            "unsupported or mismatched variant tag source type {source}"
        );
        let flag_end = self.bounds(flag.offset, leaf.size)?;
        ensure!(
            flag.offset >= member.offset
                && flag_end <= end
                && (flag_end <= storage.offset || flag.offset >= storage_end),
            "variant tag must be outside alternative storage: {key}"
        );
        ensure!(
            (arguments.len() as u128) < (1u128 << bits.context("missing tag width")?),
            "variant tag cannot encode alternatives and the valueless sentinel"
        );
        let mut tag = from_leaf(&flag, joined(path, "index"), &leaf, guard);
        tag.validity = Some(format!("active_variant:{index}"));
        self.insert(tag)?;
        let (selected, selected_end) = &payloads[index];
        let payload_path = joined(path, "value");
        let first = self.object.fields.len();
        let unresolved = self.object.unresolved.len();
        self.walk(selected, &payload_path, guard, depth + 1)?;
        ensure!(
            self.object.unresolved.len() == unresolved && self.object.fields.len() > first,
            "missing/unresolved variant payload storage"
        );
        for field in &self.object.fields[first..] {
            ensure!(
                field.offset >= selected.offset && field.offset + field.size <= *selected_end,
                "variant field outside selected compiler alternative: {}",
                field.path
            );
            let tag_alias = field
                .validity
                .as_deref()
                .is_some_and(|v| v.starts_with("active_variant:"))
                && index_alias(&llvm::clean_source(&field.source_type));
            let scalar = self.scalar(&value_type(&field.source_type)?);
            let known = scalar.is_some_and(|s| {
                if field.bit_width.is_some() {
                    !s.pointer && s.ty.starts_with('i')
                } else {
                    self.storage
                        .leaves
                        .get(&field.offset)
                        .is_some_and(|leaf| s.matches(leaf))
                }
            });
            ensure!(
                known || tag_alias,
                "unrecognized variant payload source field {}: {}",
                field.path,
                field.source_type
            );
        }
        let retain_alias = value_type(&values[index].source_type)? != "_Type";
        for field in &mut self.object.fields[first..] {
            // Preserve source spelling except for GNU's context-dependent alias;
            // its checked resolution is recorded below, including its offset.
            if field.path == payload_path && retain_alias {
                field.source_type = values[index].source_type.clone();
            }
        }
        for (start, stop) in [
            (storage.offset, selected.offset),
            (*selected_end, storage_end),
        ] {
            if start < stop {
                self.inactive.push(ByteRange {
                    offset: start,
                    size: stop - start,
                    reason: format!("inactive variant storage (selected {key} index {index})"),
                });
            }
        }
        self.object.validation.push(format!(
            "{key}: selected actual compiler union member {index}"
        ));
        self.object.validation.push(format!(
            "{key}: compiler variant tag {} ({}) at {}; alternative {index} {} resolves to {} at {}; storage {}..{}; explicit initial selection, not automatic C++ validity; parent must require the selected index in pre and post state",
            flag.name, flag.source_type, flag.offset, values[index].source_type,
            selected.source_type, selected.offset, storage.offset, storage_end
        ));
        Ok(())
    }

    fn variant_parts(
        &mut self,
        member: &RecordMember,
        depth: usize,
        root: bool,
        parts: &mut Parts,
    ) -> Result<()> {
        self.visit(depth)?;
        self.bounds(member.offset, 0)?;
        ensure!(no_bits(member), "variant wrapper/tag is a bitfield");
        if !root && FLAGS.contains(&member.name.as_str()) {
            ensure!(
                !member.is_base && !member.is_empty && self.members(member)?.is_empty(),
                "invalid variant tag member"
            );
            parts.flags.push(member.clone());
            return Ok(());
        }
        let source = llvm::clean_source(&member.source_type);
        if !root && source.starts_with("std::_Variant_storage_<") {
            ensure!(
                member.is_base,
                "MSVC variant storage must be a compiler base"
            );
            parts.storage.push((false, member.clone()));
            return Ok(());
        }
        if !root && member.name == "_M_u" {
            ensure!(
                !member.is_base && self.is_union(member)?,
                "GNU variant storage must be its compiler union"
            );
            parts.storage.push((true, member.clone()));
            return Ok(());
        }
        ensure!(
            (root || member.is_base) && !self.is_union(member)? && self.scalar(&source).is_none(),
            "unknown semantic variant wrapper member {}: {source}",
            member.name
        );
        let children = self.members(member)?;
        ensure!(
            !children.is_empty() || (!root && member.is_empty),
            "missing variant wrapper members"
        );
        for child in &children {
            self.variant_parts(child, depth + 1, false, parts)?;
        }
        Ok(())
    }

    fn variant_chain(
        &mut self,
        member: &RecordMember,
        expected: &[String],
        gnu: bool,
        depth: usize,
        values: &mut Vec<RecordMember>,
    ) -> Result<()> {
        self.visit(depth)?;
        self.bounds(member.offset, 0)?;
        ensure!(no_bits(member), "variant storage is a bitfield");
        let (name, mut arguments) = template(&member.source_type)?;
        if !gnu {
            ensure!(
                name == MSVC && !self.is_union(member)? && trivial_argument(&arguments),
                "unknown MSVC variant storage"
            );
            arguments.remove(0);
        } else {
            ensure!(name == GNU && self.is_union(member)?, "GNU union mismatch");
        }
        ensure!(
            arguments == expected,
            "variant head/tail arguments do not match source alternative order"
        );
        let mut children = self.members(member)?;
        if expected.is_empty() {
            ensure!(
                member.is_empty && children.is_empty(),
                "variant terminal storage must be compiler-empty"
            );
            return Ok(());
        }
        ensure!(!member.is_empty, "nonterminal variant storage is empty");
        if !gnu {
            ensure!(
                children.len() == 1
                    && children[0].name.is_empty()
                    && !children[0].is_base
                    && children[0].offset == member.offset
                    && !children[0].is_empty
                    && no_bits(&children[0])
                    && self.is_union(&children[0])?,
                "MSVC variant storage requires only its compiler anonymous union"
            );
            self.visit(depth + 1)?;
            children = self.members(&children[0])?;
        }
        let (head, tail) = if !gnu {
            ("_Head", "_Tail")
        } else {
            ("_M_first", "_M_rest")
        };
        ensure!(
            children.len() == 2
                && children[0].name == head
                && children[1].name == tail
                && children
                    .iter()
                    .all(|c| !c.is_base && c.offset == member.offset && no_bits(c)),
            "variant storage requires an ordered {head}/{tail} chain at compiler union offsets"
        );
        let value = if !gnu {
            children[0].clone()
        } else {
            self.variant_gnu_value(&children[0], &expected[0], depth + 1)?
        };
        values.push(value);
        self.variant_chain(&children[1], &expected[1..], gnu, depth + 1, values)
    }

    fn variant_gnu_value(
        &mut self,
        member: &RecordMember,
        expected: &str,
        depth: usize,
    ) -> Result<RecordMember> {
        let (name, arguments) = template(&member.source_type)?;
        ensure!(
            name == GNU_VALUE
                && !member.is_empty
                && arguments.first().is_some_and(|a| a == expected)
                && (arguments.len() == 1
                    || (arguments.len() == 2 && trivial_argument(&arguments[1..]))),
            "GNU variant head must wrap its corresponding source alternative"
        );
        let mut pending = vec![(member.clone(), depth)];
        let mut values = Vec::new();
        while let Some((wrapper, depth)) = pending.pop() {
            self.visit(depth)?;
            self.bounds(wrapper.offset, 0)?;
            ensure!(no_bits(&wrapper), "GNU variant value wrapper is a bitfield");
            ensure!(!self.is_union(&wrapper)?, "unknown GNU value wrapper union");
            let children = self.members(&wrapper)?;
            ensure!(
                !children.is_empty() || wrapper.is_empty,
                "missing GNU variant value storage"
            );
            for child in children {
                if child.name == "_M_storage" && !child.is_base {
                    values.push(child);
                } else {
                    ensure!(
                        child.is_base
                            && self.scalar(&child.source_type).is_none()
                            && !self.is_union(&child)?,
                        "extra GNU variant value wrapper member"
                    );
                    pending.push((child, depth + 1));
                }
            }
        }
        ensure!(values.len() == 1, "GNU head needs one unique _M_storage");
        if let Some(record) = self.record(&member.source_type)? {
            let mut value = values[0].clone();
            value.source_type = expected.into();
            let end = self.bounds(member.offset, record.size)?;
            ensure!(
                self.variant_value_end(&value)? <= end,
                "GNU value outside compiler wrapper"
            );
        }
        Ok(values.remove(0))
    }

    fn variant_value_end(&self, member: &RecordMember) -> Result<usize> {
        ensure!(
            no_bits(member) && !member.is_empty,
            "empty/bitfield variant"
        );
        if let Some(scalar) = self.scalar(&member.source_type) {
            ensure!(member.children.is_empty(), "variant scalar has children");
            return self.bounds(member.offset, scalar.size);
        }
        if let Some(record) = self.record(&member.source_type)? {
            let children = record
                .members
                .iter()
                .map(|m| llvm::shifted(m, 0, member.offset))
                .collect::<Result<Vec<_>>>()?;
            ensure!(
                member.children.is_empty() || member.children == children,
                "variant inline payload differs from its source record"
            );
        }
        self.named_extent(member)?
            .context("missing independent compiler variant alternative extent")
    }
}

fn no_bits(member: &RecordMember) -> bool {
    member.bit_width.is_none() && member.bit_offset.is_none()
}

fn trivial_argument(arguments: &[String]) -> bool {
    arguments
        .first()
        .is_some_and(|a| matches!(a.as_str(), "true" | "false"))
}

fn index_alias(source: &str) -> bool {
    matches!(source, "_Index_t" | "__index_type")
}

fn value_type(source: &str) -> Result<String> {
    let source = llvm::clean_source(source);
    if source
        .split_once('<')
        .is_some_and(|(name, _)| matches!(name, "remove_cv_t" | "std::remove_cv_t"))
    {
        let (_, arguments) = template(&source)?;
        ensure!(arguments.len() == 1, "malformed variant remove_cv_t alias");
        return Ok(llvm::clean_source(&arguments[0]));
    }
    Ok(source)
}

/// Split complete template arguments, not commas nested in an alternative type.
/// Unsupported literal/expression spellings fail closed rather than lose ordering.
fn template(source: &str) -> Result<(String, Vec<String>)> {
    let source = llvm::clean_source(source);
    let (name, rest) = source
        .split_once('<')
        .context("missing variant template arguments")?;
    let inner = rest
        .strip_suffix('>')
        .context("malformed variant template type")?;
    let mut stack = Vec::new();
    let mut arguments = Vec::new();
    let mut start = 0;
    for (offset, ch) in inner.char_indices() {
        match ch {
            '<' | '(' | '[' => stack.push(ch),
            '>' | ')' | ']' => {
                let open = match ch {
                    '>' => '<',
                    ')' => '(',
                    _ => '[',
                };
                ensure!(
                    stack.pop() == Some(open),
                    "unbalanced variant template argument"
                );
            }
            ',' if stack.is_empty() => {
                arguments.push(llvm::clean_source(&inner[start..offset]));
                start = offset + 1;
            }
            '\'' | '"' | '{' | '}' => anyhow::bail!("unsupported variant template argument syntax"),
            _ => {}
        }
        ensure!(
            stack.len() < llvm::MAX_DEPTH,
            "variant template nesting limit"
        );
    }
    ensure!(stack.is_empty(), "unbalanced variant template argument");
    if !inner.trim().is_empty() || !arguments.is_empty() {
        arguments.push(llvm::clean_source(&inner[start..]));
    }
    ensure!(
        arguments.len() <= llvm::MAX_ITEMS && arguments.iter().all(|a| !a.is_empty()),
        "invalid variant template argument count"
    );
    Ok((name.trim().into(), arguments))
}
