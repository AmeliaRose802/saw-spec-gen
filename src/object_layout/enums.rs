//! C++ enum representation validity, separate from user semantic constraints.
//! Only field validity and validation notes change; unresolved layout facts stay.

use crate::clang_ast::AstNode;
use crate::object_layout::{clang::normalize_name, FieldLayout, ObjectLayout};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Apply source enum facts after storage derivation and before plan validation.
/// Failure records a diagnostic but commits no field changes. Runtime-owned
/// cells and pointers are outside this pass, including filtered system types.
pub fn apply(layout: &mut ObjectLayout, ast: &AstNode) -> anyhow::Result<()> {
    let index = Index::new(ast);
    let mut updates = Vec::new();
    let mut notes = Vec::new();
    for (position, field) in layout.fields.iter().enumerate() {
        if field.runtime || field.is_pointer {
            continue;
        }
        if field
            .validity
            .as_deref()
            .is_some_and(|v| v.starts_with("active_variant:"))
        {
            continue; // Explicit active-member scope, not an enum validity fact.
        }
        let (name, explicit) = source_name(&field.source_type);
        if !explicit && builtin(&name) {
            continue;
        }
        let checked = (|| -> Result<_> {
            let Some((qualified, declarations)) = index.find(&name)? else {
                let previously_enum = field
                    .validity
                    .as_deref()
                    .is_some_and(|tag| tag.starts_with("enum_"));
                ensure!(
                    !explicit && !previously_enum,
                    "missing EnumDecl for {name:?}; the AST may have been filtered"
                );
                return Ok(None);
            };
            let bits = effective_bits(field)
                .context("expected integer iN storage and a valid bit span, 1 <= N <= 128")?;
            let bounds = enum_bounds(declarations[0])?;
            for declaration in &declarations[1..] {
                ensure!(
                    enum_bounds(declaration)? == bounds,
                    "conflicting EnumDecl validity for {qualified}"
                );
            }
            let (tag, detail) = match bounds {
                Some((tag, required)) => {
                    ensure!(
                        bits >= required,
                        "{qualified} requires {required} enum bits, but the field has {bits}; refusing truncation of nonfixed enum validity"
                    );
                    let detail = format!("nonfixed enum {qualified}: {tag} on [{bits}]");
                    (Some(tag), detail)
                }
                None => (
                    None,
                    format!("fixed/scoped enum {qualified}: all underlying values valid; bitfields retain their raw width; no enumerator-only restriction"),
                ),
            };
            Ok(Some((tag, detail)))
        })();
        match checked {
            Ok(Some((tag, detail))) => {
                updates.push((position, tag));
                notes.push(format!(
                    "{}: C++ representation validity, {detail}; user constraints remain separate",
                    field.path
                ));
            }
            Ok(None) if field.validity.as_deref() != Some("bool") => notes.push(format!(
                "warning: {}: unresolved enum validity for source scalar {:?}; no matching EnumDecl (possibly a typedef or filtered AST); no restriction guessed; user constraints remain separate",
                field.path, field.source_type
            )),
            Ok(None) => {}
            Err(error) => {
                let message = format!(
                    "{}: unresolved enum validity for {:?}: {error:#}",
                    field.path, field.source_type
                );
                note(&mut layout.validation, message.clone());
                return Err(error.context(message));
            }
        }
    }
    for (position, tag) in updates {
        layout.fields[position].validity = tag;
    }
    for message in notes {
        note(&mut layout.validation, message);
    }
    Ok(())
}

fn note(notes: &mut Vec<String>, message: String) {
    if !notes.contains(&message) {
        notes.push(message);
    }
}

fn source_name(source: &str) -> (String, bool) {
    // normalize_name strips class/struct/union, but deliberately NOT enum.
    let normalized = normalize_name(source);
    let mut words: Vec<_> = normalized.split_whitespace().collect();
    while words
        .first()
        .is_some_and(|w| matches!(*w, "const" | "volatile"))
    {
        words.remove(0);
    }
    while words
        .last()
        .is_some_and(|w| matches!(*w, "const" | "volatile"))
    {
        words.pop();
    }
    let explicit = words.first() == Some(&"enum");
    if explicit {
        words.remove(0);
    }
    (words.join(" "), explicit)
}

fn builtin(name: &str) -> bool {
    const WORDS: &str = "bool _Bool char signed unsigned short int long wchar_t char8_t char16_t \
        char32_t __int8 __int16 __int32 __int64 __int128 float double void";
    !name.is_empty()
        && name
            .split_whitespace()
            .all(|word| WORDS.split_whitespace().any(|known| known == word))
}

#[derive(Default)]
struct Index<'a> {
    declarations: BTreeMap<String, Vec<&'a AstNode>>,
    enum_names: BTreeSet<String>,
    // None is an ambiguity marker, never a usable short-name alias.
    aliases: BTreeMap<String, Option<String>>,
}

impl<'a> Index<'a> {
    fn new(ast: &'a AstNode) -> Self {
        let mut index = Self::default();
        let mut pending = vec![(ast, String::new())];
        while let Some((node, mut scope)) = pending.pop() {
            if node.is_function_like() {
                continue; // Local declarations must not leak into namespace scope.
            }
            let repeated = node.is_record()
                && node.is_implicit == Some(true)
                && scope.rsplit("::").next() == node.name.as_deref();
            let named_type = node.is_record()
                || matches!(
                    node.kind.as_str(),
                    "EnumDecl" | "TypedefDecl" | "TypeAliasDecl"
                );
            if named_type && !repeated {
                if let Some(name) = node.name.as_deref().filter(|s| !s.is_empty()) {
                    let name = qualified(&scope, name);
                    let short = name.rsplit("::").next().unwrap_or(&name);
                    let alias = index
                        .aliases
                        .entry(short.into())
                        .or_insert(Some(name.clone()));
                    if alias.as_ref() != Some(&name) {
                        *alias = None; // A typedef/record can also shadow an enum alias.
                    }
                    if node.kind == "EnumDecl" {
                        index.enum_names.insert(short.into());
                        index.declarations.entry(name).or_default().push(node);
                    }
                }
            }
            if node.kind == "EnumDecl" {
                continue; // Evaluate only enums needed by non-runtime fields.
            }
            if node.kind == "NamespaceDecl" || (node.is_record() && !repeated) {
                let name = node
                    .name
                    .as_deref()
                    .unwrap_or(if node.kind == "NamespaceDecl" {
                        "(anonymous namespace)"
                    } else {
                        ""
                    });
                if name.is_empty() {
                    continue; // Do not invent a scope for an anonymous record.
                }
                scope = qualified(&scope, name);
            }
            pending.extend(node.inner.iter().rev().map(|child| (child, scope.clone())));
        }
        index
    }

    fn find(&self, name: &str) -> Result<Option<(&str, &[&'a AstNode])>> {
        let key = if name.contains("::") {
            name.strip_prefix("::").unwrap_or(name)
        } else {
            if !self.enum_names.contains(name) {
                return Ok(None); // Ambiguous ordinary typedefs are not known enums.
            }
            match self.aliases.get(name) {
                Some(Some(qualified)) => qualified,
                Some(None) => {
                    bail!("ambiguous enum name {name:?}; use a fully qualified source type")
                }
                None => return Ok(None),
            }
        };
        // Qualified names are exact: never guess from a namespace suffix.
        Ok(self
            .declarations
            .get_key_value(key)
            .map(|(name, nodes)| (name.as_str(), nodes.as_slice())))
    }
}

fn qualified(scope: &str, name: &str) -> String {
    if scope.is_empty() || name.starts_with("::") {
        normalize_name(name.trim_start_matches("::"))
    } else {
        format!("{scope}::{}", normalize_name(name))
    }
}

fn effective_bits(field: &FieldLayout) -> Option<usize> {
    let digits = field.llvm_type.trim().strip_prefix('i')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let bits = digits
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=128).contains(n))?;
    match (field.bit_offset, field.bit_width) {
        (None, None) => Some(bits),
        (Some(offset), Some(width)) if width > 0 && offset.checked_add(width)? <= bits => {
            Some(width)
        }
        _ => None,
    }
}

fn metadata(node: &AstNode) -> bool {
    node.kind.ends_with("Attr") || node.kind == "FullComment"
}

fn enum_bounds(node: &AstNode) -> Result<Option<(String, usize)>> {
    ensure!(
        node.extra.get("isInvalid") != Some(&Value::Bool(true)),
        "invalid EnumDecl"
    );
    let scoped = node.extra.get("scopedEnumTag");
    ensure!(
        scoped.is_none_or(|tag| matches!(tag.as_str(), Some("class" | "struct"))),
        "malformed scopedEnumTag"
    );
    if node.fixed_underlying_type.is_some() || scoped.is_some() {
        return Ok(None); // [dcl.enum]/8: values are those of the underlying type.
    }
    let (mut negative, mut positive) = (0u128, 0u128);
    let mut previous: Option<Number> = None;
    for constant in node.inner.iter().filter(|n| !metadata(n)) {
        ensure!(
            constant.kind == "EnumConstantDecl",
            "unexpected child of EnumDecl"
        );
        let name = constant.name.as_deref().filter(|s| !s.is_empty());
        let name = name.context("unnamed EnumConstantDecl")?;
        ensure!(
            constant.extra.get("isInvalid") != Some(&Value::Bool(true)),
            "invalid EnumConstantDecl"
        );
        let expressions: Vec<_> = constant.inner.iter().filter(|n| !metadata(n)).collect();
        let value = if let Some(value) = &constant.value {
            Number::parse(value)?
        } else {
            match expressions.as_slice() {
                [] => {
                    ensure!(
                        constant.extra.get("hasInit") != Some(&Value::Bool(true))
                            && !constant.extra.contains_key("init"),
                        "missing enumerator initializer"
                    );
                    previous.map(Number::next).transpose()?.unwrap_or_default()
                }
                [expression] => {
                    evaluated(expression, 0).with_context(|| format!("enumerator {name}"))?
                }
                _ => bail!("multiple enumerator initializers"),
            }
        };
        if value.negative {
            negative = negative.max(value.magnitude);
        } else {
            positive = positive.max(value.magnitude);
        }
        previous = Some(value);
    }
    if negative == 0 {
        // [dcl.enum]/8: empty means a single zero. [class.bit]/2: the
        // smallest named bitfield has width one, so zero-only admits 0..1.
        let bits = (128 - positive.leading_zeros()).max(1);
        let max = u128::MAX >> (128 - bits);
        Ok(Some((format!("enum_unsigned:{max}"), bits as usize)))
    } else {
        let negative = negative
            .checked_sub(1)
            .context("negative bound underflow")?;
        let exponent = (128 - positive.leading_zeros()).max(128 - negative.leading_zeros());
        ensure!(exponent < 128, "enum range requires more than 128 bits");
        let magnitude = 1u128.checked_shl(exponent).context("enum bound overflow")?;
        let max = magnitude.checked_sub(1).context("enum bound underflow")?;
        Ok(Some((
            format!("enum_signed:-{magnitude}:{max}"),
            exponent as usize + 1,
        )))
    }
}

#[derive(Clone, Copy, Default)]
struct Number {
    negative: bool,
    magnitude: u128,
}

impl Number {
    fn parse(value: &Value) -> Result<Self> {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
            _ => bail!("enumerator value is not an evaluated integer"),
        };
        let negative = text.starts_with('-');
        let digits = text.strip_prefix(['-', '+']).unwrap_or(&text);
        ensure!(
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
            "invalid evaluated enumerator value {text:?}"
        );
        let magnitude = digits
            .parse::<u128>()
            .context("enumerator magnitude exceeds 128 bits")?;
        ensure!(
            !negative || magnitude <= (1u128 << 127),
            "negative enumerator exceeds signed 128-bit range"
        );
        Ok(Self {
            negative: negative && magnitude != 0,
            magnitude,
        })
    }

    fn next(self) -> Result<Self> {
        let magnitude = if self.negative {
            self.magnitude.checked_sub(1)
        } else {
            self.magnitude.checked_add(1)
        }
        .context("implicit enumerator increment overflow")?;
        Ok(Self {
            negative: self.negative && magnitude != 0,
            magnitude,
        })
    }
}

fn evaluated(node: &AstNode, depth: usize) -> Result<Number> {
    ensure!(depth < 128, "enum initializer nesting limit exceeded");
    ensure!(
        node.extra.get("isInvalid") != Some(&Value::Bool(true)),
        "invalid enum initializer"
    );
    if matches!(
        node.kind.as_str(),
        "ConstantExpr" | "IntegerLiteral" | "UnaryOperator"
    ) {
        if let Some(value) = &node.value {
            return Number::parse(value); // The evaluated result beats all subexpressions.
        }
    }
    let [child] = node.inner.as_slice() else {
        bail!("missing evaluated integer for {}", node.kind);
    };
    match node.kind.as_str() {
        "ConstantExpr" | "ParenExpr" => evaluated(child, depth + 1),
        // The outer conversion to the enum's chosen underlying type preserves
        // every enumerator. Other casts need Clang's evaluated ConstantExpr.
        "ImplicitCastExpr"
            if (depth == 0 && child.kind == "ConstantExpr")
                || node.extra.get("castKind").and_then(Value::as_str) == Some("NoOp") =>
        {
            evaluated(child, depth + 1)
        }
        "UnaryOperator" => {
            let opcode = node.extra.get("opcode").and_then(Value::as_str);
            ensure!(
                matches!(opcode, Some("+" | "-")),
                "unevaluated unary operator"
            );
            let mut value = evaluated(child, depth + 1)?;
            if opcode == Some("-") {
                // Never treat unsigned modular negation as mathematical minus.
                let ty = node.qual_type().or_else(|| child.qual_type());
                ensure!(
                    ty.is_none_or(signed_operand),
                    "unary minus needs an evaluated signed integer"
                );
                ensure!(
                    value.magnitude <= i128::MAX as u128,
                    "unevaluated unary minus overflow"
                );
                value.negative = !value.negative && value.magnitude != 0;
            }
            Ok(value)
        }
        _ => bail!("missing evaluated integer for {}", node.kind),
    }
}

fn signed_operand(ty: &str) -> bool {
    const WORDS: &str = "signed short int long __int8 __int16 __int32 __int64 __int128";
    !ty.is_empty()
        && ty
            .split_whitespace()
            .all(|word| WORDS.split_whitespace().any(|known| known == word))
}

/// A canonical Cryptol predicate for a semantic field value (an atom or an
/// already-parenthesized expression). Bitfield callers must extract/truncate
/// to bit_width first, as validate::field_expr does; never pass the backing word.
/// Negative typed literals denote two's-complement bitvectors, compared signed.
pub fn predicate(field: &FieldLayout, value: &str) -> Option<String> {
    if field.runtime || field.is_pointer {
        return None;
    }
    let tag = field.validity.as_deref()?;
    let bits = effective_bits(field)?;
    if let Some(index) = tag.strip_prefix("active_variant:") {
        let index = index.parse::<u128>().ok()?;
        return Some(format!("({value} == ({index} : [{bits}]))"));
    }
    if tag == "bool" {
        return Some(format!("({value} <= (1 : [{bits}]))"));
    }
    if let Some(max) = tag.strip_prefix("enum_unsigned:") {
        let max = max.parse::<u128>().ok()?;
        return (max <= u128::MAX >> (128 - bits))
            .then(|| format!("({value} <= ({max} : [{bits}]))"));
    }
    let (min, max) = tag.strip_prefix("enum_signed:")?.split_once(':')?;
    let min = min.parse::<i128>().ok()?;
    let max = max.parse::<u128>().ok()?;
    let magnitude = 1u128.checked_shl((bits - 1) as u32)?;
    (min < 0 && min.unsigned_abs() <= magnitude && max < magnitude)
        .then(|| format!("({value} <=$ ({max} : [{bits}]) && {value} >=$ ({min} : [{bits}]))"))
}

#[cfg(test)]
#[path = "enums_tests.rs"]
mod tests;
