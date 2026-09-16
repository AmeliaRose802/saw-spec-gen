//! Bind a checked layout plan to the module SAW will actually load.
//! This checks layout/ABI evidence, not equality of executable instructions.
//! Unused types may disappear; allocated aliases and reachable types may not.

use super::LayoutPlan;
use crate::verify_tools::ToolPaths;
use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Stdio};

/// Call after preparing the plan and before publishing verification artifacts.
/// No fallback to supplied IR, text files, or empty gen-only placeholders is made.
pub fn validate(bitcode: &Path, ll_ir: &str, plan: &LayoutPlan, symbol: &str) -> Result<()> {
    let dis = ToolPaths::discover().llvm_dis.context(
        "cannot validate typed layout: llvm-dis is unavailable; configure SAW_SPEC_GEN_LLVM_BIN with the matching LLVM toolchain",
    )?;
    let input = bitcode
        .canonicalize()
        .with_context(|| format!("cannot read compiled bitcode {}", bitcode.display()))?;
    let output = Command::new(&dis)
        .arg(&input)
        .args(["-o", "-"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot validate typed layout: starting {}", dis.display()))?;
    ensure!(
        output.status.success(),
        "cannot validate typed layout: llvm-dis failed for {} ({}): {}",
        bitcode.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let actual = String::from_utf8(output.stdout)
        .context("cannot validate typed layout: llvm-dis returned non-UTF-8 IR")?;
    check_ir(&actual, ll_ir, plan, symbol).with_context(|| {
        format!(
            "compiled bitcode {} does not match layout evidence",
            bitcode.display()
        )
    })
}

fn check_ir(actual: &str, expected: &str, plan: &LayoutPlan, symbol: &str) -> Result<()> {
    let actual = Snapshot::parse(actual).context("reading compiled bitcode IR")?;
    let expected = Snapshot::parse(expected).context("reading supplied LLVM IR")?;
    for (key, evidence) in [
        ("triple", &plan.target_triple),
        ("datalayout", &plan.data_layout),
    ] {
        let want = expected
            .target
            .get(key)
            .with_context(|| format!("supplied IR lacks target {key}"))?;
        let got = actual
            .target
            .get(key)
            .with_context(|| format!("compiled bitcode lacks target {key}"))?;
        ensure!(
            got == want,
            "compiled bitcode target {key} mismatch: expected {want:?}, got {got:?}"
        );
        ensure!(
            want == evidence,
            "layout plan target {key} disagrees with supplied IR"
        );
    }
    let symbol = identifier('@', symbol)?;
    let want = expected
        .functions
        .get(&symbol)
        .context("target has no supplied LLVM signature")?;
    let got = actual
        .functions
        .get(&symbol)
        .context("target has no compiled bitcode signature")?;
    ensure!(
        want.defined && got.defined,
        "target {symbol} must have a definition in both modules"
    );
    ensure!(
        got == want,
        "compiled bitcode target {symbol} ABI mismatch (return, parameters or attributes)"
    );

    let mut required = expected.reachable_types(&symbol);
    required.extend(
        expected
            .types
            .keys()
            .filter(|key| actual.types.contains_key(*key))
            .cloned(),
    );
    for (region, object) in &plan.objects {
        let root = identifier('%', &object.layout.llvm_type)?;
        let param = got
            .params
            .get(object.argument_index)
            .with_context(|| format!("{region}: planned LLVM argument index is out of range"))?;
        ensure!(
            param.first().is_some_and(|t| t == "ptr"),
            "{region}: object is not a compiled pointer argument"
        );
        if matches!(object.lowering.as_str(), "sret" | "byval") {
            ensure!(
                param
                    .windows(4)
                    .any(|w| w[0] == object.lowering && w[1] == "(" && w[2] == root && w[3] == ")"),
                "{region}: planned {} type/position disagrees with compiled ABI",
                object.lowering
            );
        } else {
            ensure!(
                !param.iter().any(|t| matches!(t.as_str(), "sret" | "byval")),
                "{region}: unplanned sret/byval argument"
            );
        }
        let allocation = lex(&object.layout.allocation_type)?;
        if allocation.is_empty() || allocation.first().is_some_and(|t| t == "llvm_alias") {
            if !allocation.is_empty() {
                ensure!(
                    allocation.len() == 2 && allocation[1] == format!("\"{}\"", &root[1..]),
                    "{region}: allocation alias disagrees with planned LLVM type {root}"
                );
            }
            ensure!(
                actual.types.contains_key(&root),
                "{region}: compiled bitcode is missing allocation type {root}"
            );
            ensure!(
                expected.types.contains_key(&root),
                "{region}: supplied IR is missing allocation type {root}"
            );
            required.insert(root);
        }
    }
    let mut seen = BTreeSet::new();
    while let Some(name) = required.pop_first() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let want = expected
            .types
            .get(&name)
            .with_context(|| format!("supplied IR is missing required LLVM type {name}"))?;
        let got = actual
            .types
            .get(&name)
            .with_context(|| format!("compiled bitcode is missing required LLVM type {name}"))?;
        ensure!(
            got == want,
            "compiled bitcode LLVM type {name} mismatch: expected {}, got {}",
            want.join(" "),
            got.join(" ")
        );
        required.extend(want.iter().filter(|t| t.starts_with('%')).cloned());
    }
    for (name, body) in &plan.llvm_types {
        let name = identifier('%', name)?;
        if let Some(got) = actual.types.get(&name) {
            ensure!(
                *got == lex(body)?,
                "compiled bitcode LLVM type {name} disagrees with layout plan snapshot"
            );
        }
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Abi {
    prefix: Vec<String>,
    params: Vec<Vec<String>>,
    address_space: Vec<String>,
    defined: bool,
}

#[derive(Default)]
struct Snapshot {
    target: BTreeMap<String, String>,
    types: BTreeMap<String, Vec<String>>,
    functions: BTreeMap<String, Abi>,
    references: BTreeMap<String, BTreeSet<String>>,
}

impl Snapshot {
    fn parse(ir: &str) -> Result<Self> {
        let tokens = lex(ir)?;
        let mut result = Self::default();
        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];
            if token == "target" {
                let key = tokens.get(i + 1).context("incomplete target property")?;
                ensure!(
                    matches!(key.as_str(), "triple" | "datalayout"),
                    "unknown target property {key}"
                );
                ensure!(
                    tokens.get(i + 2).is_some_and(|t| t == "="),
                    "invalid target {key}"
                );
                let value = tokens
                    .get(i + 3)
                    .and_then(|t| t.strip_prefix('"'))
                    .and_then(|t| t.strip_suffix('"'))
                    .filter(|t| !t.is_empty())
                    .context("missing/non-string target property")?;
                ensure!(
                    result.target.insert(key.clone(), value.into()).is_none(),
                    "duplicate target {key}"
                );
                i += 4;
            } else if token.starts_with('%') && tokens.get(i + 1).is_some_and(|t| t == "=") {
                ensure!(
                    tokens.get(i + 2).is_some_and(|t| t == "type"),
                    "invalid named type {token}"
                );
                let start = i + 3;
                let body = tokens.get(start).context("missing named type body")?;
                let end = if body == "opaque" {
                    start + 1
                } else {
                    ensure!(
                        body == "{"
                            || (body == "<" && tokens.get(start + 1).is_some_and(|t| t == "{")),
                        "unsupported named type {token}"
                    );
                    group_end(&tokens, start)?
                };
                ensure!(
                    result
                        .types
                        .insert(token.clone(), tokens[start..end].to_vec())
                        .is_none(),
                    "duplicate named type {token}"
                );
                i = end;
            } else if matches!(token.as_str(), "define" | "declare") {
                let defined = token == "define";
                let at = (i + 1..tokens.len())
                    .find(|&j| tokens[j].starts_with('@'))
                    .context("LLVM function has no symbol")?;
                ensure!(
                    tokens.get(at + 1).is_some_and(|t| t == "("),
                    "invalid LLVM function signature"
                );
                let after = group_end(&tokens, at + 1)?;
                let (header_end, end) = item_end(&tokens, after, defined)?;
                let mut address_space = Vec::new();
                for pos in after..header_end {
                    if tokens[pos] == "addrspace" {
                        address_space.extend_from_slice(&tokens[pos..group_end(&tokens, pos + 1)?]);
                    }
                }
                let abi = Abi {
                    prefix: abi_tokens(&tokens[i + 1..at])?,
                    params: parameters(&tokens[at + 2..after - 1])?,
                    address_space,
                    defined,
                };
                ensure!(
                    result.functions.insert(tokens[at].clone(), abi).is_none(),
                    "duplicate function {}",
                    tokens[at]
                );
                result
                    .references
                    .insert(tokens[at].clone(), references(&tokens[i..end]));
                i = end;
            } else if token.starts_with('@') && tokens.get(i + 1).is_some_and(|t| t == "=") {
                let (_, end) = item_end(&tokens, i + 2, false)?;
                ensure!(
                    result
                        .references
                        .insert(token.clone(), references(&tokens[i + 2..end]))
                        .is_none(),
                    "duplicate global {token}"
                );
                i = end;
            } else {
                i += 1; // Attribute groups, debug metadata and compiler identities are not layouts.
            }
        }
        Ok(result)
    }

    fn reachable_types(&self, symbol: &str) -> BTreeSet<String> {
        let mut pending = BTreeSet::from([symbol.to_owned()]);
        let mut seen = BTreeSet::new();
        let mut types = BTreeSet::new();
        while let Some(name) = pending.pop_first() {
            if !seen.insert(name.clone()) {
                continue;
            }
            if self.types.contains_key(&name) {
                types.insert(name);
            } else if let Some(next) = self.references.get(&name) {
                pending.extend(next.iter().cloned());
            }
        }
        types
    }
}

fn references(tokens: &[String]) -> BTreeSet<String> {
    tokens
        .iter()
        .filter(|t| t.starts_with('%') || t.starts_with('@'))
        .cloned()
        .collect()
}

// Retain inline attributes conservatively, including alignment/dereferenceability,
// calling conventions and sret/byval types. noalias and SSA names are not layout facts.
fn abi_tokens(tokens: &[String]) -> Result<Vec<String>> {
    ensure!(
        !tokens.iter().any(|t| t == "#"),
        "grouped ABI attributes cannot validate typed layout"
    );
    Ok(tokens.iter().filter(|t| *t != "noalias").cloned().collect())
}

fn parameters(tokens: &[String]) -> Result<Vec<Vec<String>>> {
    let (mut start, mut i) = (0, 0);
    let mut result = Vec::new();
    while i < tokens.len() {
        if closing(&tokens[i]).is_some() {
            i = group_end(tokens, i)?;
        } else if tokens[i] == "," {
            ensure!(i > start, "empty LLVM parameter");
            result.push(tokens[start..i].to_vec());
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    ensure!(
        tokens.is_empty() || start < tokens.len(),
        "trailing LLVM parameter comma"
    );
    if start < tokens.len() {
        result.push(tokens[start..].to_vec());
    }
    for param in &mut result {
        if param.len() > 1 && param.last().is_some_and(|t| t.starts_with('%')) {
            param.pop();
        }
        *param = abi_tokens(param)?;
    }
    Ok(result)
}

fn boundary(tokens: &[String], i: usize) -> bool {
    matches!(
        tokens[i].as_str(),
        "define"
            | "declare"
            | "attributes"
            | "target"
            | "source_filename"
            | "module"
            | "uselistorder"
            | "uselistorder_bb"
    ) || (tokens[i].starts_with(['@', '%', '$']) && tokens.get(i + 1).is_some_and(|t| t == "="))
        || (tokens[i] == "!" && tokens.get(i + 2).is_some_and(|t| t == "="))
}

// Return the header boundary and end of the item. Aggregate instruction operands
// are balanced, so literal structs cannot truncate a function body.
fn item_end(tokens: &[String], mut i: usize, defined: bool) -> Result<(usize, usize)> {
    while i < tokens.len() && !boundary(tokens, i) {
        if tokens[i] == "!" && tokens.get(i + 1).is_some_and(|t| t == "{") {
            i = group_end(tokens, i + 1)?;
        } else if defined && tokens[i] == "{" {
            return Ok((i, group_end(tokens, i)?));
        } else if closing(&tokens[i]).is_some() {
            i = group_end(tokens, i)?;
        } else {
            ensure!(
                !defined || !matches!(tokens[i].as_str(), "prefix" | "prologue"),
                "prefix/prologue function data cannot validate typed layout"
            );
            i += 1;
        }
    }
    ensure!(!defined, "LLVM target definition has no complete body");
    Ok((i, i))
}

fn closing(token: &str) -> Option<&'static str> {
    match token {
        "(" => Some(")"),
        "[" => Some("]"),
        "{" => Some("}"),
        "<" => Some(">"),
        _ => None,
    }
}

fn group_end(tokens: &[String], start: usize) -> Result<usize> {
    let first = tokens
        .get(start)
        .and_then(|t| closing(t))
        .context("expected LLVM delimiter")?;
    let mut stack = vec![first];
    for (i, token) in tokens.iter().enumerate().skip(start + 1) {
        if let Some(close) = closing(token) {
            ensure!(stack.len() < 128, "LLVM delimiter nesting limit exceeded");
            stack.push(close);
        } else if matches!(token.as_str(), ")" | "]" | "}" | ">") {
            ensure!(
                stack.pop() == Some(token.as_str()),
                "mismatched LLVM delimiter"
            );
            if stack.is_empty() {
                return Ok(i + 1);
            }
        }
    }
    bail!("unterminated LLVM delimiter")
}

fn identifier(sigil: char, name: &str) -> Result<String> {
    let tokens = lex(&format!("{sigil}\"{}\"", name.trim_matches('"')))?;
    ensure!(tokens.len() == 1, "invalid LLVM identifier {name:?}");
    Ok(tokens[0].clone())
}

// Token equality normalizes whitespace/comments/identifier quoting, not type
// structure or target strings. A quoted name containing punctuation stays atomic.
fn lex(mut text: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    const PUNCT: &[u8] = b"()[]{}<>,*=!#:";
    while !text.is_empty() {
        let byte = text.as_bytes()[0];
        if byte.is_ascii_whitespace() {
            text = &text[1..];
        } else if byte == b';' {
            text = text.find('\n').map_or("", |end| &text[end + 1..]);
        } else if matches!(byte, b'%' | b'@') {
            text = &text[1..];
            let name = if text.starts_with('"') {
                quoted(&mut text)?
            } else {
                let end = text
                    .bytes()
                    .take_while(|b| {
                        b.is_ascii_alphanumeric() || matches!(*b, b'-' | b'_' | b'$' | b'.')
                    })
                    .count();
                ensure!(end > 0, "empty LLVM identifier");
                let name = text[..end].to_owned();
                text = &text[end..];
                name
            };
            result.push(format!("{}{name}", byte as char));
        } else if byte == b'"' {
            result.push(format!("\"{}\"", quoted(&mut text)?));
        } else if PUNCT.contains(&byte) {
            result.push((byte as char).to_string());
            text = &text[1..];
        } else {
            let end = text
                .bytes()
                .take_while(|b| {
                    !b.is_ascii_whitespace() && !PUNCT.contains(b) && !b";\"%@".contains(b)
                })
                .count();
            ensure!(end > 0, "invalid LLVM token");
            result.push(text[..end].to_owned());
            text = &text[end..];
        }
    }
    Ok(result)
}

fn quoted(text: &mut &str) -> Result<String> {
    let bytes = text.as_bytes();
    let mut value = Vec::new();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                *text = &text[i + 1..];
                return String::from_utf8(value).context("non-UTF-8 LLVM string");
            }
            b'\\' => {
                if bytes.get(i + 1) == Some(&b'\\') {
                    value.push(b'\\');
                    i += 2;
                    continue;
                }
                ensure!(
                    bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                        && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit),
                    "invalid LLVM string escape"
                );
                value.push(u8::from_str_radix(&text[i + 1..i + 3], 16)?);
                i += 3;
            }
            byte => {
                value.push(byte);
                i += 1;
            }
        }
    }
    bail!("unterminated LLVM string")
}

#[cfg(test)]
#[path = "metadata_tests.rs"]
mod tests;
