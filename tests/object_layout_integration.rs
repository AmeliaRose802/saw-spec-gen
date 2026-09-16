//! Compiler-fact and generation regressions, not substitute verification code.
//! Only these tests run clang/CLI processes. SAW is never required or invoked;
//! the manifest-driven E2E suite owns the fixtures' proof verdicts.

use saw_spec_gen::object_layout::{
    capture, clang::normalize_name, data_layout::DataLayout, derive, emit, field_key, validate,
    CompilerLayouts, FieldLayout, LayoutConfig, LayoutPlan, ObjectLayout, ObjectPlan, Projection,
    RecordMember,
};
use saw_spec_gen::verify_tools::ToolPaths;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "object_layout_integration/abi.rs"]
mod abi;
#[path = "object_layout_integration/validation.rs"]
mod validation;

const TARGETS: [&str; 2] = ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"];
const CASES: [(&str, &str); 4] = [
    ("nested_pod", "advance_outer"),
    ("pointer_frame", "update_value"),
    ("multiple_bases", "accumulate_tail"),
    ("selected_union", "increment_left"),
];
static NEXT: AtomicU64 = AtomicU64::new(0);

fn fixture(case: (&str, &str), suffix: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/e2e/cases/15-object-layout")
        .join(case.0)
        .join(format!("{}_{suffix}", case.1))
}

fn clang() -> Option<PathBuf> {
    let clang = ToolPaths::discover().clang;
    if clang.is_none() {
        eprintln!(
            "SKIP object-layout integration: clang toolchain not discovered (SAW is not needed)"
        );
    }
    clang
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "saw-object-layout-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn diagnostics(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn checked(command: &mut Command) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(
        output.status.success(),
        "{command:?}: {}",
        diagnostics(&output)
    );
    output
}

struct Module {
    root: Scratch,
    ll: PathBuf,
    ast: PathBuf,
    bc: PathBuf,
    ir: String,
    facts: CompilerLayouts,
}

impl Module {
    fn compile(clang: &Path, source: &Path, target: &str) -> Self {
        let root = Scratch::new();
        let ll = root.0.join("module.ll");
        let ast = root.0.join("module.ast.json");
        let bc = root.0.join("module.bc");
        let common = [
            "-x",
            "c++",
            "-std=c++17",
            "-O0",
            "-fno-rtti",
            "-fno-exceptions",
            "-target",
            target,
        ];
        let mut command = Command::new(clang);
        command
            .args(common)
            .args(["-S", "-emit-llvm", "-Xclang", "-fdump-record-layouts"])
            .arg(source)
            .arg("-o")
            .arg(&ll);
        let invocation = std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let dump = checked(&mut command);
        let ir = fs::read_to_string(&ll).unwrap();
        let facts = capture::capture(&ir, &diagnostics(&dump), invocation).unwrap();
        capture::write(&ll, &facts).unwrap();
        // Assertions below consume the persisted sidecar, not fabricated facts.
        let facts = capture::load(&ll, &ir).unwrap().unwrap();
        let ast_output = checked(
            Command::new(clang)
                .args(common)
                .args(["-Xclang", "-ast-dump=json", "-fsyntax-only"])
                .arg(source),
        );
        fs::write(&ast, ast_output.stdout).unwrap();
        checked(
            Command::new(clang)
                .args(common)
                .args(["-c", "-emit-llvm"])
                .arg(source)
                .arg("-o")
                .arg(&bc),
        );
        Self {
            root,
            ll,
            ast,
            bc,
            ir,
            facts,
        }
    }

    fn config(&self, case: (&str, &str), edit: impl FnOnce(&mut toml::Value)) -> PathBuf {
        let text = fs::read_to_string(fixture(case, "spec.toml")).unwrap();
        let mut value: toml::Value = toml::from_str(&text).unwrap();
        edit(&mut value);
        let name = format!("config-{}.toml", NEXT.fetch_add(1, Ordering::Relaxed));
        self.root.write(&name, &toml::to_string(&value).unwrap())
    }

    fn generate(&self, case: (&str, &str), config: &Path) -> (Output, PathBuf) {
        let output_dir = self
            .root
            .0
            .join(format!("out-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        let output = Command::new(env!("CARGO_BIN_EXE_saw-spec-gen"))
            .current_dir(&self.root.0)
            .arg("gen-verify")
            .arg("--ast")
            .arg(&self.ast)
            .arg("--bitcode")
            .arg(&self.bc)
            .arg("--llvm-ir")
            .arg(&self.ll)
            .arg("--cryptol-spec")
            .arg(fixture(case, "spec.cry"))
            .arg("--cryptol-fn")
            .arg(format!("{}_contract", case.1))
            .arg("--function")
            .arg(case.1)
            .arg("--config")
            .arg(config)
            .arg("--output")
            .arg(&output_dir)
            .env("SAW_SPEC_GEN_SAW", self.root.0.join("saw-must-not-run"))
            .output()
            .expect("run the Cargo-built gen-verify binary");
        (output, output_dir)
    }

    fn valid(&self, case: (&str, &str), config: &Path) -> (LayoutPlan, String) {
        let (output, dir) = self.generate(case, config);
        assert!(output.status.success(), "{}", diagnostics(&output));
        assert!(
            !dir.join("result.json").exists(),
            "generation is not a proof run"
        );
        let plan =
            serde_json::from_slice(&fs::read(dir.join("layout-plan.json")).unwrap()).unwrap();
        (plan, fs::read_to_string(dir.join("verify.saw")).unwrap())
    }

    fn reject(&self, case: (&str, &str), config: &Path, expected: &str) {
        let (output, dir) = self.generate(case, config);
        let text = diagnostics(&output);
        assert!(!output.status.success(), "accepted invalid layout: {text}");
        assert!(
            text.contains(expected),
            "expected {expected:?}, got: {text}"
        );
        for artifact in ["verify.saw", "layout-plan.json", "result.json"] {
            assert!(
                !dir.join(artifact).exists(),
                "published {artifact} for an invalid layout"
            );
        }
    }
}

fn put(value: &mut toml::Value, key: &str, data: impl serde::Serialize) {
    value
        .as_table_mut()
        .unwrap()
        .insert(key.into(), toml::Value::try_from(data).unwrap());
}

fn member<'a>(members: &'a [RecordMember], name: &str) -> &'a RecordMember {
    members
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("missing compiler member {name}"))
}

fn field<'a>(layout: &'a ObjectLayout, path: &str) -> &'a FieldLayout {
    layout
        .fields
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("missing semantic field {path}"))
}

fn paths(layout: &ObjectLayout) -> Vec<&str> {
    layout.fields.iter().map(|f| f.path.as_str()).collect()
}
