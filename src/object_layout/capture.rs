//! Persist compiler layout facts alongside the IR that supplied their provenance.

use super::clang::parse_record_layouts;
use super::model::CompilerLayouts;
use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub fn sidecar_path(ll: &Path) -> PathBuf {
    ll.with_extension("layout.json")
}

/// Capture the full AST dump and exact named LLVM definitions, without sizing
/// LLVM types. Target properties and reachable !llvm.ident strings are required.
/// Multiple compiler identities are retained, in metadata order, on separate lines.
pub fn capture(ir: &str, dump: &str, command: Vec<String>) -> Result<CompilerLayouts> {
    let facts = ir_facts(ir)?;
    Ok(CompilerLayouts {
        schema_version: 1,
        target_triple: facts.target_triple,
        data_layout: facts.data_layout,
        compiler: compiler_identity(ir)?,
        command,
        llvm_types: facts.llvm_types,
        irgen_types: irgen_types(dump)?,
        records: parse_record_layouts(dump)?,
    })
}

pub fn write(ll: &Path, facts: &CompilerLayouts) -> Result<()> {
    ensure!(
        facts.schema_version == 1,
        "unsupported compiler layout schema"
    );
    let path = sidecar_path(ll);
    let mut json = serde_json::to_vec_pretty(facts)?;
    json.push(b'\n');
    fs::write(&path, json).with_context(|| format!("writing compiler layouts {}", path.display()))
}

fn irgen_types(dump: &str) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for line in dump.lines().map(str::trim) {
        let Some(definition) = line
            .strip_prefix("LLVMType:")
            .or_else(|| line.strip_prefix("NonVirtualBaseLLVMType:"))
        else {
            continue;
        };
        if let Some((name, body)) = type_definition(definition)? {
            if let Some(previous) = result.insert(name.into(), body.into()) {
                ensure!(
                    previous == body,
                    "conflicting compiler IRgen layout for {name}"
                );
            }
        }
    }
    Ok(result)
}

/// Enrich layout analysis only, not executable bitcode. Unused compiler record
/// types are allocated with exact inline types rather than invented aliases.
pub fn with_irgen_types(facts: &CompilerLayouts, ir: &str) -> Result<String> {
    let current = ir_facts(ir)?;
    let mut result = ir.to_owned();
    for (name, body) in &facts.irgen_types {
        if let Some(existing) = current.llvm_types.get(name) {
            ensure!(
                existing == body,
                "compiler IRgen/module type disagreement for {name}"
            );
        } else {
            result.push_str(&format!("\n%\"{name}\" = type {body}\n"));
        }
    }
    Ok(result)
}

/// Absence alone returns None. Invalid, stale, or incompatible sidecars are errors.
/// Every recorded named type must still match; newly added EH types are allowed.
/// Compiler/command describe the original invocation, not subsequent IR transforms.
pub fn load(ll: &Path, ir: &str) -> Result<Option<CompilerLayouts>> {
    let path = sidecar_path(ll);
    let json = match fs::read(&path) {
        Ok(json) => json,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let facts: CompilerLayouts = serde_json::from_slice(&json)
        .with_context(|| format!("invalid compiler layout sidecar {}", path.display()))?;
    ensure!(
        facts.schema_version == 1,
        "unsupported compiler layout schema {}",
        facts.schema_version
    );
    let current = ir_facts(ir)?;
    ensure!(
        facts.target_triple == current.target_triple,
        "compiler layout target triple mismatch"
    );
    ensure!(
        facts.data_layout == current.data_layout,
        "compiler layout data layout mismatch"
    );
    for (name, body) in &facts.llvm_types {
        let definition = current.llvm_types.get(name).with_context(|| {
            format!("compiler layout LLVM type %{name} is missing from current IR")
        })?;
        ensure!(
            definition == body,
            "compiler layout LLVM type %{name} changed in current IR"
        );
    }
    Ok(Some(facts))
}

struct IrFacts {
    target_triple: String,
    data_layout: String,
    llvm_types: BTreeMap<String, String>,
}

fn ir_facts(ir: &str) -> Result<IrFacts> {
    let mut triple = None;
    let mut layout = None;
    let mut llvm_types = BTreeMap::new();
    for raw in ir.lines() {
        let line = uncomment(raw).trim();
        if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "target triple" => target_value(&mut triple, value, "target triple")?,
                "target datalayout" => target_value(&mut layout, value, "target datalayout")?,
                _ => {}
            }
        }
        if let Some((name, body)) = type_definition(line)? {
            if let Some(previous) = llvm_types.insert(name.to_owned(), body.to_owned()) {
                ensure!(previous == body, "conflicting named LLVM type %{name}");
            }
        }
    }
    Ok(IrFacts {
        target_triple: triple.context("IR is missing target triple")?,
        data_layout: layout.context("IR is missing target datalayout")?,
        llvm_types,
    })
}

fn target_value(slot: &mut Option<String>, text: &str, key: &str) -> Result<()> {
    let (value, tail) = quoted(text.trim())?;
    ensure!(tail.trim().is_empty(), "trailing text in {key}");
    let value = unescape(value)?;
    ensure!(!value.is_empty(), "empty {key}");
    if let Some(previous) = slot {
        ensure!(*previous == value, "conflicting {key}");
    }
    *slot = Some(value);
    Ok(())
}

// Clean only the % sigil and surrounding quotes, not namespaces or suffixes.
// Quoted-name escapes stay in LLVM spelling, as do references in definition bodies.
fn type_definition(line: &str) -> Result<Option<(&str, &str)>> {
    let Some(rest) = line.strip_prefix('%') else {
        return Ok(None);
    };
    let (name, rest) = if rest.starts_with('"') {
        quoted(rest)?
    } else {
        let end = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || b"-_$.".contains(b))
            .count();
        (&rest[..end], &rest[end..])
    };
    let Some(rhs) = rest.trim_start().strip_prefix('=') else {
        return Ok(None);
    };
    let Some(body) = rhs.trim_start().strip_prefix("type") else {
        return Ok(None);
    };
    if !body.is_empty() && !body.starts_with(char::is_whitespace) {
        return Ok(None);
    }
    let body = body.trim();
    ensure!(!name.is_empty(), "empty named LLVM type");
    ensure!(
        body == "opaque"
            || (body.starts_with('{') && body.ends_with('}'))
            || (body.starts_with("<{") && body.ends_with("}>")),
        "unsupported or incomplete named LLVM type %{name}: {body}"
    );
    Ok(Some((name, body)))
}

fn uncomment(line: &str) -> &str {
    let mut in_string = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            ';' if !in_string => return &line[..index],
            _ => {}
        }
    }
    line
}

// LLVM strings use hexadecimal byte escapes, not JSON/C escape sequences.
fn quoted(text: &str) -> Result<(&str, &str)> {
    ensure!(text.starts_with('"'), "expected quoted LLVM string: {text}");
    let bytes = text.as_bytes();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Ok((&text[1..index], &text[index + 1..])),
            b'\\' => {
                ensure!(
                    bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                        && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit),
                    "invalid LLVM string escape"
                );
                index += 3;
            }
            _ => index += 1,
        }
    }
    bail!("unterminated LLVM string")
}

fn unescape(text: &str) -> Result<String> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < text.len() {
        if text.as_bytes()[index] == b'\\' {
            result.push(u8::from_str_radix(&text[index + 1..index + 3], 16)?);
            index += 3;
        } else {
            result.push(text.as_bytes()[index]);
            index += 1;
        }
    }
    String::from_utf8(result).context("non-UTF-8 LLVM provenance string")
}

fn compiler_identity(ir: &str) -> Result<String> {
    let mut nodes = BTreeMap::new();
    for raw in ir.lines() {
        let line = uncomment(raw).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key == "!llvm.ident"
            || key
                .strip_prefix('!')
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        {
            ensure!(
                nodes.insert(key, value.trim()).is_none(),
                "duplicate metadata node {key}"
            );
        }
    }
    let root = nodes
        .get("!llvm.ident")
        .context("IR is missing !llvm.ident compiler provenance")?;
    let mut strings = Vec::new();
    ident_strings(root, &nodes, &mut BTreeSet::new(), &mut strings)?;
    ensure!(!strings.is_empty(), "!llvm.ident has no compiler identity");
    Ok(strings.join("\n"))
}

fn ident_strings(
    value: &str,
    nodes: &BTreeMap<&str, &str>,
    active: &mut BTreeSet<String>,
    strings: &mut Vec<String>,
) -> Result<()> {
    let value = value.strip_prefix("distinct ").unwrap_or(value);
    let mut rest = value
        .strip_prefix("!{")
        .and_then(|s| s.strip_suffix('}'))
        .context("expected !llvm.ident metadata tuple")?
        .trim();
    while !rest.is_empty() {
        if rest.starts_with("!\"") {
            let (raw, tail) = quoted(&rest[1..])?;
            let identity = unescape(raw)?;
            ensure!(!identity.is_empty(), "empty compiler identity");
            if !strings.contains(&identity) {
                strings.push(identity);
            }
            rest = tail.trim_start();
        } else {
            ensure!(
                rest.starts_with('!'),
                "unsupported !llvm.ident operand: {rest}"
            );
            let end = 1 + rest[1..].bytes().take_while(|b| b.is_ascii_digit()).count();
            ensure!(end > 1, "unsupported !llvm.ident operand: {rest}");
            let reference = &rest[..end];
            ensure!(
                active.len() < 64 && active.insert(reference.into()),
                "cyclic/deep !llvm.ident metadata"
            );
            let node = nodes
                .get(reference)
                .with_context(|| format!("missing compiler metadata {reference}"))?;
            ident_strings(node, nodes, active, strings)?;
            active.remove(reference);
            rest = rest[end..].trim_start();
        }
        if !rest.is_empty() {
            rest = rest
                .strip_prefix(',')
                .context("malformed !llvm.ident operands")?
                .trim_start();
            ensure!(!rest.is_empty(), "trailing comma in !llvm.ident");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const IR: &str = r#"target datalayout = "e-m:w-p:64:64-i64:64-n8:16:32:64-S128"
target triple = "x86_64-pc-windows-msvc"
%struct.Foo = type { i32 }
%"class.ns::Box<int>.2" = type <{ i8, i32 }>
%forward = type opaque
@source_literal = private constant [24 x i8] c"clang version not-ident\00"
!llvm.ident = !{!4}
!3 = !{!"clang version unrelated metadata"}
!4 = !{!"clang version 20.1.8"}
"#;
    const DUMP: &str = "*** Dumping AST Record Layout\n         0 | struct Foo\n         0 |   int value\n           | [sizeof=4, align=4,\n           |  nvsize=4, nvalign=4]\n";

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "saw-layout-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path.join("module.ll"))
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn captures_actual_provenance_types_and_command() {
        let command = vec![
            "clang++".into(),
            "-Xclang".into(),
            "-fdump-record-layouts".into(),
        ];
        let facts = capture(IR, DUMP, command.clone()).unwrap();
        assert_eq!(facts.schema_version, 1);
        assert_eq!(facts.compiler, "clang version 20.1.8");
        assert_eq!(facts.target_triple, "x86_64-pc-windows-msvc");
        assert_eq!(facts.command, command);
        assert_eq!(facts.llvm_types.len(), 3);
        assert_eq!(facts.llvm_types["class.ns::Box<int>.2"], "<{ i8, i32 }>");
        assert_eq!(facts.llvm_types["forward"], "opaque");
        assert_eq!(facts.records["Foo"].size, 4);
    }

    #[test]
    fn requires_target_and_ident_provenance() {
        for prefix in ["target triple", "target datalayout", "!llvm.ident", "!4 ="] {
            let ir = IR
                .lines()
                .filter(|line| !line.starts_with(prefix))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(capture(&ir, DUMP, vec![]).is_err(), "missing {prefix}");
        }
        for ident in ["!{}", "!{!999}", "!{!4,}"] {
            let ir = IR.replace("!llvm.ident = !{!4}", &format!("!llvm.ident = {ident}"));
            assert!(capture(&ir, DUMP, vec![]).is_err());
        }
        let cyclic = IR.replace("!4 = !{!\"clang version 20.1.8\"}", "!4 = !{!4}");
        assert!(capture(&cyclic, DUMP, vec![]).is_err());
        assert!(capture(IR, "*** Dumping AST Record Layout\n", vec![]).is_err());
    }

    #[test]
    fn follows_ident_references_and_decodes_llvm_strings() {
        let ir = IR.replace("!llvm.ident = !{!4}", "!llvm.ident = !{!9, !4, !9}")
            + "!9 = !{!\"clang \\22build\\22; version \\32\\30\"}\n";
        let facts = capture(&ir, DUMP, vec![]).unwrap();
        assert_eq!(
            facts.compiler,
            "clang \"build\"; version 20\nclang version 20.1.8"
        );
    }

    #[test]
    fn matches_only_named_type_definition_lines() {
        assert_eq!(type_definition("%tmp = alloca i8").unwrap(), None);
        assert_eq!(type_definition("%tmp = typewriter i8").unwrap(), None);
        let ir = format!("{IR}%\"struct.semi;equals=\\22\" = type {{ ptr }} ; comment\n");
        let facts = capture(&ir, DUMP, vec![]).unwrap();
        assert_eq!(facts.llvm_types["struct.semi;equals=\\22"], "{ ptr }");
        for extra in [
            "%struct.Foo = type { i64 }",
            "%\"struct.Foo\" = type { i8 }",
            "%broken = type {",
            "% = type opaque",
        ] {
            assert!(capture(&format!("{IR}{extra}"), DUMP, vec![]).is_err());
        }
    }

    #[test]
    fn json_sidecar_round_trip_and_extra_eh_types() {
        let scratch = Scratch::new();
        let facts = capture(IR, DUMP, vec!["clang++".into()]).unwrap();
        write(&scratch.0, &facts).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(sidecar_path(&scratch.0)).unwrap()).unwrap();
        assert_eq!(json["schema_version"], 1);
        let ir = format!("{IR}%eh.transformed = type {{ i32, ptr }}\n");
        let loaded = load(&scratch.0, &ir).unwrap().unwrap();
        assert_eq!(loaded.records, facts.records);
        assert_eq!(loaded.llvm_types, facts.llvm_types);
        assert_eq!(loaded.command, facts.command);
        assert_eq!(loaded.compiler, facts.compiler);
        let stripped = ir
            .lines()
            .filter(|line| !line.starts_with('!'))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            load(&scratch.0, &stripped).unwrap().unwrap().compiler,
            facts.compiler
        );
    }

    #[test]
    fn rejects_cross_target_and_stale_definitions() {
        let scratch = Scratch::new();
        write(&scratch.0, &capture(IR, DUMP, vec![]).unwrap()).unwrap();
        for ir in [
            IR.replace("x86_64-pc-windows-msvc", "aarch64-unknown-linux-gnu"),
            IR.replace("e-m:w-p:64:64-i64:64-n8:16:32:64-S128", "E-p:32:32"),
            IR.replace("%struct.Foo = type { i32 }", "%struct.Foo = type { i64 }"),
            IR.replace("%struct.Foo = type { i32 }\n", ""),
            IR.replace("%forward = type opaque\n", ""),
        ] {
            assert!(
                load(&scratch.0, &ir).is_err(),
                "accepted incompatible IR: {ir}"
            );
        }
    }

    #[test]
    fn absence_is_distinct_from_invalid_sidecars() {
        let scratch = Scratch::new();
        assert!(load(&scratch.0, "").unwrap().is_none());
        fs::write(sidecar_path(&scratch.0), "not JSON").unwrap();
        assert!(load(&scratch.0, IR).is_err());
        let mut facts = capture(IR, DUMP, vec![]).unwrap();
        facts.schema_version = 2;
        assert!(write(&scratch.0, &facts).is_err());
        fs::write(
            sidecar_path(&scratch.0),
            serde_json::to_vec(&facts).unwrap(),
        )
        .unwrap();
        assert!(load(&scratch.0, IR).is_err());
    }

    #[test]
    fn sidecar_uses_layout_json_extension() {
        assert_eq!(
            sidecar_path(Path::new("module.bc.ll")),
            PathBuf::from("module.bc.layout.json")
        );
        assert_eq!(
            sidecar_path(Path::new("module")),
            PathBuf::from("module.layout.json")
        );
    }
}
