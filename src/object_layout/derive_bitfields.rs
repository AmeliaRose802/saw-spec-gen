//! Reconcile Clang bit positions with LLVM words, then finish semantic coverage.

use super::*;

fn integer_bits(ty: &str) -> Option<usize> {
    let digits = ty.strip_prefix('i')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|bits| (1..=128).contains(bits))
}

fn bit_range(field: &FieldLayout) -> Option<(usize, usize, usize)> {
    let bits = integer_bits(&field.llvm_type)?;
    let start = field.bit_offset?;
    let width = field.bit_width?;
    let end = start.checked_add(width)?;
    (width > 0 && end <= bits && field.size == bits.div_ceil(8) && !field.is_pointer)
        .then_some((bits, start, end))
}

impl Deriver<'_> {
    pub(super) fn bitfield(
        &mut self,
        member: &RecordMember,
        path: &str,
        guard: Option<&str>,
    ) -> Result<()> {
        let width = member.bit_width.context("missing bitfield width")?;
        ensure!(
            member.children.is_empty() && !member.is_base,
            "bitfield has record children/base metadata"
        );
        ensure!(
            width == 0 || member.bit_offset.is_some(),
            "nonzero bitfield lacks compiler bit offset"
        );
        let start = member
            .offset
            .checked_mul(8)
            .and_then(|n| n.checked_add(member.bit_offset.unwrap_or(0)))
            .context("bitfield offset overflow")?;
        let end = start
            .checked_add(width)
            .context("bitfield extent overflow")?;
        ensure!(
            end <= self
                .object
                .size
                .checked_mul(8)
                .context("object bit size overflow")?,
            "bitfield outside object bounds"
        );
        if width == 0 {
            self.object.validation.push(format!(
                "{path}: zero-width bitfield alignment barrier at Clang byte {}; no semantic field or occupied bits",
                member.offset
            ));
            return Ok(());
        }
        let leaf = self
            .storage
            .leaves
            .range(..=start / 8)
            .next_back()
            .map(|(_, leaf)| leaf)
            .filter(|leaf| end <= (leaf.offset + leaf.size) * 8)
            .cloned();
        // Even an unnamed declaration occupies compiler-identified bits. Reserve
        // every intersecting backing cell so byte padding never aliases a word.
        let first = self
            .storage
            .leaves
            .range(..=start / 8)
            .next_back()
            .map_or(start / 8, |(&offset, _)| offset);
        self.occupied.push((start / 8, end.div_ceil(8)));
        for (_, backing) in self.storage.leaves.range(first..end.div_ceil(8)) {
            if backing.offset + backing.size > start / 8 {
                self.occupied
                    .push((backing.offset, backing.offset + backing.size));
            }
        }
        let storage = leaf
            .as_ref()
            .map(|leaf| {
                format!(
                    "exact LLVM storage {} at {} ({} bytes)",
                    leaf.ty, leaf.offset, leaf.size
                )
            })
            .unwrap_or_else(|| "no single containing LLVM storage leaf".into());
        if member.name.is_empty() {
            self.object.validation.push(format!(
                "{path}: unnamed bitfield, compiler bit padding at Clang bits {start}..{end}; {storage}; backing range reserved, not a semantic field"
            ));
            return Ok(());
        }
        // Offset/size/type identify the backing word, not sizeof(source_type).
        // bit_offset is relative to that word; signed fields retain raw low bits.
        let mut field = FieldLayout {
            path: path.into(),
            source_type: member.source_type.clone(),
            llvm_type: String::new(),
            offset: member.offset,
            size: 0,
            bit_offset: member.bit_offset,
            bit_width: Some(width),
            array_count: None,
            array_stride: None,
            guard: guard.map(str::to_owned),
            validity: None,
            is_pointer: false,
            runtime: false,
        };
        if let Some(leaf) = leaf {
            field.offset = leaf.offset;
            field.size = leaf.size;
            field.llvm_type = leaf.ty;
            field.bit_offset = Some(start - leaf.offset * 8);
            field.is_pointer = leaf.pointer;
        }
        let scalar = self.scalar(&member.source_type);
        if scalar.as_ref().is_some_and(|s| s.boolean) {
            field.validity = Some("bool".into());
        }
        let source = llvm::clean_source(&member.source_type);
        let integral = scalar.as_ref().map_or_else(
            || {
                !record_spelling(&source)
                    && !matches!(
                        source.as_str(),
                        "long double"
                            | "_Float16"
                            | "_Float32"
                            | "_Float64"
                            | "_Float128"
                            | "_Float32x"
                            | "_Float64x"
                            | "_Float128x"
                            | "__float128"
                            | "__fp16"
                            | "__bf16"
                            | "f16"
                            | "f32"
                            | "f64"
                            | "f128"
                    )
            },
            |s| !s.pointer && s.ty.starts_with('i'),
        );
        let reason = if !self.dl.little_endian {
            Some("bit placement requires a little-endian target")
        } else if !integral {
            Some("source bitfield is not an integer or enum")
        } else if bit_range(&field).is_none() {
            Some("requires one containing integer LLVM iN leaf (1 <= N <= 128) and a nonzero in-range bit span")
        } else {
            None
        };
        let detail = format!(
            "bitfield {path}: Clang byte {}, bit {:?}, width {width}; {storage}",
            member.offset, member.bit_offset
        );
        if let Some(reason) = reason {
            self.object.unresolved.push(format!(
                "{detail}; {reason}; refusing unsupported bitfield emission"
            ));
        } else {
            self.object.validation.push(format!(
                "{detail}; checked little-endian semantic bitvector [{width}]"
            ));
        }
        self.insert(field)
    }

    pub(super) fn finish(&mut self) -> Result<()> {
        self.object
            .fields
            .sort_by(|a, b| (a.offset, &a.path).cmp(&(b.offset, &b.path)));
        let flags: BTreeSet<_> = self
            .object
            .fields
            .iter()
            .filter(|field| field.validity.as_deref() == Some("bool"))
            .map(|field| field.path.as_str())
            .collect();
        for field in &self.object.fields {
            if let Some(guard) = &field.guard {
                for flag in guard.split(" && ") {
                    ensure!(flags.contains(flag), "missing optional guard field {flag}");
                }
            }
        }
        check_overlaps(&self.object.fields)?;
        padding_notes(&mut self.object);
        // Conditional payloads remain occupied. Inactive union capacity is
        // labelled separately, never misreported as compiler byte padding.
        let mut events: BTreeMap<usize, Vec<(bool, Option<usize>)>> = BTreeMap::new();
        events.entry(0).or_default();
        events.entry(self.object.size).or_default();
        for &(start, end) in &self.occupied {
            if start == end {
                continue;
            }
            events.entry(start).or_default().push((true, None));
            events.entry(end).or_default().push((false, None));
        }
        for (index, range) in self.inactive.iter().enumerate() {
            events
                .entry(range.offset)
                .or_default()
                .push((true, Some(index)));
            events
                .entry(range.offset + range.size)
                .or_default()
                .push((false, Some(index)));
        }
        let (mut cursor, mut occupied) = (0, 0usize);
        let mut inactive: BTreeSet<(usize, usize)> = BTreeSet::new();
        for (offset, changes) in events {
            if cursor < offset && occupied == 0 {
                let reason = inactive
                    .iter()
                    .next()
                    .map(|&(_, index)| self.inactive[index].reason.clone())
                    .unwrap_or_else(|| "compiler padding".into());
                if let Some(last) = self
                    .object
                    .padding
                    .last_mut()
                    .filter(|last| last.offset + last.size == cursor && last.reason == reason)
                {
                    last.size += offset - cursor;
                } else {
                    self.object.padding.push(ByteRange {
                        offset: cursor,
                        size: offset - cursor,
                        reason,
                    });
                }
            }
            for (start, index) in changes {
                if let Some(index) = index {
                    let key = (self.inactive[index].size, index);
                    if start {
                        inactive.insert(key);
                    } else {
                        inactive.remove(&key);
                    }
                } else if start {
                    occupied += 1;
                } else {
                    occupied -= 1;
                }
            }
            cursor = offset;
        }
        Ok(())
    }
}

fn check_overlaps(fields: &[FieldLayout]) -> Result<()> {
    let mut sorted: Vec<_> = fields.iter().filter(|f| f.size > 0).collect();
    sorted.sort_by_key(|f| (f.offset, f.bit_offset.unwrap_or(0), &f.path));
    for pair in sorted.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.offset + a.size <= b.offset {
            continue;
        }
        let same_word = a.offset == b.offset && a.size == b.size && a.llvm_type == b.llvm_type;
        let disjoint = bit_range(a)
            .zip(bit_range(b))
            .is_some_and(|((_, _, end), (_, start, _))| end <= start);
        ensure!(
            same_word && disjoint,
            "overlapping semantic fields {} and {}",
            a.path,
            b.path
        );
        ensure!(
            a.guard == b.guard,
            "bitfield storage guards disagree for {} and {}",
            a.path,
            b.path
        );
    }
    Ok(())
}

fn padding_notes(object: &mut ObjectLayout) {
    let mut words = BTreeMap::<_, Vec<_>>::new();
    for field in &object.fields {
        if let Some((bits, start, end)) = bit_range(field) {
            words
                .entry((field.offset, field.size, &field.llvm_type, bits))
                .or_default()
                .push((start, end));
        }
    }
    for ((offset, size, ty, bits), mut ranges) in words {
        ranges.sort_unstable();
        let mut cursor = 0;
        for (start, end) in ranges.into_iter().chain(std::iter::once((bits, bits))) {
            if cursor < start {
                object.validation.push(format!(
                    "compiler bit padding: LLVM {ty} at byte {offset} ({size} bytes), bits {cursor}..{start} unasserted; all named bitfields retained"
                ));
            }
            cursor = cursor.max(end);
        }
    }
}
