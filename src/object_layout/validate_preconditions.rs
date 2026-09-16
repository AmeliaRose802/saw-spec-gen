//! Named semantic references and fail-closed precondition lowering.

use super::{field_expr, shapes, FieldLayout, LayoutPlan, ObjectPlan, Projection};
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};

fn pre_var(object: &ObjectPlan) -> String {
    if object.region == "return" {
        "result_pre".into()
    } else if object.mutable {
        format!("{}_pre", object.region)
    } else {
        object.region.clone()
    }
}

#[derive(Clone, Copy)]
pub(super) struct Reference<'a> {
    object: &'a ObjectPlan,
    field: Option<&'a FieldLayout>,
}
pub(super) type References<'a> = BTreeMap<String, Reference<'a>>;

impl Reference<'_> {
    fn value(self) -> Result<String> {
        let field = self
            .field
            .context("expected a named semantic leaf, not an object/aggregate")?;
        ensure!(
            !field.is_pointer,
            "pointer field {}.{} has no Cryptol value; it must be framed",
            self.object.region,
            field.path
        );
        ensure!(
            self.object.projection != Projection::Llvm
                || self.object.selectors.contains_key(&field.path),
            "missing validated LLVM selector for {}.{}",
            self.object.region,
            field.path
        );
        Ok(field_expr(self.object, field, &pre_var(self.object)))
    }

    fn guarded(self, condition: String) -> Result<String> {
        let field = self.field.context("guard requires a semantic leaf")?;
        let guards = shapes::guards(self.object, field)?
            .into_iter()
            .map(|g| {
                format!(
                    "({} == 1)",
                    field_expr(self.object, g, &pre_var(self.object))
                )
            })
            .collect::<Vec<_>>();
        // A local implication, never a guard around unrelated conjuncts.
        Ok(if guards.is_empty() {
            condition
        } else {
            format!("(if {} then {condition} else True)", guards.join(" && "))
        })
    }

    fn valid(self) -> Result<String> {
        let field = self.field.context("valid requires a named semantic leaf")?;
        ensure!(field.validity.as_deref() == Some("bool"),
            "valid({}.{}) needs compiler-proven bool storage; use an explicit named comparison for enums (no enumerator-only restriction is inferred)",
            self.object.region, field.path);
        self.guarded(format!("({} <= 1)", self.value()?))
    }
}

pub(super) fn references(plan: &LayoutPlan) -> Result<References<'_>> {
    let mut refs = BTreeMap::new();
    for object in plan.objects.values() {
        let aliases = BTreeSet::from([
            object.region.clone(),
            format!("{}_pre", object.region),
            pre_var(object),
        ]);
        for alias in aliases {
            ensure!(
                refs.insert(
                    alias.clone(),
                    Reference {
                        object,
                        field: None
                    }
                )
                .is_none(),
                "ambiguous object/pre-state name {alias}"
            );
            for field in &object.layout.fields {
                let name = format!("{alias}.{}", field.path);
                ensure!(
                    refs.insert(
                        name.clone(),
                        Reference {
                            object,
                            field: Some(field)
                        }
                    )
                    .is_none(),
                    "ambiguous named field {name}"
                );
            }
        }
    }
    Ok(refs)
}

fn resolve<'a>(text: &str, refs: &References<'a>) -> Result<Option<Reference<'a>>> {
    if let Some(found) = refs.get(text) {
        return Ok(Some(*found));
    }
    let root = text.split(['.', '[']).next().unwrap_or("");
    ensure!(
        !refs.contains_key(root),
        "unknown semantic field {text}; name an exact leaf, not padding or an aggregate"
    );
    Ok(None)
}

struct Token<'a> {
    text: &'a str,
    start: usize,
    end: usize,
    quoted: bool,
}

fn tokens(raw: &str) -> Result<Vec<Token<'_>>> {
    let (mut result, mut i) = (Vec::new(), 0);
    let bytes = raw.as_bytes();
    while i < bytes.len() {
        let ch = raw[i..]
            .chars()
            .next()
            .context("missing expression character")?;
        if ch.is_whitespace() {
            i += ch.len_utf8();
            continue;
        }
        if raw[i..].starts_with("//") {
            i += raw[i..].find('\n').unwrap_or(bytes.len() - i);
            continue;
        }
        if raw[i..].starts_with("/*") {
            let mut depth = 1;
            i += 2;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ensure!(depth == 0, "unterminated precondition comment");
            continue;
        }
        let start = i;
        let quoted = matches!(ch, '"' | '\'');
        if quoted {
            i += 1;
            while i < bytes.len() && bytes[i] != ch as u8 {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            ensure!(i < bytes.len(), "unterminated quoted precondition literal");
            i += 1;
        } else if ch.is_ascii_alphanumeric() || ch == '_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || b"_'".contains(&bytes[i]))
            {
                i += 1;
            }
            if !ch.is_ascii_digit() {
                loop {
                    let separator = if bytes[i..].starts_with(b"::") {
                        2
                    } else if bytes.get(i) == Some(&b'.') {
                        1
                    } else {
                        0
                    };
                    if separator > 0
                        && bytes
                            .get(i + separator)
                            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                    {
                        i += separator;
                        while i < bytes.len()
                            && (bytes[i].is_ascii_alphanumeric() || b"_'".contains(&bytes[i]))
                        {
                            i += 1;
                        }
                    } else if bytes.get(i) == Some(&b'[') {
                        let end = i
                            + 1
                            + bytes[i + 1..]
                                .iter()
                                .take_while(|b| b.is_ascii_digit())
                                .count();
                        if end == i + 1 || bytes.get(end) != Some(&b']') {
                            break;
                        }
                        i = end + 1;
                    } else {
                        break;
                    }
                }
            }
        } else {
            let operator = [
                "==>", "<=", ">=", "==", "/=", "!=", "&&", "||", "<$", ">$", "<=$", ">=$",
            ]
            .into_iter()
            .filter(|op| raw[i..].starts_with(*op))
            .max_by_key(|op| op.len());
            i += operator.map_or(ch.len_utf8(), str::len);
        }
        result.push(Token {
            text: &raw[start..i],
            start,
            end: i,
            quoted,
        });
        ensure!(result.len() <= 100_000, "precondition token limit exceeded");
    }
    Ok(result)
}

pub(super) fn lower(
    raw: &str,
    refs: &References<'_>,
    warnings: &mut Vec<String>,
) -> Result<String> {
    let ts = tokens(raw)?;
    ensure!(!ts.is_empty(), "empty precondition");
    let (mut out, mut cursor, mut i) = (String::new(), 0, 0);
    while i < ts.len() {
        let token = &ts[i];
        if token.quoted {
            i += 1;
            continue;
        }
        let (last, replacement) = if token.text == "valid" {
            ensure!(
                ts.get(i + 1).is_some_and(|t| t.text == "(")
                    && ts.get(i + 3).is_some_and(|t| t.text == ")"),
                "use valid(<region>.<semantic-leaf>)"
            );
            let reference = resolve(ts[i + 2].text, refs)?
                .context("valid refers to an unknown region/field")?;
            (i + 3, reference.valid()?)
        } else if let Some(reference) = resolve(token.text, refs)? {
            if reference.field.is_none() {
                check_index(&ts, i, reference.object, warnings)?;
                (i, pre_var(reference.object))
            } else if ts.get(i + 1).is_some_and(|t| t.text == "is") {
                ensure!(
                    ts.get(i + 2).is_some_and(|t| t.text == "valid"),
                    "expected '<field> is valid'"
                );
                (i + 2, reference.valid()?)
            } else if let Some(comparison) = comparison(&ts, i, reference, refs)? {
                comparison
            } else {
                ensure!(reference.field.is_some_and(|f| f.guard.is_none()),
                    "guarded field {} needs valid(...) or a simple named comparison; complex inactive-payload expressions are unsupported", token.text);
                (i, reference.value()?)
            }
        } else {
            i += 1;
            continue;
        };
        out.push_str(&raw[cursor..token.start]);
        out.push_str(&replacement);
        cursor = ts[last].end;
        i = last + 1;
    }
    out.push_str(&raw[cursor..]);
    Ok(out)
}

fn comparator(text: &str) -> bool {
    matches!(
        text,
        "==" | "/=" | "!=" | "<" | ">" | "<=" | ">=" | "<$" | ">$" | "<=$" | ">=$"
    )
}

fn boundary(ts: &[Token<'_>], at: usize) -> bool {
    ts.get(at).is_none_or(|t| {
        matches!(
            t.text,
            ")" | "]" | "}" | "," | "&&" | "||" | "==>" | "then" | "else"
        )
    })
}

fn predicate_start(ts: &[Token<'_>], at: usize) -> bool {
    at == 0
        || matches!(
            ts[at - 1].text,
            "(" | "[" | "{" | "," | "=" | "&&" | "||" | "==>" | "if" | "then" | "else" | "in"
        )
}

fn comparison(
    ts: &[Token<'_>],
    i: usize,
    left: Reference<'_>,
    refs: &References<'_>,
) -> Result<Option<(usize, String)>> {
    // Do not turn `f field == 0` or `1 + field == 0` into `f (field == 0)`
    // or `1 + (field == 0)`. Only a complete atomic comparison gains guards.
    if !predicate_start(ts, i) {
        return Ok(None);
    }
    let Some(op) = ts.get(i + 1).filter(|t| comparator(t.text)) else {
        return Ok(None);
    };
    let Some(rhs) = ts.get(i + 2) else {
        return Ok(None);
    };
    let (mut end, mut right_ref) = (i + 2, None);
    let right = if rhs.text.bytes().all(|b| b.is_ascii_digit()) {
        rhs.text.to_owned()
    } else if matches!(rhs.text, "-" | "+")
        && ts
            .get(i + 3)
            .is_some_and(|t| t.text.bytes().all(|b| b.is_ascii_digit()))
    {
        end += 1;
        format!("{}{}", rhs.text, ts[end].text)
    } else if let Some(reference) = resolve(rhs.text, refs)?.filter(|r| r.field.is_some()) {
        right_ref = Some(reference);
        reference.value()?
    } else {
        return Ok(None);
    };
    if !boundary(ts, end + 1) {
        return Ok(None);
    }
    let mut condition = format!("({} {} {right})", left.value()?, op.text);
    if let Some(reference) = right_ref {
        condition = reference.guarded(condition)?;
    }
    Ok(Some((end, left.guarded(condition)?)))
}

fn check_index(
    ts: &[Token<'_>],
    at: usize,
    object: &ObjectPlan,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let advice = "use named semantic fields; dynamic indexing, slicing or transforming an object cannot be validated";
    ensure!(
        object.projection == Projection::Bytes,
        "raw byte access to {} requires bytes projection; {advice}",
        object.region
    );
    let mut operator = at + 1;
    while ts.get(operator).is_some_and(|t| t.text == ")") {
        operator += 1;
    }
    let closes = operator - at - 1;
    ensure!(
        at >= closes && ts[at - closes..at].iter().all(|t| t.text == "("),
        "{advice}"
    );
    let before = at.checked_sub(closes + 1).and_then(|i| ts.get(i));
    ensure!(
        predicate_start(ts, at - closes) || before.is_some_and(|t| comparator(t.text)),
        "object {} is passed through an unvalidated expression; {advice}",
        object.region
    );
    ensure!(
        ts.get(operator).is_some_and(|t| t.text == "@"),
        "raw object {}: {advice}",
        object.region
    );
    let mut number = operator + 1;
    while ts.get(number).is_some_and(|t| t.text == "(") {
        number += 1;
    }
    let token = ts
        .get(number)
        .context("missing raw byte index; use named fields")?;
    ensure!(
        !token.quoted && token.text.bytes().all(|b| b.is_ascii_digit()),
        "nonliteral raw byte index: {advice}"
    );
    let index: usize = token.text.parse().context("raw byte index overflow")?;
    let mut end = number + 1;
    for _ in operator + 1..number {
        ensure!(
            ts.get(end).is_some_and(|t| t.text == ")"),
            "computed raw byte index: {advice}"
        );
        end += 1;
    }
    ensure!(
        boundary(ts, end) || ts.get(end).is_some_and(|t| comparator(t.text)),
        "computed raw byte index: {advice}"
    );
    ensure!(
        index < object.layout.size,
        "raw byte index {} @ {index} is out of range (compiler size {})",
        object.region,
        object.layout.size
    );
    let fields: Vec<_> = object
        .layout
        .fields
        .iter()
        .filter(|f| {
            f.offset <= index && f.offset.checked_add(f.size).is_some_and(|end| index < end)
        })
        .collect();
    ensure!(fields.len() == 1, "raw byte index {} @ {index} selects padding/inactive storage or ambiguous storage, not one semantic field", object.region);
    let field = fields[0];
    ensure!(
        !field.is_pointer,
        "raw byte indexing cannot encode pointer field {}.{} as an integer",
        object.region,
        field.path
    );
    warnings.push(format!(
        "WARNING: raw {} @ {index} selects byte {} of actual field {}.{} ({} at offset {}, {} bytes). Intended field cannot be inferred; a wrong-but-semantic offset is not detected. Use the named field, including its optional guards, instead.",
        ts[at].text, index - field.offset, object.region, field.path, field.llvm_type, field.offset, field.size
    ));
    Ok(())
}
