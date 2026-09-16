//! Parse Clang's textual AST record layouts, not AST JSON or IRgen layouts.

use super::model::{RecordLayout, RecordMember};
use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use std::iter::Enumerate;
use std::str::Lines;

const AST_HEADER: &str = "*** Dumping AST Record Layout";

/// Keys are tag-stripped, fully qualified names; source types retain their tags.
/// Offsets (including descendants) remain relative to the outermost record.
/// Unnamed/synthetic declarations have an empty name and an explicit source type.
/// No AST blocks is valid and yields an empty map. Incomplete blocks are errors.
pub fn parse_record_layouts(text: &str) -> Result<BTreeMap<String, RecordLayout>> {
    let mut records = BTreeMap::new();
    let mut lines = text.lines().enumerate();
    while let Some((line_number, line)) = lines.next() {
        if line.trim() != AST_HEADER {
            continue;
        }
        let layout = parse_block(&mut lines)
            .with_context(|| format!("AST record layout starting at line {}", line_number + 1))?;
        let key = normalize_name(&layout.source_type);
        if let Some(previous) = records.get(&key) {
            ensure!(previous == &layout, "conflicting Clang layouts for {key}");
        } else {
            records.insert(key, layout);
        }
    }
    Ok(records)
}

fn parse_block(lines: &mut Enumerate<Lines<'_>>) -> Result<RecordLayout> {
    let mut root: Option<RecordLayout> = None;
    let mut stack = Vec::new();
    let mut summary = String::new();
    for (line_number, line) in lines.by_ref() {
        if line.trim().is_empty() {
            continue;
        }
        let (offset, declaration) = line
            .split_once('|')
            .with_context(|| format!("expected layout row at line {}: {line}", line_number + 1))?;
        let offset = offset.trim();
        if offset.is_empty() {
            let layout = root
                .as_mut()
                .context("layout summary before record header")?;
            let part = declaration.trim();
            ensure!(
                !summary.is_empty() || part.starts_with("[sizeof="),
                "unrecognized layout summary: {line}"
            );
            summary.push_str(part);
            if part.ends_with(']') {
                let (size, alignment) = parse_summary(&summary)?;
                layout.size = size;
                layout.alignment = alignment;
                while !stack.is_empty() {
                    close_member(&mut stack, &mut layout.members);
                }
                let mut pending: Vec<_> = layout.members.iter().collect();
                while let Some(member) = pending.pop() {
                    ensure!(
                        member.offset <= size,
                        "member offset exceeds sizeof: {member:?}"
                    );
                    pending.extend(&member.children);
                }
                return Ok(root.expect("record header checked"));
            }
            ensure!(
                !part.contains(']'),
                "trailing text after layout summary: {line}"
            );
            continue;
        }
        ensure!(
            summary.is_empty(),
            "member after layout summary started: {line}"
        );
        let indent = declaration.bytes().take_while(|b| *b == b' ').count();
        ensure!(indent % 2 == 1, "invalid layout indentation: {line}");
        let depth = (indent - 1) / 2;
        let (offset, bit_offset, bit_width) = parse_offset(offset)?;
        let (source, is_base, is_empty) = decorations(&declaration[indent..]);
        ensure!(!source.trim().is_empty(), "missing declaration: {line}");
        if root.is_none() {
            ensure!(
                depth == 0 && offset == 0 && bit_width.is_none() && !is_base,
                "invalid record header: {line}"
            );
            let source = source.trim_end();
            let tag = ["struct", "class", "union"].into_iter().find(|tag| {
                source
                    .strip_prefix(*tag)
                    .is_some_and(|s| s.starts_with(' '))
            });
            ensure!(
                tag.is_none_or(|tag| !source[tag.len()..].trim().is_empty()),
                "missing record name"
            );
            ensure!(
                tag.is_some()
                    || source
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "unrecognized typedef-named record header: {source}"
            );
            root = Some(RecordLayout {
                source_type: source.into(),
                size: 0, // Replaced only by the mandatory compiler summary.
                alignment: 0,
                is_union: tag == Some("union"),
                members: Vec::new(),
            });
            continue;
        }
        ensure!(
            depth > 0 && depth <= stack.len() + 1,
            "invalid member nesting: {line}"
        );
        let layout = root.as_mut().expect("record header checked");
        while stack.len() >= depth {
            close_member(&mut stack, &mut layout.members);
        }
        let (source_type, name) = if is_base || source.trim().starts_with('(') {
            (source.trim_end().to_owned(), String::new())
        } else {
            split_declaration(source)
        };
        stack.push(RecordMember {
            name,
            source_type,
            offset,
            bit_offset,
            bit_width,
            is_base,
            is_empty,
            children: Vec::new(),
        });
    }
    bail!("unterminated AST record layout (missing sizeof/align summary)")
}

fn close_member(stack: &mut Vec<RecordMember>, members: &mut Vec<RecordMember>) {
    let member = stack.pop().expect("nonempty member stack");
    if let Some(parent) = stack.last_mut() {
        parent.children.push(member);
    } else {
        members.push(member);
    }
}

fn number(text: &str) -> Result<usize> {
    ensure!(
        !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()),
        "invalid nonnegative layout integer: {text:?}"
    );
    text.parse()
        .with_context(|| format!("layout integer overflow: {text}"))
}

fn parse_offset(text: &str) -> Result<(usize, Option<usize>, Option<usize>)> {
    let Some((byte, bits)) = text.split_once(':') else {
        return Ok((number(text)?, None, None));
    };
    let byte = number(byte)?;
    if bits == "-" {
        // A zero-width alignment barrier has no reported bit position.
        return Ok((byte, None, Some(0)));
    }
    let (first, last) = bits.split_once('-').context("malformed bit-field range")?;
    let first = number(first)?;
    let last = number(last)?;
    let width = last
        .checked_sub(first)
        .and_then(|width| width.checked_add(1))
        .context("reversed or overflowing bit-field range")?;
    Ok((byte, Some(first), Some(width)))
}

fn parse_summary(text: &str) -> Result<(usize, usize)> {
    let inner = text
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .context("malformed layout summary")?;
    let mut values = BTreeMap::new();
    for entry in inner.split(',') {
        let (key, value) = entry
            .trim()
            .split_once('=')
            .context("malformed layout property")?;
        let key = key.trim();
        ensure!(
            matches!(
                key,
                "sizeof" | "align" | "dsize" | "nvsize" | "nvalign" | "preferredalign"
            ),
            "unsupported layout summary property: {key}"
        );
        ensure!(
            values.insert(key, number(value.trim())?).is_none(),
            "duplicate layout summary property: {key}"
        );
    }
    let size = *values
        .get("sizeof")
        .context("missing sizeof in layout summary")?;
    let alignment = *values
        .get("align")
        .context("missing align in layout summary")?;
    ensure!(alignment > 0, "zero record alignment");
    Ok((size, alignment))
}

fn decorations(mut source: &str) -> (&str, bool, bool) {
    let mut is_base = false;
    let mut is_empty = false;
    loop {
        let trimmed = source.trim_end();
        if let Some(rest) = trimmed.strip_suffix(" (empty)") {
            source = rest;
            is_empty = true;
        } else if let Some(rest) = [
            " (base)",
            " (primary base)",
            " (virtual base)",
            " (primary virtual base)",
        ]
        .into_iter()
        .find_map(|suffix| trimmed.strip_suffix(suffix))
        {
            source = rest;
            is_base = true;
        } else {
            return (source, is_base, is_empty);
        }
    }
}

fn type_word(word: &str) -> bool {
    matches!(
        word,
        "struct"
            | "class"
            | "union"
            | "enum"
            | "const"
            | "volatile"
            | "restrict"
            | "signed"
            | "unsigned"
            | "short"
            | "long"
            | "int"
            | "char"
            | "float"
            | "double"
            | "void"
            | "bool"
            | "_Bool"
            | "wchar_t"
            | "char8_t"
            | "char16_t"
            | "char32_t"
            | "__int128"
            | "__int64"
            | "__int32"
            | "__int16"
            | "__int8"
    )
}

fn split_declaration(source: &str) -> (String, String) {
    let trimmed = source.trim_end();
    // Clang prints the separator even when FieldDecl has no name. Preserve it
    // until this point: rsplit_whitespace alone loses anonymous unions/bitfields.
    if source.len() == trimmed.len() {
        if let Some((ty, name)) = trimmed.rsplit_once(char::is_whitespace) {
            let mut chars = name.chars();
            let identifier = chars
                .next()
                .is_some_and(|c| c == '_' || c == '$' || c.is_alphabetic())
                && chars.all(|c| c == '_' || c == '$' || c.is_alphanumeric());
            // Without a declarator, "enum E" and "const volatile E" are types.
            let qualifier_only = ty.split_whitespace().all(|word| {
                matches!(
                    word,
                    "struct" | "class" | "union" | "enum" | "const" | "volatile" | "restrict"
                )
            });
            if identifier && !type_word(name) && !qualifier_only {
                return (ty.trim_end().into(), name.into());
            }
        }
    }
    // Unknown spellings remain explicit/unresolvable, never a guessed scalar.
    (trimmed.into(), String::new())
}

/// Remove standalone elaborated tags, including those in template arguments,
/// and collapse whitespace. Never strip namespaces, LLVM prefixes, or numeric
/// suffixes. Source-location descriptors and quoted literals are kept intact.
pub fn normalize_name(text: &str) -> String {
    let mut rest = text.trim();
    let mut output = String::new();
    let mut space = false;
    while !rest.is_empty() {
        let ch = rest.chars().next().expect("nonempty text");
        if ch.is_whitespace() {
            space = !output.is_empty();
            rest = &rest[ch.len_utf8()..];
            continue;
        }
        let boundary = space
            || output
                .chars()
                .last()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if boundary {
            if let Some(tail) = ["struct", "class", "union"].into_iter().find_map(|tag| {
                rest.strip_prefix(tag)
                    .filter(|tail| tail.starts_with(char::is_whitespace))
            }) {
                rest = tail.trim_start();
                continue;
            }
        }
        if space {
            output.push(' ');
            space = false;
        }
        let length = protected_prefix(rest).unwrap_or(ch.len_utf8());
        output.push_str(&rest[..length]);
        rest = &rest[length..];
    }
    output
}

fn protected_prefix(text: &str) -> Option<usize> {
    let first = text.chars().next()?;
    if matches!(first, '\'' | '"') {
        let mut escaped = false;
        for (index, ch) in text.char_indices().skip(1) {
            if !escaped && ch == first {
                return Some(index + ch.len_utf8());
            }
            escaped = !escaped && ch == '\\';
        }
        return Some(text.len());
    }
    if ["(anonymous ", "(unnamed ", "(lambda at "]
        .into_iter()
        .any(|prefix| text.starts_with(prefix))
    {
        let mut depth = 0;
        for (index, ch) in text.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index + 1);
                    }
                }
                _ => {}
            }
        }
        return Some(text.len());
    }
    None
}

#[cfg(test)]
#[path = "clang_tests.rs"]
mod tests;
